//! Le binaire `conduitctl` contre un démon en processus (M0-95).

use std::process::Command as Process;

use conduit_backend::null::{NullBackend, NullDeviceSpec};
use conduitd::config::Config;
use conduitd::{Daemon, DaemonOptions};

#[tokio::test(flavor = "multi_thread")]
async fn conduitctl_binary_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let null = NullBackend::new();
    null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
    let daemon = Daemon::spawn(
        Box::new(null.clone()),
        DaemonOptions::under(dir.path(), Config::default()),
    )
    .unwrap();
    let socket = daemon.socket.to_str().unwrap().to_string();
    let run = |args: &[&str]| {
        let out = Process::new(env!("CARGO_BIN_EXE_conduitctl"))
            .arg("--socket")
            .arg(&socket)
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (code, out, _) = tokio::task::block_in_place(|| run(&["status"]));
    assert_eq!(code, Some(0));
    assert!(out.contains("backend null"), "{out}");
    let (code, out, _) =
        tokio::task::block_in_place(|| run(&["add", "sine", "gen", "--channels", "2"]));
    assert_eq!(code, Some(0), "{out}");
    let (code, _, _) = tokio::task::block_in_place(|| run(&["link", "gen:FL", "Haut-parleurs:FL"]));
    assert_eq!(code, Some(0));
    let (code, out, _) = tokio::task::block_in_place(|| run(&["--json", "links"]));
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["links"].as_array().unwrap().len(), 1);
    let (code, _, err) =
        tokio::task::block_in_place(|| run(&["link", "gen:FL", "Haut-parleurs:FL"]));
    assert_eq!(code, Some(1));
    assert!(err.contains("existe déjà"), "{err}");
    let (code, _, err) = tokio::task::block_in_place(|| run(&["volume"]));
    assert_eq!(code, Some(2));
    assert!(err.contains("Usage") || err.contains("usage"), "{err}");
    let (code, out, _) = tokio::task::block_in_place(|| run(&["--version"]));
    assert_eq!(
        code,
        Some(2),
        "clap --version passe par l'erreur d'analyse : {out}"
    );
    daemon.shutdown().await;
}
