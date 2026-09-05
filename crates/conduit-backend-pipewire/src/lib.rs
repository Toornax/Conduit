//! `conduit-backend-pipewire` — backend Linux au-dessus de PipeWire.
//!
//! Le backend ouvre une connexion au démon PipeWire depuis un **fil dédié** qui fait
//! tourner une `pw::main_loop::MainLoopRc`. Ce fil :
//!
//! - suit le registre : chaque nœud dont `media.class` vaut `Audio/Sink` ou
//!   `Audio/Source` devient un [`DeviceInfo`](conduit_backend::DeviceInfo) et produit
//!   un [`DeviceEvent`](conduit_backend::DeviceEvent) à son apparition et à sa
//!   disparition ;
//! - suit les métadonnées `default` (`default.audio.sink` / `default.audio.source`)
//!   pour renseigner `is_default` ;
//! - exécute toutes les opérations sur les flux (`pw_stream`), reçues par
//!   `pw::channel` : la bibliothèque PipeWire n'est pas `Send`, tout ce qui la
//!   touche vit sur ce fil.
//!
//! Le rappel `process` d'un flux tourne, lui, sur le fil temps réel de PipeWire :
//! il n'alloue pas, ne verrouille pas et ne journalise pas (voir `docs/dev-guide.md`
//! §2). La coordination entre le fil de boucle et le fil temps réel passe par des
//! atomiques.
//!
//! Ce crate ne compile de code que sur Linux ; ailleurs il n'expose rien, pour que
//! `cargo check --workspace` reste vert sur toutes les cibles.
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> Result<(), conduit_backend::BackendError> {
//! use conduit_backend::Backend;
//! use conduit_backend_pipewire::PipewireBackend;
//!
//! let backend = PipewireBackend::connect(None)?;
//! for device in backend.devices()? {
//!     println!("{} ({})", device.name, device.direction);
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(target_os = "linux"))]
//! # fn main() {}
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(target_os = "linux")]
mod backend;
#[cfg(target_os = "linux")]
mod devices;
#[cfg(target_os = "linux")]
mod loop_thread;
#[cfg(target_os = "linux")]
mod stream;

#[cfg(target_os = "linux")]
pub use backend::{PipewireBackend, PipewireHandle};
#[cfg(target_os = "linux")]
pub use devices::{DEFAULT_BLOCK_FRAMES, DEFAULT_SAMPLE_RATE};
