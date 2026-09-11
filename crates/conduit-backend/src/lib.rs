//! `conduit-backend` — abstraction des API audio de plateforme.
//!
//! Un [`Backend`] énumère les périphériques, notifie leurs changements, ouvre des
//! flux avec un rappel audio temps réel et, s'il le peut, contrôle les câbles
//! virtuels de sa plateforme ([`CableControl`]).
//!
//! Le backend [`null`] simule tout cela avec des horloges virtuelles, pour les tests.
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

#![warn(missing_docs)]
// `unsafe` confiné au module `rt` (appels système de priorité), chaque bloc justifié.

pub mod cable;
pub mod device;
pub mod event;
pub mod null;
pub mod rt;

pub use cable::{
    CableControl, CableError, CableFormat, CableId, CableInfo, CableSpec, SampleDepth,
};
pub use device::{
    AudioCallback, Backend, BackendError, ClockInfo, DeviceDirection, DeviceHandle, DeviceId,
    DeviceInfo, StreamFormat, StreamIo,
};
pub use event::{DeviceEvent, EventReceiver};
pub use rt::{promote_current_thread, RtOutcome};
