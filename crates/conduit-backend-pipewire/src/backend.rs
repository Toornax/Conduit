//! [`PipewireBackend`] : l'implémentation de [`Backend`] et sa poignée de flux.

use std::sync::{mpsc, Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::Duration;

use conduit_backend::event::EventBroadcaster;
use conduit_backend::{
    AudioCallback, Backend, BackendError, CableControl, DeviceDirection, DeviceHandle, DeviceId,
    DeviceInfo, EventReceiver, StreamFormat,
};
use pipewire as pw;

use crate::devices::Shared;
use crate::loop_thread::{remote_label, Command};

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
}

impl Drop for PipewireBackend {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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

    /// L'ouverture de flux arrive avec la tâche M3-03.
    fn open(
        &mut self,
        id: &DeviceId,
        _format: StreamFormat,
        _callback: AudioCallback,
    ) -> Result<Box<dyn DeviceHandle>, BackendError> {
        let shared = self.shared.lock().expect("registre");
        if shared.get(id).is_none() {
            return Err(BackendError::NotFound(id.clone()));
        }
        Err(BackendError::Platform(
            "le backend PipeWire n'ouvre pas encore de flux (tâche M3-03)".into(),
        ))
    }

    fn subscribe(&mut self) -> EventReceiver {
        self.events.lock().expect("abonnés").subscribe()
    }

    /// PipeWire ne gère pas encore les câbles Conduit (tâche M3-05).
    fn cable_control(&mut self) -> Option<&mut dyn CableControl> {
        None
    }
}
