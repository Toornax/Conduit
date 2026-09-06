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
//! Ce crate ne compile de code que sous Windows ; ailleurs il n'expose rien, pour
//! que `cargo check --workspace` reste vert sur toutes les cibles. Le contrôle des
//! câbles (M1b-34) viendra ensuite.
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
mod mmdevice_thread;
#[cfg(windows)]
mod notify;
#[cfg(windows)]
mod open;
#[cfg(windows)]
mod stream;

#[cfg(windows)]
pub use backend::WasapiBackend;
#[cfg(windows)]
pub use clock::{ClockSource, ClockUnits};
#[cfg(windows)]
pub use convert::SampleType;
#[cfg(windows)]
pub use devices::{cable_id_from_name, PROBED_RATES};
#[cfg(windows)]
pub use exclusive::{aligned_period_hns, ExclusivePolicy, ShareMode};
#[cfg(windows)]
pub use open::{choose_period, EnginePeriods, InitPath, StreamLatency};
#[cfg(windows)]
pub use stream::{ClockStats, WasapiHandle};
