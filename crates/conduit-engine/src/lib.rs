//! `conduit-engine` — le moteur : graphe temps réel cadencé par un périphérique
//! pilote ou une horloge interne, périphériques asynchrones, câbles, commandes.
//!
//! - [`key`] : clés stables de nœuds (F-30) ;
//! - [`device_node`] : nœud de périphérique dont le rôle change à chaud ;
//! - [`clock`] : horloge interne de secours (F-22) ;
//! - [`stats`] : compteurs temps réel et file d'événements ;

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod clock;
pub mod device_node;
pub mod key;
pub mod stats;

pub use key::{NodeKey, Registry};
pub use stats::{CycleTiming, EngineEvent, TimingSnapshot};
