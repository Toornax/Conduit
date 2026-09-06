//! `conduitd autostart` (M1b-35) : la sous-commande vue depuis le binaire.
//!
//! Sous Windows, le cycle complet est joué sur une tâche **jetable**
//! (`Conduit\conduitd-test-<pid>-<cas>`, `--task-name`) : la vraie tâche de
//! l'utilisateur n'est jamais touchée, et une garde la supprime même si le test échoue.

use std::process::{Command as Process, Output};

/// Lance `conduitd` avec les arguments donnés.
fn conduitd(args: &[&str]) -> Output {
    Process::new(env!("CARGO_BIN_EXE_conduitd"))
        .args(args)
        .output()
        .expect("lancement de conduitd")
}

/// Sortie standard et d'erreur réunies, pour les assertions.
fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// L'aide décrit la sous-commande et ce qu'elle fait.
#[test]
fn autostart_is_documented_in_the_help() {
    let out = conduitd(&["--help"]);
    assert!(out.status.success());
    let help = text(&out);
    assert!(help.contains("autostart"), "{help}");
    let out = conduitd(&["autostart", "--help"]);
    assert!(out.status.success());
    let help = text(&out);
    for expected in ["enable", "disable", "status"] {
        assert!(
            help.contains(expected),
            "« {expected} » absent de :\n{help}"
        );
    }
}

/// Hors Windows : la sous-commande explique quel mécanisme prendra le relais, et sort en 2.
#[cfg(not(windows))]
#[test]
fn other_platforms_point_at_systemd_or_launchagent() {
    for action in ["status", "enable", "disable"] {
        let out = conduitd(&["autostart", action]);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        let message = text(&out);
        assert!(
            message.contains("systemd") || message.contains("LaunchAgent"),
            "{message}"
        );
    }
}

/// Nom de tâche jetable, propre à ce processus et à ce cas de test.
#[cfg(windows)]
fn test_task_name(case: &str) -> String {
    format!(r"Conduit\conduitd-test-{}-{case}", std::process::id())
}

/// Supprime la tâche quoi qu'il arrive, y compris si le test panique.
#[cfg(windows)]
struct Cleanup(String);

#[cfg(windows)]
impl Drop for Cleanup {
    fn drop(&mut self) {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let schtasks = std::path::Path::new(&system_root)
            .join("System32")
            .join("schtasks.exe");
        let _ = Process::new(schtasks)
            .args(["/Delete", "/TN", &self.0, "/F"])
            .output();
    }
}

/// Sur une machine sans la tâche : « absente », code 4.
#[cfg(windows)]
#[test]
fn status_reports_an_absent_task_with_code_4() {
    let name = test_task_name("absente");
    let out = conduitd(&["autostart", "status", "--task-name", &name]);
    let message = text(&out);
    assert_eq!(out.status.code(), Some(4), "{message}");
    assert!(message.contains("absente"), "{message}");
    assert!(message.contains("autostart enable"), "{message}");
}

/// `enable` → `status` → `disable` → `status` sur une tâche jetable.
#[cfg(windows)]
#[test]
fn enable_status_disable_round_trip() {
    let name = test_task_name("cycle");
    let _cleanup = Cleanup(name.clone());

    let out = conduitd(&["autostart", "enable", "--task-name", &name, "--delay", "45"]);
    let message = text(&out);
    if !out.status.success() {
        // Une stratégie de groupe peut interdire la création de tâches : le test n'a
        // alors rien à prouver sur cette machine.
        if message.contains("refus") || message.to_lowercase().contains("denied") {
            eprintln!("test sauté : création de tâche planifiée refusée\n{message}");
            return;
        }
        panic!("`autostart enable` a échoué :\n{message}");
    }
    assert!(message.contains(&name), "{message}");

    let out = conduitd(&["autostart", "status", "--task-name", &name]);
    let message = text(&out);
    assert_eq!(out.status.code(), Some(0), "{message}");
    assert!(message.contains("présente"), "{message}");
    assert!(
        message.contains("conduitd.exe"),
        "la commande de la tâche doit être notre exécutable :\n{message}"
    );

    let out = conduitd(&["autostart", "disable", "--task-name", &name]);
    let message = text(&out);
    assert_eq!(out.status.code(), Some(0), "{message}");

    let out = conduitd(&["autostart", "status", "--task-name", &name]);
    let message = text(&out);
    assert_eq!(out.status.code(), Some(4), "{message}");
    assert!(message.contains("absente"), "{message}");

    // `disable` est idempotent : la désinstallation peut l'appeler deux fois.
    let out = conduitd(&["autostart", "disable", "--task-name", &name]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
}
