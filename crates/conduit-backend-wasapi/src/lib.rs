//! `conduit-backend-wasapi` — backend Windows au-dessus de MMDevice et WASAPI.
//!
//! Le backend dialogue avec l'API MMDevice depuis un **fil dédié**
//! (`mmdevice_thread`) qui initialise COM en MTA, crée l'`IMMDeviceEnumerator`
//! et enregistre un `IMMNotificationClient`. Ce fil :
//!
//! - énumère les endpoints audio **actifs** (`DEVICE_STATE_ACTIVE`) et traduit
//!   chacun en `DeviceInfo` : identifiant d'endpoint, nom convivial, sens,
//!   **format de mixage** du moteur audio (canaux, fréquence),
//!   fréquences acceptées en mode partagé, période par défaut, périphérique par
//!   défaut (`eConsole`) ;
//! - reçoit les rappels du client de notification et en fait des
//!   `DeviceEvent` (`Added`, `Removed`, `DefaultChanged`), diffusés aux
//!   abonnés de `Backend::subscribe` ;
//! - ouvre les flux (`Backend::open`) : crée et initialise les objets WASAPI,
//!   événementiels, au format demandé — en mode **partagé** par défaut, en mode
//!   **exclusif** si `WasapiBackend::set_exclusive_policy` l'a demandé
//!   (`ExclusivePolicy`, module `exclusive`) ;
//! - sert les commandes du `WasapiBackend` par `std::sync::mpsc`.
//!
//! Les rappels COM arrivent sur un fil que Windows choisit : ils ne font que
//! **poster un message** au fil MMDevice, qui seul touche l'énumérateur. Toute la
//! logique (ré-énumération d'un endpoint, filtrage du rôle, mise à jour de la table
//! des périphériques connus) vit donc sur un seul fil.
//!
//! Chaque flux ouvert a ensuite **son propre fil** (`WasapiHandle`, module
//! `stream`) : créé par `start()`, promu temps réel, réveillé par l'événement du
//! tampon, joint par `stop()`. Le rappel reçoit exactement le format demandé :
//! sans conversion quand il coïncide avec le format de mixage
//! (`IAudioClient3`, période au choix), converti par Windows sinon. Sa position
//! d'horloge vient d'`IAudioClock` (module `clock`) : position matérielle en
//! trames du format livré, horodatage `QueryPerformanceCounter` commun à tous les
//! flux du processus (`ClockSource`).
//!
//! La **capture en écho** (module `loopback`) est la troisième façon d'ouvrir un
//! flux : `WasapiBackend::open_loopback` ouvre un endpoint de **rendu** avec
//! `AUDCLNT_STREAMFLAGS_LOOPBACK` et prélève le mélange du moteur audio avant que
//! le pilote ne le consomme. Le flux se comporte alors comme une capture. C'est
//! l'outil qui coupe en deux une chaîne muette : le signal entendu en écho met le
//! moteur hors de cause, un écho silencieux met le pilote hors de cause. L'écho
//! n'existe qu'en mode partagé.
//!
//! Le **volume d'un endpoint** (module `volume`) est la quatrième chose que ce
//! crate sait faire d'un `IMMDevice` : `EndpointVolumeControl` l'active en
//! `IAudioEndpointVolume` et lit ou écrit le volume maître scalaire (0 à 1) et la
//! coupure. Un volume nul ou un endpoint coupé explique à lui seul toute chaîne
//! muette, écho compris : c'est la première hypothèse à écarter avant d'accuser le
//! pilote. Le même contrôle rend aussi la **plage** de l'endpoint en décibels
//! (`VolumeRange`, `read_range`) : minimum, maximum et pas, c'est-à-dire l'échelle
//! que le pilote de cet endpoint déclare — de quoi mesurer, plutôt que supposer,
//! celle que notre propre pilote exposera. Le module `session` répond à la
//! deuxième question de la même enquête —
//! le processus tourne-t-il seulement dans une session qui a de l'audio ? Ni l'un
//! ni l'autre n'entre dans le trait `Backend`, portable.
//!
//! Le **mode exclusif** (M1b-32, module `exclusive`) est un réglage du backend,
//! `Never` par défaut : le flux prend alors le périphérique pour lui seul, au
//! format que le matériel accepte — souvent de l'entier, que le fil du flux
//! convertit lui-même (module `convert`) puisqu'il n'y a plus d'`AUTOCONVERTPCM`.
//!
//! Les noms cités ici sont en code et non en liens : ils ne désignent, comme
//! tout ce crate, que des éléments compilés sous Windows, et un lien vers eux
//! ne se résoudrait pas quand la documentation est produite sur une autre
//! plateforme — ce que `cargo doc` traite en erreur (`-D warnings`).
//!
//! Le **jeu de propriétés KS privé du pilote** (module `cable`, M1b-04) est la
//! cinquième chose que ce crate sait faire, et la première qui ne passe ni par COM
//! ni par MMDevice : il ouvre par `CreateFileW` l'interface `KSCATEGORY_TOPOLOGY`
//! d'un câble — reconnue à sa **chaîne de référence** `TopoRender<n>` /
//! `TopoCapture<n>` — et lui envoie `IOCTL_KS_PROPERTY` pour lire ou écrire
//! `KSPROPERTY_CONDUIT_CABLE_STATE` et lire `KSPROPERTY_CONDUIT_VERSION`. Le
//! contrat (GUID, identifiants, disposition, parseur) vient de `conduit-kmd-core`,
//! partagé avec le pilote : rien n'en est recopié. C'est le transport que le
//! `CableControl` de M1b-34 utilisera ; il n'ouvre aucun flux et n'émet aucun son.
//!
//! Ce crate ne compile de code que sous Windows ; ailleurs il n'expose rien, pour
//! que `cargo check --workspace` reste vert sur toutes les cibles.
//!
//! ```no_run
//! # #[cfg(windows)]
//! # fn main() -> Result<(), conduit_backend::BackendError> {
//! use conduit_backend::Backend;
//! use conduit_backend_wasapi::WasapiBackend;
//!
//! let backend = WasapiBackend::new()?;
//! for device in backend.devices()? {
//!     println!("{} ({})", device.name, device.direction);
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(windows))]
//! # fn main() {}
//! ```
//!
//! `Backend::subscribe`: conduit_backend::Backend::subscribe
//! `Backend::open`: conduit_backend::Backend::open

// `unsafe` confiné aux appels COM, chaque bloc justifié par un `SAFETY:`.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[cfg(windows)]
mod backend;
#[cfg(windows)]
pub mod cable;
#[cfg(windows)]
mod clock;
#[cfg(windows)]
mod com;
#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod devices;
#[cfg(windows)]
mod exclusive;
#[cfg(windows)]
mod loopback;
#[cfg(windows)]
mod mmdevice_thread;
#[cfg(windows)]
mod notify;
#[cfg(windows)]
mod open;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod stream;
#[cfg(windows)]
mod volume;

#[cfg(windows)]
pub use backend::WasapiBackend;
#[cfg(windows)]
pub use cable::{
    cable_id, driver_index, matches_reference, topology_interfaces, BadInput, CableConfigError,
    FilterSide, OsError, TopologyFilter,
};
#[cfg(windows)]
pub use clock::{ClockSource, ClockUnits};
#[cfg(windows)]
pub use convert::SampleType;
#[cfg(windows)]
pub use devices::{
    cable_id_from_endpoint, cable_id_from_name, cable_name, CableName, PROBED_RATES,
};
#[cfg(windows)]
pub use exclusive::{aligned_period_hns, ExclusivePolicy, ShareMode};
#[cfg(windows)]
pub use open::{choose_period, EnginePeriods, InitPath, StreamLatency};
#[cfg(windows)]
pub use session::{current_session_id, SERVICES_SESSION};
#[cfg(windows)]
pub use stream::{ClockStats, WasapiHandle};
#[cfg(windows)]
pub use volume::{EndpointVolume, EndpointVolumeControl, VolumeRange};
