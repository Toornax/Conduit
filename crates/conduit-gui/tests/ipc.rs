//! Test bout en bout de la couche IPC de l'interface contre un démon `null`
//! en processus, **sans ouvrir de fenêtre** (M2-02).
//!
//! Il exerce le chemin complet : connexion, abonnement, chargement initial du
//! miroir, envoi d'une commande et mise à jour de l'état par la notification
//! reçue en retour (aucun optimisme local).

use std::time::Duration;

use conduit_backend::null::{NullBackend, NullDeviceSpec};
use conduit_backend::{CableId, CableSpec};
use conduit_core::types::ChannelCount;
use conduit_gui::ipc::{self, Event, Requester};
use conduit_gui::model::Mirror;
use conduit_protocol::{Command, Notification};
use conduitd::config::Config;
use conduitd::{Daemon, DaemonOptions};
use tokio::sync::mpsc;

/// Délai maximal d'attente d'un événement : large, mais borné.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Attend l'événement suivant, en échouant plutôt qu'en bloquant.
async fn next(rx: &mut mpsc::Receiver<Event>) -> Event {
    tokio::time::timeout(TIMEOUT, rx.recv())
        .await
        .expect("aucun événement reçu à temps")
        .expect("la couche IPC s'est arrêtée")
}

/// Consomme les événements jusqu'au chargement initial, en alimentant `mirror`.
async fn wait_ready(rx: &mut mpsc::Receiver<Event>, mirror: &mut Mirror) -> Requester {
    let mut requester = None;
    loop {
        match next(rx).await {
            Event::Started(r) => requester = Some(r),
            Event::Connecting => {}
            Event::Ready { server, snapshot } => {
                assert!(server.contains("conduitd"), "démon inattendu : {server}");
                mirror.reset(*snapshot);
                return requester.expect("la poignée d'envoi précède la connexion");
            }
            Event::Notified(n) => mirror.apply(&n),
            other => panic!("événement inattendu avant le chargement : {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mirror_loads_then_follows_the_cable_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let null = NullBackend::new();
    null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
    null.add_device(NullDeviceSpec::capture("Micro").layout(1, 240));
    let daemon = Daemon::spawn(
        Box::new(null.clone()),
        DaemonOptions::under(dir.path(), Config::with_default_cables()),
    )
    .unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let task = tokio::spawn(ipc::run(daemon.socket.clone(), tx));

    // 1. Chargement initial : le miroir connaît le moteur, les nœuds et les
    //    deux câbles créés par la configuration par défaut.
    let mut mirror = Mirror::default();
    let requester = wait_ready(&mut rx, &mut mirror).await;
    assert!(mirror.is_loaded());
    assert_eq!(mirror.status.as_ref().unwrap().backend, "null");
    assert!(
        mirror.nodes.iter().any(|n| n.label == "Haut-parleurs"),
        "nœuds chargés : {:?}",
        mirror.nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
    );
    assert_eq!(mirror.cables.len(), 2, "câbles : {:?}", mirror.cables);

    // 2. Une commande envoyée par l'interface : le miroir ne change qu'à la
    //    réception de la notification.
    requester.send(Command::CableAdd {
        spec: CableSpec {
            name: Some("Musique".into()),
            channels: ChannelCount::new(4).unwrap(),
        },
    });
    let added = wait_cable_changed(&mut rx, &mut mirror, 3).await;
    assert_eq!(added.name, "Musique");
    assert_eq!(added.channels, ChannelCount::new(4).unwrap());
    assert_eq!(mirror.cables.len(), 3);

    // 3. Renommage puis suppression, toujours par notification.
    requester.send(Command::CableRename {
        id: added.id,
        name: "Jeu".into(),
    });
    let renamed = wait_cable_changed(&mut rx, &mut mirror, 3).await;
    assert_eq!(renamed.name, "Jeu");

    requester.send(Command::CableRemove { id: added.id });
    let removed_id = added.id;
    loop {
        match next(&mut rx).await {
            Event::Notified(n) => {
                mirror.apply(&n);
                if matches!(*n, Notification::CableChanged { id, info: None } if id == removed_id) {
                    break;
                }
            }
            Event::Failed(message) => panic!("commande refusée : {message}"),
            other => panic!("événement inattendu : {other:?}"),
        }
    }
    assert!(mirror.cable(removed_id).is_none());
    assert_eq!(mirror.cables.len(), 2);

    task.abort();
    daemon.shutdown().await;
}

/// Attend un `CableChanged` non nul, en appliquant tout au miroir.
async fn wait_cable_changed(
    rx: &mut mpsc::Receiver<Event>,
    mirror: &mut Mirror,
    expected: usize,
) -> conduit_backend::CableInfo {
    loop {
        match next(rx).await {
            Event::Notified(n) => {
                mirror.apply(&n);
                if let Notification::CableChanged {
                    id,
                    info: Some(info),
                } = *n
                {
                    assert_eq!(mirror.cables.len(), expected);
                    assert_eq!(mirror.cable(id).map(|c| &c.name), Some(&info.name));
                    return info;
                }
            }
            Event::Failed(message) => panic!("commande refusée par le démon : {message}"),
            other => panic!("événement inattendu : {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_command_is_reported_without_breaking_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let null = NullBackend::new();
    null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
    let daemon = Daemon::spawn(
        Box::new(null.clone()),
        DaemonOptions::under(dir.path(), Config::default()),
    )
    .unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let task = tokio::spawn(ipc::run(daemon.socket.clone(), tx));
    let mut mirror = Mirror::default();
    let requester = wait_ready(&mut rx, &mut mirror).await;

    // Câble inexistant : le démon refuse, la session continue.
    requester.send(Command::CableRemove { id: CableId(42) });
    let message = loop {
        match next(&mut rx).await {
            Event::Failed(message) => break message,
            Event::Notified(n) => mirror.apply(&n),
            other => panic!("événement inattendu : {other:?}"),
        }
    };
    assert!(
        !message.is_empty(),
        "le message d'erreur doit être affichable"
    );

    // La session répond toujours : un ajout aboutit ensuite.
    requester.send(Command::CableAdd {
        spec: CableSpec::default(),
    });
    let added = wait_cable_changed(&mut rx, &mut mirror, 1).await;
    assert_eq!(added.channels, ChannelCount::STEREO);

    task.abort();
    daemon.shutdown().await;
}
