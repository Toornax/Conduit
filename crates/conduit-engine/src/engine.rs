//! L'[`Engine`] : relie backend, graphe et fil audio ; expose une API de commandes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use conduit_backend::{
    Backend, BackendError, CableError, ClockInfo, DeviceDirection, DeviceEvent, DeviceHandle,
    DeviceId, DeviceInfo, EventReceiver, StreamFormat, StreamIo,
};
use conduit_core::asyncport::{input_port, output_port, AsyncPortConfig, AsyncStats};
use conduit_core::dsp::ResampleQuality;
use conduit_core::executor::Executor;
use conduit_core::graph::{Direction, GraphBuilder, GraphError, LinkId, NodeId, PortId};
use conduit_core::node::Node;
use conduit_core::nodes::{
    ChannelAdapter, EqBandControl, EqualizerNode, MeterNode, MeterReading, MeterShared, MixerNode,
    NoiseColor, NoiseControl, NoiseNode, SilenceNode, SineControl, SineNode, SplitterNode,
};
use conduit_core::ring::{RingBuffer, RingConsumer};
use conduit_core::slot::{GraphSlot, PublishError};
use conduit_core::types::{Quantum, SampleRate};

use conduit_protocol::api::{
    Command, DeviceStatus, DriverChoice, DriverStatus, EngineEvent, EngineStatus, ErrorCode,
    InternalKind, LinkDescriptor, NodeDescriptor, NodeState, Notification, ProtocolError, Reply,
};

use crate::clock::InternalClock;
use crate::device_node::{DeviceNode, DeviceNodeControl, Role};
use crate::key::{NodeKey, Registry};
use crate::stats::{CycleTiming, EventQueue};

/// Configuration du moteur.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// Fréquence du graphe.
    pub sample_rate: SampleRate,
    /// Quantum.
    pub quantum: Quantum,
    /// Pilote.
    pub driver: DriverChoice,
    /// Qualité du rééchantillonnage des périphériques asynchrones.
    pub quality: ResampleQuality,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::HZ_48000,
            quantum: Quantum::DEFAULT,
            driver: DriverChoice::Auto,
            quality: ResampleQuality::Normal,
        }
    }
}

/// Erreurs du moteur. Les messages disent quoi faire.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    /// Erreur de graphe (cycle, port inconnu, …).
    #[error("{0}")]
    Graph(#[from] GraphError),
    /// Erreur du backend.
    #[error("{0}")]
    Backend(#[from] BackendError),
    /// Erreur de câble.
    #[error("{0}")]
    Cable(#[from] CableError),
    /// Nœud inconnu par sa clé.
    #[error("nœud inconnu : {0}")]
    UnknownKey(NodeKey),
    /// Port inconnu par son nom.
    #[error("port {name} inconnu sur {node} ({direction})")]
    UnknownPortName {
        /// Nœud.
        node: NodeId,
        /// Direction.
        direction: Direction,
        /// Nom demandé.
        name: String,
    },
    /// Le nom interne est déjà pris.
    #[error("un nœud interne nommé « {0} » existe déjà : choisissez un autre nom")]
    DuplicateName(String),
    /// Le périphérique ne peut pas piloter le graphe.
    #[error("{device} ne peut pas piloter le graphe : {reason}")]
    CannotDrive {
        /// Périphérique.
        device: DeviceId,
        /// Raison.
        reason: String,
    },
    /// Paramètre inconnu ou hors bornes.
    #[error("paramètre invalide « {name} » : {reason}")]
    InvalidParam {
        /// Nom.
        name: String,
        /// Raison.
        reason: String,
    },
    /// Le fil audio n'adopte pas les changements.
    #[error("{0}")]
    Publish(#[from] PublishError),
    /// Cette plateforme ne gère pas les câbles.
    #[error("le backend {0} ne gère pas les câbles virtuels")]
    NoCableControl(&'static str),
    /// Commande gérée par le démon, pas par le moteur.
    #[error("commande « {0} » non supportée par le moteur seul")]
    Unsupported(&'static str),
    /// Un nœud de périphérique ne peut pas être retiré tant que le périphérique est présent.
    #[error("le périphérique {0} est présent : débranchez-le ou désactivez-le avant de retirer son nœud")]
    DevicePresent(DeviceId),
}

struct DeviceEntry {
    info: DeviceInfo,
    node: NodeId,
    control: DeviceNodeControl,
    handle: Option<Box<dyn DeviceHandle>>,
    state: NodeState,
    stats: Option<Arc<AsyncStats>>,
    present: bool,
    /// Derniers compteurs de xruns rapportés (sous-alimentations, débordements).
    last_xruns_cell: core::cell::Cell<(u64, u64)>,
}

struct InternalEntry {
    kind: InternalKind,
    /// Paramètres réglés à chaud (nom → valeur), pour la persistance.
    params: BTreeMap<String, f32>,
    sine: Option<Arc<SineControl>>,
    noise: Option<Arc<NoiseControl>>,
    meter: Option<Arc<MeterShared>>,
    eq: Vec<Arc<EqBandControl>>,
}

enum DriverState {
    None,
    Internal(InternalClock),
    Device(DeviceId),
}

/// Le moteur.
pub struct Engine {
    backend: Box<dyn Backend>,
    backend_name: &'static str,
    config: EngineConfig,
    builder: GraphBuilder,
    slot: GraphSlot,
    executor: Arc<Mutex<Executor>>,
    registry: Registry,
    devices: BTreeMap<DeviceId, DeviceEntry>,
    internals: BTreeMap<NodeId, InternalEntry>,
    driver: DriverState,
    timing: Arc<CycleTiming>,
    rt_events: Vec<RingConsumer<EngineEvent>>,
    backend_events: EventReceiver,
    notifications: Vec<Notification>,
}

impl core::fmt::Debug for Engine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Engine")
            .field("backend", &self.backend_name)
            .field("devices", &self.devices.len())
            .field("internals", &self.internals.len())
            .field("driver", &self.driver_status())
            .finish()
    }
}

impl Engine {
    /// Crée le moteur : énumère les périphériques, les ouvre, choisit le pilote.
    pub fn new(mut backend: Box<dyn Backend>, config: EngineConfig) -> Result<Self, EngineError> {
        let backend_events = backend.subscribe();
        let backend_name = backend.name();
        let (slot, executor) = GraphSlot::new();
        let budget_ns = (config.quantum.get() as f64 / config.sample_rate.as_f64() * 1e9) as u64;
        let mut engine = Self {
            backend,
            backend_name,
            builder: GraphBuilder::new(config.sample_rate, config.quantum),
            config,
            slot,
            executor: Arc::new(Mutex::new(executor)),
            registry: Registry::default(),
            devices: BTreeMap::new(),
            internals: BTreeMap::new(),
            driver: DriverState::None,
            timing: Arc::new(CycleTiming::new(budget_ns)),
            rt_events: Vec::new(),
            backend_events,
            notifications: Vec::new(),
        };
        for info in engine.backend.devices()? {
            engine.add_device(info);
        }
        engine.apply_driver_policy()?;
        engine.publish()?;
        Ok(engine)
    }

    /// Configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Registre des clés.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Accès au graphe modifiable (lecture).
    pub fn graph(&self) -> &GraphBuilder {
        &self.builder
    }

    /// Accès au backend.
    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    /// Exécute une commande.
    pub fn execute(&mut self, cmd: Command) -> Result<Reply, EngineError> {
        match cmd {
            Command::Status => Ok(Reply::Status(self.status())),
            Command::Nodes => Ok(Reply::Nodes {
                nodes: self.nodes(),
            }),
            Command::Ports { node } => self.node_descriptor(node).map(Reply::Node),
            Command::Links => Ok(Reply::Links {
                links: self.links(),
            }),
            Command::Link { src, dst } => self.link(src, dst).map(Reply::Link),
            Command::LinkByName {
                src_node,
                src_port,
                dst_node,
                dst_port,
            } => {
                let src = self.port_by_name(src_node, Direction::Output, &src_port)?;
                let dst = self.port_by_name(dst_node, Direction::Input, &dst_port)?;
                self.link(src, dst).map(Reply::Link)
            }
            Command::Unlink { link } => self.unlink(link).map(|()| Reply::Ok),
            Command::SetNodeGain {
                node,
                gain_db,
                muted,
            } => {
                let g = self.builder.node_gain(node)?;
                if let Some(db) = gain_db {
                    g.set_db(db);
                }
                if let Some(m) = muted {
                    g.set_muted(m);
                }
                Ok(Reply::Ok)
            }
            Command::SetLinkGain {
                link,
                gain_db,
                muted,
            } => {
                let g = self.builder.link_gain(link)?;
                if let Some(db) = gain_db {
                    g.set_db(db);
                }
                if let Some(m) = muted {
                    g.set_muted(m);
                }
                Ok(Reply::Ok)
            }
            Command::SetLabel { node, label } => {
                self.builder.set_label(node, label)?;
                Ok(Reply::Ok)
            }
            Command::SetDriver { choice } => {
                self.set_driver(choice)?;
                Ok(Reply::Ok)
            }
            Command::AddInternal { name, kind } => self.add_internal(&name, kind).map(Reply::Node),
            Command::RemoveNode { node } => self.remove_node(node).map(|()| Reply::Ok),
            Command::SetParam { node, name, value } => {
                self.set_param(node, &name, value).map(|()| Reply::Ok)
            }
            Command::ReadMeter { node } => self
                .read_meter(node)
                .map(|channels| Reply::Meter { channels }),
            Command::CableList => Ok(Reply::Cables {
                cables: self.cables()?.list()?,
            }),
            Command::CableAdd { spec } => self.cable_op(|c| c.create(spec)).map(Reply::Cable),
            Command::CableRemove { id } => self.cable_op(|c| c.remove(id)).map(|()| Reply::Ok),
            Command::CableRename { id, name } => {
                self.cable_op(|c| c.rename(id, &name)).map(Reply::Cable)
            }
            Command::CableSetChannels { id, channels } => self
                .cable_op(|c| c.set_channels(id, channels))
                .map(Reply::Cable),
            Command::Dump => Ok(Reply::Dump { text: self.dump() }),
            Command::Subscribe { .. } => Err(EngineError::Unsupported("subscribe")),
            Command::Save => Err(EngineError::Unsupported("save")),
            Command::Load => Err(EngineError::Unsupported("load")),
            Command::ResetXruns => {
                self.timing.reset();
                for d in self.devices.values() {
                    if let Some(s) = &d.stats {
                        s.reset_xruns();
                    }
                    d.set_last_xruns((0, 0));
                }
                Ok(Reply::Ok)
            }
        }
    }

    /// Traite les événements du backend et du fil audio, libère les ressources
    /// retirées. À appeler régulièrement (quelques dizaines de ms). Retourne les
    /// notifications produites depuis le dernier appel.
    pub fn tick(&mut self) -> Vec<Notification> {
        let mut backend_events = Vec::new();
        while let Ok(e) = self.backend_events.try_recv() {
            backend_events.push(e);
        }
        for e in backend_events {
            self.handle_backend_event(e);
        }
        self.slot.collect();
        for d in self.devices.values_mut() {
            d.control.collect();
        }
        self.rt_events
            .retain(|c| !(c.is_abandoned() && c.is_empty()));
        let mut rt = Vec::new();
        for c in &mut self.rt_events {
            while let Some(e) = c.pop() {
                rt.push(e);
            }
        }
        for e in rt {
            self.notifications.push(Notification::Rt(e));
        }
        self.report_device_xruns();
        std::mem::take(&mut self.notifications)
    }

    fn report_device_xruns(&mut self) {
        let mut new_events = Vec::new();
        for d in self.devices.values_mut() {
            if let Some(s) = &d.stats {
                let (u, o) = (s.underruns(), s.overruns());
                let key = (u, o);
                if d.last_xruns() != key {
                    let prev = d.last_xruns();
                    if u > prev.0 {
                        new_events.push(EngineEvent::DeviceXrun {
                            device: d.info.id.clone(),
                            underrun: true,
                        });
                    }
                    if o > prev.1 {
                        new_events.push(EngineEvent::DeviceXrun {
                            device: d.info.id.clone(),
                            underrun: false,
                        });
                    }
                    d.set_last_xruns(key);
                }
            }
        }
        for e in new_events {
            self.notifications.push(Notification::Rt(e));
        }
    }

    // ----- état -----

    /// Pilote courant.
    pub fn driver_status(&self) -> DriverStatus {
        match &self.driver {
            DriverState::None => DriverStatus::None,
            DriverState::Internal(_) => DriverStatus::Internal,
            DriverState::Device(id) => DriverStatus::Device { id: id.clone() },
        }
    }

    /// État global.
    pub fn status(&self) -> EngineStatus {
        let ex = self.executor.lock().unwrap();
        let timing = self.timing.snapshot();
        let devices: Vec<DeviceStatus> = self
            .devices
            .values()
            .map(|d| {
                let (underruns, overruns, fill, ratio, locked) = match &d.stats {
                    Some(s) => (
                        s.underruns(),
                        s.overruns(),
                        s.fill(),
                        s.ratio(),
                        s.is_locked(),
                    ),
                    None => (0, 0, 0, 1.0, false),
                };
                DeviceStatus {
                    id: d.info.id.clone(),
                    node: d.node,
                    state: d.state,
                    underruns,
                    overruns,
                    fill,
                    ratio,
                    locked,
                }
            })
            .collect();
        let xruns = timing.overruns
            + devices
                .iter()
                .map(|d| d.underruns + d.overruns)
                .sum::<u64>();
        EngineStatus {
            backend: self.backend_name.to_string(),
            sample_rate: self.config.sample_rate,
            quantum: self.config.quantum,
            driver: self.driver_status(),
            driver_choice: self.config.driver.clone(),
            nodes: self.builder.node_count(),
            links: self.builder.link_count(),
            timing,
            xruns,
            devices,
            position: ex.position(),
            cycles: ex.cycle(),
        }
    }

    /// Tous les nœuds.
    pub fn nodes(&self) -> Vec<NodeDescriptor> {
        self.builder
            .nodes()
            .filter_map(|n| self.node_descriptor(n.id).ok())
            .collect()
    }

    /// Description d'un nœud.
    pub fn node_descriptor(&self, id: NodeId) -> Result<NodeDescriptor, EngineError> {
        let info = self.builder.node(id)?;
        let gain = self.builder.node_gain(id)?;
        let key = self
            .registry
            .key(id)
            .cloned()
            .unwrap_or_else(|| NodeKey::internal(format!("?{id}")));
        let device = key.device_id().and_then(|d| self.devices.get(d));
        Ok(NodeDescriptor {
            id,
            key,
            label: info.label.clone(),
            type_name: info.type_name.clone(),
            inputs: info.inputs.clone(),
            outputs: info.outputs.clone(),
            state: device.map_or(NodeState::Internal, |d| d.state),
            gain_db: gain.db(),
            muted: gain.is_muted(),
            device: device.map(|d| d.info.clone()),
        })
    }

    /// Tous les liens.
    pub fn links(&self) -> Vec<LinkDescriptor> {
        self.builder
            .links()
            .filter_map(|l| self.link_descriptor(l.id).ok())
            .collect()
    }

    fn link_descriptor(&self, id: LinkId) -> Result<LinkDescriptor, EngineError> {
        let link = self.builder.link(id)?.clone();
        let g = self.builder.link_gain(id)?;
        Ok(LinkDescriptor {
            link,
            gain_db: g.db(),
            muted: g.is_muted(),
        })
    }

    /// Identifiant de nœud pour une clé.
    pub fn node_by_key(&self, key: &NodeKey) -> Option<NodeId> {
        self.registry.id(key)
    }

    /// Port par nom.
    pub fn port_by_name(
        &self,
        node: NodeId,
        direction: Direction,
        name: &str,
    ) -> Result<PortId, EngineError> {
        self.builder
            .find_port(node, direction, name)
            .ok_or_else(|| EngineError::UnknownPortName {
                node,
                direction,
                name: name.to_string(),
            })
    }

    // ----- graphe -----

    /// Crée un lien et publie.
    pub fn link(&mut self, src: PortId, dst: PortId) -> Result<LinkDescriptor, EngineError> {
        let id = self.builder.add_link(src, dst)?;
        self.publish()?;
        let d = self.link_descriptor(id)?;
        self.notifications.push(Notification::LinkAdded(d.clone()));
        Ok(d)
    }

    /// Supprime un lien et publie.
    pub fn unlink(&mut self, id: LinkId) -> Result<(), EngineError> {
        self.builder.remove_link(id)?;
        self.publish()?;
        self.notifications.push(Notification::LinkRemoved { id });
        Ok(())
    }

    /// Ajoute un nœud interne.
    pub fn add_internal(
        &mut self,
        name: &str,
        kind: InternalKind,
    ) -> Result<NodeDescriptor, EngineError> {
        let key = NodeKey::internal(name);
        if self.registry.id(&key).is_some() {
            return Err(EngineError::DuplicateName(name.to_string()));
        }
        let mut entry = InternalEntry {
            kind: kind.clone(),
            params: BTreeMap::new(),
            sine: None,
            noise: None,
            meter: None,
            eq: Vec::new(),
        };
        let node: Box<dyn Node> = match kind {
            InternalKind::Silence { channels } => Box::new(SilenceNode::new(channels)),
            InternalKind::Sine {
                frequency,
                amplitude,
                channels,
            } => {
                let n = SineNode::new(frequency, amplitude, channels);
                entry.sine = Some(n.control());
                Box::new(n)
            }
            InternalKind::Noise {
                pink,
                amplitude,
                channels,
            } => {
                let n = NoiseNode::new(
                    if pink {
                        NoiseColor::Pink
                    } else {
                        NoiseColor::White
                    },
                    amplitude,
                    channels,
                );
                entry.noise = Some(n.control());
                Box::new(n)
            }
            InternalKind::Mixer { buses, channels } => Box::new(MixerNode::new(buses, channels)),
            InternalKind::Splitter { channels, copies } => {
                Box::new(SplitterNode::new(channels, copies))
            }
            InternalKind::Meter { channels } => {
                let n = MeterNode::new(channels);
                entry.meter = Some(n.shared());
                Box::new(n)
            }
            InternalKind::Equalizer { channels, bands } => {
                let n = EqualizerNode::new(channels, &bands);
                entry.eq = n.controls();
                Box::new(n)
            }
            InternalKind::Adapter { inputs, outputs } => {
                Box::new(ChannelAdapter::auto(inputs, outputs))
            }
        };
        let id = self.builder.add_node(node, name);
        self.registry.insert(key, id);
        self.internals.insert(id, entry);
        self.publish()?;
        let d = self.node_descriptor(id)?;
        self.notifications.push(Notification::NodeAdded(d.clone()));
        Ok(d)
    }

    /// Retire un nœud interne ou un nœud de périphérique absent.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), EngineError> {
        let key = self
            .registry
            .key(id)
            .cloned()
            .ok_or(GraphError::UnknownNode(id))?;
        if let Some(dev_id) = key.device_id() {
            let present = self.devices.get(dev_id).map(|d| d.present).unwrap_or(false);
            if present {
                return Err(EngineError::DevicePresent(dev_id.clone()));
            }
            self.devices.remove(dev_id);
        }
        self.internals.remove(&id);
        let removed_links: Vec<LinkId> = self
            .builder
            .links()
            .filter(|l| l.src.node == id || l.dst.node == id)
            .map(|l| l.id)
            .collect();
        self.builder.remove_node(id)?;
        self.registry.remove_id(id);
        self.publish()?;
        for l in removed_links {
            self.notifications.push(Notification::LinkRemoved { id: l });
        }
        self.notifications
            .push(Notification::NodeRemoved { id, key });
        Ok(())
    }

    /// Règle un paramètre d'un nœud interne.
    pub fn set_param(&mut self, node: NodeId, name: &str, value: f32) -> Result<(), EngineError> {
        let entry = self
            .internals
            .get(&node)
            .ok_or(GraphError::UnknownNode(node))?;
        let invalid = |reason: &str| EngineError::InvalidParam {
            name: name.to_string(),
            reason: reason.to_string(),
        };
        if !value.is_finite() {
            return Err(invalid("valeur non finie"));
        }
        match name {
            "frequency" => {
                let s = entry
                    .sine
                    .as_ref()
                    .ok_or_else(|| invalid("ce nœud n'a pas de fréquence"))?;
                if value <= 0.0 || value >= self.config.sample_rate.as_f64() as f32 / 2.0 {
                    return Err(invalid("hors de ]0, fréquence/2["));
                }
                s.frequency.set(value);
            }
            "amplitude" => {
                if !(0.0..=16.0).contains(&value) {
                    return Err(invalid("hors de [0, 16]"));
                }
                if let Some(s) = &entry.sine {
                    s.amplitude.set(value);
                } else if let Some(n) = &entry.noise {
                    n.amplitude.set(value);
                } else {
                    return Err(invalid("ce nœud n'a pas d'amplitude"));
                }
            }
            _ => {
                let mut parts = name.split('.');
                match (parts.next(), parts.next(), parts.next(), parts.next()) {
                    (Some("band"), Some(i), Some(field), None) => {
                        let i: usize = i.parse().map_err(|_| invalid("index de bande invalide"))?;
                        let band = entry
                            .eq
                            .get(i)
                            .ok_or_else(|| invalid("bande inexistante"))?;
                        match field {
                            "frequency" => band.frequency.set(value),
                            "q" => band.q.set(value),
                            "gain_db" => band.gain_db.set(value),
                            "enabled" => band.enabled.set(value != 0.0),
                            _ => {
                                return Err(invalid(
                                    "champ inconnu (frequency, q, gain_db, enabled)",
                                ))
                            }
                        }
                        band.commit();
                    }
                    _ => return Err(invalid("paramètre inconnu")),
                }
            }
        }
        if let Some(entry) = self.internals.get_mut(&node) {
            entry.params.insert(name.to_string(), value);
        }
        Ok(())
    }

    /// Lit un VU-mètre.
    pub fn read_meter(&self, node: NodeId) -> Result<Vec<MeterReading>, EngineError> {
        let entry = self
            .internals
            .get(&node)
            .ok_or(GraphError::UnknownNode(node))?;
        let m = entry
            .meter
            .as_ref()
            .ok_or_else(|| EngineError::InvalidParam {
                name: "meter".into(),
                reason: "ce nœud n'est pas un VU-mètre".into(),
            })?;
        Ok(m.read_all())
    }

    /// Type d'un nœud interne.
    pub fn internal_kind(&self, node: NodeId) -> Option<&InternalKind> {
        self.internals.get(&node).map(|e| &e.kind)
    }

    /// Paramètres réglés à chaud d'un nœud interne.
    pub fn internal_params(&self, node: NodeId) -> Option<&BTreeMap<String, f32>> {
        self.internals.get(&node).map(|e| &e.params)
    }

    fn publish(&mut self) -> Result<(), EngineError> {
        self.slot.publish(&mut self.builder)?;
        Ok(())
    }

    // ----- câbles -----

    fn cables(&mut self) -> Result<&mut dyn conduit_backend::CableControl, EngineError> {
        let name = self.backend_name;
        self.backend
            .cable_control()
            .ok_or(EngineError::NoCableControl(name))
    }

    fn cable_op<T>(
        &mut self,
        f: impl FnOnce(&mut dyn conduit_backend::CableControl) -> Result<T, CableError>,
    ) -> Result<T, EngineError> {
        let r = f(self.cables()?)?;
        // Les périphériques du câble arrivent par les événements du backend.
        let _ = self.tick_backend_events();
        Ok(r)
    }

    fn tick_backend_events(&mut self) -> usize {
        let mut n = 0;
        let mut events = Vec::new();
        while let Ok(e) = self.backend_events.try_recv() {
            events.push(e);
        }
        for e in events {
            self.handle_backend_event(e);
            n += 1;
        }
        n
    }

    // ----- périphériques -----

    fn handle_backend_event(&mut self, event: DeviceEvent) {
        match event {
            DeviceEvent::Added(info) => {
                self.add_device(info);
                let _ = self.apply_driver_policy();
                let _ = self.publish();
            }
            DeviceEvent::Removed { id } => {
                self.suspend_device(&id);
                let _ = self.apply_driver_policy();
                let _ = self.publish();
            }
            DeviceEvent::DefaultChanged { direction, id } => {
                for d in self.devices.values_mut() {
                    if d.info.direction == direction {
                        d.info.is_default = Some(&d.info.id) == id.as_ref();
                    }
                }
                if self.config.driver == DriverChoice::Auto {
                    let _ = self.apply_driver_policy();
                    let _ = self.publish();
                }
            }
            DeviceEvent::CableChanged { id, info } => {
                self.notifications
                    .push(Notification::CableChanged { id, info });
            }
        }
    }

    /// Ajoute (ou réactive) un périphérique.
    fn add_device(&mut self, info: DeviceInfo) {
        let key = NodeKey::device(self.backend_name, info.id.clone());
        if let Some(existing) = self.devices.get_mut(&info.id) {
            // Réapparition.
            if existing.info.channels == info.channels && existing.info.direction == info.direction
            {
                existing.info = info;
                existing.present = true;
                let id = existing.info.id.clone();
                self.activate_async(&id);
                return;
            }
            // Disposition différente : le nœud est recréé (les liens sont perdus).
            let node = existing.node;
            self.devices.remove(&info.id);
            let _ = self.builder.remove_node(node);
            self.registry.remove_id(node);
            self.notifications.push(Notification::NodeRemoved {
                id: node,
                key: key.clone(),
            });
        }
        let (node, control) =
            DeviceNode::new(info.direction, info.channels, self.config.quantum.get());
        let id = self.builder.add_node(Box::new(node), info.name.clone());
        self.registry.insert(key, id);
        self.devices.insert(
            info.id.clone(),
            DeviceEntry {
                info: info.clone(),
                node: id,
                control,
                handle: None,
                state: NodeState::Suspended,
                stats: None,
                present: true,
                last_xruns_cell: core::cell::Cell::new((0, 0)),
            },
        );
        self.activate_async(&info.id);
        if let Ok(d) = self.node_descriptor(id) {
            self.notifications.push(Notification::NodeAdded(d));
        }
    }

    /// Ouvre un périphérique en asynchrone (rôle `Async*`). En cas d'échec, reste suspendu.
    fn activate_async(&mut self, id: &DeviceId) {
        let Some(entry) = self.devices.get_mut(id) else {
            return;
        };
        entry.handle = None;
        let info = entry.info.clone();
        let cfg = AsyncPortConfig {
            channels: info.channels,
            graph_rate: self.config.sample_rate,
            device_rate: info.sample_rate,
            quantum: self.config.quantum.get(),
            device_block: info.default_block.max(1),
            target_fill: None,
            quality: self.config.quality,
            dll: Default::default(),
        };
        let format = StreamFormat {
            sample_rate: info.sample_rate,
            channels: info.channels,
            block_frames: info.default_block.max(1),
        };
        let (role, stats, callback): (Role, Arc<AsyncStats>, conduit_backend::AudioCallback) =
            match info.direction {
                DeviceDirection::Capture => {
                    let (mut writer, reader) = input_port(&cfg);
                    let stats = reader.stats();
                    (
                        Role::AsyncCapture(reader),
                        stats,
                        Box::new(move |io: &mut StreamIo<'_>, _: &ClockInfo| {
                            if let Some(input) = io.input {
                                writer.write(input);
                            }
                        }),
                    )
                }
                DeviceDirection::Render => {
                    let (writer, mut reader) = output_port(&cfg);
                    let stats = writer.stats();
                    (
                        Role::AsyncRender(writer),
                        stats,
                        Box::new(move |io: &mut StreamIo<'_>, _: &ClockInfo| {
                            if let Some(out) = io.output.as_deref_mut() {
                                reader.read(out);
                            }
                        }),
                    )
                }
            };
        let opened = self
            .backend
            .open(&info.id, format, callback)
            .and_then(|mut h| h.start().map(|()| h));
        let entry = self.devices.get_mut(id).expect("présent");
        match opened {
            Ok(handle) => {
                entry.handle = Some(handle);
                if entry.control.set_role(role).is_err() {
                    entry.state = NodeState::Suspended;
                    return;
                }
                entry.stats = Some(stats);
                entry.set_last_xruns((0, 0));
                entry.state = NodeState::Active;
                self.notifications.push(Notification::NodeStateChanged {
                    id: entry.node,
                    state: NodeState::Active,
                });
            }
            Err(_) => {
                let _ = entry.control.set_role(Role::Suspended);
                entry.state = NodeState::Suspended;
                entry.stats = None;
            }
        }
    }

    /// Périphérique disparu : ferme, suspend, garde le nœud et ses liens.
    fn suspend_device(&mut self, id: &DeviceId) {
        let Some(entry) = self.devices.get_mut(id) else {
            return;
        };
        entry.present = false;
        entry.handle = None;
        entry.stats = None;
        let _ = entry.control.set_role(Role::Suspended);
        entry.state = NodeState::Suspended;
        let node = entry.node;
        self.notifications.push(Notification::NodeStateChanged {
            id: node,
            state: NodeState::Suspended,
        });
        if matches!(&self.driver, DriverState::Device(d) if d == id) {
            self.driver = DriverState::None;
        }
    }

    // ----- pilote -----

    /// Change le pilote (met à jour la configuration).
    pub fn set_driver(&mut self, choice: DriverChoice) -> Result<(), EngineError> {
        if let DriverChoice::Device { id } = &choice {
            let entry = self
                .devices
                .get(id)
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            if !entry.present {
                return Err(EngineError::CannotDrive {
                    device: id.clone(),
                    reason: "périphérique absent".into(),
                });
            }
            if !entry.info.supports_rate(self.config.sample_rate) {
                return Err(EngineError::CannotDrive {
                    device: id.clone(),
                    reason: format!(
                        "il ne supporte pas {} (natif : {}) ; changez la fréquence du moteur ou choisissez un autre pilote",
                        self.config.sample_rate, entry.info.sample_rate
                    ),
                });
            }
        }
        self.config.driver = choice;
        self.apply_driver_policy()?;
        self.publish()
    }

    fn resolve_driver(&self) -> DriverStatus {
        let candidate = |dir: DeviceDirection| {
            let default = self.backend.default_device(dir);
            self.devices
                .values()
                .filter(|d| {
                    d.present
                        && d.info.direction == dir
                        && d.info.supports_rate(self.config.sample_rate)
                })
                .min_by_key(|d| (Some(&d.info.id) != default.as_ref(), d.info.id.clone()))
                .map(|d| d.info.id.clone())
        };
        match &self.config.driver {
            DriverChoice::Internal => DriverStatus::Internal,
            DriverChoice::Device { id } => match self.devices.get(id) {
                Some(d) if d.present && d.info.supports_rate(self.config.sample_rate) => {
                    DriverStatus::Device { id: id.clone() }
                }
                _ => DriverStatus::Internal,
            },
            DriverChoice::Auto => candidate(DeviceDirection::Render)
                .or_else(|| candidate(DeviceDirection::Capture))
                .map_or(DriverStatus::Internal, |id| DriverStatus::Device { id }),
        }
    }

    /// Applique la politique de pilote : démarre/arrête l'horloge interne, bascule
    /// les rôles des périphériques.
    fn apply_driver_policy(&mut self) -> Result<(), EngineError> {
        let target = self.resolve_driver();
        if target == self.driver_status() {
            return Ok(());
        }
        // 1. Libérer l'ancien pilote.
        match std::mem::replace(&mut self.driver, DriverState::None) {
            DriverState::Internal(mut clock) => clock.stop(),
            DriverState::Device(old) => {
                if let Some(e) = self.devices.get_mut(&old) {
                    e.handle = None;
                }
                if self.devices.get(&old).map(|e| e.present).unwrap_or(false) {
                    self.activate_async(&old);
                }
            }
            DriverState::None => {}
        }
        // 2. Installer le nouveau.
        match target {
            DriverStatus::Internal => {
                let (producer, consumer) = EventQueue::with_capacity(EventQueue::DEFAULT_CAPACITY);
                self.rt_events.push(consumer);
                let clock = InternalClock::start(
                    Arc::clone(&self.executor),
                    self.config.sample_rate,
                    self.config.quantum.get(),
                    Arc::clone(&self.timing),
                    producer,
                );
                self.driver = DriverState::Internal(clock);
            }
            DriverStatus::Device { id } => match self.open_driver(&id) {
                Ok(()) => self.driver = DriverState::Device(id),
                Err(e) => {
                    // Repli : horloge interne.
                    self.notifications
                        .push(Notification::Rt(EngineEvent::ExecutorBusy));
                    let _ = e;
                    self.config.driver = DriverChoice::Internal;
                    return self.apply_driver_policy();
                }
            },
            DriverStatus::None => {}
        }
        let status = self.driver_status();
        self.notifications.push(Notification::DriverChanged(status));
        Ok(())
    }

    fn open_driver(&mut self, id: &DeviceId) -> Result<(), EngineError> {
        let entry = self
            .devices
            .get_mut(id)
            .ok_or_else(|| BackendError::NotFound(id.clone()))?;
        entry.handle = None;
        entry.stats = None;
        let info = entry.info.clone();
        let quantum = self.config.quantum.get();
        let ch = info.channels;
        let format = StreamFormat {
            sample_rate: self.config.sample_rate,
            channels: ch,
            block_frames: quantum,
        };
        let (producer, consumer) = EventQueue::with_capacity(EventQueue::DEFAULT_CAPACITY);
        self.rt_events.push(consumer);
        let (ring_p, ring_c) = RingBuffer::with_capacity::<f32>(quantum * ch * 4);
        let executor = Arc::clone(&self.executor);
        let timing = Arc::clone(&self.timing);
        let dev_id = info.id.clone();
        let (role, callback): (Role, conduit_backend::AudioCallback) = match info.direction {
            DeviceDirection::Render => {
                let mut ring_c = ring_c;
                let mut events = producer;
                let mut first = true;
                (
                    Role::DriverRender(ring_p),
                    Box::new(move |io: &mut StreamIo<'_>, _: &ClockInfo| {
                        let Some(out) = io.output.as_deref_mut() else {
                            return;
                        };
                        let Ok(mut ex) = executor.try_lock() else {
                            out.fill(0.0);
                            events.push(EngineEvent::ExecutorBusy);
                            return;
                        };
                        if first {
                            events.push(EngineEvent::DriverStarted {
                                device: Some(dev_id.clone()),
                            });
                            first = false;
                        }
                        for chunk in out.chunks_mut(quantum * ch) {
                            let frames = chunk.len() / ch;
                            let t0 = Instant::now();
                            ex.run(frames);
                            let dur = t0.elapsed().as_nanos() as u64;
                            if timing.record(dur) {
                                events.push(EngineEvent::CycleOverrun {
                                    cycle: ex.cycle(),
                                    duration_ns: dur,
                                });
                            }
                            let got = ring_c.read(chunk);
                            chunk[got..].fill(0.0);
                        }
                    }),
                )
            }
            DeviceDirection::Capture => {
                let mut ring_p = ring_p;
                let mut events = producer;
                let mut first = true;
                (
                    Role::DriverCapture(ring_c),
                    Box::new(move |io: &mut StreamIo<'_>, _: &ClockInfo| {
                        let Some(input) = io.input else { return };
                        let Ok(mut ex) = executor.try_lock() else {
                            events.push(EngineEvent::ExecutorBusy);
                            return;
                        };
                        if first {
                            events.push(EngineEvent::DriverStarted {
                                device: Some(dev_id.clone()),
                            });
                            first = false;
                        }
                        for chunk in input.chunks(quantum * ch) {
                            ring_p.write(chunk);
                            let t0 = Instant::now();
                            ex.run(chunk.len() / ch);
                            let dur = t0.elapsed().as_nanos() as u64;
                            if timing.record(dur) {
                                events.push(EngineEvent::CycleOverrun {
                                    cycle: ex.cycle(),
                                    duration_ns: dur,
                                });
                            }
                        }
                    }),
                )
            }
        };
        let mut handle = self.backend.open(&info.id, format, callback)?;
        handle.start()?;
        let entry = self.devices.get_mut(id).expect("présent");
        entry.handle = Some(handle);
        if let Err(_role) = entry.control.set_role(role) {
            entry.handle = None;
            return Err(EngineError::CannotDrive {
                device: id.clone(),
                reason: "boîte aux lettres pleine".into(),
            });
        }
        entry.state = NodeState::Driver;
        self.notifications.push(Notification::NodeStateChanged {
            id: entry.node,
            state: NodeState::Driver,
        });
        Ok(())
    }

    /// Arrête le moteur : ferme les périphériques et l'horloge.
    pub fn shutdown(&mut self) {
        if let DriverState::Internal(mut c) = std::mem::replace(&mut self.driver, DriverState::None)
        {
            c.stop();
        }
        for d in self.devices.values_mut() {
            d.handle = None;
            let _ = d.control.set_role(Role::Suspended);
            d.state = NodeState::Suspended;
            d.stats = None;
        }
        let _ = self.publish();
    }
}

impl Engine {
    /// Rapport de diagnostic textuel : versions, périphériques, graphe, compteurs.
    /// Sans chemin utilisateur ni nom de machine.
    pub fn dump(&self) -> String {
        use core::fmt::Write;
        let st = self.status();
        let mut out = String::new();
        let _ = writeln!(out, "conduit-engine {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(out, "backend: {}", st.backend);
        let _ = writeln!(
            out,
            "fréquence: {}  quantum: {}",
            st.sample_rate, st.quantum
        );
        let _ = writeln!(
            out,
            "pilote: {:?} (choix: {:?})",
            st.driver, st.driver_choice
        );
        let _ = writeln!(
            out,
            "cycles: {}  position: {}  temps de cycle min/moy/max: {}/{}/{} µs  budget: {} µs  dépassements: {}",
            st.cycles,
            st.position,
            st.timing.min_ns / 1000,
            st.timing.avg_ns / 1000,
            st.timing.max_ns / 1000,
            st.timing.budget_ns / 1000,
            st.timing.overruns
        );
        let _ = writeln!(out, "xruns: {}", st.xruns);
        let _ = writeln!(out, "périphériques ({}):", st.devices.len());
        for d in &st.devices {
            let _ = writeln!(
                out,
                "  {} → {} {:?} sous-alim={} débord={} remplissage={} ratio={:.6} verrouillé={}",
                d.id, d.node, d.state, d.underruns, d.overruns, d.fill, d.ratio, d.locked
            );
        }
        let nodes = self.nodes();
        let _ = writeln!(out, "nœuds ({}):", nodes.len());
        for n in &nodes {
            let _ = writeln!(
                out,
                "  {} [{}] {:?} \"{}\" in={} out={} gain={} muet={}",
                n.id,
                n.type_name,
                n.state,
                n.label,
                n.inputs.len(),
                n.outputs.len(),
                n.gain_db,
                n.muted
            );
        }
        let links = self.links();
        let _ = writeln!(out, "liens ({}):", links.len());
        for l in &links {
            let _ = writeln!(
                out,
                "  {} {} → {} gain={} muet={}",
                l.link.id, l.link.src, l.link.dst, l.gain_db, l.muted
            );
        }
        out
    }
}

impl From<EngineError> for ProtocolError {
    fn from(e: EngineError) -> Self {
        let code = match &e {
            EngineError::Graph(GraphError::WouldCycle { .. }) => ErrorCode::WouldCycle,
            EngineError::Graph(GraphError::UnknownNode(_))
            | EngineError::Graph(GraphError::UnknownPort(_))
            | EngineError::Graph(GraphError::UnknownLink(_))
            | EngineError::UnknownKey(_)
            | EngineError::UnknownPortName { .. } => ErrorCode::NotFound,
            EngineError::Graph(_)
            | EngineError::DuplicateName(_)
            | EngineError::InvalidParam { .. }
            | EngineError::DevicePresent(_) => ErrorCode::Invalid,
            EngineError::Backend(BackendError::NotFound(_)) => ErrorCode::NotFound,
            EngineError::Backend(_) | EngineError::CannotDrive { .. } => ErrorCode::Device,
            EngineError::Cable(_) | EngineError::NoCableControl(_) => ErrorCode::Cable,
            EngineError::Publish(_) => ErrorCode::Busy,
            EngineError::Unsupported(_) => ErrorCode::Unsupported,
        };
        ProtocolError::new(code, e.to_string())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl DeviceEntry {
    fn last_xruns(&self) -> (u64, u64) {
        self.last_xruns_cell.get()
    }
    fn set_last_xruns(&self, v: (u64, u64)) {
        self.last_xruns_cell.set(v);
    }
}
