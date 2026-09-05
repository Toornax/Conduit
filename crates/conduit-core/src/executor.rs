//! Exécution d'un cycle sur un [`CompiledGraph`], côté fil audio.

use crate::gain::RampPlan;
use crate::graph::{CompiledGraph, GraphStats};
use crate::node::{NodeIo, ProcessContext};
use crate::ring::{RingConsumer, RingProducer};
use crate::types::SampleRate;

/// Résultat d'un cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleReport {
    /// Nœuds exécutés.
    pub nodes_run: usize,
    /// Nœuds absents de la table (version de graphe incohérente) : sorties mises au silence.
    pub nodes_missing: usize,
    /// Une nouvelle version du graphe a été installée au début de ce cycle.
    pub graph_swapped: bool,
}

/// Exécuteur : possède la version courante du graphe et les nœuds, exécute les cycles.
///
/// Vit sur le fil audio. Reçoit les nouvelles versions par une file SPSC, renvoie
/// les anciennes par une autre file pour libération hors temps réel. Aucune
/// allocation ni libération dans [`run`](Self::run) une fois les files dimensionnées.
pub struct Executor {
    graph: Option<Box<CompiledGraph>>,
    pending: RingConsumer<Box<CompiledGraph>>,
    retired: RingProducer<Box<CompiledGraph>>,
    /// Réserve si la file de retour est pleine (pré-allouée, jamais agrandie).
    #[allow(clippy::vec_box)]
    // le Box circule tel quel entre les files, sans déplacement du graphe
    overflow: Vec<Box<CompiledGraph>>,
    position: u64,
    cycle: u64,
}

impl core::fmt::Debug for Executor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Executor")
            .field("graph", &self.graph.as_ref().map(|g| g.stats()))
            .field("position", &self.position)
            .field("cycle", &self.cycle)
            .finish()
    }
}

impl Executor {
    pub(crate) fn new(
        pending: RingConsumer<Box<CompiledGraph>>,
        retired: RingProducer<Box<CompiledGraph>>,
        overflow_capacity: usize,
    ) -> Self {
        Self {
            graph: None,
            pending,
            retired,
            overflow: Vec::with_capacity(overflow_capacity),
            position: 0,
            cycle: 0,
        }
    }

    /// Exécuteur autonome sans file de publication (tests, usage embarqué) : le graphe
    /// est fourni directement et jamais échangé.
    pub fn standalone(graph: Box<CompiledGraph>) -> Self {
        let (retired_p, _retired_c) = crate::ring::RingBuffer::with_capacity(1);
        let (_pending_p, pending_c) = crate::ring::RingBuffer::with_capacity(1);
        let mut e = Self::new(pending_c, retired_p, 1);
        e.install(graph);
        e
    }

    /// Vrai si un graphe est installé.
    pub fn has_graph(&self) -> bool {
        self.graph.is_some()
    }

    /// Statistiques du graphe courant.
    pub fn stats(&self) -> Option<GraphStats> {
        self.graph.as_ref().map(|g| g.stats())
    }

    /// Graphe courant, en lecture.
    pub fn graph(&self) -> Option<&CompiledGraph> {
        self.graph.as_deref()
    }

    /// Fréquence du graphe courant.
    pub fn sample_rate(&self) -> Option<SampleRate> {
        self.graph.as_ref().map(|g| g.sample_rate)
    }

    /// Position en trames depuis la création.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Nombre de cycles exécutés.
    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    /// Adopte les versions en attente (toutes, dans l'ordre). Retourne le nombre installé.
    ///
    /// Temps réel : oui.
    pub fn poll(&mut self) -> usize {
        let mut n = 0;
        while let Some(g) = self.pending.pop() {
            self.install(g);
            n += 1;
        }
        n
    }

    fn install(&mut self, mut new: Box<CompiledGraph>) {
        if let Some(old) = self.graph.as_mut() {
            let n = new.nodes.len().min(old.nodes.len());
            for i in 0..n {
                if new.inherit[i] && new.nodes[i].is_none() {
                    new.nodes[i] = old.nodes[i].take();
                }
            }
        }
        if let Some(old) = self.graph.replace(new) {
            self.retire(old);
        }
    }

    fn retire(&mut self, old: Box<CompiledGraph>) {
        if let Err(old) = self.retired.push(old) {
            if self.overflow.len() < self.overflow.capacity() {
                self.overflow.push(old);
            } else {
                // Réserve épuisée : fuite volontaire plutôt qu'une libération sur le fil
                // audio. Ne se produit que si le fil de gestion ne collecte jamais.
                debug_assert!(false, "réserve de graphes retirés épuisée");
                core::mem::forget(old);
            }
        } else {
            // Profite de la place pour vider la réserve.
            while let Some(g) = self.overflow.pop() {
                if let Err(g) = self.retired.push(g) {
                    self.overflow.push(g);
                    break;
                }
            }
        }
    }

    /// Exécute un cycle de `frames` trames (≤ quantum du graphe). Adopte d'abord les
    /// versions en attente.
    ///
    /// Temps réel : oui. Sans graphe installé, ne fait rien.
    pub fn run(&mut self, frames: usize) -> CycleReport {
        let swapped = self.poll() > 0;
        let Some(g) = self.graph.as_deref_mut() else {
            return CycleReport {
                nodes_run: 0,
                nodes_missing: 0,
                graph_swapped: swapped,
            };
        };
        let frames = frames.min(g.max_frames);
        let ctx = ProcessContext {
            frames,
            max_frames: g.max_frames,
            sample_rate: g.sample_rate,
            position: self.position,
            cycle: self.cycle,
        };
        let CompiledGraph {
            nodes,
            exec,
            inputs,
            links,
            in_pool,
            in_connected,
            out_pool,
            ..
        } = g;
        let mut run = 0;
        let mut missing = 0;
        for e in exec.iter_mut() {
            // 1. Somme des liens entrants dans chaque port d'entrée.
            for p in 0..e.in_count {
                let gi = e.in_start + p;
                let port = inputs[gi];
                let dst = &mut in_pool[gi][..frames];
                if port.links_count == 0 {
                    in_connected[gi] = false;
                    dst.fill(0.0);
                    continue;
                }
                in_connected[gi] = true;
                for (k, l) in links[port.links_start..port.links_start + port.links_count]
                    .iter_mut()
                    .enumerate()
                {
                    let target = l.gain.target();
                    let plan = l.ramp.step(target, frames);
                    let src = &out_pool[l.src_out][..frames];
                    if k == 0 {
                        plan.copy(src, dst);
                    } else {
                        plan.add(src, dst);
                    }
                    l.gain.store_current(target);
                }
            }
            // 2. Traitement.
            let outs = &mut out_pool[e.out_start..e.out_start + e.out_count];
            match nodes[e.slot as usize].as_mut() {
                Some(node) => {
                    let mut io = NodeIo::new(
                        &in_pool[e.in_start..e.in_start + e.in_count],
                        &in_connected[e.in_start..e.in_start + e.in_count],
                        outs,
                        frames,
                    );
                    node.process(&ctx, &mut io);
                    run += 1;
                }
                None => {
                    for o in outs.iter_mut() {
                        o[..frames].fill(0.0);
                    }
                    missing += 1;
                    continue;
                }
            }
            // 3. Gain de nœud sur toutes les sorties.
            let target = e.gain.target();
            let plan = e.ramp.step(target, frames);
            if plan != RampPlan::Constant(1.0) {
                for o in outs.iter_mut() {
                    plan.apply(&mut o[..frames]);
                }
            }
            e.gain.store_current(target);
        }
        self.position += frames as u64;
        self.cycle += 1;
        CycleReport {
            nodes_run: run,
            nodes_missing: missing,
            graph_swapped: swapped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;
    use crate::node::{Node, PortSpec};
    use crate::types::{Db, Quantum};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Sortie constante.
    struct Const(f32);
    impl Node for Const {
        fn type_name(&self) -> &'static str {
            "const"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            vec![]
        }
        fn outputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(1)
        }
        fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
            io.output(0).fill(self.0);
        }
    }

    /// Multiplie l'entrée par un facteur.
    struct Mul(f32);
    impl Node for Mul {
        fn type_name(&self) -> &'static str {
            "mul"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(1)
        }
        fn outputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(1)
        }
        fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
            let (i, o) = io.in_out(0, 0);
            for (o, i) in o.iter_mut().zip(i) {
                *o = i * self.0;
            }
        }
    }

    /// Enregistre la dernière trame reçue (bits f32) et le nombre de cycles.
    struct Probe {
        last: Arc<AtomicU32>,
        cycles: Arc<AtomicU32>,
        connected: Arc<AtomicU32>,
    }
    impl Node for Probe {
        fn type_name(&self) -> &'static str {
            "probe"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(1)
        }
        fn outputs(&self) -> Vec<PortSpec> {
            vec![]
        }
        fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
            let x = *io.input(0).last().unwrap_or(&0.0);
            self.last.store(x.to_bits(), Ordering::Relaxed);
            self.cycles.fetch_add(1, Ordering::Relaxed);
            self.connected
                .store(io.is_input_connected(0) as u32, Ordering::Relaxed);
        }
    }

    fn probe() -> (Probe, Arc<AtomicU32>, Arc<AtomicU32>, Arc<AtomicU32>) {
        let last = Arc::new(AtomicU32::new(0));
        let cycles = Arc::new(AtomicU32::new(0));
        let connected = Arc::new(AtomicU32::new(0));
        (
            Probe {
                last: last.clone(),
                cycles: cycles.clone(),
                connected: connected.clone(),
            },
            last,
            cycles,
            connected,
        )
    }

    fn last_f32(a: &AtomicU32) -> f32 {
        f32::from_bits(a.load(Ordering::Relaxed))
    }

    #[test]
    fn const_gain_probe_chain() {
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(32).unwrap());
        let c = b.add_node(Box::new(Const(0.5)), "c");
        let m = b.add_node(Box::new(Mul(2.0)), "m");
        let (p, last, cycles, connected) = probe();
        let p = b.add_node(Box::new(p), "p");
        b.add_link(b.node(c).unwrap().output(0), b.node(m).unwrap().input(0))
            .unwrap();
        b.add_link(b.node(m).unwrap().output(0), b.node(p).unwrap().input(0))
            .unwrap();
        let mut ex = Executor::standalone(b.compile());
        assert!(ex.has_graph());
        let r = ex.run(32);
        assert_eq!(
            r,
            CycleReport {
                nodes_run: 3,
                nodes_missing: 0,
                graph_swapped: false
            }
        );
        assert_eq!(last_f32(&last), 1.0);
        assert_eq!(cycles.load(Ordering::Relaxed), 1);
        assert_eq!(connected.load(Ordering::Relaxed), 1);
        assert_eq!(ex.position(), 32);
        assert_eq!(ex.cycle(), 1);
        assert_eq!(ex.sample_rate(), Some(SampleRate::HZ_48000));
        ex.run(64); // borné au quantum
        assert_eq!(ex.position(), 64);
        assert!(format!("{ex:?}").contains("position"));
    }

    #[test]
    fn unconnected_input_is_silent_and_flagged() {
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(32).unwrap());
        let (p, last, _, connected) = probe();
        b.add_node(Box::new(p), "p");
        let mut ex = Executor::standalone(b.compile());
        ex.run(32);
        assert_eq!(last_f32(&last), 0.0);
        assert_eq!(connected.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn multiple_links_are_summed_with_link_gain() {
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(32).unwrap());
        let c1 = b.add_node(Box::new(Const(1.0)), "c1");
        let c2 = b.add_node(Box::new(Const(0.25)), "c2");
        let (p, last, _, _) = probe();
        let p = b.add_node(Box::new(p), "p");
        let pin = b.node(p).unwrap().input(0);
        let l1 = b.add_link(b.node(c1).unwrap().output(0), pin).unwrap();
        let l2 = b.add_link(b.node(c2).unwrap().output(0), pin).unwrap();
        b.link_gain(l2).unwrap().set(crate::types::Gain::new(2.0));
        let mut ex = Executor::standalone(b.compile());
        // Premier cycle : fondu entrant des nouveaux liens, la dernière trame atteint la cible.
        ex.run(32);
        assert!((last_f32(&last) - 1.5).abs() < 1e-6);
        ex.run(32);
        assert!((last_f32(&last) - 1.5).abs() < 1e-6);
        b.link_gain(l1).unwrap().set_muted(true);
        ex.run(32);
        assert!((last_f32(&last) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn link_fade_in_has_no_discontinuity() {
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(64).unwrap());
        let c = b.add_node(Box::new(Const(1.0)), "c");
        let m = b.add_node(Box::new(Mul(1.0)), "m");
        b.add_link(b.node(c).unwrap().output(0), b.node(m).unwrap().input(0))
            .unwrap();
        let mut ex = Executor::standalone(b.compile());
        ex.run(64);
        let g = ex.graph().unwrap();
        let out = g.output_buffer(g.output_index(m, 0).unwrap());
        assert!((out[0] - 1.0 / 64.0).abs() < 1e-6);
        assert!((out[63] - 1.0).abs() < 1e-6);
        let max_step = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step <= 1.0 / 64.0 + 1e-6);
    }

    #[test]
    fn node_gain_and_mute_apply_to_outputs() {
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(32).unwrap());
        let c = b.add_node(Box::new(Const(1.0)), "c");
        let (p, last, _, _) = probe();
        let p = b.add_node(Box::new(p), "p");
        b.add_link(b.node(c).unwrap().output(0), b.node(p).unwrap().input(0))
            .unwrap();
        let gain = b.node_gain(c).unwrap();
        let mut ex = Executor::standalone(b.compile());
        ex.run(32);
        assert!((last_f32(&last) - 1.0).abs() < 1e-6);
        gain.set_db(Db::new(-20.0));
        ex.run(32);
        assert!((last_f32(&last) - 0.1).abs() < 1e-5);
        gain.set_muted(true);
        ex.run(32);
        assert!(last_f32(&last).abs() < 1e-6, "fin de rampe vers le muet");
        ex.run(32);
        assert_eq!(last_f32(&last), 0.0, "cycle suivant : silence exact");
        assert_eq!(gain.current(), 0.0);
    }

    #[test]
    fn missing_node_outputs_silence() {
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(32).unwrap());
        let c = b.add_node(Box::new(Const(1.0)), "c");
        let (p, last, _, _) = probe();
        let p = b.add_node(Box::new(p), "p");
        b.add_link(b.node(c).unwrap().output(0), b.node(p).unwrap().input(0))
            .unwrap();
        let _first = b.compile(); // emporte les nœuds, jamais installé
        let second = b.compile(); // tout en « hérité », rien à hériter
        let mut ex = Executor::standalone(second);
        let r = ex.run(32);
        assert_eq!(r.nodes_missing, 2);
        assert_eq!(r.nodes_run, 0);
        assert_eq!(last_f32(&last), 0.0);
    }

    #[test]
    fn run_without_graph_is_noop() {
        let (rp, _rc) = crate::ring::RingBuffer::with_capacity(1);
        let (_pp, pc) = crate::ring::RingBuffer::with_capacity(1);
        let mut ex = Executor::new(pc, rp, 1);
        assert!(!ex.has_graph());
        assert_eq!(ex.run(32).nodes_run, 0);
        assert_eq!(ex.stats(), None);
        assert_eq!(ex.position(), 0);
    }
}
