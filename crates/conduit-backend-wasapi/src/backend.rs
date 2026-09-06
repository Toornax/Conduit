//! [`WasapiBackend`] : l'implémentation de [`Backend`] qui dialogue avec le fil MMDevice.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use conduit_backend::{
    AudioCallback, Backend, BackendError, CableControl, DeviceDirection, DeviceHandle, DeviceId,
    DeviceInfo, EventReceiver, StreamFormat,
};

use crate::exclusive::ExclusivePolicy;
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
    /// Ce que les prochaines ouvertures font du mode exclusif (M1b-32).
    policy: ExclusivePolicy,
}

impl core::fmt::Debug for WasapiBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WasapiBackend")
            .field("thread_alive", &self.thread.is_some())
            .field("exclusive_policy", &self.policy)
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
                policy: ExclusivePolicy::default(),
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

    /// Ce que les prochaines ouvertures feront du **mode exclusif** WASAPI.
    ///
    /// Réglage propre à ce backend, volontairement **hors** du trait
    /// [`Backend`] : celui-ci est portable (PipeWire, CoreAudio) et ne doit pas
    /// gagner une notion Windows. Le défaut est [`ExclusivePolicy::Never`] — un
    /// câble Conduit doit coexister avec les autres applications, et un flux
    /// exclusif prend le périphérique pour lui seul (SPEC §5.6 : le mode exclusif
    /// est un bonus de latence, pas la norme). Voir le module `exclusive`.
    ///
    /// Le changement ne concerne que les ouvertures suivantes : les flux déjà
    /// ouverts gardent leur mode.
    pub fn set_exclusive_policy(&mut self, policy: ExclusivePolicy) {
        self.policy = policy;
    }

    /// Politique de mode exclusif en vigueur ([`Self::set_exclusive_policy`]).
    pub fn exclusive_policy(&self) -> ExclusivePolicy {
        self.policy
    }

    /// Comme [`Backend::open`], mais rend le type concret : utile pour
    /// [`WasapiHandle::latency`], [`WasapiHandle::share_mode`] et
    /// [`WasapiHandle::rt_outcome`], hors trait.
    pub fn open_handle(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<WasapiHandle, BackendError> {
        self.open_any(id, format, callback, false)
    }

    /// Ouvre un endpoint de **rendu** en **capture d'écho**
    /// (`AUDCLNT_STREAMFLAGS_LOOPBACK`, module `loopback`).
    ///
    /// Le flux prélève le mélange que le moteur audio de Windows écrit vers cet
    /// endpoint, **avant** que le pilote du périphérique ne le consomme, et se
    /// comporte pour le reste comme une capture ordinaire : le rappel reçoit des
    /// trames d'entrée (`StreamIo::input`), jamais de sortie.
    ///
    /// C'est **l'outil de diagnostic** d'une chaîne muette : entendre le signal en
    /// écho prouve que le moteur délivre et met la moitié amont hors de cause ; un
    /// écho silencieux prouve l'inverse et disculpe le pilote.
    ///
    /// Méthode **hors du trait [`Backend`]**, comme
    /// [`Self::set_exclusive_policy`] : le trait est portable (PipeWire,
    /// CoreAudio) et ne doit pas gagner une notion propre à Windows.
    ///
    /// La poignée rendue porte toujours le [`DeviceInfo`] de l'endpoint de rendu
    /// (`direction` = rendu, c'est bien ce qu'il est) ; son
    /// [`WasapiHandle::latency`] rend un [`InitPath::Loopback`].
    ///
    /// # Erreurs
    ///
    /// [`BackendError::NotFound`] si l'endpoint n'existe pas ou n'est pas actif ;
    /// [`BackendError::UnsupportedFormat`] si l'endpoint est une **capture**, si la
    /// politique du backend demande le mode **exclusif** (incompatible avec l'écho)
    /// ou si le moteur refuse le format demandé — le message nomme alors le format
    /// de mixage à demander ; [`BackendError::Platform`] sinon.
    ///
    /// [`InitPath::Loopback`]: crate::InitPath::Loopback
    pub fn open_loopback(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<WasapiHandle, BackendError> {
        self.open_any(id, format, callback, true)
    }

    /// Corps commun de [`Self::open_handle`] et [`Self::open_loopback`].
    fn open_any(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
        loopback: bool,
    ) -> Result<WasapiHandle, BackendError> {
        let id = id.clone();
        let policy = self.policy;
        let opened = self.request(
            move |reply| Command::Open {
                id,
                format,
                policy,
                loopback,
                reply,
            },
            if loopback {
                "ouverture en écho"
            } else {
                "ouverture"
            },
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

    /// Ouvre un flux événementiel au format demandé (voir `open` et `stream`), en
    /// mode partagé sauf si [`Self::set_exclusive_policy`] a demandé l'exclusif.
    /// La poignée rendue est un [`WasapiHandle`] ; le rappel n'est appelé qu'après
    /// [`DeviceHandle::start`].
    ///
    /// # Erreurs
    ///
    /// [`BackendError::NotFound`] si l'endpoint n'existe pas ou n'est pas actif,
    /// [`BackendError::UnsupportedFormat`] pour zéro canal, pour un format que le
    /// moteur refuse malgré la conversion, ou pour un mode exclusif exigé
    /// ([`ExclusivePolicy::Required`]) mais refusé par le matériel ou déjà pris ;
    /// [`BackendError::Platform`] sinon.
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
