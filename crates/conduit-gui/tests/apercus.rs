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
use std::time::Duration;

use conduit_backend::{CableId, CableInfo, DeviceDirection, DeviceId, DeviceInfo};
use conduit_core::graph::{Direction, LinkId, LinkInfo, NodeId, PortId};
use conduit_core::node::PortSpec;
use conduit_core::types::{ChannelCount, Db, Quantum, SampleRate};
use conduit_protocol::api::NodeKey;
use conduit_protocol::{
    DeviceStatus, DriverChoice, DriverStatus, EngineStatus, LinkDescriptor, NodeDescriptor,
    NodeState, TimingSnapshot,
};

use conduit_gui::app::{App, Message};
use conduit_gui::ipc;
use conduit_gui::model::Snapshot;
use conduit_gui::patchbay::Geste;
use conduit_gui::preferences::Preferences;
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

/// L'état d'un périphérique ouvert par le moteur, tel que `Status` le décrit.
///
/// Les valeurs sont celles que le backend `null` produit en marche : un
/// remplissage de tampon de l'ordre du millier de trames et un ratio de
/// rééchantillonnage à quelques millionièmes de 1.
fn etat_de_peripherique(
    node: u32,
    nom: &str,
    etat: NodeState,
    underruns: u64,
    overruns: u64,
    fill: u32,
    ratio: f64,
) -> DeviceStatus {
    DeviceStatus {
        id: DeviceId::new(format!("null:{}", nom.to_lowercase().replace(' ', "-"))),
        node: NodeId::new(node, 0),
        state: etat,
        underruns,
        overruns,
        fill,
        ratio,
        locked: etat != NodeState::Suspended,
    }
}

/// Un lien du premier port de sortie du nœud `src` à la première entrée de
/// `dst`.
fn lien(index: u32, src: u32, dst: u32) -> LinkDescriptor {
    LinkDescriptor {
        link: LinkInfo {
            id: LinkId::new(index, 0),
            src: PortId::new(NodeId::new(src, 0), Direction::Output, 0),
            dst: PortId::new(NodeId::new(dst, 0), Direction::Input, 0),
        },
        gain_db: Db::UNITY,
        muted: false,
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
            links: 4,
            // Trois heures de marche à 256 trames et 48 kHz : le budget d'un
            // cycle vaut 5 333 µs, et le moteur en consomme le tiers.
            timing: TimingSnapshot {
                count: 2_025_000,
                min_ns: 511_000,
                avg_ns: 1_854_000,
                max_ns: 3_704_000,
                last_ns: 1_792_000,
                budget_ns: 5_333_000,
                overruns: 5,
            },
            xruns: 7,
            // Les périphériques que le moteur a ouverts : le pilote du graphe,
            // les deux côtés des deux câbles, et une interface suspendue —
            // celle-ci pour voir, dans la colonne « Latence », le tiret
            // cadratin d'un nœud qui ne tourne pas, et une barre de tampon
            // vide.
            devices: vec![
                etat_de_peripherique(5, "Haut-parleurs", NodeState::Driver, 0, 0, 831, 1.0),
                etat_de_peripherique(2, "Conduit 1", NodeState::Active, 3, 0, 960, 1.000_018),
                etat_de_peripherique(3, "Conduit 2", NodeState::Active, 0, 1, 892, 0.999_821),
                etat_de_peripherique(1, "Micro USB", NodeState::Active, 1, 0, 480, 1.000_204),
                etat_de_peripherique(6, "Interface Scarlett", NodeState::Suspended, 0, 0, 0, 1.0),
            ],
            position: 48_000 * 3_600 * 3,
            cycles: 2_025_000,
        },
        // Représentatif de ce qu'une carte peut être : les cinq étiquettes,
        // les trois colonnes de rôle, un nœud sans entrée et un sans sortie —
        // et, pour le pied de gain, un gain positif, un gain négatif, un nœud
        // au silence, un nœud coupé et un nœud suspendu, dont les réglages
        // sont désactivés.
        nodes: vec![
            NodeDescriptor {
                gain_db: Db::new(3.0),
                ..noeud(0, "Lecteur de musique", 0, 2)
            },
            peripherique(1, "Micro USB", 0, 1, NodeState::Active, None),
            NodeDescriptor {
                gain_db: Db::new(-6.0),
                ..peripherique(2, "Conduit 1", 2, 2, NodeState::Active, Some(1))
            },
            NodeDescriptor {
                muted: true,
                ..peripherique(3, "Conduit 2", 1, 1, NodeState::Active, Some(2))
            },
            NodeDescriptor {
                gain_db: Db::NEG_INF,
                ..noeud(4, "Visioconférence", 2, 2)
            },
            peripherique(5, "Haut-parleurs", 2, 0, NodeState::Driver, None),
            peripherique(6, "Interface Scarlett", 2, 2, NodeState::Suspended, None),
        ],
        // Les deux trajets que la maquette montre : la musique qui sort par
        // les haut-parleurs, le micro qui entre en visioconférence. Le premier
        // porte un gain, que l'aperçu « patchbay-lien » montre dans l'en-tête.
        links: vec![
            lien(0, 0, 2),
            LinkDescriptor {
                gain_db: Db::new(-4.5),
                ..lien(1, 2, 5)
            },
            lien(2, 1, 3),
            lien(3, 3, 4),
        ],
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
    rendre_avec(nom, onglet, theme, true, messages);
}

/// Comme [`rendre`], mais `charge` à faux laisse l'application sans miroir :
/// le démon n'a jamais répondu, et c'est l'écran d'état qui s'affiche.
fn rendre_avec(nom: &str, onglet: Tab, theme: &iced::Theme, charge: bool, messages: Vec<Message>) {
    let mut app = App::new(PathBuf::from("/tmp/conduitd.sock"));
    let (requester, _commandes) = ipc::Requester::channel(8);
    app.apply_ipc(ipc::Event::Started(requester));
    if charge {
        app.apply_ipc(ipc::Event::Ready {
            server: "conduitd 0.1.0".into(),
            snapshot: Box::new(chargement()),
        });
    }
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
    // Le lien « Conduit 1 → Haut-parleurs » sélectionné : la courbe passe à
    // l'accent du mode, et l'en-tête montre « Supprimer le lien ».
    let selection = || vec![Message::Patchbay(Geste::Selection(Some(LinkId::new(1, 0))))];
    rendre("patchbay-lien", Tab::Patchbay, &theme::clair(), selection());
    rendre(
        "patchbay-lien-sombre",
        Tab::Patchbay,
        &theme::sombre(),
        selection(),
    );
    // L'écran « démon absent » : aucun `Ready`, une connexion perdue. Le
    // contenu et l'en-tête disparaissent, la barre latérale reste avec sa
    // pastille garance.
    let perdu = || {
        vec![Message::Ipc(ipc::Event::Lost {
            reason: "connexion perdue".into(),
            retry_in: Duration::from_secs(2),
        })]
    };
    rendre_avec("demon-absent", Tab::Cables, &theme::clair(), false, perdu());
    rendre_avec(
        "demon-absent-sombre",
        Tab::Cables,
        &theme::sombre(),
        false,
        perdu(),
    );
    // L'écran de premier lancement : miroir chargé, et des préférences qui
    // disent que l'accueil n'a jamais été acquitté.
    let premier = || vec![Message::Preferences(Box::<Preferences>::default())];
    rendre("accueil", Tab::Cables, &theme::clair(), premier());
    rendre("accueil-sombre", Tab::Cables, &theme::sombre(), premier());
}
