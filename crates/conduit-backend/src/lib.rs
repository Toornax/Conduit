//! `conduit-backend` — abstraction des API audio de plateforme.
//!
//! Un [`Backend`] énumère les périphériques, notifie leurs changements, ouvre des
//! flux avec un rappel audio temps réel et, s'il le peut, contrôle les câbles
//! virtuels de sa plateforme ([`CableControl`]).
//!
//! # Garanties du rappel audio
//!
//! Le rappel passé à [`Backend::open`] est appelé depuis le fil audio du backend :
//! il doit respecter les contraintes temps réel de `conduit-core` (pas d'allocation,
//! pas de verrou, pas de syscall bloquant, pas de log). Le backend garantit :
//!
//! - un seul appel à la fois par flux ;
//! - des tampons entrelacés au format négocié ([`DeviceHandle::format`]) ;
//! - une [`ClockInfo`] cohérente : `position` avance exactement de `frames` à chaque
//!   appel, `timestamp_ns` est monotone ;
//! - plus aucun appel après [`DeviceHandle::stop`] ou la destruction du handle.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cable;
pub mod device;
pub mod event;

pub use cable::{CableControl, CableError, CableId, CableInfo, CableSpec};
pub use device::{
    AudioCallback, Backend, BackendError, ClockInfo, DeviceDirection, DeviceHandle, DeviceId,
    DeviceInfo, StreamFormat, StreamIo,
};
pub use event::{DeviceEvent, EventReceiver};
