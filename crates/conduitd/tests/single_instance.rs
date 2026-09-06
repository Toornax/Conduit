//! Instance unique (M1b-35, ADR-013) : un démon seul démarre, un second sur la même
//! racine sort en 3 avec un message qui dit quoi faire.

use std::process::{Child, Command as Process, Stdio};
use std::time::Duration;

use conduit_protocol::client::Client;
use conduit_protocol::{Command, Reply};

/// Lance un démon sur `root` (backend simulé, sans watchdog ni persistance).
fn spawn_daemon(root: &std::path::Path) -> Child {
    Process::new(env!("CARGO_BIN_EXE_conduitd"))
        .args([
            "--backend",
            "null",
            "--root",
            root.to_str().unwrap(),
            "--no-watchdog",
            "--no-persist",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de conduitd")
}

/// Attend que le démon ouvre son point de contrôle (5 s au plus).
async fn connect(socket: &std::path::Path) -> Client {
    for _ in 0..100 {
        if let Ok(c) = Client::connect(socket, "test").await {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("le démon n'a pas ouvert son point de contrôle en 5 s");
}

/// Le premier démon sert, le second refuse de démarrer.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_on_the_same_root_exits_with_code_3() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("conduitd.sock");
    let mut first = spawn_daemon(dir.path());

    // Un démon seul démarre normalement et répond.
    let mut client = connect(&socket).await;
    let Reply::Status(status) = client.call(Command::Status).await.unwrap() else {
        panic!("réponse inattendue à Status")
    };
    assert_eq!(status.backend, "null");
    drop(client);

    // Le second voit le point de contrôle occupé.
    let second = Process::new(env!("CARGO_BIN_EXE_conduitd"))
        .args([
            "--backend",
            "null",
            "--root",
            dir.path().to_str().unwrap(),
            "--no-watchdog",
            "--no-persist",
        ])
        .output()
        .expect("lancement du second conduitd");
    let err = String::from_utf8_lossy(&second.stderr);
    assert_eq!(second.status.code(), Some(3), "sortie : {err}");
    assert!(
        err.contains("déjà en cours") && err.contains("conduitctl"),
        "message attendu absent : {err}"
    );

    // Le premier est toujours là après la tentative.
    let mut client = connect(&socket).await;
    assert!(matches!(
        client.call(Command::Status).await.unwrap(),
        Reply::Status(_)
    ));
    drop(client);

    first.kill().unwrap();
    first.wait().unwrap();
}

/// Deux racines différentes : deux démons cohabitent (un pipe et un socket par racine).
#[tokio::test(flavor = "multi_thread")]
async fn two_roots_give_two_independent_daemons() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut first = spawn_daemon(a.path());
    let mut second = spawn_daemon(b.path());
    let _ = connect(&a.path().join("conduitd.sock")).await;
    let _ = connect(&b.path().join("conduitd.sock")).await;
    for child in [&mut first, &mut second] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
