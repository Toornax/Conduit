//! Miroir local de l'état du démon.
//!
//! Le miroir est chargé une fois à la connexion ([`Snapshot`], obtenu par
//! `Status`, `Nodes`, `Links` et `CableList`) puis maintenu **uniquement** par
//! réduction des notifications : [`Mirror::apply`] est une fonction pure, sans
//! entrée/sortie, ce qui la rend testable sans démon ni fenêtre.
//!
//! L'interface n'anticipe jamais le résultat d'une commande : elle envoie la
//! commande et attend la notification correspondante.

use conduit_backend::{CableId, CableInfo};
use conduit_core::graph::{LinkId, NodeId};
use conduit_protocol::{
    DriverStatus, EngineEvent, EngineStatus, LinkDescriptor, NodeDescriptor, Notification,
};

/// État initial du démon, lu au moment de la connexion.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// État global du moteur.
    pub status: EngineStatus,
    /// Nœuds du graphe.
    pub nodes: Vec<NodeDescriptor>,
    /// Liens du graphe.
    pub links: Vec<LinkDescriptor>,
    /// Câbles virtuels ; vide si le backend n'en gère pas.
    pub cables: Vec<CableInfo>,
}

/// Miroir de l'état du démon tel que l'interface le connaît.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mirror {
    /// État global, `None` tant qu'aucun chargement n'a eu lieu.
    pub status: Option<EngineStatus>,
    /// Nœuds, dans l'ordre reçu.
    pub nodes: Vec<NodeDescriptor>,
    /// Liens, dans l'ordre reçu.
    pub links: Vec<LinkDescriptor>,
    /// Câbles, triés par identifiant.
    pub cables: Vec<CableInfo>,
    /// Xruns connus : compteur du démon au chargement, puis incrémenté par les
    /// événements du fil audio.
    pub xruns: u64,
    /// Dernier événement du fil audio reçu.
    pub last_rt: Option<EngineEvent>,
    /// Vrai après [`Notification::Shutdown`] : le démon s'arrête.
    pub stopped: bool,
}

impl Mirror {
    /// Remplace tout l'état par un chargement initial.
    pub fn reset(&mut self, snapshot: Snapshot) {
        self.xruns = snapshot.status.xruns;
        self.status = Some(snapshot.status);
        self.nodes = snapshot.nodes;
        self.links = snapshot.links;
        self.cables = snapshot.cables;
        self.cables.sort_by_key(|c| c.id);
        self.last_rt = None;
        self.stopped = false;
    }

    /// Vrai si un chargement initial a eu lieu.
    pub fn is_loaded(&self) -> bool {
        self.status.is_some()
    }

    /// Un câble par identifiant.
    pub fn cable(&self, id: CableId) -> Option<&CableInfo> {
        self.cables.iter().find(|c| c.id == id)
    }

    /// Un nœud par identifiant.
    pub fn node(&self, id: NodeId) -> Option<&NodeDescriptor> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Un lien par identifiant.
    pub fn link(&self, id: LinkId) -> Option<&LinkDescriptor> {
        self.links.iter().find(|l| l.link.id == id)
    }

    /// Remplace les nœuds par ce que le démon vient de décrire.
    ///
    /// Sert aux relectures ([`crate::ipc::Relecture`]) : le protocole ne
    /// diffuse aucune notification pour un changement de gain ou de coupure,
    /// l'interface redemande donc la liste plutôt que de supposer le résultat.
    pub fn remplace_nodes(&mut self, nodes: Vec<NodeDescriptor>) {
        self.nodes = nodes;
    }

    /// Remplace les liens par ce que le démon vient de décrire. Même raison que
    /// [`Mirror::remplace_nodes`].
    pub fn remplace_links(&mut self, links: Vec<LinkDescriptor>) {
        self.links = links;
    }

    /// Réduit une notification dans l'état. Pure : aucune entrée/sortie, et
    /// idempotente pour les ajouts (le démon peut rediffuser un état déjà
    /// contenu dans le chargement initial).
    pub fn apply(&mut self, notification: &Notification) {
        match notification {
            Notification::NodeAdded(node) => {
                match self.nodes.iter_mut().find(|n| n.id == node.id) {
                    Some(existing) => *existing = node.clone(),
                    None => self.nodes.push(node.clone()),
                }
            }
            Notification::NodeRemoved { id, .. } => {
                self.nodes.retain(|n| n.id != *id);
                // Les liens d'un nœud disparu n'existent plus, que le démon
                // les annonce séparément ou non.
                self.links
                    .retain(|l| l.link.src.node != *id && l.link.dst.node != *id);
            }
            Notification::NodeStateChanged { id, state } => {
                if let Some(node) = self.nodes.iter_mut().find(|n| n.id == *id) {
                    node.state = *state;
                }
            }
            Notification::LinkAdded(link) => {
                match self.links.iter_mut().find(|l| l.link.id == link.link.id) {
                    Some(existing) => *existing = link.clone(),
                    None => self.links.push(link.clone()),
                }
            }
            Notification::LinkRemoved { id } => self.links.retain(|l| l.link.id != *id),
            Notification::DriverChanged(driver) => {
                if let Some(status) = &mut self.status {
                    status.driver = driver.clone();
                }
            }
            Notification::CableChanged { id, info } => match info {
                Some(info) => match self.cables.iter_mut().find(|c| c.id == *id) {
                    Some(existing) => *existing = info.clone(),
                    None => {
                        let at = self.cables.partition_point(|c| c.id < *id);
                        self.cables.insert(at, info.clone());
                    }
                },
                None => self.cables.retain(|c| c.id != *id),
            },
            Notification::Rt(event) => {
                if is_xrun(event) {
                    self.xruns += 1;
                }
                if let (EngineEvent::DriverStarted { device }, Some(status)) =
                    (event, self.status.as_mut())
                {
                    status.driver = match device {
                        Some(id) => DriverStatus::Device { id: id.clone() },
                        None => DriverStatus::Internal,
                    };
                }
                self.last_rt = Some(event.clone());
            }
            Notification::Shutdown => self.stopped = true,
        }
    }
}

/// Vrai si l'événement du fil audio compte comme un xrun : cycle hors budget,
/// xrun d'un périphérique asynchrone, ou cycle sauté faute d'exécuteur.
pub fn is_xrun(event: &EngineEvent) -> bool {
    match event {
        EngineEvent::CycleOverrun { .. }
        | EngineEvent::DeviceXrun { .. }
        | EngineEvent::ExecutorBusy => true,
        EngineEvent::DriverStarted { .. } => false,
    }
}

/// Fixtures partagées par les tests du crate.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use conduit_core::graph::{Direction, LinkInfo, PortId};
    use conduit_core::node::PortSpec;
    use conduit_core::types::{ChannelCount, Db, Quantum, SampleRate};
    use conduit_protocol::api::NodeKey;
    use conduit_protocol::{DriverChoice, NodeState, TimingSnapshot};

    /// État global minimal, avec trois xruns déjà comptés par le démon.
    pub(crate) fn status() -> EngineStatus {
        EngineStatus {
            backend: "null".into(),
            sample_rate: SampleRate::HZ_48000,
            quantum: Quantum::DEFAULT,
            driver: DriverStatus::Internal,
            driver_choice: DriverChoice::Auto,
            nodes: 0,
            links: 0,
            timing: TimingSnapshot::default(),
            xruns: 3,
            devices: vec![],
            position: 0,
            cycles: 0,
        }
    }

    /// Nœud interne nommé.
    pub(crate) fn node(index: u32, label: &str) -> NodeDescriptor {
        NodeDescriptor {
            id: NodeId::new(index, 0),
            key: NodeKey::internal(label),
            label: label.into(),
            type_name: "sine".into(),
            inputs: vec![],
            outputs: PortSpec::stereo(),
            state: NodeState::Internal,
            gain_db: Db::UNITY,
            muted: false,
            device: None,
        }
    }

    /// Lien entre les premiers ports de deux nœuds.
    pub(crate) fn link(index: u32, src: NodeId, dst: NodeId) -> LinkDescriptor {
        LinkDescriptor {
            link: LinkInfo {
                id: LinkId::new(index, 0),
                src: PortId::new(src, Direction::Output, 0),
                dst: PortId::new(dst, Direction::Input, 0),
            },
            gain_db: Db::UNITY,
            muted: false,
        }
    }

    /// Câble stéréo actif.
    pub(crate) fn cable(id: u32, name: &str) -> CableInfo {
        CableInfo {
            id: CableId(id),
            name: name.into(),
            channels: ChannelCount::STEREO,
            active: true,
            render: conduit_backend::DeviceId::new(format!("cable-{id}-render")),
            capture: conduit_backend::DeviceId::new(format!("cable-{id}-capture")),
        }
    }

    /// Chargement initial : deux nœuds, un lien, deux câbles dans le désordre.
    pub(crate) fn snapshot() -> Snapshot {
        Snapshot {
            status: status(),
            nodes: vec![node(0, "a"), node(1, "b")],
            links: vec![link(0, NodeId::new(0, 0), NodeId::new(1, 0))],
            cables: vec![cable(2, "Conduit 2"), cable(1, "Conduit 1")],
        }
    }

    /// Miroir déjà chargé avec [`snapshot`].
    pub(crate) fn loaded() -> Mirror {
        let mut m = Mirror::default();
        m.reset(snapshot());
        m
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use conduit_core::types::ChannelCount;
    use conduit_protocol::api::NodeKey;
    use conduit_protocol::NodeState;

    #[test]
    fn initial_load_sorts_cables_and_adopts_the_xrun_counter() {
        let m = loaded();
        assert!(m.is_loaded());
        assert_eq!(
            m.cables.iter().map(|c| c.id.0).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(m.xruns, 3);
        assert!(!m.stopped);
    }

    #[test]
    fn node_added_appends_then_replaces() {
        let mut m = loaded();
        m.apply(&Notification::NodeAdded(node(2, "c")));
        assert_eq!(m.nodes.len(), 3);
        let mut renamed = node(2, "c");
        renamed.label = "c2".into();
        m.apply(&Notification::NodeAdded(renamed));
        assert_eq!(m.nodes.len(), 3, "un même identifiant ne double pas");
        assert_eq!(m.node(NodeId::new(2, 0)).unwrap().label, "c2");
    }

    #[test]
    fn node_removed_drops_the_node_and_its_links() {
        let mut m = loaded();
        m.apply(&Notification::NodeRemoved {
            id: NodeId::new(1, 0),
            key: NodeKey::internal("b"),
        });
        assert!(m.node(NodeId::new(1, 0)).is_none());
        assert!(m.links.is_empty(), "les liens du nœud disparaissent aussi");
        assert_eq!(m.nodes.len(), 1);
    }

    #[test]
    fn node_state_changed_updates_only_that_node() {
        let mut m = loaded();
        m.apply(&Notification::NodeStateChanged {
            id: NodeId::new(1, 0),
            state: NodeState::Suspended,
        });
        assert_eq!(
            m.node(NodeId::new(1, 0)).unwrap().state,
            NodeState::Suspended
        );
        assert_eq!(
            m.node(NodeId::new(0, 0)).unwrap().state,
            NodeState::Internal
        );
        // Un identifiant inconnu ne crée rien.
        m.apply(&Notification::NodeStateChanged {
            id: NodeId::new(9, 0),
            state: NodeState::Active,
        });
        assert_eq!(m.nodes.len(), 2);
    }

    #[test]
    fn link_added_appends_then_replaces() {
        let mut m = loaded();
        m.apply(&Notification::LinkAdded(link(
            1,
            NodeId::new(1, 0),
            NodeId::new(0, 0),
        )));
        assert_eq!(m.links.len(), 2);
        let mut muted = link(1, NodeId::new(1, 0), NodeId::new(0, 0));
        muted.muted = true;
        m.apply(&Notification::LinkAdded(muted));
        assert_eq!(m.links.len(), 2);
        assert!(m.link(LinkId::new(1, 0)).unwrap().muted);
    }

    #[test]
    fn link_removed_drops_it() {
        let mut m = loaded();
        m.apply(&Notification::LinkRemoved {
            id: LinkId::new(0, 0),
        });
        assert!(m.links.is_empty());
        // Idempotent.
        m.apply(&Notification::LinkRemoved {
            id: LinkId::new(0, 0),
        });
        assert!(m.links.is_empty());
    }

    #[test]
    fn driver_changed_updates_the_status() {
        let mut m = loaded();
        m.apply(&Notification::DriverChanged(DriverStatus::Device {
            id: "hp".into(),
        }));
        assert_eq!(
            m.status.as_ref().unwrap().driver,
            DriverStatus::Device { id: "hp".into() }
        );
        // Sans chargement initial, la notification est sans effet.
        let mut vide = Mirror::default();
        vide.apply(&Notification::DriverChanged(DriverStatus::None));
        assert!(vide.status.is_none());
    }

    #[test]
    fn cable_changed_inserts_updates_and_removes_in_order() {
        let mut m = loaded();
        m.apply(&Notification::CableChanged {
            id: CableId(3),
            info: Some(cable(3, "Jeu")),
        });
        assert_eq!(
            m.cables.iter().map(|c| c.id.0).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let mut renamed = cable(2, "Musique");
        renamed.channels = ChannelCount::MONO;
        m.apply(&Notification::CableChanged {
            id: CableId(2),
            info: Some(renamed),
        });
        let c = m.cable(CableId(2)).unwrap();
        assert_eq!(c.name, "Musique");
        assert_eq!(c.channels, ChannelCount::MONO);
        m.apply(&Notification::CableChanged {
            id: CableId(1),
            info: None,
        });
        assert!(m.cable(CableId(1)).is_none());
        assert_eq!(
            m.cables.iter().map(|c| c.id.0).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn rt_events_count_xruns_and_track_the_driver() {
        let mut m = loaded();
        m.apply(&Notification::Rt(EngineEvent::CycleOverrun {
            cycle: 1,
            duration_ns: 9,
        }));
        m.apply(&Notification::Rt(EngineEvent::DeviceXrun {
            device: "hp".into(),
            underrun: true,
        }));
        m.apply(&Notification::Rt(EngineEvent::ExecutorBusy));
        assert_eq!(m.xruns, 6, "3 xruns au-dessus des 3 du chargement");
        m.apply(&Notification::Rt(EngineEvent::DriverStarted {
            device: Some("hp".into()),
        }));
        assert_eq!(m.xruns, 6, "le démarrage du pilote n'est pas un xrun");
        assert_eq!(
            m.status.as_ref().unwrap().driver,
            DriverStatus::Device { id: "hp".into() }
        );
        m.apply(&Notification::Rt(EngineEvent::DriverStarted {
            device: None,
        }));
        assert_eq!(m.status.as_ref().unwrap().driver, DriverStatus::Internal);
        assert_eq!(m.last_rt, Some(EngineEvent::DriverStarted { device: None }));
    }

    #[test]
    fn shutdown_marks_the_daemon_as_stopped() {
        let mut m = loaded();
        m.apply(&Notification::Shutdown);
        assert!(m.stopped);
        // Un rechargement le remet à zéro.
        m.reset(Snapshot {
            status: status(),
            nodes: vec![],
            links: vec![],
            cables: vec![],
        });
        assert!(!m.stopped);
    }
}
