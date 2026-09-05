//! Le binaire `conduitd` démarre, répond sur son socket et s'arrête proprement (M0-95).

use std::process::{Command as Process, Stdio};
use std::time::Duration;

use conduit_protocol::client::Client;
use conduit_protocol::{Command, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn conduitd_binary_serves_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("conduitd.sock");
    std::fs::write(
        dir.path().join("conduit.toml"),
        "[[cable]]\nid = 1\nalias = \"Musique\"\n[log]\nfile = false\n",
    )
    .unwrap();
    let mut child = Process::new(env!("CARGO_BIN_EXE_conduitd"))
        .args([
            "--backend",
            "null",
            "--root",
            dir.path().to_str().unwrap(),
            "--config",
            dir.path().join("conduit.toml").to_str().unwrap(),
            "--socket",
            socket.to_str().unwrap(),
            "--no-watchdog",
            "--log-level",
            "debug",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de conduitd");
    // Attend le socket.
    let mut client = None;
    for _ in 0..100 {
        if let Ok(c) = Client::connect(&socket, "test").await {
            client = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut client = client.expect("le démon n'a pas ouvert son socket en 5 s");
    let Reply::Status(status) = client.call(Command::Status).await.unwrap() else {
        panic!()
    };
    assert_eq!(status.backend, "null");
    let Reply::Cables { cables } = client.call(Command::CableList).await.unwrap() else {
        panic!()
    };
    assert_eq!(cables.len(), 1);
    assert_eq!(cables[0].name, "Musique");
    assert!(dir.path().join("data").is_dir());
    // Arrêt propre par signal.
    #[cfg(unix)]
    {
        let pid = child.id();
        let killed = Process::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        let status = child.wait().unwrap();
        assert!(status.success(), "sortie {status:?}");
        assert!(!socket.exists(), "le socket est retiré à l'arrêt");
        assert!(
            dir.path().join("data/state.json").exists(),
            "état sauvegardé à l'arrêt"
        );
    }
    #[cfg(not(unix))]
    {
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[test]
fn conduitd_binary_rejects_bad_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.toml"), "[engine]\nquantum = 300\n").unwrap();
    let out = Process::new(env!("CARGO_BIN_EXE_conduitd"))
        .args([
            "--backend",
            "null",
            "--root",
            dir.path().to_str().unwrap(),
            "--config",
            dir.path().join("bad.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("puissance de deux"), "{err}");
    let out = Process::new(env!("CARGO_BIN_EXE_conduitd"))
        .args(["--backend", "martien"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
