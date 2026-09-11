//! Types de l'API : commandes, réponses, notifications, descripteurs.
//!
//! Tous les types sont `serde` ; les énumérations sont étiquetées (`cmd`, `reply`,
//! `event`, `kind`, …) pour rester lisibles en JSON et stables en MessagePack.

use conduit_backend::{CableFormat, CableId, CableInfo, CableSpec, DeviceId, DeviceInfo};
use conduit_core::graph::{LinkId, LinkInfo, NodeId, PortId};
use conduit_core::node::PortSpec;
use conduit_core::nodes::{EqBand, MeterReading};
use conduit_core::types::{ChannelCount, Db, Quantum, SampleRate};
use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

/// Clé stable d'un nœud (miroir de `conduit_engine::NodeKey`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeKey {
    /// Périphérique d'un backend.
    Device {
        /// Nom du backend.
        backend: String,
        /// Identifiant OS.
        id: DeviceId,
    },
    /// Nœud interne nommé.
    Internal {
        /// Nom unique.
        name: String,
    },
}

impl NodeKey {
    /// Clé de périphérique.
    pub fn device(backend: &str, id: DeviceId) -> Self {
        NodeKey::Device {
            backend: backend.to_string(),
            id,
        }
    }

    /// Clé interne.
    pub fn internal(name: impl Into<String>) -> Self {
        NodeKey::Internal { name: name.into() }
    }

    /// Identifiant de périphérique, si c'en est un.
    pub fn device_id(&self) -> Option<&DeviceId> {
        match self {
            NodeKey::Device { id, .. } => Some(id),
            NodeKey::Internal { .. } => None,
        }
    }
}

impl core::fmt::Display for NodeKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NodeKey::Device { backend, id } => write!(f, "{backend}:{id}"),
            NodeKey::Internal { name } => write!(f, "internal:{name}"),
        }
    }
}

/// Choix du pilote de graphe.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DriverChoice {
    /// Périphérique de rendu par défaut, sinon capture par défaut, sinon horloge interne.
    #[default]
    Auto,
    /// Horloge interne.
    Internal,
    /// Un périphérique précis.
    Device {
        /// Identifiant.
        id: DeviceId,
    },
}

/// État d'un nœud dans le moteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// Nœud interne, toujours actif.
    Internal,
    /// Périphérique ouvert en asynchrone.
    Active,
    /// Périphérique pilote du graphe.
    Driver,
    /// Périphérique absent ou fermé ; liens conservés.
    Suspended,
}

/// Description d'un nœud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct NodeDescriptor {
    /// Identifiant de graphe (peut changer si le nœud est recréé).
    pub id: NodeId,
    /// Clé stable.
    pub key: NodeKey,
    /// Nom d'affichage.
    pub label: String,
    /// Type (`"sine"`, `"device-render"`, …).
    pub type_name: String,
    /// Entrées.
    pub inputs: Vec<PortSpec>,
    /// Sorties.
    pub outputs: Vec<PortSpec>,
    /// État.
    pub state: NodeState,
    /// Gain (dB).
    pub gain_db: Db,
    /// Muet.
    pub muted: bool,
    /// Périphérique associé.
    pub device: Option<DeviceInfo>,
}

/// Description d'un lien.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct LinkDescriptor {
    /// Lien.
    #[serde(flatten)]
    pub link: LinkInfo,
    /// Gain (dB).
    pub gain_db: Db,
    /// Muet.
    pub muted: bool,
}

/// État d'un périphérique.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct DeviceStatus {
    /// Périphérique.
    pub id: DeviceId,
    /// Nœud.
    pub node: NodeId,
    /// État.
    pub state: NodeState,
    /// Sous-alimentations.
    pub underruns: u64,
    /// Débordements.
    pub overruns: u64,
    /// Remplissage du tampon (trames périphérique).
    pub fill: u32,
    /// Ratio de rééchantillonnage.
    pub ratio: f64,
    /// DLL verrouillée.
    pub locked: bool,
}

/// Pilote courant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DriverStatus {
    /// Aucun (moteur arrêté).
    None,
    /// Horloge interne.
    Internal,
    /// Périphérique.
    Device {
        /// Identifiant.
        id: DeviceId,
    },
}

/// Instantané des temps de cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct TimingSnapshot {
    /// Cycles mesurés.
    pub count: u64,
    /// Durée minimale (ns).
    pub min_ns: u64,
    /// Durée moyenne (ns).
    pub avg_ns: u64,
    /// Durée maximale (ns).
    pub max_ns: u64,
    /// Dernière durée (ns).
    pub last_ns: u64,
    /// Budget d'un cycle (ns).
    pub budget_ns: u64,
    /// Cycles au-delà du budget.
    pub overruns: u64,
}

/// État global du moteur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct EngineStatus {
    /// Backend.
    pub backend: String,
    /// Fréquence.
    pub sample_rate: SampleRate,
    /// Quantum.
    pub quantum: Quantum,
    /// Pilote effectif.
    pub driver: DriverStatus,
    /// Choix configuré.
    pub driver_choice: DriverChoice,
    /// Nœuds.
    pub nodes: usize,
    /// Liens.
    pub links: usize,
    /// Temps de cycle.
    pub timing: TimingSnapshot,
    /// Xruns cumulés.
    pub xruns: u64,
    /// Périphériques.
    pub devices: Vec<DeviceStatus>,
    /// Position du graphe (trames).
    pub position: u64,
    /// Cycles exécutés.
    pub cycles: u64,
}

/// Type de nœud interne à créer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InternalKind {
    /// Silence.
    Silence {
        /// Canaux.
        channels: usize,
    },
    /// Sinus.
    Sine {
        /// Fréquence (Hz).
        frequency: f32,
        /// Amplitude (crête).
        amplitude: f32,
        /// Canaux.
        channels: usize,
    },
    /// Bruit.
    Noise {
        /// Rose (`true`) ou blanc.
        pink: bool,
        /// Amplitude.
        amplitude: f32,
        /// Canaux.
        channels: usize,
    },
    /// Mixeur N bus → 1.
    Mixer {
        /// Bus.
        buses: usize,
        /// Canaux par bus.
        channels: usize,
    },
    /// Duplicateur 1 → N.
    Splitter {
        /// Canaux.
        channels: usize,
        /// Copies.
        copies: usize,
    },
    /// VU-mètre.
    Meter {
        /// Canaux.
        channels: usize,
    },
    /// Égaliseur paramétrique.
    Equalizer {
        /// Canaux.
        channels: usize,
        /// Bandes.
        bands: Vec<EqBand>,
    },
    /// Adaptation de canaux automatique.
    Adapter {
        /// Entrées.
        inputs: usize,
        /// Sorties.
        outputs: usize,
    },
}

/// Commande adressée au moteur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// État global.
    Status,
    /// Liste des nœuds.
    Nodes,
    /// Description d'un nœud et de ses ports.
    Ports {
        /// Nœud.
        node: NodeId,
    },
    /// Liste des liens.
    Links,
    /// Crée un lien.
    Link {
        /// Source (sortie).
        src: PortId,
        /// Destination (entrée).
        dst: PortId,
    },
    /// Crée un lien par noms de ports.
    LinkByName {
        /// Nœud source.
        src_node: NodeId,
        /// Port de sortie.
        src_port: String,
        /// Nœud destination.
        dst_node: NodeId,
        /// Port d'entrée.
        dst_port: String,
    },
    /// Supprime un lien.
    Unlink {
        /// Lien.
        link: LinkId,
    },
    /// Gain d'un nœud.
    SetNodeGain {
        /// Nœud.
        node: NodeId,
        /// Gain (dB), `None` = inchangé.
        gain_db: Option<Db>,
        /// Muet, `None` = inchangé.
        muted: Option<bool>,
    },
    /// Gain d'un lien.
    SetLinkGain {
        /// Lien.
        link: LinkId,
        /// Gain (dB).
        gain_db: Option<Db>,
        /// Muet.
        muted: Option<bool>,
    },
    /// Nom d'affichage d'un nœud.
    SetLabel {
        /// Nœud.
        node: NodeId,
        /// Nom.
        label: String,
    },
    /// Change le pilote.
    SetDriver {
        /// Choix.
        choice: DriverChoice,
    },
    /// Ajoute un nœud interne.
    AddInternal {
        /// Nom unique.
        name: String,
        /// Type.
        kind: InternalKind,
    },
    /// Retire un nœud (interne, ou périphérique absent).
    RemoveNode {
        /// Nœud.
        node: NodeId,
    },
    /// Règle un paramètre d'un nœud interne (`frequency`, `amplitude`,
    /// `band.<i>.{frequency,q,gain_db,enabled}`).
    SetParam {
        /// Nœud.
        node: NodeId,
        /// Nom.
        name: String,
        /// Valeur.
        value: f32,
    },
    /// Lit un VU-mètre.
    ReadMeter {
        /// Nœud.
        node: NodeId,
    },
    /// Liste les câbles.
    CableList,
    /// Crée un câble.
    CableAdd {
        /// Spécification.
        spec: CableSpec,
    },
    /// Supprime un câble.
    CableRemove {
        /// Câble.
        id: CableId,
    },
    /// Renomme un câble.
    CableRename {
        /// Câble.
        id: CableId,
        /// Nom.
        name: String,
    },
    /// Change les canaux d'un câble.
    CableSetChannels {
        /// Câble.
        id: CableId,
        /// Canaux.
        channels: ChannelCount,
    },
    /// Change le **format** d'un câble : fréquence, profondeur, canaux.
    ///
    /// Sous Windows, la seule commande qui change réellement les canaux d'un câble —
    /// `cable_set_channels` se fait refuser par le pilote dès que le compte demandé n'est
    /// pas celui du format configuré. Elle exige un câble **déconnecté** et coûte environ
    /// une seconde de silence sur les seize câbles : le format d'un endpoint est figé à sa
    /// création, et l'appliquer demande de redémarrer le périphérique du pilote.
    ///
    /// Ajoutée après [`PROTOCOL_VERSION`](crate::wire::PROTOCOL_VERSION) 1 sans
    /// l'incrémenter : c'est une **variante de plus** dans un énuméré étiqueté par `cmd`,
    /// qu'un client plus ancien n'émet jamais et dont il n'a donc rien à savoir.
    CableSetFormat {
        /// Câble.
        id: CableId,
        /// Format voulu.
        format: CableFormat,
    },
    /// Remet les compteurs de xruns à zéro.
    ResetXruns,
    /// S'abonne (ou se désabonne) aux notifications.
    Subscribe {
        /// Recevoir les notifications.
        enabled: bool,
    },
    /// Rapport de diagnostic (sans donnée personnelle).
    Dump,
    /// Sauvegarde l'état maintenant.
    Save,
    /// Recharge l'état persisté.
    Load,
}

/// Réponse à une commande.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// Succès sans donnée.
    Ok,
    /// État.
    Status(EngineStatus),
    /// Nœuds.
    Nodes {
        /// Liste.
        nodes: Vec<NodeDescriptor>,
    },
    /// Un nœud.
    Node(NodeDescriptor),
    /// Liens.
    Links {
        /// Liste.
        links: Vec<LinkDescriptor>,
    },
    /// Un lien.
    Link(LinkDescriptor),
    /// Mesures d'un VU-mètre, un élément par canal.
    Meter {
        /// Canaux.
        channels: Vec<MeterReading>,
    },
    /// Câbles.
    Cables {
        /// Liste.
        cables: Vec<CableInfo>,
    },
    /// Un câble.
    Cable(CableInfo),
    /// Rapport de diagnostic (texte).
    Dump {
        /// Texte.
        text: String,
    },
}

/// Événement du fil audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Le cycle a dépassé son budget.
    CycleOverrun {
        /// Numéro du cycle.
        cycle: u64,
        /// Durée mesurée (ns).
        duration_ns: u64,
    },
    /// Xrun sur un périphérique asynchrone.
    DeviceXrun {
        /// Périphérique.
        device: DeviceId,
        /// Sous-alimentation (`true`) ou débordement.
        underrun: bool,
    },
    /// Le pilote de graphe a exécuté son premier cycle.
    DriverStarted {
        /// Périphérique pilote (`None` = horloge interne).
        device: Option<DeviceId>,
    },
    /// L'exécuteur était occupé : cycle sauté, silence.
    ExecutorBusy,
}

/// Notification diffusée aux abonnés (F-43).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Notification {
    /// Nœud ajouté.
    NodeAdded(NodeDescriptor),
    /// Nœud retiré.
    NodeRemoved {
        /// Identifiant.
        id: NodeId,
        /// Clé.
        key: NodeKey,
    },
    /// État d'un nœud changé.
    NodeStateChanged {
        /// Identifiant.
        id: NodeId,
        /// Nouvel état.
        state: NodeState,
    },
    /// Lien ajouté.
    LinkAdded(LinkDescriptor),
    /// Lien retiré.
    LinkRemoved {
        /// Identifiant.
        id: LinkId,
    },
    /// Pilote changé.
    DriverChanged(DriverStatus),
    /// Câble changé (`None` = supprimé).
    CableChanged {
        /// Identifiant.
        id: CableId,
        /// État.
        info: Option<CableInfo>,
    },
    /// Événement du fil audio.
    Rt(EngineEvent),
    /// Le démon s'arrête.
    Shutdown,
}

/// Code d'erreur stable, pour les clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Nœud, port ou lien inconnu.
    NotFound,
    /// Le lien créerait une boucle.
    WouldCycle,
    /// Requête invalide (direction, doublon, paramètre).
    Invalid,
    /// Périphérique ou backend en erreur.
    Device,
    /// Câble : limite, droits, non supporté.
    Cable,
    /// Le moteur ne peut pas appliquer maintenant (réessayer).
    Busy,
    /// Commande inconnue ou non supportée par ce démon.
    Unsupported,
    /// Erreur interne.
    Internal,
}

/// Erreur renvoyée à un client : code stable + message destiné à l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[error("{message}")]
pub struct ProtocolError {
    /// Code.
    pub code: ErrorCode,
    /// Message lisible, indiquant quoi faire.
    pub message: String,
}

impl ProtocolError {
    /// Construit.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_core::graph::Direction;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + core::fmt::Debug>(v: &T) {
        let json = serde_json::to_string(v).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, v, "json {json}");
        let bin = rmp_serde::to_vec_named(v).unwrap();
        let back: T = rmp_serde::from_slice(&bin).unwrap();
        assert_eq!(&back, v);
    }

    fn node() -> NodeDescriptor {
        NodeDescriptor {
            id: NodeId::new(3, 1),
            key: NodeKey::internal("s"),
            label: "Sinus".into(),
            type_name: "sine".into(),
            inputs: vec![],
            outputs: PortSpec::stereo(),
            state: NodeState::Internal,
            gain_db: Db::new(-3.0),
            muted: false,
            device: None,
        }
    }

    #[test]
    fn every_command_variant_roundtrips() {
        let n = NodeId::new(1, 0);
        let p = PortId::new(n, Direction::Output, 0);
        let q = PortId::new(NodeId::new(2, 0), Direction::Input, 1);
        let cmds = vec![
            Command::Status,
            Command::Nodes,
            Command::Ports { node: n },
            Command::Links,
            Command::Link { src: p, dst: q },
            Command::LinkByName {
                src_node: n,
                src_port: "FL".into(),
                dst_node: n,
                dst_port: "FR".into(),
            },
            Command::Unlink {
                link: LinkId::new(4, 2),
            },
            Command::SetNodeGain {
                node: n,
                gain_db: Some(Db::NEG_INF),
                muted: None,
            },
            Command::SetLinkGain {
                link: LinkId::new(0, 0),
                gain_db: None,
                muted: Some(true),
            },
            Command::SetLabel {
                node: n,
                label: "x".into(),
            },
            Command::SetDriver {
                choice: DriverChoice::Device { id: "d".into() },
            },
            Command::SetDriver {
                choice: DriverChoice::Auto,
            },
            Command::AddInternal {
                name: "eq".into(),
                kind: InternalKind::Equalizer {
                    channels: 2,
                    bands: vec![EqBand::peaking(1000.0, 1.0, 3.0)],
                },
            },
            Command::AddInternal {
                name: "n".into(),
                kind: InternalKind::Noise {
                    pink: true,
                    amplitude: 0.1,
                    channels: 1,
                },
            },
            Command::RemoveNode { node: n },
            Command::SetParam {
                node: n,
                name: "band.0.gain_db".into(),
                value: -2.5,
            },
            Command::ReadMeter { node: n },
            Command::CableList,
            Command::CableAdd {
                spec: CableSpec {
                    name: Some("Musique".into()),
                    channels: ChannelCount::MONO,
                    format: None,
                },
            },
            // Le même, avec un format : `None` et `Some` sont deux chemins de
            // sérialisation distincts, et le second est celui qui est neuf.
            Command::CableAdd {
                spec: CableSpec {
                    name: None,
                    channels: ChannelCount::STEREO,
                    format: Some(CableFormat {
                        sample_rate: SampleRate::HZ_96000,
                        depth: conduit_backend::SampleDepth::Pcm24,
                        channels: ChannelCount::new(6).unwrap(),
                    }),
                },
            },
            Command::CableRemove { id: CableId(1) },
            Command::CableRename {
                id: CableId(1),
                name: "Jeu".into(),
            },
            Command::CableSetChannels {
                id: CableId(2),
                channels: ChannelCount::new(6).unwrap(),
            },
            Command::CableSetFormat {
                id: CableId(2),
                format: CableFormat {
                    sample_rate: SampleRate::HZ_44100,
                    depth: conduit_backend::SampleDepth::Pcm16,
                    channels: ChannelCount::new(4).unwrap(),
                },
            },
            Command::ResetXruns,
            Command::Subscribe { enabled: true },
            Command::Dump,
            Command::Save,
            Command::Load,
        ];
        for c in &cmds {
            roundtrip(c);
        }
        assert_eq!(
            serde_json::to_value(&Command::Status).unwrap(),
            serde_json::json!({"cmd": "status"})
        );
        let j = serde_json::to_value(&Command::SetDriver {
            choice: DriverChoice::Internal,
        })
        .unwrap();
        assert_eq!(
            j,
            serde_json::json!({"cmd": "set_driver", "choice": {"mode": "internal"}})
        );
    }

    #[test]
    fn every_reply_variant_roundtrips() {
        let status = EngineStatus {
            backend: "null".into(),
            sample_rate: SampleRate::HZ_48000,
            quantum: Quantum::DEFAULT,
            driver: DriverStatus::Device { id: "d".into() },
            driver_choice: DriverChoice::Auto,
            nodes: 2,
            links: 1,
            timing: TimingSnapshot {
                count: 10,
                min_ns: 1,
                avg_ns: 2,
                max_ns: 3,
                last_ns: 2,
                budget_ns: 5_333_333,
                overruns: 0,
            },
            xruns: 0,
            devices: vec![DeviceStatus {
                id: "d".into(),
                node: NodeId::new(0, 0),
                state: NodeState::Driver,
                underruns: 0,
                overruns: 0,
                fill: 512,
                ratio: 1.0,
                locked: true,
            }],
            position: 4096,
            cycles: 16,
        };
        let link = LinkDescriptor {
            link: LinkInfo {
                id: LinkId::new(0, 0),
                src: PortId::new(NodeId::new(0, 0), Direction::Output, 0),
                dst: PortId::new(NodeId::new(1, 0), Direction::Input, 0),
            },
            gain_db: Db::UNITY,
            muted: false,
        };
        let cable = CableInfo {
            id: CableId(1),
            name: "Conduit 1".into(),
            channels: ChannelCount::STEREO,
            format: CableFormat::default(),
            active: true,
            render: "r".into(),
            capture: "c".into(),
        };
        let replies = vec![
            Reply::Ok,
            Reply::Status(status),
            Reply::Nodes {
                nodes: vec![node()],
            },
            Reply::Node(node()),
            Reply::Links {
                links: vec![link.clone()],
            },
            Reply::Link(link),
            Reply::Meter {
                channels: vec![MeterReading {
                    peak: 0.5,
                    rms: 0.3,
                    peak_hold: 0.9,
                }],
            },
            Reply::Cables {
                cables: vec![cable.clone()],
            },
            Reply::Cable(cable),
            Reply::Dump {
                text: "rapport".into(),
            },
        ];
        for r in &replies {
            roundtrip(r);
        }
        let j = serde_json::to_value(&Reply::Ok).unwrap();
        assert_eq!(j, serde_json::json!({"reply": "ok"}));
    }

    #[test]
    fn every_notification_variant_roundtrips() {
        let events = vec![
            Notification::NodeAdded(node()),
            Notification::NodeRemoved {
                id: NodeId::new(1, 1),
                key: NodeKey::device("null", "x".into()),
            },
            Notification::NodeStateChanged {
                id: NodeId::new(1, 1),
                state: NodeState::Suspended,
            },
            Notification::LinkAdded(LinkDescriptor {
                link: LinkInfo {
                    id: LinkId::new(0, 0),
                    src: PortId::new(NodeId::new(0, 0), Direction::Output, 0),
                    dst: PortId::new(NodeId::new(1, 0), Direction::Input, 0),
                },
                gain_db: Db::UNITY,
                muted: false,
            }),
            Notification::LinkRemoved {
                id: LinkId::new(0, 0),
            },
            Notification::DriverChanged(DriverStatus::Internal),
            Notification::DriverChanged(DriverStatus::None),
            Notification::CableChanged {
                id: CableId(1),
                info: None,
            },
            Notification::Rt(EngineEvent::CycleOverrun {
                cycle: 1,
                duration_ns: 9,
            }),
            Notification::Rt(EngineEvent::DeviceXrun {
                device: "d".into(),
                underrun: false,
            }),
            Notification::Rt(EngineEvent::DriverStarted { device: None }),
            Notification::Rt(EngineEvent::ExecutorBusy),
            Notification::Shutdown,
        ];
        for e in &events {
            roundtrip(e);
        }
        let err = ProtocolError::new(ErrorCode::WouldCycle, "boucle");
        roundtrip(&err);
        assert_eq!(err.to_string(), "boucle");
        assert_eq!(NodeKey::internal("a").to_string(), "internal:a");
        assert_eq!(
            NodeKey::device("null", "x".into())
                .device_id()
                .unwrap()
                .as_str(),
            "x"
        );
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_optionals_default() {
        // Compatibilité ascendante : un client plus récent peut ajouter des champs.
        let c: Command = serde_json::from_str(r#"{"cmd":"status","future_field":1}"#).unwrap();
        assert_eq!(c, Command::Status);
        let c: Command =
            serde_json::from_str(r#"{"cmd":"set_node_gain","node":{"index":0,"generation":0},"gain_db":null,"muted":true}"#)
                .unwrap();
        assert!(matches!(
            c,
            Command::SetNodeGain {
                gain_db: None,
                muted: Some(true),
                ..
            }
        ));
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"teleport"}"#).is_err());
    }
}
