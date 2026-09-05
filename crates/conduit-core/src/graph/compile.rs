//! Compilation du graphe en plan d'exécution.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::{Direction, GraphBuilder, NodeId, RawIndex};
use crate::gain::{GainParam, Ramp};
use crate::node::Node;
use crate::types::SampleRate;

/// Un nœud dans la liste d'exécution.
#[derive(Debug)]
pub(crate) struct NodeExec {
    pub(crate) slot: RawIndex,
    pub(crate) id: NodeId,
    pub(crate) in_start: usize,
    pub(crate) in_count: usize,
    pub(crate) out_start: usize,
    pub(crate) out_count: usize,
    pub(crate) gain: Arc<GainParam>,
    pub(crate) ramp: Ramp,
}

/// Un port d'entrée compilé : plage de liens entrants.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InputPort {
    pub(crate) links_start: usize,
    pub(crate) links_count: usize,
}

/// Un lien compilé.
#[derive(Debug)]
pub(crate) struct CompiledLink {
    /// Index global du tampon de sortie source.
    pub(crate) src_out: usize,
    pub(crate) gain: Arc<GainParam>,
    pub(crate) ramp: Ramp,
}

/// Statistiques de structure d'un graphe compilé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphStats {
    /// Nœuds dans la liste d'exécution.
    pub nodes: usize,
    /// Liens.
    pub links: usize,
    /// Ports d'entrée (tampons de mixage).
    pub input_ports: usize,
    /// Ports de sortie (tampons de sortie).
    pub output_ports: usize,
    /// Nœuds hérités de la version précédente (déjà côté fil audio).
    pub inherited: usize,
    /// Nœuds nouveaux transportés par cette version.
    pub incoming: usize,
}

/// Graphe compilé : ordre topologique, tampons pré-alloués, table des nœuds.
///
/// Construit hors temps réel par [`GraphBuilder::compile`], puis transmis au fil
/// audio qui l'exécute. Sa structure ne change plus ; seuls l'état des nœuds et des
/// rampes de gain évoluent pendant l'exécution.
pub struct CompiledGraph {
    pub(crate) sample_rate: SampleRate,
    pub(crate) max_frames: usize,
    /// Table des nœuds indexée par emplacement. `None` : soit à hériter, soit vide.
    pub(crate) nodes: Vec<Option<Box<dyn Node>>>,
    /// Emplacements dont le nœud doit être repris de la version précédente.
    pub(crate) inherit: Vec<bool>,
    pub(crate) exec: Vec<NodeExec>,
    pub(crate) inputs: Vec<InputPort>,
    pub(crate) links: Vec<CompiledLink>,
    pub(crate) in_pool: Vec<Box<[f32]>>,
    pub(crate) in_connected: Vec<bool>,
    pub(crate) out_pool: Vec<Box<[f32]>>,
    /// Nombre de nœuds nouveaux transportés (pour les statistiques).
    pub(crate) incoming: usize,
}

impl core::fmt::Debug for CompiledGraph {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CompiledGraph")
            .field("sample_rate", &self.sample_rate)
            .field("max_frames", &self.max_frames)
            .field("stats", &self.stats())
            .field("order", &self.order().collect::<Vec<_>>())
            .finish()
    }
}

impl CompiledGraph {
    /// Fréquence.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Taille maximale d'un cycle (quantum).
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Statistiques de structure.
    pub fn stats(&self) -> GraphStats {
        GraphStats {
            nodes: self.exec.len(),
            links: self.links.len(),
            input_ports: self.in_pool.len(),
            output_ports: self.out_pool.len(),
            inherited: self.inherit.iter().filter(|&&b| b).count(),
            incoming: self.incoming,
        }
    }

    /// Ordre d'exécution des nœuds.
    pub fn order(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.exec.iter().map(|e| e.id)
    }

    /// Vrai si tous les nœuds requis sont présents dans la table (après installation).
    pub fn is_complete(&self) -> bool {
        self.exec
            .iter()
            .all(|e| self.nodes[e.slot as usize].is_some())
    }

    /// Extrait les nœuds transportés (non hérités) avec leur emplacement.
    pub(crate) fn into_incoming(self) -> impl Iterator<Item = (RawIndex, Box<dyn Node>)> {
        self.nodes
            .into_iter()
            .enumerate()
            .filter_map(|(i, n)| n.map(|n| (i as RawIndex, n)))
    }

    /// Lecture d'un tampon de sortie (index global), pour les tests et le moniteur.
    pub fn output_buffer(&self, global_index: usize) -> &[f32] {
        &self.out_pool[global_index]
    }

    /// Index global du tampon de sortie d'un port, s'il est dans ce graphe.
    pub fn output_index(&self, node: NodeId, port: usize) -> Option<usize> {
        self.exec
            .iter()
            .find(|e| e.id == node)
            .filter(|e| port < e.out_count)
            .map(|e| e.out_start + port)
    }
}

pub(super) fn compile(b: &mut GraphBuilder) -> Box<CompiledGraph> {
    let slot_count = b.slot_count();
    let max_frames = b.quantum().get();

    // Liens par nœud source (index brut) → nœud destination, pour Kahn.
    let mut indegree = vec![0usize; slot_count];
    let mut succ: Vec<Vec<RawIndex>> = vec![Vec::new(); slot_count];
    let live: Vec<bool> = (0..slot_count).map(|i| b.entry_at(i).is_some()).collect();
    for (l, _) in b.link_entries() {
        succ[l.src.node.index() as usize].push(l.dst.node.index());
        indegree[l.dst.node.index() as usize] += 1;
    }

    // Ordre topologique déterministe : parmi les nœuds prêts, le plus petit index d'abord.
    let mut heap: BinaryHeap<Reverse<RawIndex>> = (0..slot_count)
        .filter(|&i| live[i] && indegree[i] == 0)
        .map(|i| Reverse(i as RawIndex))
        .collect();
    let mut order: Vec<RawIndex> = Vec::with_capacity(slot_count);
    while let Some(Reverse(i)) = heap.pop() {
        order.push(i);
        for &s in &succ[i as usize] {
            indegree[s as usize] -= 1;
            if indegree[s as usize] == 0 {
                heap.push(Reverse(s));
            }
        }
    }
    debug_assert_eq!(
        order.len(),
        live.iter().filter(|&&l| l).count(),
        "le graphe contient un cycle"
    );

    // Attribution des tampons et extraction des nœuds.
    let mut nodes: Vec<Option<Box<dyn Node>>> = Vec::with_capacity(slot_count);
    let mut inherit = vec![false; slot_count];
    let mut exec = Vec::with_capacity(order.len());
    let mut in_base = vec![0usize; slot_count];
    let mut out_base = vec![0usize; slot_count];
    let mut in_total = 0usize;
    let mut out_total = 0usize;
    let mut incoming = 0usize;
    nodes.resize_with(slot_count, || None);
    for &i in &order {
        let (info, gain, node) = b.entry_at(i as usize).expect("nœud vivant dans l'ordre");
        let in_count = info.inputs.len();
        let out_count = info.outputs.len();
        in_base[i as usize] = in_total;
        out_base[i as usize] = out_total;
        exec.push(NodeExec {
            slot: i,
            id: info.id,
            in_start: in_total,
            in_count,
            out_start: out_total,
            out_count,
            gain: Arc::clone(gain),
            ramp: Ramp::new(gain.current()),
        });
        in_total += in_count;
        out_total += out_count;
        match node.take() {
            Some(n) => {
                nodes[i as usize] = Some(n);
                incoming += 1;
            }
            None => inherit[i as usize] = true,
        }
    }

    // Liens groupés par port d'entrée, dans l'ordre des ports.
    let mut inputs = Vec::with_capacity(in_total);
    let mut links = Vec::new();
    for e in &exec {
        for p in 0..e.in_count {
            let start = links.len();
            for (l, gain) in b.link_entries() {
                if l.dst.node.index() == e.slot
                    && l.dst.direction == Direction::Input
                    && l.dst.index as usize == p
                {
                    links.push(CompiledLink {
                        src_out: out_base[l.src.node.index() as usize] + l.src.index as usize,
                        gain: Arc::clone(gain),
                        ramp: Ramp::new(gain.current()),
                    });
                }
            }
            inputs.push(InputPort {
                links_start: start,
                links_count: links.len() - start,
            });
        }
    }

    Box::new(CompiledGraph {
        sample_rate: b.sample_rate(),
        max_frames,
        nodes,
        inherit,
        exec,
        inputs,
        links,
        in_pool: (0..in_total)
            .map(|_| vec![0.0; max_frames].into_boxed_slice())
            .collect(),
        in_connected: vec![false; in_total],
        out_pool: (0..out_total)
            .map(|_| vec![0.0; max_frames].into_boxed_slice())
            .collect(),
        incoming,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::LinkId;
    use crate::node::{NodeIo, PortSpec, ProcessContext};
    use crate::types::Quantum;

    struct Dummy(usize, usize);
    impl Node for Dummy {
        fn type_name(&self) -> &'static str {
            "dummy"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(self.0)
        }
        fn outputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(self.1)
        }
        fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
            io.silence_outputs();
        }
    }

    fn builder() -> GraphBuilder {
        GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(64).unwrap())
    }

    fn link(b: &mut GraphBuilder, s: NodeId, sp: usize, d: NodeId, dp: usize) -> LinkId {
        let src = b.node(s).unwrap().output(sp);
        let dst = b.node(d).unwrap().input(dp);
        b.add_link(src, dst).unwrap()
    }

    #[test]
    fn chain_is_ordered_source_to_sink_regardless_of_insertion() {
        let mut b = builder();
        let sink = b.add_node(Box::new(Dummy(1, 0)), "sink");
        let mid = b.add_node(Box::new(Dummy(1, 1)), "mid");
        let src = b.add_node(Box::new(Dummy(0, 1)), "src");
        link(&mut b, src, 0, mid, 0);
        link(&mut b, mid, 0, sink, 0);
        let g = b.compile();
        assert_eq!(g.order().collect::<Vec<_>>(), vec![src, mid, sink]);
        let st = g.stats();
        assert_eq!(
            (st.nodes, st.links, st.input_ports, st.output_ports),
            (3, 2, 2, 2)
        );
        assert_eq!(st.incoming, 3);
        assert_eq!(st.inherited, 0);
        assert!(g.is_complete());
        assert!(format!("{g:?}").contains("order"));
    }

    #[test]
    fn diamond_keeps_dependencies_and_is_deterministic() {
        let mut b = builder();
        let a = b.add_node(Box::new(Dummy(0, 1)), "a");
        let c = b.add_node(Box::new(Dummy(1, 1)), "c");
        let d = b.add_node(Box::new(Dummy(1, 1)), "d");
        let e = b.add_node(Box::new(Dummy(2, 0)), "e");
        link(&mut b, a, 0, d, 0);
        link(&mut b, a, 0, c, 0);
        link(&mut b, d, 0, e, 1);
        link(&mut b, c, 0, e, 0);
        let g = b.compile();
        let order: Vec<_> = g.order().collect();
        assert_eq!(order, vec![a, c, d, e]);
        let pos = |n: NodeId| order.iter().position(|&x| x == n).unwrap();
        assert!(pos(a) < pos(c) && pos(a) < pos(d) && pos(c) < pos(e) && pos(d) < pos(e));
        // Deux liens arrivent sur des ports différents de e ; chaque port a son tampon.
        assert_eq!(g.inputs.len(), 4);
        let e_exec = g.exec.iter().find(|x| x.id == e).unwrap();
        let p0 = g.inputs[e_exec.in_start];
        let p1 = g.inputs[e_exec.in_start + 1];
        assert_eq!((p0.links_count, p1.links_count), (1, 1));
        assert_eq!(
            g.links[p0.links_start].src_out,
            g.output_index(c, 0).unwrap()
        );
        assert_eq!(
            g.links[p1.links_start].src_out,
            g.output_index(d, 0).unwrap()
        );
    }

    #[test]
    fn no_buffer_is_shared_between_ports() {
        let mut b = builder();
        let a = b.add_node(Box::new(Dummy(0, 2)), "a");
        let c = b.add_node(Box::new(Dummy(2, 2)), "c");
        link(&mut b, a, 0, c, 0);
        link(&mut b, a, 1, c, 1);
        link(&mut b, a, 0, c, 1); // mixage : deux liens sur c:in1
        let g = b.compile();
        let ptrs: std::collections::HashSet<*const f32> = g
            .in_pool
            .iter()
            .chain(g.out_pool.iter())
            .map(|b| b.as_ptr())
            .collect();
        assert_eq!(ptrs.len(), g.in_pool.len() + g.out_pool.len());
        assert_eq!(g.stats().links, 3);
        let c_exec = g.exec.iter().find(|x| x.id == c).unwrap();
        assert_eq!(g.inputs[c_exec.in_start + 1].links_count, 2);
        assert_eq!(g.output_index(c, 2), None);
        assert_eq!(g.output_buffer(0).len(), 64);
    }

    #[test]
    fn second_compile_inherits_nodes_and_carries_new_ones() {
        let mut b = builder();
        let a = b.add_node(Box::new(Dummy(0, 1)), "a");
        let g1 = b.compile();
        assert_eq!(g1.stats().incoming, 1);
        let c = b.add_node(Box::new(Dummy(1, 0)), "c");
        link(&mut b, a, 0, c, 0);
        let g2 = b.compile();
        let st = g2.stats();
        assert_eq!((st.incoming, st.inherited), (1, 1));
        assert!(g2.inherit[a.index() as usize]);
        assert!(!g2.inherit[c.index() as usize]);
        assert!(!g2.is_complete(), "a n'est pas encore hérité");
        // Reprise des nœuds d'un graphe jamais installé.
        b.reclaim(g2);
        let g3 = b.compile();
        assert_eq!(g3.stats().incoming, 1, "c est de nouveau transporté");
        b.reclaim(g1);
        let g4 = b.compile();
        assert_eq!(g4.stats().incoming, 1, "a est repris ; c est parti dans g3");
        // Un nœud supprimé entre-temps est simplement libéré à la reprise.
        b.remove_node(c).unwrap();
        b.reclaim(g3);
        let g5 = b.compile();
        assert_eq!(g5.stats().incoming, 0);
        assert_eq!(g5.stats().nodes, 1);
    }

    #[test]
    fn removed_slots_are_skipped() {
        let mut b = builder();
        let a = b.add_node(Box::new(Dummy(0, 1)), "a");
        let c = b.add_node(Box::new(Dummy(1, 0)), "c");
        b.remove_node(a).unwrap();
        let g = b.compile();
        assert_eq!(g.order().collect::<Vec<_>>(), vec![c]);
        assert_eq!(g.nodes.len(), 2);
        assert!(g.nodes[0].is_none() && !g.inherit[0]);
    }
}
