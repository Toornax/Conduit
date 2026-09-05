//! `conduit-core` — moteur de graphe audio portable.
//!
//! Ce crate ne dépend d'aucune API de plateforme. Il fournit :
//!
//! - les types de base ([`types`]) : fréquence, nombre de trames, canaux, gains ;
//! - les tampons audio planaires pré-alloués ([`buffer`]) ;
//! - un tampon circulaire SPSC temps réel ([`ring`]) ;
//! - le modèle de graphe et sa construction validée ([`graph`]) ;
//! - le trait [`Node`] et ses tampons d'entrée/sortie ([`node`]) ;
//! - les gains partagés avec rampe ([`gain`]) ;
//! - l'exécution d'un cycle ([`executor`]) et l'échange atomique de version ([`slot`]) ;
//! - des paramètres atomiques ([`param`]), des briques DSP ([`dsp`]) et des nœuds
//!   utilitaires ([`nodes`]) : générateurs, mixeur, VU-mètre, adaptation de canaux,
//!   égaliseur ;
//! - le port asynchrone ([`asyncport`]) : tampon + rééchantillonneur + DLL pour les
//!   flux à horloge étrangère.
//!
//! # Contraintes temps réel
//!
//! Tout ce qui est appelé depuis le fil audio doit respecter la règle :
//! **pas d'allocation, pas de verrou, pas de syscall bloquant, pas de log**.
//! Les fonctions concernées sont documentées comme telles.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod asyncport;
pub mod buffer;
pub mod dsp;
pub mod executor;
pub mod gain;
pub mod graph;
pub mod node;
pub mod nodes;
pub mod param;
pub mod ring;
pub mod slot;
pub mod types;

pub use buffer::AudioBuffer;
pub use executor::{CycleReport, Executor};
pub use gain::GainParam;
pub use graph::{
    CompiledGraph, Direction, GraphBuilder, GraphError, LinkId, LinkInfo, NodeId, NodeInfo, PortId,
};
pub use node::{ChannelLabel, Node, NodeIo, PortSpec, ProcessContext};
pub use ring::{RingBuffer, RingConsumer, RingProducer};
pub use slot::{GraphSlot, PublishError};
pub use types::{ChannelCount, Db, Frames, Gain, Quantum, SampleRate};
