//! `conduit-core` — moteur de graphe audio portable.
//!
//! Ce crate ne dépend d'aucune API de plateforme. Il fournit :
//!
//! - les types de base ([`types`]) : fréquence, nombre de trames, canaux, gains ;
//! - les tampons audio planaires pré-alloués ([`buffer`]) ;
//!
//! # Contraintes temps réel
//!
//! Tout ce qui est appelé depuis le fil audio doit respecter la règle :
//! **pas d'allocation, pas de verrou, pas de syscall bloquant, pas de log**.
//! Les fonctions concernées sont documentées comme telles.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod buffer;
pub mod types;

pub use buffer::AudioBuffer;
pub use types::{ChannelCount, Db, Frames, Gain, Quantum, SampleRate};
