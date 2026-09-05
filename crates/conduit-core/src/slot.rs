//! Publication atomique du graphe : côté fil de gestion.

use crate::executor::Executor;
use crate::graph::{CompiledGraph, GraphBuilder};
use crate::ring::{RingBuffer, RingConsumer, RingProducer};

/// Erreur de publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    /// Trop de versions en attente : le fil audio ne les adopte pas (arrêté ?).
    /// Les nœuds ont été repris par le builder ; réessayer plus tard.
    #[error("le fil audio n'adopte pas les nouvelles versions du graphe (file pleine)")]
    Backlog,
    /// Le fil audio a disparu (exécuteur libéré).
    #[error("l'exécuteur a été libéré")]
    Detached,
}

/// Côté gestion de l'échange de graphe.
///
/// [`publish`](Self::publish) compile le builder et envoie la version au fil audio ;
/// [`collect`](Self::collect) libère les versions retirées. Ni l'un ni l'autre ne
/// bloque.
pub struct GraphSlot {
    pending: RingProducer<Box<CompiledGraph>>,
    retired: RingConsumer<Box<CompiledGraph>>,
    published: u64,
    collected: u64,
}

impl core::fmt::Debug for GraphSlot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GraphSlot")
            .field("published", &self.published)
            .field("collected", &self.collected)
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl GraphSlot {
    /// Capacité par défaut des files (versions en attente).
    pub const DEFAULT_CAPACITY: usize = 16;

    /// Crée la paire gestion / exécuteur.
    pub fn new() -> (GraphSlot, Executor) {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    /// Crée la paire avec une capacité de file donnée (≥ 1).
    pub fn with_capacity(capacity: usize) -> (GraphSlot, Executor) {
        let capacity = capacity.max(1);
        let (pending_p, pending_c) = RingBuffer::with_capacity(capacity);
        // Tout graphe retiré correspond à une version installée : au plus
        // `capacity` en attente + la version courante, plus une marge.
        let (retired_p, retired_c) = RingBuffer::with_capacity(capacity + 2);
        (
            GraphSlot {
                pending: pending_p,
                retired: retired_c,
                published: 0,
                collected: 0,
            },
            Executor::new(pending_c, retired_p, capacity + 2),
        )
    }

    /// Compile le builder et publie la version. Collecte d'abord les versions retirées.
    pub fn publish(&mut self, builder: &mut GraphBuilder) -> Result<(), PublishError> {
        self.collect();
        if self.pending.is_abandoned() {
            return Err(PublishError::Detached);
        }
        let graph = builder.compile();
        match self.pending.push(graph) {
            Ok(()) => {
                self.published += 1;
                Ok(())
            }
            Err(graph) => {
                builder.reclaim(graph);
                Err(PublishError::Backlog)
            }
        }
    }

    /// Libère les versions retirées par le fil audio. Retourne le nombre libéré.
    pub fn collect(&mut self) -> usize {
        let mut n = 0;
        while let Some(g) = self.retired.pop() {
            drop(g);
            n += 1;
        }
        self.collected += n as u64;
        n
    }

    /// Versions publiées pas encore adoptées par le fil audio.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Total publié.
    pub fn published(&self) -> u64 {
        self.published
    }

    /// Total libéré.
    pub fn collected(&self) -> u64 {
        self.collected
    }

    /// Vrai si l'exécuteur a été libéré.
    pub fn is_detached(&self) -> bool {
        self.pending.is_abandoned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Node, NodeIo, PortSpec, ProcessContext};
    use crate::types::{Quantum, SampleRate};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread::ThreadId;

    /// Nœud qui note sur quel fil il est libéré.
    struct DropTracker {
        dropped_on: Arc<std::sync::Mutex<Vec<ThreadId>>>,
    }
    impl Node for DropTracker {
        fn type_name(&self) -> &'static str {
            "drop-tracker"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            vec![]
        }
        fn outputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(1)
        }
        fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
            io.output(0).fill(1.0);
        }
    }
    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.dropped_on
                .lock()
                .unwrap()
                .push(std::thread::current().id());
        }
    }

    struct Counter(Arc<AtomicUsize>);
    impl Node for Counter {
        fn type_name(&self) -> &'static str {
            "counter"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            vec![]
        }
        fn outputs(&self) -> Vec<PortSpec> {
            vec![]
        }
        fn process(&mut self, _: &ProcessContext, _: &mut NodeIo<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn builder() -> GraphBuilder {
        GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(32).unwrap())
    }

    #[test]
    fn publish_then_run_installs_and_inherits() {
        let (mut slot, mut ex) = GraphSlot::new();
        let mut b = builder();
        let hits = Arc::new(AtomicUsize::new(0));
        b.add_node(Box::new(Counter(hits.clone())), "c");
        slot.publish(&mut b).unwrap();
        assert_eq!(slot.pending_count(), 1);
        let r = ex.run(32);
        assert!(r.graph_swapped);
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        // Deuxième version : le compteur est hérité, pas recréé.
        b.add_node(Box::new(Counter(hits.clone())), "c2");
        slot.publish(&mut b).unwrap();
        let r = ex.run(32);
        assert!(r.graph_swapped);
        assert_eq!(r.nodes_run, 2);
        assert_eq!(r.nodes_missing, 0);
        assert!(ex.graph().unwrap().is_complete());
        assert_eq!(hits.load(Ordering::Relaxed), 3);
        assert_eq!(slot.collect(), 1);
        assert_eq!(slot.published(), 2);
        assert_eq!(slot.collected(), 1);
        assert!(format!("{slot:?}").contains("published"));
    }

    #[test]
    fn several_pending_versions_are_installed_in_order_without_loss() {
        let (mut slot, mut ex) = GraphSlot::with_capacity(4);
        let mut b = builder();
        let hits = Arc::new(AtomicUsize::new(0));
        b.add_node(Box::new(Counter(hits.clone())), "a");
        slot.publish(&mut b).unwrap();
        b.add_node(Box::new(Counter(hits.clone())), "b");
        slot.publish(&mut b).unwrap();
        b.add_node(Box::new(Counter(hits.clone())), "c");
        slot.publish(&mut b).unwrap();
        assert_eq!(slot.pending_count(), 3);
        let r = ex.run(32);
        assert_eq!(
            r.nodes_run, 3,
            "les trois versions ont été adoptées en séquence"
        );
        assert!(ex.graph().unwrap().is_complete());
        assert_eq!(slot.collect(), 2);
    }

    #[test]
    fn backlog_returns_nodes_to_builder() {
        let (mut slot, mut ex) = GraphSlot::with_capacity(1);
        let mut b = builder();
        let hits = Arc::new(AtomicUsize::new(0));
        b.add_node(Box::new(Counter(hits.clone())), "a");
        slot.publish(&mut b).unwrap();
        b.add_node(Box::new(Counter(hits.clone())), "b");
        assert_eq!(slot.publish(&mut b), Err(PublishError::Backlog));
        ex.run(32);
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        // Après adoption, la republication transporte « b » (repris par le builder).
        slot.publish(&mut b).unwrap();
        ex.run(32);
        assert_eq!(hits.load(Ordering::Relaxed), 3);
        assert!(ex.graph().unwrap().is_complete());
    }

    #[test]
    fn nodes_are_never_dropped_on_the_audio_thread() {
        let (mut slot, ex) = GraphSlot::new();
        let dropped_on = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut b = builder();
        let n = b.add_node(
            Box::new(DropTracker {
                dropped_on: dropped_on.clone(),
            }),
            "t",
        );
        slot.publish(&mut b).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<ThreadId>();
        let audio = std::thread::spawn(move || {
            let mut ex = ex;
            ex.run(32);
            done_tx.send(std::thread::current().id()).unwrap();
            // Attend l'ordre d'adopter la version suivante puis termine.
            rx.recv().unwrap();
            ex.run(32);
            rx.recv().unwrap();
            drop(ex);
        });
        let audio_id = done_rx.recv().unwrap();
        b.remove_node(n).unwrap();
        slot.publish(&mut b).unwrap();
        tx.send(()).unwrap();
        // Le nœud retiré est dans l'ancien graphe, renvoyé au fil de gestion.
        let mut collected = 0;
        for _ in 0..1000 {
            collected += slot.collect();
            if collected > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(collected, 1);
        let dropped = dropped_on.lock().unwrap().clone();
        assert_eq!(dropped.len(), 1);
        assert_ne!(dropped[0], audio_id);
        assert_eq!(dropped[0], std::thread::current().id());
        tx.send(()).unwrap();
        audio.join().unwrap();
        assert!(slot.is_detached());
        assert_eq!(slot.publish(&mut b), Err(PublishError::Detached));
    }

    #[test]
    fn concurrent_publish_and_run_stress() {
        let (mut slot, mut ex) = GraphSlot::with_capacity(8);
        let mut b = builder();
        let hits = Arc::new(AtomicUsize::new(0));
        let mut ids = Vec::new();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let audio = std::thread::spawn(move || {
            let mut cycles = 0u64;
            let mut missing = 0usize;
            while !stop2.load(Ordering::Relaxed) {
                let r = ex.run(32);
                missing += r.nodes_missing;
                cycles += 1;
            }
            // Vide ce qui reste en attente.
            ex.poll();
            (cycles, missing, ex)
        });
        for i in 0..2000 {
            if i % 3 == 2 && !ids.is_empty() {
                let id = ids.remove(0);
                b.remove_node(id).unwrap();
            } else {
                ids.push(b.add_node(Box::new(Counter(hits.clone())), "x"));
            }
            loop {
                match slot.publish(&mut b) {
                    Ok(()) => break,
                    Err(PublishError::Backlog) => std::thread::yield_now(),
                    Err(e) => panic!("{e}"),
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        let (cycles, missing, ex) = audio.join().unwrap();
        assert!(cycles > 0);
        assert_eq!(
            missing, 0,
            "aucun nœud manquant malgré les échanges concurrents"
        );
        assert!(ex.graph().unwrap().is_complete());
        assert_eq!(ex.stats().unwrap().nodes, b.node_count());
        drop(ex);
        slot.collect();
    }
}
