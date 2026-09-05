//! `conduit-backend-wasapi` — backend Windows au-dessus de MMDevice et WASAPI.
//!
//! Le backend dialogue avec l'API MMDevice depuis un **fil dédié**
//! (`mmdevice_thread`) qui initialise COM en MTA, crée l'`IMMDeviceEnumerator`
//! et enregistre un `IMMNotificationClient`. Ce fil :
//!
//! - énumère les endpoints audio **actifs** (`DEVICE_STATE_ACTIVE`) et traduit
//!   chacun en [`DeviceInfo`](conduit_backend::DeviceInfo) : identifiant d'endpoint,
//!   nom convivial, sens, format du moteur audio, fréquences acceptées en mode
//!   partagé, période par défaut, périphérique par défaut (`eConsole`) ;
//! - reçoit les rappels du client de notification et en fait des
//!   [`DeviceEvent`](conduit_backend::DeviceEvent) (`Added`, `Removed`,
//!   `DefaultChanged`), diffusés aux abonnés de [`Backend::subscribe`] ;
//! - sert les commandes du [`WasapiBackend`] par `std::sync::mpsc`.
//!
//! Les rappels COM arrivent sur un fil que Windows choisit : ils ne font que
//! **poster un message** au fil MMDevice, qui seul touche l'énumérateur. Toute la
//! logique (ré-énumération d'un endpoint, filtrage du rôle, mise à jour de la table
//! des périphériques connus) vit donc sur un seul fil.
//!
//! Ce crate ne compile de code que sous Windows ; ailleurs il n'expose rien, pour
//! que `cargo check --workspace` reste vert sur toutes les cibles. Les flux audio
//! ([`Backend::open`]) et le contrôle des câbles arrivent avec M1b-31 et M1b-34.
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
//! [`Backend::subscribe`]: conduit_backend::Backend::subscribe
//! [`Backend::open`]: conduit_backend::Backend::open

// `unsafe` confiné aux appels COM, chaque bloc justifié par un `SAFETY:`.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod com;
#[cfg(windows)]
mod devices;
#[cfg(windows)]
mod mmdevice_thread;
#[cfg(windows)]
mod notify;

#[cfg(windows)]
pub use backend::WasapiBackend;
#[cfg(windows)]
pub use devices::{cable_id_from_name, PROBED_RATES};
