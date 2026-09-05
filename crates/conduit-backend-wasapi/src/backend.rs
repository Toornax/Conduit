//! [`WasapiBackend`] : l'implémentation de [`Backend`] qui dialogue avec le fil MMDevice.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use conduit_backend::{
    AudioCallback, Backend, BackendError, CableControl, DeviceDirection, DeviceHandle, DeviceId,
    DeviceInfo, EventReceiver, StreamFormat,
};

use crate::mmdevice_thread::{Command, Message};
use crate::stream::WasapiHandle;

/// Délai au-delà duquel on considère que le fil MMDevice ne répond plus.
///
/// Une énumération interroge chaque endpoint (`GetMixFormat`, `GetDevicePeriod`) et
/// une ouverture initialise un `IAudioClient` : quelques dizaines de millisecondes,
/// loin de cette borne.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Backend Windows (MMDevice + WASAPI).
///
/// Créé par [`WasapiBackend::new`], qui ne rend la main qu'une fois le fil MMDevice
/// prêt : COM initialisé, client de notification enregistré, énumération initiale
/// faite. Le backend est `Send` : il ne détient que l'émetteur du canal et la
/// poignée du fil.
pub struct WasapiBackend {
    sender: mpsc::Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl core::fmt::Debug for WasapiBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WasapiBackend")
            .field("thread_alive", &self.thread.is_some())
            .finish()
    }
}

impl WasapiBackend {
    /// Démarre le fil MMDevice et attend qu'il soit prêt.
    ///
    /// # Erreurs
    ///
    /// [`BackendError::Platform`] si COM, l'énumérateur ou l'énumération initiale
    /// échouent ; le message nomme l'appel fautif et son `HRESULT`.
    pub fn new() -> Result<Self, BackendError> {
        let (sender, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = {
            let sender = sender.clone();
            std::thread::Builder::new()
                .name("conduit-mmdevice".into())
                .spawn(move || crate::mmdevice_thread::run(receiver, sender, ready_tx))
                .map_err(|e| BackendError::Platform(format!("fil MMDevice non démarré : {e}")))?
        };
        match ready_rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                sender,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = sender.send(Message::Command(Command::Shutdown));
                let _ = thread.join();
                Err(BackendError::Platform(format!(
                    "le fil MMDevice n'a pas répondu en {} s : COM ou le service audio \
                     Windows (AudioSrv) est-il bloqué ?",
                    REPLY_TIMEOUT.as_secs()
                )))
            }
        }
    }

    /// Comme [`Backend::open`], mais rend le type concret : utile pour
    /// [`WasapiHandle::latency`] et [`WasapiHandle::rt_outcome`], hors trait.
    pub fn open_handle(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<WasapiHandle, BackendError> {
        let id = id.clone();
        let opened = self.request(
            move |reply| Command::Open { id, format, reply },
            "ouverture",
        )??;
        WasapiHandle::new(opened, callback)
    }

    /// Envoie une commande au fil MMDevice et attend sa réponse.
    fn request<T: Send + 'static>(
        &self,
        make: impl FnOnce(mpsc::Sender<T>) -> Command,
        what: &str,
    ) -> Result<T, BackendError> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Message::Command(make(tx)))
            .map_err(|_| thread_gone(what))?;
        rx.recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| thread_gone(what))
    }
}

fn thread_gone(what: &str) -> BackendError {
    BackendError::Platform(format!(
        "le fil MMDevice ne répond plus ({what}) : recréez le backend"
    ))
}

impl Drop for WasapiBackend {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Command(Command::Shutdown));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Backend for WasapiBackend {
    fn name(&self) -> &'static str {
        "wasapi"
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        self.request(|reply| Command::Enumerate { reply }, "énumération")?
    }

    fn default_device(&self, direction: DeviceDirection) -> Option<DeviceId> {
        self.request(
            |reply| Command::DefaultDevice { direction, reply },
            "périphérique par défaut",
        )
        .ok()?
        .ok()?
    }

    /// Ouvre un flux en mode partagé, événementiel, au format demandé (voir
    /// `open` et `stream`). La poignée rendue est un [`WasapiHandle`] ; le rappel
    /// n'est appelé qu'après [`DeviceHandle::start`].
    ///
    /// # Erreurs
    ///
    /// [`BackendError::NotFound`] si l'endpoint n'existe pas ou n'est pas actif,
    /// [`BackendError::UnsupportedFormat`] pour zéro canal ou un format que le
    /// moteur refuse malgré la conversion, [`BackendError::Platform`] sinon.
    fn open(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<Box<dyn DeviceHandle>, BackendError> {
        Ok(Box::new(self.open_handle(id, format, callback)?))
    }

    fn subscribe(&mut self) -> EventReceiver {
        self.request(|reply| Command::Subscribe { reply }, "abonnement")
            .unwrap_or_else(|_| {
                // Fil parti : un récepteur dont l'émetteur est déjà détruit rend
                // `Disconnected` immédiatement, ce qui est la vérité.
                let (_tx, rx) = mpsc::channel();
                rx
            })
    }

    /// Le contrôle des câbles passe par le service d'assistance (M1b-34).
    fn cable_control(&mut self) -> Option<&mut dyn CableControl> {
        None
    }
}
