//! Fil de gestion : possède le moteur, exécute les commandes, applique les règles,
//! persiste l'état, surveille le fil audio (M0-83 à M0-87).

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use conduit_engine::{
    Command, DriverChoice, DriverStatus, Engine, NodeDescriptor, NodeKey, NodeState, Notification,
    Reply,
};
use conduit_protocol::api::{ErrorCode, ProtocolError};
use tokio::sync::{broadcast, oneshot};

use crate::autoconnect::AutoConnectRule;
use crate::persist::{apply_link, restore, PersistedLink, PersistedState};

/// Options du service.
#[derive(Debug, Clone)]
pub struct ServiceOptions {
    /// Fichier d'état (`None` = pas de persistance).
    pub state_file: Option<PathBuf>,
    /// Règles d'auto-connexion.
    pub rules: Vec<AutoConnectRule>,
    /// Surveiller le fil audio et redémarrer le pilote s'il se bloque.
    pub watchdog: bool,
    /// Délai de regroupement des sauvegardes.
    pub save_debounce: Duration,
    /// Période de `tick`.
    pub tick_period: Duration,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        Self {
            state_file: None,
            rules: Vec::new(),
            watchdog: true,
            save_debounce: Duration::from_millis(500),
            tick_period: Duration::from_millis(20),
        }
    }
}

enum Msg {
    Command(Command, oneshot::Sender<Result<Reply, ProtocolError>>),
    Shutdown,
}

/// Poignée du fil de gestion.
pub struct Service {
    tx: mpsc::Sender<Msg>,
    events: broadcast::Sender<Notification>,
    thread: Option<JoinHandle<()>>,
}

impl core::fmt::Debug for Service {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Service")
            .field("subscribers", &self.events.receiver_count())
            .finish()
    }
}

/// Capacité du canal de diffusion des notifications.
pub const EVENT_CAPACITY: usize = 1024;

impl Service {
    /// Démarre le fil de gestion avec un moteur. Restaure l'état persisté s'il existe.
    pub fn start(engine: Engine, options: ServiceOptions) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let events2 = events.clone();
        let thread = std::thread::Builder::new()
            .name("conduitd-manager".into())
            .spawn(move || Manager::new(engine, options, events2).run(rx))
            .expect("fil de gestion");
        Self {
            tx,
            events,
            thread: Some(thread),
        }
    }

    /// Exécute une commande (asynchrone).
    pub async fn execute(&self, cmd: Command) -> Result<Reply, ProtocolError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self.tx.send(Msg::Command(cmd, reply_tx)).is_err() {
            return Err(ProtocolError::new(
                ErrorCode::Internal,
                "le moteur est arrêté",
            ));
        }
        reply_rx.await.unwrap_or_else(|_| {
            Err(ProtocolError::new(
                ErrorCode::Internal,
                "le moteur n'a pas répondu",
            ))
        })
    }

    /// Exécute une commande (bloquant, hors runtime async).
    pub fn execute_blocking(&self, cmd: Command) -> Result<Reply, ProtocolError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self.tx.send(Msg::Command(cmd, reply_tx)).is_err() {
            return Err(ProtocolError::new(
                ErrorCode::Internal,
                "le moteur est arrêté",
            ));
        }
        reply_rx.blocking_recv().unwrap_or_else(|_| {
            Err(ProtocolError::new(
                ErrorCode::Internal,
                "le moteur n'a pas répondu",
            ))
        })
    }

    /// S'abonne aux notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    /// Arrête le fil de gestion (sauvegarde finale, fermeture des périphériques).
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Manager {
    engine: Engine,
    options: ServiceOptions,
    events: broadcast::Sender<Notification>,
    pending_links: Vec<PersistedLink>,
    dirty: bool,
    last_change: Instant,
    last_watchdog: Instant,
    last_cycles: u64,
    stalled_checks: u32,
}

impl Manager {
    fn new(
        mut engine: Engine,
        options: ServiceOptions,
        events: broadcast::Sender<Notification>,
    ) -> Self {
        let mut pending_links = Vec::new();
        if let Some(path) = &options.state_file {
            match PersistedState::load(path) {
                Ok(Some(state)) => {
                    let report = restore(&mut engine, &state);
                    tracing::info!(
                        internal = report.internal_nodes,
                        links = report.links,
                        pending = report.pending.len(),
                        "état restauré"
                    );
                    for e in &report.errors {
                        tracing::warn!("restauration : {e}");
                    }
                    pending_links = report.pending;
                }
                Ok(None) => tracing::info!("pas d'état persisté, démarrage à vide"),
                Err(e) => tracing::warn!("{e}"),
            }
        }
        let cycles = engine.status().cycles;
        let mut m = Self {
            engine,
            options,
            events,
            pending_links,
            dirty: false,
            last_change: Instant::now(),
            last_watchdog: Instant::now(),
            last_cycles: cycles,
            stalled_checks: 0,
        };
        // Règles sur les nœuds déjà présents.
        let nodes = m.engine.nodes();
        for n in nodes {
            m.on_node_available(&n);
        }
        m.drain_notifications();
        m
    }

    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            match rx.recv_timeout(self.options.tick_period) {
                Ok(Msg::Command(cmd, reply)) => {
                    let r = self.handle(cmd);
                    let _ = reply.send(r);
                }
                Ok(Msg::Shutdown) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            self.drain_notifications();
            self.watchdog();
            if self.dirty && self.last_change.elapsed() >= self.options.save_debounce {
                self.save();
            }
        }
        // Sauvegarde finale inconditionnelle : l'état sur disque reflète toujours le
        // dernier arrêt propre.
        self.save();
        let _ = self.events.send(Notification::Shutdown);
        self.engine.shutdown();
    }

    fn handle(&mut self, cmd: Command) -> Result<Reply, ProtocolError> {
        let mutating = !matches!(
            cmd,
            Command::Status
                | Command::Nodes
                | Command::Ports { .. }
                | Command::Links
                | Command::ReadMeter { .. }
                | Command::CableList
                | Command::Dump
                | Command::Subscribe { .. }
                | Command::Save
                | Command::Load
        );
        let result = match cmd {
            Command::Save => {
                self.save();
                Ok(Reply::Ok)
            }
            Command::Load => self.load().map(|()| Reply::Ok),
            Command::Dump => Ok(Reply::Dump { text: self.dump() }),
            Command::Subscribe { .. } => Ok(Reply::Ok),
            other => self.engine.execute(other).map_err(ProtocolError::from),
        };
        if mutating && result.is_ok() {
            self.mark_dirty();
        }
        self.drain_notifications();
        result
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    fn save(&mut self) {
        if let Some(path) = &self.options.state_file {
            let state = PersistedState::capture(&self.engine);
            match state.save_atomic(path) {
                Ok(()) => tracing::debug!("état sauvegardé"),
                Err(e) => tracing::error!("sauvegarde impossible : {e}"),
            }
        }
        self.dirty = false;
    }

    fn load(&mut self) -> Result<(), ProtocolError> {
        let Some(path) = &self.options.state_file else {
            return Err(ProtocolError::new(
                ErrorCode::Unsupported,
                "persistance désactivée",
            ));
        };
        let state = PersistedState::load(path)
            .map_err(|e| ProtocolError::new(ErrorCode::Internal, e.to_string()))?
            .ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "aucun état persisté"))?;
        let report = restore(&mut self.engine, &state);
        self.pending_links = report.pending;
        Ok(())
    }

    fn dump(&self) -> String {
        let mut out = format!(
            "conduitd {} ({} {})\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        out.push_str(&format!(
            "règles d'auto-connexion : {}\n",
            self.options.rules.len()
        ));
        out.push_str(&format!(
            "liens en attente : {}\n",
            self.pending_links.len()
        ));
        out.push_str(&format!(
            "persistance : {}\n",
            if self.options.state_file.is_some() {
                "activée"
            } else {
                "désactivée"
            }
        ));
        out.push_str(&self.engine.dump());
        out
    }

    fn drain_notifications(&mut self) {
        let notes = self.engine.tick();
        for n in notes {
            match &n {
                Notification::NodeAdded(desc) => {
                    let desc = desc.clone();
                    self.on_node_available(&desc);
                }
                Notification::NodeStateChanged {
                    id,
                    state: NodeState::Active | NodeState::Driver,
                } => {
                    if let Ok(desc) = self.engine.node_descriptor(*id) {
                        self.on_node_available(&desc);
                    }
                }
                Notification::LinkAdded(_)
                | Notification::LinkRemoved { .. }
                | Notification::NodeRemoved { .. } => {
                    self.mark_dirty();
                }
                _ => {}
            }
            let _ = self.events.send(n);
        }
    }

    /// Un nœud vient d'apparaître ou de redevenir actif : liens en attente et règles.
    fn on_node_available(&mut self, node: &NodeDescriptor) {
        // Liens persistés en attente.
        let mut still_pending = Vec::new();
        for l in std::mem::take(&mut self.pending_links) {
            if l.src == node.key || l.dst == node.key {
                match apply_link(&mut self.engine, &l) {
                    Ok(true) => tracing::info!(
                        "lien restauré {}:{} → {}:{}",
                        l.src,
                        l.src_port,
                        l.dst,
                        l.dst_port
                    ),
                    Ok(false) => still_pending.push(l),
                    Err(e) => tracing::warn!("lien en attente impossible : {e}"),
                }
            } else {
                still_pending.push(l);
            }
        }
        self.pending_links = still_pending;
        // Règles d'auto-connexion.
        let rules = self.options.rules.clone();
        let all = self.engine.nodes();
        for rule in &rules {
            let mut plans = Vec::new();
            if rule.matches(node) {
                for target in all
                    .iter()
                    .filter(|t| t.id != node.id && rule.target_matches(t))
                {
                    plans.extend(rule.plan(node, target));
                }
            }
            if rule.target_matches(node) {
                for trigger in all.iter().filter(|t| t.id != node.id && rule.matches(t)) {
                    plans.extend(rule.plan(trigger, node));
                }
            }
            for p in plans {
                let cmd = Command::LinkByName {
                    src_node: p.src,
                    src_port: p.src_port,
                    dst_node: p.dst,
                    dst_port: p.dst_port,
                };
                match self.engine.execute(cmd) {
                    Ok(_) => tracing::info!("auto-connexion appliquée pour {}", node.key),
                    Err(conduit_engine::EngineError::Graph(
                        conduit_core::graph::GraphError::DuplicateLink(_),
                    )) => {}
                    Err(e) => tracing::warn!("auto-connexion : {e}"),
                }
            }
        }
    }

    /// Détecte un fil audio bloqué (> 1 s sans cycle avec un pilote périphérique) et
    /// redémarre le pilote.
    fn watchdog(&mut self) {
        if !self.options.watchdog || self.last_watchdog.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_watchdog = Instant::now();
        let status = self.engine.status();
        let device_driver = matches!(status.driver, DriverStatus::Device { .. });
        if device_driver && status.cycles == self.last_cycles {
            self.stalled_checks += 1;
            if self.stalled_checks >= 1 {
                tracing::error!(
                    "fil audio bloqué depuis plus d'une seconde : redémarrage du pilote"
                );
                let choice = self.engine.config().driver.clone();
                let _ = self.engine.set_driver(DriverChoice::Internal);
                if let Err(e) = self.engine.set_driver(choice) {
                    tracing::warn!("pilote non rétabli ({e}) : horloge interne");
                }
                let _ = self
                    .events
                    .send(Notification::DriverChanged(self.engine.driver_status()));
                self.stalled_checks = 0;
            }
        } else {
            self.stalled_checks = 0;
        }
        self.last_cycles = status.cycles;
    }
}

/// Clé d'un nœud décrit, utilitaire pour les journaux.
pub fn key_of(node: &NodeDescriptor) -> &NodeKey {
    &node.key
}
