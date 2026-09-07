//! Le binaire `conduit-looptest` sur la machine de développement (M1a-10).
//!
//! Aucun de ces tests ne demande le pilote Conduit : `--list` ne fait
//! qu'énumérer, `--self-test` n'ouvre aucun périphérique. La vraie boucle
//! (`conduit-looptest --repeat 10`) se lance à la main dans la VM, une fois le
//! pilote installé.
//!
//! Une exception : [`echo_du_rendu_par_defaut_entend_le_sinus`] **émet un son**,
//! très bas (2 % d'amplitude) et bref — c'est le seul moyen de prouver que le mode
//! `--loopback` prélève bien le mélange du moteur. Il se saute s'il n'y a pas
//! d'endpoint de rendu par défaut.

#![cfg(windows)]

use std::process::Command;

use conduit_backend::{Backend, DeviceDirection};
use conduit_backend_wasapi::WasapiBackend;

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

/// `--list --show-volume` ne change **rien** : il ne fait que lire.
#[test]
fn liste_les_volumes_sans_rien_changer() {
    let (code, out, err) = run(&["--list", "--show-volume"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("endpoint(s) actif(s)"), "{out}");
    // Sur une machine sans aucune carte, il n'y a rien à décrire : on n'exige les
    // lignes de volume et de plage que s'il y a au moins un endpoint.
    if out.contains("      id ") {
        assert!(out.contains("      volume "), "{out}");
        // La plage se relève toujours : soit elle s'affiche, soit l'outil dit
        // pourquoi il n'a pas pu — mais elle ne disparaît jamais en silence.
        assert!(out.contains("      plage"), "{out}");
    }
}

/// Aucun test ne doit **écrire** un volume sur la machine qui lance la suite : les
/// réglages ne sont éprouvés que par leurs refus. L'écriture est testée là où elle
/// peut être restaurée, dans `conduit-backend-wasapi/tests/volume.rs`.
#[test]
fn un_volume_hors_bornes_donne_le_code_environnement() {
    let (code, _, err) = run(&["--set-volume=1.5"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--set-volume"), "{err}");
}

#[test]
fn regler_le_volume_sans_endpoint_est_refuse() {
    let (code, _, err) = run(&["--self-test", "--unmute"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--self-test"), "{err}");
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

#[test]
fn echo_et_capture_se_contredisent() {
    let (code, _, err) = run(&["--loopback", "--capture", "Conduit 1"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--capture"), "{err}");
}

/// **Émet un son** : 1,5 s de sinus à 2 % d'amplitude sur le rendu par défaut.
///
/// Le verdict de la passe n'est pas ce qui est vérifié — l'écho prélève *tout* le
/// mélange, donc aussi ce que jouent les autres applications, ce qui peut faire
/// échouer la continuité de phase ou l'amplitude sans rien dire du pilote. Ce qui
/// est vérifié, c'est la seule chose que le mode promet : le sinus a été
/// **retrouvé** dans l'écho, et l'outil en tire la bonne conclusion.
#[test]
#[ignore = "émet un son sur le rendu par défaut : à lancer à la main (cargo test … -- --ignored)"]
fn echo_du_rendu_par_defaut_entend_le_sinus() {
    let backend = match WasapiBackend::new() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("test sauté : backend WASAPI indisponible ({e})");
            return;
        }
    };
    let Some(id) = backend.default_device(DeviceDirection::Render) else {
        eprintln!("test sauté : aucun périphérique de rendu par défaut");
        return;
    };
    drop(backend);

    let (code, out, err) = run(&[
        "--render",
        id.as_str(),
        "--loopback",
        "--seconds",
        "1.5",
        "--amplitude",
        "0.02",
    ]);
    assert_ne!(code, Some(2), "l'écho n'a pas pu être mesuré :\n{out}{err}");
    assert!(out.contains("écho de ce même endpoint"), "{out}");
    assert!(
        out.contains("le moteur audio de Windows délivre bien"),
        "le sinus n'a pas été retrouvé dans l'écho :\n{out}{err}"
    );
}
