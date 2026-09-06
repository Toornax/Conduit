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

use conduit_backend::{CableId, CableInfo, DeviceDirection, DeviceId, DeviceInfo};
use conduit_core::graph::NodeId;
use conduit_core::node::PortSpec;
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

/// Un câble de démonstration : `nom` vide montre l'invite « Sans alias ».
fn cable(id: u32, nom: &str, canaux: u8, actif: bool) -> CableInfo {
    CableInfo {
        id: CableId(id),
        name: nom.into(),
        channels: ChannelCount::new(canaux).expect("canaux hors bornes"),
        active: actif,
        render: DeviceId::new(format!("null:cable{id}:render")),
        capture: DeviceId::new(format!("null:cable{id}:capture")),
    }
}

/// Un nœud interne de démonstration : ni périphérique, ni câble.
fn noeud(index: u32, nom: &str, entrees: usize, sorties: usize) -> NodeDescriptor {
    NodeDescriptor {
        id: NodeId::new(index, 0),
        key: NodeKey::Internal { name: nom.into() },
        label: nom.into(),
        type_name: "null".into(),
        inputs: PortSpec::layout(entrees),
        outputs: PortSpec::layout(sorties),
        state: NodeState::Internal,
        gain_db: Db::UNITY,
        muted: false,
        device: None,
    }
}

/// Un nœud adossé à un périphérique du système ; `cable` en fait un côté d'un
/// câble Conduit.
fn peripherique(
    index: u32,
    nom: &str,
    entrees: usize,
    sorties: usize,
    etat: NodeState,
    cable: Option<u32>,
) -> NodeDescriptor {
    let id = DeviceId::new(format!("null:{}", nom.to_lowercase().replace(' ', "-")));
    NodeDescriptor {
        key: NodeKey::Device {
            backend: "null".into(),
            id: id.clone(),
        },
        state: etat,
        device: Some(DeviceInfo {
            id,
            name: nom.into(),
            direction: if entrees == 0 {
                DeviceDirection::Capture
            } else {
                DeviceDirection::Render
            },
            channels: entrees.max(sorties),
            sample_rate: SampleRate::HZ_48000,
            sample_rates: vec![],
            default_block: 480,
            is_default: false,
            cable: cable.map(CableId),
        }),
        ..noeud(index, nom, entrees, sorties)
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
            nodes: 7,
            links: 0,
            timing: TimingSnapshot::default(),
            xruns: 0,
            devices: vec![],
            position: 48_000 * 3_600 * 3,
            cycles: 0,
        },
        // Représentatif de ce qu'une carte peut être : les cinq étiquettes,
        // les trois colonnes de rôle, un nœud sans entrée et un sans sortie.
        nodes: vec![
            noeud(0, "Lecteur de musique", 0, 2),
            peripherique(1, "Micro USB", 0, 1, NodeState::Active, None),
            peripherique(2, "Conduit 1", 2, 2, NodeState::Active, Some(1)),
            peripherique(3, "Conduit 2", 1, 1, NodeState::Active, Some(2)),
            noeud(4, "Visioconférence", 2, 2),
            peripherique(5, "Haut-parleurs", 2, 0, NodeState::Driver, None),
            peripherique(6, "Interface Scarlett", 2, 2, NodeState::Suspended, None),
        ],
        links: vec![],
        // Représentatif des états qu'une ligne peut prendre : alias donné ou
        // absent, canaux au-delà de la stéréo, câble inactif.
        cables: vec![
            cable(1, "Musique", 2, true),
            cable(2, "Micro traité", 1, true),
            cable(3, "", 2, true),
            cable(4, "Multipiste", 6, false),
        ],
    }
}

/// Dessine une vue dans un mode donné et l'écrit dans `target/apercus/`.
///
/// `messages` amène l'application dans l'état à regarder — une confirmation
/// de suppression ouverte, par exemple.
fn rendre(nom: &str, onglet: Tab, theme: &iced::Theme, messages: Vec<Message>) {
    let mut app = App::new(PathBuf::from("/tmp/conduitd.sock"));
    let (requester, _commandes) = ipc::Requester::channel(8);
    app.apply_ipc(ipc::Event::Started(requester));
    app.apply_ipc(ipc::Event::Ready {
        server: "conduitd 0.1.0".into(),
        snapshot: Box::new(chargement()),
    });
    let _ = app.update(Message::Tab(onglet));
    for message in messages {
        let _ = app.update(message);
    }

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
        rendre(nom, onglet, &theme::clair(), vec![]);
        rendre(&format!("{nom}-sombre"), onglet, &theme::sombre(), vec![]);
    }
    // La ligne dont la suppression attend confirmation.
    let confirmation = || vec![Message::AskRemove(CableId(2))];
    rendre(
        "cables-confirmation",
        Tab::Cables,
        &theme::clair(),
        confirmation(),
    );
    rendre(
        "cables-confirmation-sombre",
        Tab::Cables,
        &theme::sombre(),
        confirmation(),
    );
}
