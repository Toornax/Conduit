//! Tests bout en bout : démon null + client IPC (M0-82 à M0-88).

use std::time::Duration;

use conduit_backend::null::{NullBackend, NullDeviceSpec};
use conduit_core::graph::Direction;
use conduit_core::graph::PortId;
use conduit_protocol::client::Client;
use conduit_protocol::framing::{encode_message, Decoder};
use conduit_protocol::wire::{Hello, Message};
use conduit_protocol::{Command, DriverStatus, InternalKind, NodeState, Notification, Reply};
use conduitd::config::Config;
use conduitd::{Daemon, DaemonOptions};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn null_with_speakers() -> NullBackend {
    let null = NullBackend::new();
    null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
    null
}

async fn spawn(
    root: &std::path::Path,
    null: &NullBackend,
    config: Config,
    watchdog: bool,
) -> Daemon {
    let mut opts = DaemonOptions::under(root, config);
    opts.watchdog = watchdog;
    Daemon::spawn(Box::new(null.clone()), opts).expect("démon")
}

async fn client(d: &Daemon, name: &str) -> Client {
    Client::connect(&d.socket, name).await.expect("connexion")
}

/// Connexion brute (sans `Hello`) pour envoyer des trames arbitraires au démon :
/// socket Unix ou named pipe selon la plateforme.
#[cfg(unix)]
async fn raw_connect(path: &std::path::Path) -> impl AsyncRead + AsyncWrite + Unpin {
    tokio::net::UnixStream::connect(path).await.unwrap()
}

/// Même logique de nouvelle tentative sur `ERROR_PIPE_BUSY` (231) que `Client::connect`
/// (copie minimale : `connect_stream` n'est pas exposée par `conduit_protocol::client`).
#[cfg(windows)]
async fn raw_connect(path: &std::path::Path) -> impl AsyncRead + AsyncWrite + Unpin {
    use tokio::net::windows::named_pipe::ClientOptions;
    let name = conduit_protocol::client::pipe_name(path);
    for _ in 0..50 {
        match ClientOptions::new().open(&name) {
            Ok(s) => return s,
            Err(e) if e.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("connexion brute au pipe : {e}"),
        }
    }
    panic!("named pipe occupé");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_and_version_negotiation() {
    let dir = tempfile::tempdir().unwrap();
    let null = null_with_speakers();
    let d = spawn(dir.path(), &null, Config::default(), false).await;
    let mut a = client(&d, "a").await;
    let mut b = client(&d, "b").await;
    assert!(a.server().server.starts_with("conduitd"));
    let (ra, rb) = tokio::join!(a.call(Command::Status), b.call(Command::Status));
    let (Reply::Status(sa), Reply::Status(sb)) = (ra.unwrap(), rb.unwrap()) else {
        panic!()
    };
    assert_eq!(sa.backend, "null");
    assert_eq!(sa.nodes, sb.nodes);
    assert!(matches!(sa.driver, DriverStatus::Device { .. }));
    // Client trop récent : refusé avec conseil.
    let mut raw = raw_connect(&d.socket).await;
    raw.write_all(
        &encode_message(&Message::Hello(Hello {
            version: 99,
            client: "futur".into(),
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let mut dec = Decoder::new();
    let mut buf = [0u8; 1024];
    let reply = loop {
        if let Some(m) = dec.next_message::<Message>().unwrap() {
            break m;
        }
        let n = raw.read(&mut buf).await.unwrap();
        assert!(n > 0);
        dec.feed(&buf[..n]);
    };
    let Message::HelloReply(r) = reply else {
        panic!("{reply:?}")
    };
    assert!(!r.accepted);
    assert!(r.reason.unwrap().contains("mettez à jour"));
    // Trame corrompue : le démon déconnecte, sans paniquer.
    let mut bad = raw_connect(&d.socket).await;
    bad.write_all(&[0xff, 0xff, 0xff, 0x7f, 1, 2, 3])
        .await
        .unwrap();
    let mut tmp = [0u8; 16];
    let closed = tokio::time::timeout(Duration::from_secs(2), bad.read(&mut tmp)).await;
    assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))), "{closed:?}");
    // Le démon fonctionne toujours.
    assert!(a.call(Command::Nodes).await.is_ok());
    d.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn events_reach_subscribers_only() {
    let dir = tempfile::tempdir().unwrap();
    let null = null_with_speakers();
    let d = spawn(dir.path(), &null, Config::default(), false).await;
    let mut sub = client(&d, "abonné").await;
    let mut other = client(&d, "muet").await;
    sub.subscribe().await.unwrap();
    let Reply::Node(n) = other
        .call(Command::AddInternal {
            name: "gen".into(),
            kind: InternalKind::Sine {
                frequency: 440.0,
                amplitude: 0.5,
                channels: 2,
            },
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let ev = sub
        .next_event_timeout(Duration::from_secs(2))
        .await
        .unwrap()
        .expect("événement");
    assert!(
        matches!(&ev, Notification::NodeAdded(d) if d.id == n.id),
        "{ev:?}"
    );
    assert!(other
        .next_event_timeout(Duration::from_millis(200))
        .await
        .unwrap()
        .is_none());
    // Périphérique branché à chaud → NodeAdded chez l'abonné.
    null.add_device(NullDeviceSpec::capture("Micro").layout(1, 240));
    let mut got_added = false;
    for _ in 0..5 {
        if let Some(Notification::NodeAdded(desc)) = sub
            .next_event_timeout(Duration::from_secs(2))
            .await
            .unwrap()
        {
            if desc.label == "Micro" {
                got_added = true;
                break;
            }
        }
    }
    assert!(got_added);
    d.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn state_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let null = null_with_speakers();
    let spk = null.default_device_id();
    {
        let d = spawn(dir.path(), &null, Config::default(), false).await;
        let mut c = client(&d, "c").await;
        let Reply::Node(gen) = c
            .call(Command::AddInternal {
                name: "gen".into(),
                kind: InternalKind::Sine {
                    frequency: 1000.0,
                    amplitude: 0.25,
                    channels: 2,
                },
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        let Reply::Nodes { nodes } = c.call(Command::Nodes).await.unwrap() else {
            panic!()
        };
        let spk_node = nodes
            .iter()
            .find(|n| n.device.as_ref().map(|d| &d.id) == Some(&spk))
            .unwrap();
        c.call(Command::LinkByName {
            src_node: gen.id,
            src_port: "FL".into(),
            dst_node: spk_node.id,
            dst_port: "FL".into(),
        })
        .await
        .unwrap();
        c.call(Command::SetParam {
            node: gen.id,
            name: "frequency".into(),
            value: 1234.0,
        })
        .await
        .unwrap();
        c.call(Command::Save).await.unwrap();
        d.shutdown().await;
    }
    assert!(dir.path().join("data/state.json").exists());
    let null2 = null_with_speakers();
    let d = spawn(dir.path(), &null2, Config::default(), false).await;
    let mut c = client(&d, "c2").await;
    let Reply::Links { links } = c.call(Command::Links).await.unwrap() else {
        panic!()
    };
    assert_eq!(links.len(), 1, "lien restauré (F-30)");
    let Reply::Nodes { nodes } = c.call(Command::Nodes).await.unwrap() else {
        panic!()
    };
    assert!(nodes
        .iter()
        .any(|n| n.label == "gen" && n.type_name == "sine"));
    let Reply::Dump { text } = c.call(Command::Dump).await.unwrap() else {
        panic!()
    };
    assert!(text.contains("liens (1)"));
    assert!(
        !text.contains(&dir.path().display().to_string()),
        "aucun chemin utilisateur dans le rapport"
    );
    assert!(!text.contains(&std::env::var("USER").unwrap_or_else(|_| "\u{0}".into())));
    // Load recharge sans dupliquer.
    c.call(Command::Load).await.unwrap();
    let Reply::Links { links } = c.call(Command::Links).await.unwrap() else {
        panic!()
    };
    assert_eq!(links.len(), 1);
    d.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn autoconnect_rule_applies_to_new_device_and_pending_links() {
    let dir = tempfile::tempdir().unwrap();
    let null = null_with_speakers();
    let config = Config::parse(
        r#"
[[autoconnect]]
match  = { node = "Micro*", direction = "capture" }
target = { node = "Haut-parleurs", ports = ["FL", "FR"] }
"#,
    )
    .unwrap();
    let d = spawn(dir.path(), &null, config, false).await;
    let mut c = client(&d, "c").await;
    c.subscribe().await.unwrap();
    null.add_device(NullDeviceSpec::capture("Micro USB").layout(1, 240));
    let mut links_seen = 0;
    for _ in 0..10 {
        match c.next_event_timeout(Duration::from_secs(2)).await.unwrap() {
            Some(Notification::LinkAdded(_)) => {
                links_seen += 1;
                if links_seen == 1 {
                    break;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    let Reply::Links { links } = c.call(Command::Links).await.unwrap() else {
        panic!()
    };
    assert_eq!(
        links.len(),
        1,
        "mono → FL seulement (un port source) : {links:?}"
    );
    let Reply::Nodes { nodes } = c.call(Command::Nodes).await.unwrap() else {
        panic!()
    };
    let mic = nodes.iter().find(|n| n.label == "Micro USB").unwrap();
    assert_eq!(links[0].link.src, PortId::new(mic.id, Direction::Output, 0));
    d.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cables_from_config_are_created_with_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let null = NullBackend::new();
    let config = Config::parse(
        r#"
[[cable]]
id = 1
alias = "Musique"

[[cable]]
id = 2
channels = 1
"#,
    )
    .unwrap();
    let d = spawn(dir.path(), &null, config, false).await;
    let mut c = client(&d, "c").await;
    let Reply::Cables { cables } = c.call(Command::CableList).await.unwrap() else {
        panic!()
    };
    assert_eq!(cables.len(), 2);
    assert_eq!(cables[0].name, "Musique");
    assert_eq!(cables[1].channels.get(), 1);
    let Reply::Nodes { nodes } = c.call(Command::Nodes).await.unwrap() else {
        panic!()
    };
    assert_eq!(nodes.len(), 4);
    // Redémarrage avec la même config : pas de doublon.
    d.shutdown().await;
    let d = spawn(
        dir.path(),
        &null,
        Config::parse("[[cable]]\nid = 1\nalias = \"Jeu\"").unwrap(),
        false,
    )
    .await;
    let mut c = client(&d, "c").await;
    let Reply::Cables { cables } = c.call(Command::CableList).await.unwrap() else {
        panic!()
    };
    assert_eq!(cables.len(), 2);
    assert_eq!(cables[0].name, "Jeu", "alias mis à jour");
    d.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn watchdog_restarts_stalled_driver() {
    let dir = tempfile::tempdir().unwrap();
    // Backend null en mode manuel : le pilote ne produit jamais de cycle → blocage simulé.
    let null = null_with_speakers();
    let d = spawn(dir.path(), &null, Config::default(), true).await;
    let mut c = client(&d, "c").await;
    c.subscribe().await.unwrap();
    let mut restarted = false;
    for _ in 0..20 {
        match c.next_event_timeout(Duration::from_secs(3)).await.unwrap() {
            Some(Notification::DriverChanged(DriverStatus::Device { .. })) => {
                restarted = true;
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    assert!(restarted, "le watchdog doit redémarrer le pilote bloqué");
    let Reply::Status(s) = c.call(Command::Status).await.unwrap() else {
        panic!()
    };
    assert!(matches!(s.driver, DriverStatus::Device { .. }));
    d.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_carry_codes_and_shutdown_notifies() {
    let dir = tempfile::tempdir().unwrap();
    let null = null_with_speakers();
    let d = spawn(dir.path(), &null, Config::default(), false).await;
    let mut c = client(&d, "c").await;
    c.subscribe().await.unwrap();
    let err = c
        .request(Command::RemoveNode {
            node: conduit_core::graph::NodeId::new(99, 0),
        })
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, conduit_protocol::ErrorCode::NotFound);
    let Reply::Nodes { nodes } = c.call(Command::Nodes).await.unwrap() else {
        panic!()
    };
    assert!(nodes.iter().all(|n| n.state != NodeState::Suspended));
    d.shutdown().await;
    let last = c.next_event_timeout(Duration::from_secs(2)).await;
    assert!(
        matches!(last, Ok(Some(Notification::Shutdown)) | Err(_)),
        "{last:?}"
    );
}

trait DefaultDevice {
    fn default_device_id(&self) -> conduit_backend::DeviceId;
}

impl DefaultDevice for NullBackend {
    fn default_device_id(&self) -> conduit_backend::DeviceId {
        use conduit_backend::Backend;
        self.default_device(conduit_backend::DeviceDirection::Render)
            .unwrap()
    }
}
