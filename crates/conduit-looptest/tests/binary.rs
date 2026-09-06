//! Le binaire `conduit-looptest` sur la machine de développement (M1a-10).
//!
//! Aucun de ces tests ne demande le pilote Conduit ni n'émet de son : `--list`
//! ne fait qu'énumérer, `--self-test` n'ouvre aucun périphérique. La vraie boucle
//! (`conduit-looptest --repeat 10`) se lance à la main dans la VM, une fois le
//! pilote installé.

#![cfg(windows)]

use std::process::Command;

/// Lance le binaire et rend (code de retour, stdout, stderr).
fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_conduit-looptest"))
        .args(args)
        .output()
        .expect("lancement de conduit-looptest");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn liste_les_endpoints() {
    let (code, out, err) = run(&["--list"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("endpoint(s) actif(s)"), "{out}");
}

#[test]
fn self_test_passe() {
    let (code, out, err) = run(&["--self-test", "--seconds", "1"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("passe 1/1"), "{out}");
    assert!(out.ends_with("\n"), "{out}");
    assert!(out.contains("→ OK"), "{out}");
    assert!(out.contains("résumé : 1/1 passe(s) OK"), "{out}");
}

#[test]
fn self_test_avec_glitch_echoue() {
    let (code, out, err) = run(&["--self-test", "--seconds", "1", "--inject-glitch"]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("→ ÉCHEC"), "{out}");
    assert!(out.contains("continuité"), "{out}");
}

#[test]
fn self_test_json_est_du_json() {
    let (code, out, err) = run(&["--self-test", "--seconds", "1", "--json", "--repeat", "2"]);
    assert_eq!(code, Some(0), "{out}{err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("JSON valide");
    assert_eq!(value["resume"]["verdict"], "ok");
    assert_eq!(value["resume"]["passes"], 2);
    assert_eq!(value["passes"].as_array().unwrap().len(), 2);
    assert!(value["passes"][0]["analysis"]["detected_hz"].is_number());
}

#[test]
fn endpoint_absent_donne_le_code_environnement() {
    let (code, _, err) = run(&[
        "--render",
        "endpoint-qui-n-existe-pas",
        "--capture",
        "endpoint-qui-n-existe-pas",
    ]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--list"), "{err}");
}

#[test]
fn options_incoherentes_donnent_le_code_environnement() {
    let (code, _, err) = run(&["--self-test", "--amplitude", "3"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--amplitude"), "{err}");
}
