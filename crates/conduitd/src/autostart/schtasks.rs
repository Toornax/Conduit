//! Enregistrement de la tâche planifiée par `schtasks.exe` (ADR-013).

use std::path::PathBuf;
use std::process::Command;

use super::xml::{self, TaskDefinition};
use super::{Options, EXIT_ABSENT, EXIT_FAILED, EXIT_OK};

/// Chemin absolu de `schtasks.exe`.
///
/// Jamais « `schtasks` » tout court : un exécutable de ce nom déposé dans le répertoire
/// courant ou dans le `PATH` serait lancé à sa place.
fn schtasks() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(root).join("System32").join("schtasks.exe")
}

/// `DOMAINE\utilisateur` de la session courante.
fn user_id() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() && !user.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    }
}

/// Décode la sortie de `schtasks`.
///
/// C'est un programme console : quand sa sortie est redirigée, il écrit dans la page de
/// codes de la console, qui vaut UTF-8 sur un Windows récent mais pas partout. On tente
/// l'UTF-8, puis on se rabat sur un décodage indulgent — au pire un accent devient
/// « � » dans un message d'état, jamais dans une donnée exploitée.
fn decode(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Ligne d'arguments de l'action, avec les arguments contenant une espace entre
/// guillemets.
fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') && !a.starts_with('"') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `\Conduit\conduitd` à partir de `Conduit\conduitd`.
fn uri(task_name: &str) -> String {
    format!("\\{}", task_name.trim_start_matches('\\'))
}

/// Interroge le Planificateur en silence : la tâche existe-t-elle ?
fn query(task_name: &str) -> std::io::Result<std::process::Output> {
    Command::new(schtasks())
        .args(["/Query", "/TN", task_name, "/FO", "CSV", "/V", "/NH"])
        .output()
}

/// Enregistre (ou remplace) la tâche.
pub fn enable(options: &Options) -> i32 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("chemin de l'exécutable courant introuvable : {e}");
            return EXIT_FAILED;
        }
    };
    let mut def = TaskDefinition::new(
        &uri(&options.task_name),
        &user_id(),
        &exe.display().to_string(),
    );
    def.delay_seconds = options.delay_secs;
    def.arguments = join_args(&options.args);
    if let Some(parent) = exe.parent() {
        def.working_directory = parent.display().to_string();
    }
    let document = xml::task_xml(&def);

    // Fichier temporaire propre au processus : deux `autostart enable` en parallèle
    // (les tests) ne se marchent pas dessus.
    let file = std::env::temp_dir().join(format!("conduitd-task-{}.xml", std::process::id()));
    if let Err(e) = std::fs::write(&file, xml::utf16le(&document)) {
        eprintln!("écriture de {} : {e}", file.display());
        return EXIT_FAILED;
    }
    let out = Command::new(schtasks())
        .args([
            "/Create",
            "/TN",
            &options.task_name,
            "/XML",
            &file.display().to_string(),
            "/F",
        ])
        .output();
    let _ = std::fs::remove_file(&file);
    match out {
        Err(e) => {
            eprintln!("{} : {e}", schtasks().display());
            EXIT_FAILED
        }
        Ok(out) if out.status.success() => {
            println!(
                "tâche planifiée « {} » enregistrée :\n  \
                 exécutable  : {}\n  \
                 déclencheur : ouverture de session de {}, après {} s\n  \
                 le démon démarrera à la prochaine ouverture de session (« Exécuter » \
                 dans le Planificateur de tâches pour le lancer tout de suite).",
                options.task_name,
                exe.display(),
                def.user_id,
                options.delay_secs
            );
            EXIT_OK
        }
        Ok(out) => {
            eprintln!(
                "l'enregistrement de la tâche « {} » a échoué :\n{}{}",
                options.task_name,
                decode(&out.stdout),
                decode(&out.stderr)
            );
            EXIT_FAILED
        }
    }
}

/// Supprime la tâche. Absente, c'est un succès : la désinstallation doit être idempotente.
pub fn disable(options: &Options) -> i32 {
    match query(&options.task_name) {
        Ok(out) if !out.status.success() => {
            println!(
                "tâche planifiée « {} » : absente, rien à supprimer.",
                options.task_name
            );
            return EXIT_OK;
        }
        Err(e) => {
            eprintln!("{} : {e}", schtasks().display());
            return EXIT_FAILED;
        }
        Ok(_) => {}
    }
    match Command::new(schtasks())
        .args(["/Delete", "/TN", &options.task_name, "/F"])
        .output()
    {
        Err(e) => {
            eprintln!("{} : {e}", schtasks().display());
            EXIT_FAILED
        }
        Ok(out) if out.status.success() => {
            println!(
                "tâche planifiée « {} » supprimée : le démon ne démarrera plus tout seul \
                 (`conduitd` à la main, ou la GUI, continuent de marcher).",
                options.task_name
            );
            EXIT_OK
        }
        Ok(out) => {
            eprintln!(
                "la suppression de la tâche « {} » a échoué :\n{}{}",
                options.task_name,
                decode(&out.stdout),
                decode(&out.stderr)
            );
            EXIT_FAILED
        }
    }
}

/// Colonnes de `schtasks /Query /FO CSV /V`, dans l'ordre où le programme les écrit.
/// Seules celles qui nous intéressent sont nommées.
mod column {
    /// Nom complet de la tâche.
    pub const TASK_NAME: usize = 1;
    /// Prochaine exécution prévue.
    pub const NEXT_RUN: usize = 2;
    /// État courant (« Prêt », « En cours d'exécution »…).
    pub const STATUS: usize = 3;
    /// Dernière exécution.
    pub const LAST_RUN: usize = 5;
    /// Code de retour de la dernière exécution.
    pub const LAST_RESULT: usize = 6;
    /// Commande exécutée.
    pub const TASK_TO_RUN: usize = 8;
    /// Activée ou désactivée.
    pub const STATE: usize = 11;
    /// Utilisateur.
    pub const RUN_AS: usize = 14;
}

/// Affiche l'état de la tâche.
pub fn status(options: &Options) -> i32 {
    let out = match query(&options.task_name) {
        Ok(out) => out,
        Err(e) => {
            eprintln!("{} : {e}", schtasks().display());
            return EXIT_FAILED;
        }
    };
    if !out.status.success() {
        println!(
            "tâche planifiée « {} » : absente.\n\
             `conduitd autostart enable` l'enregistre pour l'utilisateur courant.",
            options.task_name
        );
        let detail = format!("{}{}", decode(&out.stdout), decode(&out.stderr));
        let detail = detail.trim();
        if !detail.is_empty() {
            println!("(schtasks : {detail})");
        }
        return EXIT_ABSENT;
    }
    let text = decode(&out.stdout);
    let Some(row) = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('"'))
        .map(super::csv_fields)
    else {
        eprintln!(
            "sortie de `schtasks /Query` incompréhensible :\n{}",
            text.trim()
        );
        return EXIT_FAILED;
    };
    let field = |i: usize| row.get(i).map(String::as_str).unwrap_or("?");
    println!("tâche planifiée « {} » : présente.", options.task_name);
    println!("  nom complet          : {}", field(column::TASK_NAME));
    println!("  état                 : {}", field(column::STATE));
    println!("  statut               : {}", field(column::STATUS));
    println!("  prochaine exécution  : {}", field(column::NEXT_RUN));
    println!("  dernière exécution   : {}", field(column::LAST_RUN));
    println!("  dernier résultat     : {}", field(column::LAST_RESULT));
    println!("  commande             : {}", field(column::TASK_TO_RUN));
    println!("  exécutée en tant que : {}", field(column::RUN_AS));
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_gets_a_leading_backslash_exactly_once() {
        assert_eq!(uri(r"Conduit\conduitd"), r"\Conduit\conduitd");
        assert_eq!(uri(r"\Conduit\conduitd"), r"\Conduit\conduitd");
    }

    #[test]
    fn arguments_with_spaces_are_quoted() {
        let args = vec![
            "--backend".to_string(),
            "null".to_string(),
            r"--root=C:\a b".to_string(),
        ];
        assert_eq!(join_args(&args), r#"--backend null "--root=C:\a b""#);
        assert_eq!(join_args(&[]), "");
    }

    #[test]
    fn schtasks_is_taken_from_system32() {
        let p = schtasks();
        assert!(p.is_absolute(), "{}", p.display());
        assert!(p.ends_with(r"System32\schtasks.exe"), "{}", p.display());
        assert!(p.exists(), "{} devrait exister sous Windows", p.display());
    }

    #[test]
    fn invalid_utf8_output_is_decoded_without_panicking() {
        assert_eq!(decode(b"Pr\xC3\xAAt"), "Prêt");
        assert!(!decode(b"Pr\x88t").is_empty());
    }
}
