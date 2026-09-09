//! [`WasapiBackend`] : l'implémentation de [`Backend`] qui dialogue avec le fil MMDevice.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use conduit_backend::{
    AudioCallback, Backend, BackendError, CableControl, CableError, CableId, CableInfo, CableSpec,
    DeviceDirection, DeviceHandle, DeviceId, DeviceInfo, EventReceiver, StreamFormat,
};
use conduit_core::types::ChannelCount;

use crate::devices::{cable_name, CableName, EndpointInfo};
use crate::exclusive::ExclusivePolicy;
use crate::lowlat::SharedPeriod;
use crate::mmdevice_thread::{Command, Message};
use crate::stream::WasapiHandle;

/// Délai au-delà duquel on considère que le fil MMDevice ne répond plus.
///
/// Une énumération interroge chaque endpoint (`GetMixFormat`, `GetDevicePeriod`) et
/// une ouverture initialise un `IAudioClient` : quelques dizaines de millisecondes,
/// loin de cette borne.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Ce que rend une opération sur les câbles quand personne n'a appelé
/// [`WasapiBackend::set_cable_control`].
///
/// Le cas n'arrive que si le dorsal est construit sans être relié au service : c'est un
/// défaut de montage, pas une panne de la machine, et le message le dit. Le démon, lui,
/// fait la liaison au démarrage (`conduitd`, `native_backend`).
const SANS_CONTROLE: &str =
    "ce dorsal n'a pas reçu de contrôle des câbles : le démon ne l'a pas relié au service \
     d'assistance Conduit";

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
    /// Ce que les prochaines ouvertures **partagées** demandent comme période
    /// (module `lowlat`). Défaut : au choix du moteur, comme avant.
    period: SharedPeriod,
    /// Le contrôle des câbles, **injecté** : voir [`WasapiBackend::set_cable_control`].
    cables: Option<Box<dyn CableControl + Send>>,
}

impl core::fmt::Debug for WasapiBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WasapiBackend")
            .field("thread_alive", &self.thread.is_some())
            .field("exclusive_policy", &self.policy)
            .field("shared_period", &self.period)
            .field("cable_control", &self.cables.is_some())
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
                period: SharedPeriod::default(),
                cables: None,
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

    /// Quelle **période** les prochaines ouvertures **partagées** demanderont au
    /// moteur audio (module `lowlat`).
    ///
    /// Réglage propre à ce backend, hors du trait [`Backend`] pour la même raison
    /// que [`Self::set_exclusive_policy`] : le trait est portable. Le défaut est
    /// [`SharedPeriod::Default`], c'est-à-dire le comportement de toujours — la
    /// période que le moteur choisit, et le chemin `IAudioClient3` seulement quand
    /// il se présente.
    ///
    /// [`SharedPeriod::Minimal`] et [`SharedPeriod::Requested`] **exigent**
    /// `IAudioClient3::InitializeSharedAudioStream` au format de mixage : le flux
    /// reste partagé — le périphérique demeure utilisable par les autres
    /// applications — mais sa période est la plus courte que le moteur accepte, ou
    /// celle qu'on lui demande. Un refus (interface absente, périodicité déjà
    /// verrouillée par un autre flux, format demandé différent du mélange) devient
    /// une [`BackendError::UnsupportedFormat`] : jamais un repli silencieux sur la
    /// période par défaut, qui ferait mesurer le moteur audio là où on croit mesurer
    /// le transport du pilote.
    ///
    /// Le changement ne concerne que les ouvertures suivantes.
    pub fn set_shared_period(&mut self, period: SharedPeriod) {
        self.period = period;
    }

    /// Politique de période partagée en vigueur ([`Self::set_shared_period`]).
    pub fn shared_period(&self) -> SharedPeriod {
        self.period
    }

    /// Installe le contrôle des câbles que [`Backend::cable_control`] rendra (M1b-34).
    ///
    /// # Pourquoi une injection, et pas un `CableControl` écrit ici
    ///
    /// Le contrôle des câbles passe par le **service d'assistance** `ConduitHelper` : le
    /// démon tourne sans privilèges et le pilote exige `SeLoadDriverPrivilege` armé pour
    /// toute écriture (mesuré, `conduit_helper::cables`). Son client vit donc dans
    /// `conduit-helper`, qui dépend déjà de ce crate pour le transport KS — l'inverse
    /// ferait un cycle entre les deux paquets, ce que Cargo refuse. C'est donc
    /// `conduitd`, qui dépend des deux, qui relie les bouts en appelant cette méthode.
    ///
    /// Un dorsal qui n'a rien reçu rend `None` : les câbles ne sont alors pas pilotables
    /// et le moteur le dit (`EngineError::NoCableControl`).
    pub fn set_cable_control(&mut self, controle: Box<dyn CableControl + Send>) {
        self.cables = Some(controle);
    }

    /// Le contrôle injecté, ou l'erreur qui dit que le montage n'a pas été fait.
    fn cables(&self) -> Result<&dyn CableControl, CableError> {
        self.cables
            .as_deref()
            .map(|controle| controle as &dyn CableControl)
            .ok_or_else(|| CableError::Unavailable(SANS_CONTROLE.to_owned()))
    }

    /// L'énumération **complète** : ce que publie [`Backend::devices`], plus la
    /// description de chaque endpoint (voir `devices::EndpointInfo`).
    fn endpoints(&self) -> Result<Vec<EndpointInfo>, BackendError> {
        self.request(|reply| Command::Enumerate { reply }, "énumération")?
    }

    /// Remplace les identifiants d'endpoint **de repli** par ceux que MMDevice publie, et
    /// le nom canonique par celui que le câble affiche vraiment.
    ///
    /// Le service ne connaît que les filtres de topologie du pilote ; les endpoints,
    /// eux, sont l'affaire de ce crate. Une énumération suffit pour tous les câbles :
    /// chaque [`DeviceInfo`] porte déjà son [`CableId`] (`devices::cable_id_from_endpoint`).
    ///
    /// # Le nom, et pourquoi il vient d'ici
    ///
    /// La réponse du protocole du service est de **taille fixe** et ne transporte aucun
    /// nom : `CableControl::list` rend donc « Conduit *N* », le nom canonique, même pour
    /// un câble renommé « Musique ». Le dorsal, lui, lit la description des endpoints
    /// (`devices::EndpointInfo::description`), c'est-à-dire exactement la valeur que le
    /// renommage de M1b-21 écrit. C'est donc lui qui complète le nom, comme il complète
    /// déjà les identifiants.
    ///
    /// Quand les deux sens portent des noms **différents** — un renommage à moitié fait,
    /// que `registre::appliquer` ne produit pas mais qu'une interruption peut laisser —,
    /// le rendu l'emporte et la divergence est **journalisée** : la taire ferait passer
    /// cet état anormal pour la normale, et c'est précisément le genre de silence qui
    /// coûte une enquête.
    ///
    /// Un endpoint introuvable laisse le repli en place, et c'est la vérité : le câble
    /// est inactif — ses endpoints n'existent pas — ou Windows ne les a pas encore
    /// publiés (77 ms mesurées entre l'écriture et l'apparition). Une énumération qui
    /// échoue ne fait pas échouer l'opération : le câble est bien activé, seuls les deux
    /// noms manquent.
    fn resoudre_endpoints(&self, cables: &mut [CableInfo]) {
        if cables.is_empty() {
            return;
        }
        let Ok(endpoints) = self.endpoints() else {
            return;
        };
        for info in cables.iter_mut() {
            let (mut rendu, mut capture) = (None, None);
            for endpoint in endpoints.iter().filter(|e| e.info.cable == Some(info.id)) {
                match endpoint.info.direction {
                    DeviceDirection::Render => {
                        info.render = endpoint.info.id.clone();
                        rendu = endpoint.description.as_deref();
                    }
                    DeviceDirection::Capture => {
                        info.capture = endpoint.info.id.clone();
                        capture = endpoint.description.as_deref();
                    }
                }
            }
            let nom = cable_name(rendu, capture);
            if let CableName::Divergent { retenu, ecarte } = nom {
                tracing::warn!(
                    "câble {numero} : le rendu s'appelle « {retenu} » et la capture \
                     « {ecarte} » — renommage à moitié fait ; « {retenu} » est retenu. \
                     `conduitctl cable rename {numero} \"{retenu}\"` remettra les deux \
                     côtés d'accord.",
                    numero = info.id.0
                );
            }
            if let Some(nom) = nom.retenu() {
                info.name = nom.to_owned();
            }
        }
    }

    /// [`Self::resoudre_endpoints`] pour un seul câble.
    fn resoudre_un(&self, mut info: CableInfo) -> CableInfo {
        self.resoudre_endpoints(core::slice::from_mut(&mut info));
        info
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
        let period = self.period;
        let opened = self.request(
            move |reply| Command::Open {
                id,
                format,
                policy,
                period,
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
        Ok(self
            .endpoints()?
            .into_iter()
            .map(|endpoint| endpoint.info)
            .collect())
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

    /// Le contrôle des câbles, s'il a été installé par
    /// [`Self::set_cable_control`] (M1b-34).
    ///
    /// `None` tant que personne ne l'a relié au service d'assistance : le dorsal seul ne
    /// peut pas activer un câble, faute de `SeLoadDriverPrivilege`.
    fn cable_control(&mut self) -> Option<&mut dyn CableControl> {
        self.cables
            .is_some()
            .then_some(self as &mut dyn CableControl)
    }
}

/// Le contrôle des câbles du dorsal : les ordres partent au service, les **endpoints** et
/// le **nom affiché** sont résolus ici (M1b-34).
///
/// Le partage suit ce que chacun sait : le service connaît les filtres de topologie du
/// pilote et l'état de connexion, le dorsal connaît les endpoints MMDevice — leurs
/// identifiants comme le nom que Windows leur donne. Chaque [`CableInfo`] traverse donc
/// `WasapiBackend::resoudre_endpoints` avant d'être rendu, et l'appelant reçoit les
/// identifiants qu'il pourrait ouvrir — pas des jetons de repli — et le nom que
/// l'utilisateur voit dans les réglages Son, non le « Conduit *N* » canonique.
impl CableControl for WasapiBackend {
    fn max_cables(&self) -> usize {
        // Sans contrôle installé, aucun câble n'est pilotable : le dire par 0 vaut mieux
        // qu'annoncer une réserve qu'on ne saurait pas servir.
        self.cables.as_ref().map_or(0, |c| c.max_cables())
    }

    fn list(&self) -> Result<Vec<CableInfo>, CableError> {
        let mut cables = self.cables()?.list()?;
        self.resoudre_endpoints(&mut cables);
        Ok(cables)
    }

    fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError> {
        // Le bloc ferme l'emprunt mutable du contrôle avant que `resoudre_un` ne
        // réemprunte le dorsal pour énumérer.
        let info = {
            let controle = self
                .cables
                .as_mut()
                .ok_or_else(|| CableError::Unavailable(SANS_CONTROLE.to_owned()))?;
            controle.create(spec)?
        };
        Ok(self.resoudre_un(info))
    }

    fn remove(&mut self, id: CableId) -> Result<(), CableError> {
        self.cables
            .as_mut()
            .ok_or_else(|| CableError::Unavailable(SANS_CONTROLE.to_owned()))?
            .remove(id)
    }

    fn set_channels(
        &mut self,
        id: CableId,
        channels: ChannelCount,
    ) -> Result<CableInfo, CableError> {
        let info = {
            let controle = self
                .cables
                .as_mut()
                .ok_or_else(|| CableError::Unavailable(SANS_CONTROLE.to_owned()))?;
            controle.set_channels(id, channels)?
        };
        Ok(self.resoudre_un(info))
    }

    fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError> {
        let info = {
            let controle = self
                .cables
                .as_mut()
                .ok_or_else(|| CableError::Unavailable(SANS_CONTROLE.to_owned()))?;
            controle.rename(id, name)?
        };
        Ok(self.resoudre_un(info))
    }

    fn get(&self, id: CableId) -> Result<CableInfo, CableError> {
        Ok(self.resoudre_un(self.cables()?.get(id)?))
    }
}
