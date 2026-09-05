//! Trait [`Node`] et contexte de traitement.

use crate::types::SampleRate;
use core::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Position d'un canal dans une disposition standard.
///
/// Sert à l'adaptation automatique de canaux (F-15) et aux règles d'auto-connexion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "UPPERCASE")
)]
pub enum ChannelLabel {
    /// Mono ou sans position définie.
    #[default]
    Mono,
    /// Avant gauche.
    FL,
    /// Avant droit.
    FR,
    /// Centre.
    FC,
    /// Caisson de basses.
    LFE,
    /// Arrière gauche.
    RL,
    /// Arrière droit.
    RR,
    /// Latéral gauche.
    SL,
    /// Latéral droit.
    SR,
    /// Auxiliaire numéroté (sans position spatiale).
    Aux,
}

impl ChannelLabel {
    /// Disposition standard pour `n` canaux (1 → Mono, 2 → FL FR, …), `Aux` au-delà de 8.
    pub fn layout(n: usize) -> impl Iterator<Item = ChannelLabel> {
        use ChannelLabel::*;
        const LAYOUTS: [&[ChannelLabel]; 9] = [
            &[],
            &[Mono],
            &[FL, FR],
            &[FL, FR, FC],
            &[FL, FR, RL, RR],
            &[FL, FR, FC, RL, RR],
            &[FL, FR, FC, LFE, RL, RR],
            &[FL, FR, FC, LFE, RL, RR, SL],
            &[FL, FR, FC, LFE, RL, RR, SL, SR],
        ];
        let base = LAYOUTS[n.min(8)];
        base.iter()
            .copied()
            .chain(core::iter::repeat_n(Aux, n.saturating_sub(8)))
    }

    /// Nom court (`FL`, `FR`, `MONO`, …).
    pub fn short_name(self) -> &'static str {
        match self {
            ChannelLabel::Mono => "MONO",
            ChannelLabel::FL => "FL",
            ChannelLabel::FR => "FR",
            ChannelLabel::FC => "FC",
            ChannelLabel::LFE => "LFE",
            ChannelLabel::RL => "RL",
            ChannelLabel::RR => "RR",
            ChannelLabel::SL => "SL",
            ChannelLabel::SR => "SR",
            ChannelLabel::Aux => "AUX",
        }
    }

    /// Vrai pour un canal « gauche » (FL, RL, SL).
    pub fn is_left(self) -> bool {
        matches!(self, ChannelLabel::FL | ChannelLabel::RL | ChannelLabel::SL)
    }

    /// Vrai pour un canal « droit » (FR, RR, SR).
    pub fn is_right(self) -> bool {
        matches!(self, ChannelLabel::FR | ChannelLabel::RR | ChannelLabel::SR)
    }
}

impl fmt::Display for ChannelLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.short_name())
    }
}

/// Description d'un port mono d'un nœud.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PortSpec {
    /// Nom du port, unique parmi les ports de même direction d'un nœud.
    pub name: String,
    /// Position de canal.
    pub label: ChannelLabel,
}

impl PortSpec {
    /// Port nommé avec étiquette.
    pub fn new(name: impl Into<String>, label: ChannelLabel) -> Self {
        Self {
            name: name.into(),
            label,
        }
    }

    /// Port mono nommé.
    pub fn mono(name: impl Into<String>) -> Self {
        Self::new(name, ChannelLabel::Mono)
    }

    /// Disposition standard de `n` ports : `MONO` ; `FL`, `FR` ; … ; `AUX1`… au-delà.
    pub fn layout(n: usize) -> Vec<PortSpec> {
        let mut aux = 0;
        ChannelLabel::layout(n)
            .map(|l| {
                if l == ChannelLabel::Aux {
                    aux += 1;
                    PortSpec::new(format!("AUX{aux}"), l)
                } else {
                    PortSpec::new(l.short_name(), l)
                }
            })
            .collect()
    }

    /// Disposition stéréo `FL`, `FR`.
    pub fn stereo() -> Vec<PortSpec> {
        Self::layout(2)
    }
}

/// Contexte d'un cycle de traitement, en lecture seule pour les nœuds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessContext {
    /// Nombre de trames à traiter dans ce cycle (≤ `max_frames`).
    pub frames: usize,
    /// Nombre maximal de trames par cycle (quantum), taille des tampons.
    pub max_frames: usize,
    /// Fréquence d'échantillonnage du graphe.
    pub sample_rate: SampleRate,
    /// Position en trames depuis le démarrage du moteur, au début de ce cycle.
    pub position: u64,
    /// Numéro du cycle (monotone).
    pub cycle: u64,
}

/// Accès aux tampons d'entrée et de sortie d'un nœud pendant `process`.
///
/// Tous les accès sont bornés à `frames` trames. Aucune allocation.
#[derive(Debug)]
pub struct NodeIo<'a> {
    inputs: &'a [Box<[f32]>],
    connected: &'a [bool],
    outputs: &'a mut [Box<[f32]>],
    frames: usize,
}

impl<'a> NodeIo<'a> {
    /// Construit une vue. `inputs`, `connected` et `outputs` sont les tranches propres
    /// au nœud dans les réserves de tampons.
    pub fn new(
        inputs: &'a [Box<[f32]>],
        connected: &'a [bool],
        outputs: &'a mut [Box<[f32]>],
        frames: usize,
    ) -> Self {
        debug_assert_eq!(inputs.len(), connected.len());
        Self {
            inputs,
            connected,
            outputs,
            frames,
        }
    }

    /// Nombre de trames du cycle.
    #[inline]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Nombre de ports d'entrée.
    #[inline]
    pub fn num_inputs(&self) -> usize {
        self.inputs.len()
    }

    /// Nombre de ports de sortie.
    #[inline]
    pub fn num_outputs(&self) -> usize {
        self.outputs.len()
    }

    /// Échantillons du port d'entrée `i` (silence si non connecté).
    #[inline]
    pub fn input(&self, i: usize) -> &[f32] {
        &self.inputs[i][..self.frames]
    }

    /// Vrai si au moins un lien alimente le port d'entrée `i`.
    #[inline]
    pub fn is_input_connected(&self, i: usize) -> bool {
        self.connected[i]
    }

    /// Échantillons du port de sortie `i`, à remplir. Contenu initial indéfini
    /// (celui du cycle précédent) : un nœud doit écrire toutes ses sorties.
    #[inline]
    pub fn output(&mut self, i: usize) -> &mut [f32] {
        &mut self.outputs[i][..self.frames]
    }

    /// Accès simultané à une entrée et une sortie (pour les nœuds « en place »).
    #[inline]
    pub fn in_out(&mut self, input: usize, output: usize) -> (&[f32], &mut [f32]) {
        (
            &self.inputs[input][..self.frames],
            &mut self.outputs[output][..self.frames],
        )
    }

    /// Toutes les entrées et toutes les sorties, séparées.
    #[inline]
    pub fn split(&mut self) -> (Inputs<'_>, Outputs<'_>) {
        (
            Inputs {
                bufs: self.inputs,
                connected: self.connected,
                frames: self.frames,
            },
            Outputs {
                bufs: self.outputs,
                frames: self.frames,
            },
        )
    }

    /// Met toutes les sorties au silence.
    pub fn silence_outputs(&mut self) {
        for o in self.outputs.iter_mut() {
            o[..self.frames].fill(0.0);
        }
    }
}

/// Entrées d'un nœud (vue partagée).
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    bufs: &'a [Box<[f32]>],
    connected: &'a [bool],
    frames: usize,
}

impl Inputs<'_> {
    /// Nombre d'entrées.
    pub fn len(&self) -> usize {
        self.bufs.len()
    }
    /// Vrai si aucune entrée.
    pub fn is_empty(&self) -> bool {
        self.bufs.is_empty()
    }
    /// Échantillons de l'entrée `i`.
    #[inline]
    pub fn get(&self, i: usize) -> &[f32] {
        &self.bufs[i][..self.frames]
    }
    /// Vrai si l'entrée `i` est connectée.
    pub fn is_connected(&self, i: usize) -> bool {
        self.connected[i]
    }
    /// Itère sur les entrées.
    pub fn iter(&self) -> impl Iterator<Item = &[f32]> + '_ {
        self.bufs.iter().map(move |b| &b[..self.frames])
    }
}

/// Sorties d'un nœud (vue mutable).
#[derive(Debug)]
pub struct Outputs<'a> {
    bufs: &'a mut [Box<[f32]>],
    frames: usize,
}

impl Outputs<'_> {
    /// Nombre de sorties.
    pub fn len(&self) -> usize {
        self.bufs.len()
    }
    /// Vrai si aucune sortie.
    pub fn is_empty(&self) -> bool {
        self.bufs.is_empty()
    }
    /// Échantillons de la sortie `i`.
    #[inline]
    pub fn get(&mut self, i: usize) -> &mut [f32] {
        &mut self.bufs[i][..self.frames]
    }
    /// Itère mutablement sur les sorties.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut [f32]> + '_ {
        let n = self.frames;
        self.bufs.iter_mut().map(move |b| &mut b[..n])
    }
}

/// Un nœud du graphe : produit et/ou consomme de l'audio.
///
/// # Contrat
///
/// - [`inputs`](Node::inputs) et [`outputs`](Node::outputs) sont appelées hors temps
///   réel à l'insertion dans le graphe et doivent retourner des listes stables.
/// - [`prepare`](Node::prepare) est appelée hors temps réel avant tout `process` ;
///   c'est le moment d'allouer.
/// - [`process`](Node::process) est appelée depuis le fil audio : **aucune allocation,
///   aucun verrou, aucun syscall bloquant, aucun log**. Le nœud doit écrire toutes
///   ses sorties.
/// - L'état interne persiste entre les versions du graphe : ajouter un lien ne
///   réinitialise pas un nœud.
pub trait Node: Send + 'static {
    /// Nom du type de nœud, pour le diagnostic (`"sine"`, `"mixer"`, …).
    fn type_name(&self) -> &'static str;

    /// Ports d'entrée.
    fn inputs(&self) -> Vec<PortSpec>;

    /// Ports de sortie.
    fn outputs(&self) -> Vec<PortSpec>;

    /// Prépare le nœud pour une fréquence et une taille de cycle maximale. Hors temps réel.
    fn prepare(&mut self, sample_rate: SampleRate, max_frames: usize) {
        let _ = (sample_rate, max_frames);
    }

    /// Traite un cycle. Temps réel.
    fn process(&mut self, ctx: &ProcessContext, io: &mut NodeIo<'_>);

    /// Remet l'état interne à zéro (phase, filtres, …). Temps réel.
    fn reset(&mut self) {}
}

impl fmt::Debug for dyn Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Node({})", self.type_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nœud passe-plat de test : copie chaque entrée vers la sortie de même index.
    struct PassThrough(usize);

    impl Node for PassThrough {
        fn type_name(&self) -> &'static str {
            "passthrough"
        }
        fn inputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(self.0)
        }
        fn outputs(&self) -> Vec<PortSpec> {
            PortSpec::layout(self.0)
        }
        fn process(&mut self, _ctx: &ProcessContext, io: &mut NodeIo<'_>) {
            for i in 0..self.0 {
                let (inp, out) = io.in_out(i, i);
                out.copy_from_slice(inp);
            }
        }
    }

    #[test]
    fn passthrough_runs_through_node_io() {
        let mut node = PassThrough(2);
        assert_eq!(node.inputs().len(), 2);
        assert_eq!(node.outputs()[1].label, ChannelLabel::FR);
        let inputs: Vec<Box<[f32]>> = vec![vec![1.0; 8].into(), vec![2.0; 8].into()];
        let connected = [true, false];
        let mut outputs: Vec<Box<[f32]>> = vec![vec![0.0; 8].into(), vec![0.0; 8].into()];
        let ctx = ProcessContext {
            frames: 4,
            max_frames: 8,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        };
        let mut io = NodeIo::new(&inputs, &connected, &mut outputs, 4);
        assert!(io.is_input_connected(0));
        assert!(!io.is_input_connected(1));
        node.process(&ctx, &mut io);
        assert_eq!(&outputs[0][..], &[1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&outputs[1][..4], &[2.0; 4]);
    }

    #[test]
    fn layouts() {
        let l: Vec<_> = PortSpec::layout(1);
        assert_eq!(l[0].name, "MONO");
        let s = PortSpec::stereo();
        assert_eq!((s[0].name.as_str(), s[1].name.as_str()), ("FL", "FR"));
        let big = PortSpec::layout(10);
        assert_eq!(big.len(), 10);
        assert_eq!(big[8].name, "AUX1");
        assert_eq!(big[9].name, "AUX2");
        assert_eq!(big[9].label, ChannelLabel::Aux);
        assert!(ChannelLabel::FL.is_left() && !ChannelLabel::FL.is_right());
        assert!(ChannelLabel::SR.is_right());
        assert_eq!(ChannelLabel::LFE.to_string(), "LFE");
        assert_eq!(PortSpec::layout(0).len(), 0);
    }

    #[test]
    fn split_gives_both_views() {
        let inputs: Vec<Box<[f32]>> = vec![vec![3.0; 4].into()];
        let connected = [true];
        let mut outputs: Vec<Box<[f32]>> = vec![vec![0.0; 4].into(), vec![0.0; 4].into()];
        let mut io = NodeIo::new(&inputs, &connected, &mut outputs, 4);
        let (i, mut o) = io.split();
        assert_eq!(i.len(), 1);
        assert!(!i.is_empty());
        assert!(i.is_connected(0));
        assert_eq!(o.len(), 2);
        assert!(!o.is_empty());
        for out in o.iter_mut() {
            out.copy_from_slice(i.get(0));
        }
        assert_eq!(i.iter().count(), 1);
        io.silence_outputs();
        assert_eq!(&outputs[1][..], &[0.0; 4]);
    }
}
