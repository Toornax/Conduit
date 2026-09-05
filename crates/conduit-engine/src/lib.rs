//! `conduit-engine` — le moteur : graphe temps réel cadencé par un périphérique
//! pilote ou une horloge interne, périphériques asynchrones, câbles, commandes.
//!
//! - [`key`] : clés stables de nœuds (F-30) ;
//! - [`device_node`] : nœud de périphérique dont le rôle change à chaud ;
//! - [`clock`] : horloge interne de secours (F-22) ;
//! - [`stats`] : compteurs temps réel et file d'événements ;
//! - [`engine`] : l'[`Engine`] et son API de commandes.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod clock;
pub mod device_node;
pub mod engine;
pub mod key;
pub mod stats;

pub use conduit_protocol::api::{
    Command, DeviceStatus, DriverChoice, DriverStatus, EngineEvent, EngineStatus, InternalKind,
    LinkDescriptor, NodeDescriptor, NodeKey, NodeState, Notification, Reply, TimingSnapshot,
};
pub use engine::{Engine, EngineConfig, EngineError};
pub use key::Registry;
pub use stats::CycleTiming;
