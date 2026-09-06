//! Aperçus hors écran des vues, rendus en PNG (préparation de M2-14).
//!
//! `iced_test` sait construire une interface et la dessiner **sans fenêtre ni
//! serveur graphique** : c'est le seul moyen de regarder le rendu réel de la
//! GUI dans un environnement sans écran, et la base des captures de référence
//! de M2-14.
//!
//! Le test est marqué `#[ignore]` : il écrit des fichiers et sert à l'œil, pas
//! à la CI. Il se lance à la main, après avoir effacé les aperçus précédents —
//! `Snapshot::matches_image` ne réécrit pas un fichier qui existe déjà :
//!
//! ```text
//! rm -f target/apercus/*.png
//! cargo test -p conduit-gui --test apercus -- --ignored
//! ```
//!
//! Les images atterrissent dans `target/apercus/`, suffixées du nom du moteur
//! de rendu (`-wgpu` ou `-tiny-skia`). Le moteur se force par la variable
//! d'environnement `ICED_TEST_BACKEND`.

use std::path::PathBuf;

use conduit_backend::{CableId, CableInfo, DeviceId};
use conduit_core::graph::NodeId;
use conduit_core::types::{ChannelCount, Db, Quantum, SampleRate};
use conduit_protocol::api::NodeKey;
use conduit_protocol::{
    DriverChoice, DriverStatus, EngineStatus, NodeDescriptor, NodeState, TimingSnapshot,
};

use conduit_gui::app::{App, Message};
use conduit_gui::ipc;
use conduit_gui::model::Snapshot;
use conduit_gui::shell::Tab;
use conduit_gui::{theme, typo};

/// Taille de la fenêtre dessinée, celle de la maquette.
const FENETRE: (f32, f32) = (1120.0, 720.0);

/// Un câble de démonstration.
fn cable(id: u32, nom: &str) -> CableInfo {
    CableInfo {
        id: CableId(id),
        name: nom.into(),
        channels: ChannelCount::STEREO,
        active: true,
        render: DeviceId::new(format!("null:cable{id}:render")),
        capture: DeviceId::new(format!("null:cable{id}:capture")),
    }
}

/// Un nœud de démonstration.
fn noeud(index: u32, nom: &str, etat: NodeState) -> NodeDescriptor {
    NodeDescriptor {
        id: NodeId::new(index, 0),
        key: NodeKey::Internal { name: nom.into() },
        label: nom.into(),
        type_name: "null".into(),
        inputs: vec![],
        outputs: vec![],
        state: etat,
        gain_db: Db::UNITY,
        muted: false,
        device: None,
    }
}

/// Un chargement initial représentatif : le démon `null` avec ses deux câbles.
fn chargement() -> Snapshot {
    Snapshot {
        status: EngineStatus {
            backend: "null".into(),
            sample_rate: SampleRate::HZ_48000,
            quantum: Quantum::DEFAULT,
            driver: DriverStatus::Internal,
            driver_choice: DriverChoice::Auto,
            nodes: 4,
            links: 0,
            timing: TimingSnapshot::default(),
            xruns: 0,
            devices: vec![],
            position: 48_000 * 3_600 * 3,
            cycles: 0,
        },
        nodes: vec![
            noeud(0, "Conduit 1", NodeState::Active),
            noeud(1, "Conduit 2", NodeState::Active),
        ],
        links: vec![],
        cables: vec![cable(1, "Musique"), cable(2, "Micro traité")],
    }
}

/// Dessine une vue dans un mode donné et l'écrit dans `target/apercus/`.
fn rendre(nom: &str, onglet: Tab, theme: &iced::Theme) {
    let mut app = App::new(PathBuf::from("/tmp/conduitd.sock"));
    let (requester, _commandes) = ipc::Requester::channel(8);
    app.apply_ipc(ipc::Event::Started(requester));
    app.apply_ipc(ipc::Event::Ready {
        server: "conduitd 0.1.0".into(),
        snapshot: Box::new(chargement()),
    });
    let _ = app.update(Message::Tab(onglet));

    let reglages = iced_test::core::Settings {
        fonts: vec![
            typo::POLICE_INTER.into(),
            typo::POLICE_FRAUNCES.into(),
            typo::POLICE_SPECTRAL.into(),
            typo::POLICE_SPECTRAL_LIGHT.into(),
        ],
        default_font: iced::Font::with_name("Inter"),
        default_text_size: iced::Pixels(14.0),
        ..Default::default()
    };
    let mut ui = iced_test::Simulator::with_size(
        reglages,
        iced::Size::new(FENETRE.0, FENETRE.1),
        app.view(),
    );
    ui.snapshot(theme)
        .expect("le rendu hors écran doit aboutir")
        .matches_image(format!("../../target/apercus/{nom}"))
        .expect("l'aperçu doit pouvoir être écrit ou comparé");
}

#[test]
#[ignore = "écrit des images ; sert à l'œil, pas à la CI"]
fn apercus_des_vues() {
    for (nom, onglet) in [
        ("cables", Tab::Cables),
        ("patchbay", Tab::Patchbay),
        ("diagnostic", Tab::Diagnostic),
    ] {
        rendre(nom, onglet, &theme::clair());
        rendre(&format!("{nom}-sombre"), onglet, &theme::sombre());
    }
}
