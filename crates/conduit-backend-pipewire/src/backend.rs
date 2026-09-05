//! [`PipewireBackend`] : l'implémentation de [`Backend`] et sa poignée de flux.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::Duration;

use conduit_backend::event::EventBroadcaster;
use conduit_backend::{
    AudioCallback, Backend, BackendError, CableControl, ClockInfo, DeviceDirection, DeviceHandle,
    DeviceId, DeviceInfo, EventReceiver, StreamFormat,
};
use pipewire as pw;

use crate::devices::Shared;
use crate::loop_thread::{remote_label, Command, OpenRequest, StreamKey};
use crate::stream::StreamShared;

/// Délai au-delà duquel on considère que le fil de boucle ne répond plus.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// `pw_init()` n'est appelé qu'une fois par processus.
static INIT: Once = Once::new();

/// Backend PipeWire.
///
/// Créé par [`PipewireBackend::connect`], qui ne rend la main qu'une fois
/// l'énumération initiale des nœuds terminée : [`Backend::devices`] est exploitable
/// immédiatement.
pub struct PipewireBackend {
    remote: String,
    shared: Arc<Mutex<Shared>>,
    events: Arc<Mutex<EventBroadcaster>>,
    sender: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    /// Faux dès que le fil de boucle s'est arrêté : les poignées survivantes
    /// n'attendent alors plus une réponse qui ne viendra jamais.
    alive: Arc<AtomicBool>,
    next_key: AtomicU64,
}

impl core::fmt::Debug for PipewireBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let devices = self.shared.lock().map(|s| s.list().len()).unwrap_or(0);
        f.debug_struct("PipewireBackend")
            .field("remote", &self.remote)
            .field("devices", &devices)
            .finish()
    }
}

impl PipewireBackend {
    /// Se connecte au démon PipeWire.
    ///
    /// `remote` est le nom du démon (`pipewire-0` par défaut) ; `None` laisse
    /// PipeWire lire la variable d'environnement `PIPEWIRE_REMOTE`.
    ///
    /// # Erreurs
    ///
    /// [`BackendError::Platform`] si le démon est injoignable ; le message dit quoi
    /// vérifier.
    pub fn connect(remote: Option<&str>) -> Result<Self, BackendError> {
        INIT.call_once(pw::init);
        let label = remote_label(remote);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let events = Arc::new(Mutex::new(EventBroadcaster::default()));
        let (sender, receiver) = pw::channel::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread = {
            let remote = remote.map(str::to_string);
            let shared = Arc::clone(&shared);
            let events = Arc::clone(&events);
            std::thread::Builder::new()
                .name("conduit-pipewire".into())
                .spawn(move || {
                    crate::loop_thread::run(remote, shared, events, receiver, ready_tx);
                })
                .map_err(|e| BackendError::Platform(format!("fil PipeWire non démarré : {e}")))?
        };

        let outcome = ready_rx.recv_timeout(REPLY_TIMEOUT);
        match outcome {
            Ok(Ok(())) => Ok(Self {
                remote: label,
                shared,
                events,
                sender,
                thread: Some(thread),
                alive: Arc::new(AtomicBool::new(true)),
                next_key: AtomicU64::new(1),
            }),
            Ok(Err(reason)) => {
                let _ = thread.join();
                Err(BackendError::Platform(format!(
                    "démon PipeWire injoignable sur « {label} » : est-il démarré ? \
                     (vérifiez `systemctl --user status pipewire` et la variable \
                     PIPEWIRE_REMOTE) — {reason}"
                )))
            }
            Err(_) => {
                let _ = sender.send(Command::Quit);
                let _ = thread.join();
                Err(BackendError::Platform(format!(
                    "démon PipeWire injoignable sur « {label} » : aucune réponse en \
                     {} s, est-il démarré ?",
                    REPLY_TIMEOUT.as_secs()
                )))
            }
        }
    }

    /// Nom du démon auquel ce backend est connecté.
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// Envoie une commande au fil de boucle et attend sa réponse.
    fn request<T: Send + 'static>(
        &self,
        make: impl FnOnce(mpsc::Sender<T>) -> Command,
        what: &str,
    ) -> Result<T, BackendError> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(make(tx))
            .map_err(|_| self.loop_gone(what))?;
        rx.recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| self.loop_gone(what))
    }

    fn loop_gone(&self, what: &str) -> BackendError {
        BackendError::Platform(format!(
            "le fil PipeWire de « {} » ne répond plus ({what}) : reconnectez le backend",
            self.remote
        ))
    }
}

impl Drop for PipewireBackend {
    fn drop(&mut self) {
        // `Quit` ferme d'abord tous les flux encore ouverts : une poignée détruite
        // après le backend n'a plus rien à faire.
        let _ = self.sender.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.alive.store(false, Ordering::Release);
    }
}

impl Backend for PipewireBackend {
    fn name(&self) -> &'static str {
        "pipewire"
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(self.shared.lock().expect("registre").list())
    }

    fn default_device(&self, direction: DeviceDirection) -> Option<DeviceId> {
        self.shared
            .lock()
            .expect("registre")
            .default_device(direction)
    }

    fn open(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<Box<dyn DeviceHandle>, BackendError> {
        let info = {
            let shared = self.shared.lock().expect("registre");
            shared
                .get(id)
                .cloned()
                .ok_or_else(|| BackendError::NotFound(id.clone()))?
        };
        if format.channels == 0 {
            return Err(BackendError::UnsupportedFormat {
                device: id.clone(),
                reason: "un flux doit avoir au moins un canal".into(),
            });
        }
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        let stream_shared = Arc::new(StreamShared::default());
        let request_shared = Arc::clone(&stream_shared);
        let id_for_request = id.clone();
        self.request(
            move |reply| {
                Command::Open(Box::new(OpenRequest {
                    key,
                    id: id_for_request,
                    format,
                    callback,
                    shared: request_shared,
                    reply,
                }))
            },
            "ouverture",
        )??;
        Ok(Box::new(PipewireHandle {
            info,
            format,
            key,
            shared: stream_shared,
            sender: self.sender.clone(),
            alive: Arc::clone(&self.alive),
            remote: self.remote.clone(),
        }))
    }

    fn subscribe(&mut self) -> EventReceiver {
        self.events.lock().expect("abonnés").subscribe()
    }

    /// PipeWire ne gère pas encore les câbles Conduit (tâche M3-05).
    fn cable_control(&mut self) -> Option<&mut dyn CableControl> {
        None
    }
}

/// Poignée d'un flux PipeWire ouvert.
///
/// Toutes les opérations sont exécutées sur le fil de la boucle PipeWire ; la
/// poignée ne fait qu'envoyer un ordre et attendre l'accusé de réception.
pub struct PipewireHandle {
    info: DeviceInfo,
    format: StreamFormat,
    key: StreamKey,
    shared: Arc<StreamShared>,
    sender: pw::channel::Sender<Command>,
    alive: Arc<AtomicBool>,
    remote: String,
}

impl PipewireHandle {
    fn set_active(&self, active: bool) -> Result<(), BackendError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(self.loop_gone());
        }
        let key = self.key;
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Command::SetActive {
                key,
                active,
                reply: tx,
            })
            .map_err(|_| self.loop_gone())?;
        rx.recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| self.loop_gone())?
    }

    fn loop_gone(&self) -> BackendError {
        BackendError::Platform(format!(
            "le fil PipeWire de « {} » ne répond plus : le flux {} est perdu",
            self.remote, self.info.id
        ))
    }
}

impl core::fmt::Debug for PipewireHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PipewireHandle")
            .field("device", &self.info.id)
            .field("format", &self.format)
            .field("running", &self.shared.is_running())
            .finish()
    }
}

impl DeviceHandle for PipewireHandle {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn format(&self) -> StreamFormat {
        self.format
    }

    fn start(&mut self) -> Result<(), BackendError> {
        if self.shared.is_disconnected() {
            return Err(BackendError::Disconnected(self.info.id.clone()));
        }
        self.set_active(true)
    }

    fn stop(&mut self) -> Result<(), BackendError> {
        self.set_active(false)
    }

    fn is_running(&self) -> bool {
        self.shared.is_running()
    }

    fn clock(&self) -> ClockInfo {
        self.shared.clock()
    }
}

impl Drop for PipewireHandle {
    fn drop(&mut self) {
        if !self.alive.load(Ordering::Acquire) {
            // Le backend est déjà détruit : le fil de boucle a fermé les flux.
            return;
        }
        let (tx, rx) = mpsc::channel();
        if self
            .sender
            .send(Command::Close {
                key: self.key,
                reply: tx,
            })
            .is_ok()
        {
            // Attendre l'accusé garantit qu'aucun rappel ne peut plus survenir.
            let _ = rx.recv_timeout(REPLY_TIMEOUT);
        }
    }
}
