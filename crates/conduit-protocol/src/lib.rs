//! `conduit-protocol` — protocole de contrôle entre le démon et ses clients.
//!
//! - [`api`] : commandes, réponses, notifications et descripteurs (source de vérité
//!   de l'API, implémentée par `conduit-engine`) ;
//! - [`wire`] : enveloppe [`Message`], négociation [`Hello`], version ;
//! - [`framing`] : trames `u32` longueur + MessagePack, décodeur incrémental borné ;
//! - [`schema`] (feature `schema`) : export JSON Schema et documentation.
//!
//! Compatibilité : l'ajout de champs est toujours optionnel ; un changement
//! incompatible incrémente [`PROTOCOL_VERSION`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod framing;
#[cfg(feature = "schema")]
pub mod schema;
pub mod wire;

pub use api::{
    Command, DeviceStatus, DriverChoice, DriverStatus, EngineEvent, EngineStatus, ErrorCode,
    InternalKind, LinkDescriptor, NodeDescriptor, NodeState, Notification, ProtocolError, Reply,
    TimingSnapshot,
};
pub use framing::{Decoder, FramingError, MAX_FRAME_LEN};
pub use wire::{Event, Hello, HelloReply, Message, Request, Response, PROTOCOL_VERSION};
