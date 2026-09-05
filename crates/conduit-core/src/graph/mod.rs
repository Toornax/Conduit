//! Modèle du graphe : identifiants, description des nœuds, construction et validation.
//!
//! Le [`GraphBuilder`] est la représentation **modifiable** du graphe, côté fil de
//! gestion. Il valide chaque modification (ports existants, directions, absence de
//! cycle) et produit à la demande un [`CompiledGraph`] destiné au fil audio.

mod compile;
mod ids;

pub use compile::{CompiledGraph, GraphStats};
pub use ids::{LinkId, NodeId, PortId, PortIndex, RawIndex};

use std::sync::Arc;

use crate::gain::GainParam;
use crate::node::{Node, PortSpec};
use crate::types::{Gain, Quantum, SampleRate};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Direction d'un port, vue du nœud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum Direction {
    /// Le port reçoit de l'audio.
    Input,
    /// Le port produit de l'audio.
    Output,
}

impl Direction {
    /// Direction opposée.
    pub fn opposite(self) -> Direction {
        match self {
            Direction::Input => Direction::Output,
            Direction::Output => Direction::Input,
        }
    }
}

impl core::fmt::Display for Direction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Direction::Input => "entrée",
            Direction::Output => "sortie",
        })
    }
}

/// Erreurs de modification du graphe.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// Le nœud n'existe pas (ou plus).
    #[error("nœud inconnu {0}")]
    UnknownNode(NodeId),
    /// Le port n'existe pas sur ce nœud.
    #[error("port inconnu {0}")]
    UnknownPort(PortId),
    /// Le lien n'existe pas (ou plus).
    #[error("lien inconnu {0}")]
    UnknownLink(LinkId),
    /// La source d'un lien doit être une sortie et la destination une entrée.
    #[error("un lien va d'une sortie vers une entrée, pas de {src} vers {dst}")]
    WrongDirection {
        /// Direction du port source fourni.
        src: Direction,
        /// Direction du port destination fourni.
        dst: Direction,
    },
    /// Le lien relierait un nœud à lui-même.
    #[error("un nœud ne peut pas être relié à lui-même ({0})")]
    SelfLink(NodeId),
    /// Le lien créerait un cycle ; `path` va de la source à la destination puis
    /// revient à la source.
    #[error("ce lien créerait une boucle : {}", format_path(path))]
    WouldCycle {
        /// Nœuds du cycle, premier = dernier.
        path: Vec<NodeId>,
    },
    /// Ce lien existe déjà.
    #[error("ce lien existe déjà ({0})")]
    DuplicateLink(LinkId),
}

fn format_path(path: &[NodeId]) -> String {
    path.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Description d'un nœud du graphe (hors état de traitement).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NodeInfo {
    /// Identifiant.
    pub id: NodeId,
    /// Type de nœud (`"sine"`, `"mixer"`, …).
    pub type_name: &'static str,
    /// Nom d'affichage.
    pub label: String,
    /// Ports d'entrée.
    pub inputs: Vec<PortSpec>,
    /// Ports de sortie.
    pub outputs: Vec<PortSpec>,
}

impl NodeInfo {
    /// Ports dans une direction.
    pub fn ports(&self, dir: Direction) -> &[PortSpec] {
        match dir {
            Direction::Input => &self.inputs,
            Direction::Output => &self.outputs,
        }
    }

    /// Cherche un port par nom.
    pub fn port(&self, dir: Direction, name: &str) -> Option<PortId> {
        self.ports(dir)
            .iter()
            .position(|p| p.name == name)
            .map(|i| PortId::new(self.id, dir, i as PortIndex))
    }

    /// Identifiant du `i`-ème port d'entrée.
    pub fn input(&self, i: usize) -> PortId {
        assert!(i < self.inputs.len(), "entrée {i} hors bornes");
        PortId::new(self.id, Direction::Input, i as PortIndex)
    }

    /// Identifiant du `i`-ème port de sortie.
    pub fn output(&self, i: usize) -> PortId {
        assert!(i < self.outputs.len(), "sortie {i} hors bornes");
        PortId::new(self.id, Direction::Output, i as PortIndex)
    }
}

/// Description d'un lien.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LinkInfo {
    /// Identifiant.
    pub id: LinkId,
    /// Port source (une sortie).
    pub src: PortId,
    /// Port destination (une entrée).
    pub dst: PortId,
}

struct NodeSlot {
    generation: u32,
    entry: Option<NodeEntry>,
}

struct NodeEntry {
    info: NodeInfo,
    gain: Arc<GainParam>,
    /// `Some` tant que le nœud n'a pas été transmis au fil audio.
    node: Option<Box<dyn Node>>,
}

struct LinkSlot {
    generation: u32,
    entry: Option<LinkEntry>,
}

struct LinkEntry {
    info: LinkInfo,
    gain: Arc<GainParam>,
}

/// Graphe modifiable, côté fil de gestion.
pub struct GraphBuilder {
    sample_rate: SampleRate,
    quantum: Quantum,
    nodes: Vec<NodeSlot>,
    links: Vec<LinkSlot>,
    free_nodes: Vec<RawIndex>,
    free_links: Vec<RawIndex>,
}

impl core::fmt::Debug for GraphBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GraphBuilder")
            .field("sample_rate", &self.sample_rate)
            .field("quantum", &self.quantum)
            .field("nodes", &self.node_count())
            .field("links", &self.link_count())
            .finish()
    }
}

impl GraphBuilder {
    /// Graphe vide pour une fréquence et un quantum donnés.
    pub fn new(sample_rate: SampleRate, quantum: Quantum) -> Self {
        Self {
            sample_rate,
            quantum,
            nodes: Vec::new(),
            links: Vec::new(),
            free_nodes: Vec::new(),
            free_links: Vec::new(),
        }
    }

    /// Fréquence du graphe.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Quantum du graphe.
    pub fn quantum(&self) -> Quantum {
        self.quantum
    }

    /// Nombre de nœuds vivants.
    pub fn node_count(&self) -> usize {
        self.nodes.iter().filter(|s| s.entry.is_some()).count()
    }

    /// Nombre de liens vivants.
    pub fn link_count(&self) -> usize {
        self.links.iter().filter(|s| s.entry.is_some()).count()
    }

    /// Ajoute un nœud avec un nom d'affichage. Appelle `prepare` sur le nœud.
    pub fn add_node(&mut self, mut node: Box<dyn Node>, label: impl Into<String>) -> NodeId {
        node.prepare(self.sample_rate, self.quantum.get());
        let make_entry = |id: NodeId| NodeEntry {
            info: NodeInfo {
                id,
                type_name: node.type_name(),
                label: label.into(),
                inputs: node.inputs(),
                outputs: node.outputs(),
            },
            gain: Arc::new(GainParam::new(Gain::UNITY)),
            node: Some(node),
        };
        if let Some(index) = self.free_nodes.pop() {
            let slot = &mut self.nodes[index as usize];
            let id = NodeId::new(index, slot.generation);
            slot.entry = Some(make_entry(id));
            id
        } else {
            let index = self.nodes.len() as RawIndex;
            let id = NodeId::new(index, 0);
            self.nodes.push(NodeSlot {
                generation: 0,
                entry: Some(make_entry(id)),
            });
            id
        }
    }

    /// Retire un nœud et tous ses liens. Le nœud lui-même est libéré par le fil de
    /// gestion quand le fil audio a adopté la version suivante du graphe.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), GraphError> {
        let slot = self.slot_mut(id)?;
        slot.entry = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_nodes.push(id.index());
        let to_remove: Vec<LinkId> = self
            .links
            .iter()
            .filter_map(|s| s.entry.as_ref())
            .filter(|e| e.info.src.node == id || e.info.dst.node == id)
            .map(|e| e.info.id)
            .collect();
        for l in to_remove {
            let _ = self.remove_link(l);
        }
        Ok(())
    }

    /// Change le nom d'affichage d'un nœud.
    pub fn set_label(&mut self, id: NodeId, label: impl Into<String>) -> Result<(), GraphError> {
        self.entry_mut(id)?.info.label = label.into();
        Ok(())
    }

    /// Description d'un nœud.
    pub fn node(&self, id: NodeId) -> Result<&NodeInfo, GraphError> {
        self.entry(id).map(|e| &e.info)
    }

    /// Tous les nœuds vivants, par ordre d'index.
    pub fn nodes(&self) -> impl Iterator<Item = &NodeInfo> + '_ {
        self.nodes
            .iter()
            .filter_map(|s| s.entry.as_ref())
            .map(|e| &e.info)
    }

    /// Paramètre de gain global (et muet) d'un nœud, appliqué à toutes ses sorties.
    pub fn node_gain(&self, id: NodeId) -> Result<Arc<GainParam>, GraphError> {
        self.entry(id).map(|e| Arc::clone(&e.gain))
    }

    /// Vrai si le port existe.
    pub fn has_port(&self, port: PortId) -> bool {
        self.entry(port.node)
            .map(|e| (port.index as usize) < e.info.ports(port.direction).len())
            .unwrap_or(false)
    }

    /// Cherche un port par nom sur un nœud.
    pub fn find_port(&self, node: NodeId, dir: Direction, name: &str) -> Option<PortId> {
        self.node(node).ok()?.port(dir, name)
    }

    /// Crée un lien d'une sortie vers une entrée, gain unité.
    pub fn add_link(&mut self, src: PortId, dst: PortId) -> Result<LinkId, GraphError> {
        self.add_link_with_gain(src, dst, Gain::UNITY)
    }

    /// Crée un lien avec un gain initial.
    pub fn add_link_with_gain(
        &mut self,
        src: PortId,
        dst: PortId,
        gain: Gain,
    ) -> Result<LinkId, GraphError> {
        if src.direction != Direction::Output || dst.direction != Direction::Input {
            return Err(GraphError::WrongDirection {
                src: src.direction,
                dst: dst.direction,
            });
        }
        self.check_port(src)?;
        self.check_port(dst)?;
        if src.node == dst.node {
            return Err(GraphError::SelfLink(src.node));
        }
        if let Some(existing) = self.find_link(src, dst) {
            return Err(GraphError::DuplicateLink(existing));
        }
        if let Some(mut path) = self.path(dst.node, src.node) {
            path.insert(0, src.node);
            return Err(GraphError::WouldCycle { path });
        }
        let make_entry = |id: LinkId| LinkEntry {
            info: LinkInfo { id, src, dst },
            gain: Arc::new(GainParam::new_faded_in(gain)),
        };
        if let Some(index) = self.free_links.pop() {
            let slot = &mut self.links[index as usize];
            let id = LinkId::new(index, slot.generation);
            slot.entry = Some(make_entry(id));
            Ok(id)
        } else {
            let index = self.links.len() as RawIndex;
            let id = LinkId::new(index, 0);
            self.links.push(LinkSlot {
                generation: 0,
                entry: Some(make_entry(id)),
            });
            Ok(id)
        }
    }

    /// Supprime un lien.
    pub fn remove_link(&mut self, id: LinkId) -> Result<(), GraphError> {
        let slot = self
            .links
            .get_mut(id.index() as usize)
            .filter(|s| s.generation == id.generation() && s.entry.is_some())
            .ok_or(GraphError::UnknownLink(id))?;
        slot.entry = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_links.push(id.index());
        Ok(())
    }

    /// Description d'un lien.
    pub fn link(&self, id: LinkId) -> Result<&LinkInfo, GraphError> {
        self.link_entry(id).map(|e| &e.info)
    }

    /// Paramètre de gain (et muet) d'un lien.
    pub fn link_gain(&self, id: LinkId) -> Result<Arc<GainParam>, GraphError> {
        self.link_entry(id).map(|e| Arc::clone(&e.gain))
    }

    /// Tous les liens vivants, par ordre d'index.
    pub fn links(&self) -> impl Iterator<Item = &LinkInfo> + '_ {
        self.links
            .iter()
            .filter_map(|s| s.entry.as_ref())
            .map(|e| &e.info)
    }

    /// Lien existant entre deux ports, s'il y en a un.
    pub fn find_link(&self, src: PortId, dst: PortId) -> Option<LinkId> {
        self.links()
            .find(|l| l.src == src && l.dst == dst)
            .map(|l| l.id)
    }

    /// Liens arrivant sur un port d'entrée.
    pub fn links_into(&self, dst: PortId) -> impl Iterator<Item = &LinkInfo> + '_ {
        self.links().filter(move |l| l.dst == dst)
    }

    /// Liens partant d'un port de sortie.
    pub fn links_from(&self, src: PortId) -> impl Iterator<Item = &LinkInfo> + '_ {
        self.links().filter(move |l| l.src == src)
    }

    /// Chemin de `from` à `to` en suivant les liens, s'il existe (`from` inclus,
    /// `to` inclus). `Some(vec![from])` si `from == to`.
    pub fn path(&self, from: NodeId, to: NodeId) -> Option<Vec<NodeId>> {
        let mut stack = vec![from];
        let mut parent: Vec<Option<NodeId>> = vec![None; self.nodes.len()];
        let mut visited = vec![false; self.nodes.len()];
        visited[from.index() as usize] = true;
        while let Some(cur) = stack.pop() {
            if cur == to {
                let mut path = vec![cur];
                let mut c = cur;
                while let Some(p) = parent[c.index() as usize] {
                    path.push(p);
                    c = p;
                }
                path.reverse();
                return Some(path);
            }
            for l in self.links() {
                if l.src.node == cur {
                    let next = l.dst.node;
                    let vi = next.index() as usize;
                    if !visited[vi] {
                        visited[vi] = true;
                        parent[vi] = Some(cur);
                        stack.push(next);
                    }
                }
            }
        }
        None
    }

    /// Compile l'état courant en un graphe exécutable. Les nœuds pas encore transmis
    /// au fil audio sont déplacés dans le résultat.
    pub fn compile(&mut self) -> Box<CompiledGraph> {
        compile::compile(self)
    }

    /// Reprend les nœuds contenus dans un graphe compilé qui n'a **pas** été installé
    /// par le fil audio, pour qu'ils repartent dans la prochaine compilation.
    pub fn reclaim(&mut self, graph: Box<CompiledGraph>) {
        for (index, node) in graph.into_incoming() {
            if let Some(entry) = self
                .nodes
                .get_mut(index as usize)
                .and_then(|s| s.entry.as_mut())
            {
                if entry.node.is_none() {
                    entry.node = Some(node);
                }
            }
            // Sinon le nœud a été retiré entre-temps : on le libère ici, hors temps réel.
        }
    }

    // --- internes ---

    fn slot_mut(&mut self, id: NodeId) -> Result<&mut NodeSlot, GraphError> {
        self.nodes
            .get_mut(id.index() as usize)
            .filter(|s| s.generation == id.generation() && s.entry.is_some())
            .ok_or(GraphError::UnknownNode(id))
    }

    fn entry(&self, id: NodeId) -> Result<&NodeEntry, GraphError> {
        self.nodes
            .get(id.index() as usize)
            .filter(|s| s.generation == id.generation())
            .and_then(|s| s.entry.as_ref())
            .ok_or(GraphError::UnknownNode(id))
    }

    fn entry_mut(&mut self, id: NodeId) -> Result<&mut NodeEntry, GraphError> {
        self.slot_mut(id)
            .map(|s| s.entry.as_mut().expect("vérifié par slot_mut"))
    }

    fn link_entry(&self, id: LinkId) -> Result<&LinkEntry, GraphError> {
        self.links
            .get(id.index() as usize)
            .filter(|s| s.generation == id.generation())
            .and_then(|s| s.entry.as_ref())
            .ok_or(GraphError::UnknownLink(id))
    }

    fn check_port(&self, port: PortId) -> Result<(), GraphError> {
        if self.has_port(port) {
            Ok(())
        } else if self.entry(port.node).is_err() {
            Err(GraphError::UnknownNode(port.node))
        } else {
            Err(GraphError::UnknownPort(port))
        }
    }

    /// Accès interne pour la compilation : nombre de slots (vivants ou non).
    pub(crate) fn slot_count(&self) -> usize {
        self.nodes.len()
    }

    /// Accès interne : entrée d'un slot par index brut, si vivante.
    #[allow(clippy::type_complexity)]
    pub(crate) fn entry_at(
        &mut self,
        index: usize,
    ) -> Option<(&NodeInfo, &Arc<GainParam>, &mut Option<Box<dyn Node>>)> {
        self.nodes[index]
            .entry
            .as_mut()
            .map(|e| (&e.info, &e.gain, &mut e.node))
    }

    /// Accès interne : liens vivants avec leur paramètre de gain.
    pub(crate) fn link_entries(&self) -> impl Iterator<Item = (&LinkInfo, &Arc<GainParam>)> + '_ {
        self.links
            .iter()
            .filter_map(|s| s.entry.as_ref())
            .map(|e| (&e.info, &e.gain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeIo, ProcessContext};

    struct Dummy {
        ins: usize,
        outs: usize,
    }

    impl Node for Dummy {
        fn type_name(&self) -> &'static str {
            "dummy"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(self.ins)
        }
        fn outputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(self.outs)
        }
        fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
            io.silence_outputs();
        }
    }

    fn dummy(b: &mut GraphBuilder, ins: usize, outs: usize) -> NodeId {
        b.add_node(Box::new(Dummy { ins, outs }), "d")
    }

    fn builder() -> GraphBuilder {
        GraphBuilder::new(SampleRate::HZ_48000, Quantum::DEFAULT)
    }

    #[test]
    fn add_and_remove_nodes_with_generations() {
        let mut b = builder();
        let a = dummy(&mut b, 1, 1);
        assert_eq!(b.node_count(), 1);
        assert_eq!(b.node(a).unwrap().type_name, "dummy");
        b.remove_node(a).unwrap();
        assert_eq!(b.node_count(), 0);
        assert_eq!(b.remove_node(a), Err(GraphError::UnknownNode(a)));
        let a2 = dummy(&mut b, 1, 1);
        assert_eq!(a2.index(), a.index(), "l'emplacement est réutilisé");
        assert_ne!(a2, a, "mais la génération diffère");
        assert!(b.node(a).is_err());
        assert!(b.node(a2).is_ok());
    }

    #[test]
    fn links_validate_ports_and_directions() {
        let mut b = builder();
        let a = dummy(&mut b, 0, 2);
        let c = dummy(&mut b, 2, 0);
        let out0 = b.node(a).unwrap().output(0);
        let in0 = b.node(c).unwrap().input(0);
        let l = b.add_link(out0, in0).unwrap();
        assert_eq!(b.link(l).unwrap().src, out0);
        assert_eq!(b.add_link(out0, in0), Err(GraphError::DuplicateLink(l)));
        assert_eq!(
            b.add_link(in0, out0),
            Err(GraphError::WrongDirection {
                src: Direction::Input,
                dst: Direction::Output
            })
        );
        let bad = PortId::new(a, Direction::Output, 5);
        assert_eq!(b.add_link(bad, in0), Err(GraphError::UnknownPort(bad)));
        let ghost = NodeId::new(42, 0);
        assert_eq!(
            b.add_link(PortId::new(ghost, Direction::Output, 0), in0),
            Err(GraphError::UnknownNode(ghost))
        );
        assert_eq!(b.links_into(in0).count(), 1);
        assert_eq!(b.links_from(out0).count(), 1);
        assert_eq!(b.find_link(out0, in0), Some(l));
        b.remove_link(l).unwrap();
        assert_eq!(b.remove_link(l), Err(GraphError::UnknownLink(l)));
        assert_eq!(b.link_count(), 0);
    }

    #[test]
    fn self_link_is_refused() {
        let mut b = builder();
        let a = dummy(&mut b, 1, 1);
        let info = b.node(a).unwrap().clone();
        assert_eq!(
            b.add_link(info.output(0), info.input(0)),
            Err(GraphError::SelfLink(a))
        );
    }

    #[test]
    fn direct_cycle_is_refused_with_path() {
        let mut b = builder();
        let a = dummy(&mut b, 1, 1);
        let c = dummy(&mut b, 1, 1);
        let (ai, ci) = (b.node(a).unwrap().clone(), b.node(c).unwrap().clone());
        b.add_link(ai.output(0), ci.input(0)).unwrap();
        let err = b.add_link(ci.output(0), ai.input(0)).unwrap_err();
        assert_eq!(
            err,
            GraphError::WouldCycle {
                path: vec![c, a, c]
            }
        );
        assert!(err.to_string().contains("boucle"));
    }

    #[test]
    fn indirect_cycle_is_refused_with_full_path() {
        let mut b = builder();
        let ids: Vec<_> = (0..4).map(|_| dummy(&mut b, 1, 1)).collect();
        let infos: Vec<_> = ids.iter().map(|&i| b.node(i).unwrap().clone()).collect();
        for w in infos.windows(2) {
            b.add_link(w[0].output(0), w[1].input(0)).unwrap();
        }
        let err = b
            .add_link(infos[3].output(0), infos[0].input(0))
            .unwrap_err();
        assert_eq!(
            err,
            GraphError::WouldCycle {
                path: vec![ids[3], ids[0], ids[1], ids[2], ids[3]]
            }
        );
        // Un lien parallèle (même sens) ne crée pas de cycle.
        let d = dummy(&mut b, 1, 1);
        let di = b.node(d).unwrap().clone();
        b.add_link(infos[0].output(0), di.input(0)).unwrap();
        b.add_link(di.output(0), infos[2].input(0)).unwrap();
    }

    #[test]
    fn removing_node_removes_its_links_and_frees_links() {
        let mut b = builder();
        let a = dummy(&mut b, 0, 1);
        let c = dummy(&mut b, 1, 1);
        let d = dummy(&mut b, 1, 0);
        let (ai, ci, di) = (
            b.node(a).unwrap().clone(),
            b.node(c).unwrap().clone(),
            b.node(d).unwrap().clone(),
        );
        b.add_link(ai.output(0), ci.input(0)).unwrap();
        let l2 = b.add_link(ci.output(0), di.input(0)).unwrap();
        b.remove_node(c).unwrap();
        assert_eq!(b.link_count(), 0);
        assert!(b.link(l2).is_err());
        // Après suppression du nœud intermédiaire, a → d est possible.
        b.add_link(ai.output(0), di.input(0)).unwrap();
    }

    #[test]
    fn labels_and_port_lookup() {
        let mut b = builder();
        let a = b.add_node(Box::new(Dummy { ins: 2, outs: 2 }), "Casque");
        assert_eq!(b.node(a).unwrap().label, "Casque");
        b.set_label(a, "Enceintes").unwrap();
        assert_eq!(b.node(a).unwrap().label, "Enceintes");
        let fr = b.find_port(a, Direction::Input, "FR").unwrap();
        assert_eq!(fr.index, 1);
        assert!(b.find_port(a, Direction::Input, "FC").is_none());
        assert!(b.has_port(fr));
        assert!(!b.has_port(PortId::new(a, Direction::Output, 2)));
        assert_eq!(b.node_gain(a).unwrap().target(), 1.0);
        assert_eq!(Direction::Input.opposite(), Direction::Output);
        assert_eq!(Direction::Output.to_string(), "sortie");
    }

    #[test]
    fn path_queries() {
        let mut b = builder();
        let a = dummy(&mut b, 1, 1);
        let c = dummy(&mut b, 1, 1);
        assert_eq!(b.path(a, a), Some(vec![a]));
        assert_eq!(b.path(a, c), None);
        let (ai, ci) = (b.node(a).unwrap().clone(), b.node(c).unwrap().clone());
        b.add_link(ai.output(0), ci.input(0)).unwrap();
        assert_eq!(b.path(a, c), Some(vec![a, c]));
        assert_eq!(b.path(c, a), None);
    }
}
