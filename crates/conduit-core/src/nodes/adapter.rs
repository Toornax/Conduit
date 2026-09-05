//! Adaptation du nombre de canaux (F-15) : mono → stéréo, stéréo → mono, table libre.

use crate::node::{ChannelLabel, Node, NodeIo, PortSpec, ProcessContext};

/// Table de mixage entrée → sortie : `weights[out][in]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMap {
    inputs: usize,
    outputs: usize,
    weights: Vec<f32>,
}

impl ChannelMap {
    /// Table nulle (`inputs` × `outputs`).
    pub fn zeros(inputs: usize, outputs: usize) -> Self {
        Self {
            inputs,
            outputs,
            weights: vec![0.0; inputs * outputs],
        }
    }

    /// Identité tronquée : entrée `i` → sortie `i` tant que les deux existent.
    pub fn identity(inputs: usize, outputs: usize) -> Self {
        let mut m = Self::zeros(inputs, outputs);
        for i in 0..inputs.min(outputs) {
            m.set(i, i, 1.0);
        }
        m
    }

    /// Duplication mono → `outputs` canaux (gain 1 partout).
    pub fn mono_to(outputs: usize) -> Self {
        let mut m = Self::zeros(1, outputs);
        for o in 0..outputs {
            m.set(0, o, 1.0);
        }
        m
    }

    /// Somme pondérée `inputs` canaux → mono (chaque entrée à `1/inputs`, pas
    /// d'écrêtage pour des signaux corrélés).
    pub fn to_mono(inputs: usize) -> Self {
        let mut m = Self::zeros(inputs, 1);
        if inputs > 0 {
            let w = 1.0 / inputs as f32;
            for i in 0..inputs {
                m.set(i, 0, w);
            }
        }
        m
    }

    /// Table automatique entre deux dispositions standard :
    /// - même nombre : identité ;
    /// - 1 → N : duplication ;
    /// - N → 1 : somme pondérée ;
    /// - sinon : appariement par étiquette (FL→FL, …), les canaux sans homologue
    ///   sont ignorés ; le centre est envoyé à −3 dB sur FL et FR quand la sortie n'a
    ///   pas de centre.
    pub fn auto(inputs: usize, outputs: usize) -> Self {
        if inputs == outputs {
            return Self::identity(inputs, outputs);
        }
        if inputs == 1 {
            return Self::mono_to(outputs);
        }
        if outputs == 1 {
            return Self::to_mono(inputs);
        }
        let ins: Vec<ChannelLabel> = ChannelLabel::layout(inputs).collect();
        let outs: Vec<ChannelLabel> = ChannelLabel::layout(outputs).collect();
        let mut m = Self::zeros(inputs, outputs);
        for (i, li) in ins.iter().enumerate() {
            if let Some(o) = outs.iter().position(|lo| lo == li) {
                m.set(i, o, 1.0);
            } else if *li == ChannelLabel::FC {
                for (o, lo) in outs.iter().enumerate() {
                    if matches!(lo, ChannelLabel::FL | ChannelLabel::FR) {
                        m.set(i, o, core::f32::consts::FRAC_1_SQRT_2);
                    }
                }
            } else if li.is_left() {
                if let Some(o) = outs.iter().position(|lo| *lo == ChannelLabel::FL) {
                    m.add(i, o, 1.0);
                }
            } else if li.is_right() {
                if let Some(o) = outs.iter().position(|lo| *lo == ChannelLabel::FR) {
                    m.add(i, o, 1.0);
                }
            }
        }
        m
    }

    /// Nombre d'entrées.
    pub fn inputs(&self) -> usize {
        self.inputs
    }

    /// Nombre de sorties.
    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// Poids entrée `i` → sortie `o`.
    pub fn get(&self, input: usize, output: usize) -> f32 {
        self.weights[output * self.inputs + input]
    }

    /// Fixe un poids.
    pub fn set(&mut self, input: usize, output: usize, weight: f32) {
        self.weights[output * self.inputs + input] = weight;
    }

    /// Ajoute à un poids.
    pub fn add(&mut self, input: usize, output: usize, weight: f32) {
        self.weights[output * self.inputs + input] += weight;
    }

    /// Poids d'une sortie, dans l'ordre des entrées.
    pub fn row(&self, output: usize) -> &[f32] {
        &self.weights[output * self.inputs..(output + 1) * self.inputs]
    }
}

/// Nœud appliquant une [`ChannelMap`].
#[derive(Debug, Clone)]
pub struct ChannelAdapter {
    map: ChannelMap,
}

impl ChannelAdapter {
    /// Crée avec une table.
    pub fn new(map: ChannelMap) -> Self {
        Self { map }
    }

    /// Crée avec la table automatique.
    pub fn auto(inputs: usize, outputs: usize) -> Self {
        Self::new(ChannelMap::auto(inputs, outputs))
    }

    /// Table utilisée.
    pub fn map(&self) -> &ChannelMap {
        &self.map
    }
}

impl Node for ChannelAdapter {
    fn type_name(&self) -> &'static str {
        "channel-adapter"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.map.inputs)
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.map.outputs)
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let (ins, mut outs) = io.split();
        for o in 0..self.map.outputs {
            let row = self.map.row(o);
            let out = outs.get(o);
            out.fill(0.0);
            for (i, &w) in row.iter().enumerate() {
                if w == 0.0 {
                    continue;
                }
                for (d, s) in out.iter_mut().zip(ins.get(i)) {
                    *d += s * w;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SampleRate;

    fn run(map: ChannelMap, inputs: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let n = inputs.first().map_or(4, Vec::len);
        let mut node = ChannelAdapter::new(map.clone());
        let ins: Vec<Box<[f32]>> = inputs
            .iter()
            .map(|v| v.clone().into_boxed_slice())
            .collect();
        let conn = vec![true; ins.len()];
        let mut outs: Vec<Box<[f32]>> = (0..map.outputs()).map(|_| vec![9.0; n].into()).collect();
        let ctx = ProcessContext {
            frames: n,
            max_frames: n,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        };
        node.process(&ctx, &mut NodeIo::new(&ins, &conn, &mut outs, n));
        assert_eq!(node.inputs().len(), map.inputs());
        assert_eq!(node.map(), &map);
        outs.into_iter().map(|b| b.to_vec()).collect()
    }

    #[test]
    fn mono_to_stereo_duplicates() {
        let out = run(ChannelMap::auto(1, 2), &[vec![0.5; 4]]);
        assert_eq!(out, vec![vec![0.5; 4], vec![0.5; 4]]);
    }

    #[test]
    fn stereo_to_mono_is_weighted_sum() {
        let out = run(ChannelMap::auto(2, 1), &[vec![1.0; 4], vec![0.0; 4]]);
        assert_eq!(out, vec![vec![0.5; 4]]);
        let out = run(ChannelMap::auto(2, 1), &[vec![1.0; 4], vec![1.0; 4]]);
        assert_eq!(out, vec![vec![1.0; 4]], "signal corrélé : pas d'écrêtage");
    }

    #[test]
    fn explicit_map_swaps_channels() {
        let mut m = ChannelMap::zeros(2, 2);
        m.set(0, 1, 1.0);
        m.set(1, 0, 0.5);
        assert_eq!(m.get(1, 0), 0.5);
        assert_eq!(m.row(1), &[1.0, 0.0]);
        let out = run(m, &[vec![1.0; 2], vec![2.0; 2]]);
        assert_eq!(out, vec![vec![1.0; 2], vec![1.0; 2]]);
    }

    #[test]
    fn auto_maps_by_label_for_surround() {
        // 5.1 → stéréo : FL→FL, FR→FR, FC à −3 dB des deux côtés, LFE ignoré, RL→FL, RR→FR.
        let m = ChannelMap::auto(6, 2);
        assert_eq!(m.get(0, 0), 1.0);
        assert_eq!(m.get(1, 1), 1.0);
        assert!((m.get(2, 0) - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((m.get(2, 1) - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(m.get(3, 0), 0.0);
        assert_eq!(m.get(4, 0), 1.0);
        assert_eq!(m.get(5, 1), 1.0);
        assert_eq!(m.get(4, 1), 0.0);
        // Stéréo → 5.1 : FL, FR seulement.
        let up = ChannelMap::auto(2, 6);
        assert_eq!(up.get(0, 0), 1.0);
        assert_eq!(up.get(1, 1), 1.0);
        assert_eq!(up.row(2), &[0.0, 0.0]);
        assert_eq!(ChannelMap::auto(3, 3), ChannelMap::identity(3, 3));
        assert_eq!(ChannelAdapter::auto(1, 2).outputs().len(), 2);
        assert_eq!(ChannelMap::to_mono(0).outputs(), 1);
    }
}
