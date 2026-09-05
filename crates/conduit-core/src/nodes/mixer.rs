//! Mixeur N bus → 1 bus et duplicateur 1 → N.

use std::sync::Arc;

use crate::gain::{GainParam, Ramp};
use crate::node::{Node, NodeIo, PortSpec, ProcessContext};

/// Commandes d'un [`MixerNode`] : un gain par bus d'entrée.
#[derive(Debug)]
pub struct MixerControl {
    /// Gain (et muet) de chaque bus.
    pub bus_gains: Vec<Arc<GainParam>>,
}

/// Mixeur : `buses` bus de `channels` canaux chacun, sommés vers un bus de sortie.
///
/// Les entrées sont ordonnées bus par bus (`1:FL, 1:FR, 2:FL, 2:FR, …`). Chaque bus a
/// un gain avec rampe.
#[derive(Debug)]
pub struct MixerNode {
    buses: usize,
    channels: usize,
    ctrl: Arc<MixerControl>,
    ramps: Vec<Ramp>,
}

impl MixerNode {
    /// Crée un mixeur.
    pub fn new(buses: usize, channels: usize) -> Self {
        let bus_gains = (0..buses).map(|_| Arc::new(GainParam::default())).collect();
        Self {
            buses,
            channels,
            ctrl: Arc::new(MixerControl { bus_gains }),
            ramps: vec![Ramp::new(1.0); buses],
        }
    }

    /// Poignée de commande.
    pub fn control(&self) -> Arc<MixerControl> {
        Arc::clone(&self.ctrl)
    }

    /// Index du port d'entrée `channel` du bus `bus`.
    pub fn input_index(&self, bus: usize, channel: usize) -> usize {
        bus * self.channels + channel
    }
}

impl Node for MixerNode {
    fn type_name(&self) -> &'static str {
        "mixer"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        (0..self.buses)
            .flat_map(|b| {
                PortSpec::layout(self.channels)
                    .into_iter()
                    .map(move |p| PortSpec::new(format!("{}:{}", b + 1, p.name), p.label))
            })
            .collect()
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let frames = io.frames();
        let (ins, mut outs) = io.split();
        for (bus, ramp) in self.ramps.iter_mut().enumerate() {
            let target = self.ctrl.bus_gains[bus].target();
            let plan = ramp.step(target, frames);
            for ch in 0..self.channels {
                let src = ins.get(bus * self.channels + ch);
                let dst = outs.get(ch);
                if bus == 0 {
                    plan.copy(src, dst);
                } else {
                    plan.add(src, dst);
                }
            }
            self.ctrl.bus_gains[bus].store_current(target);
        }
        if self.buses == 0 {
            for o in outs.iter_mut() {
                o.fill(0.0);
            }
        }
    }
}

/// Duplicateur : un bus de `channels` canaux copié vers `copies` bus de sortie.
#[derive(Debug, Clone)]
pub struct SplitterNode {
    channels: usize,
    copies: usize,
}

impl SplitterNode {
    /// Crée un duplicateur.
    pub fn new(channels: usize, copies: usize) -> Self {
        Self { channels, copies }
    }

    /// Index du port de sortie `channel` de la copie `copy`.
    pub fn output_index(&self, copy: usize, channel: usize) -> usize {
        copy * self.channels + channel
    }
}

impl Node for SplitterNode {
    fn type_name(&self) -> &'static str {
        "splitter"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn outputs(&self) -> Vec<PortSpec> {
        (0..self.copies)
            .flat_map(|c| {
                PortSpec::layout(self.channels)
                    .into_iter()
                    .map(move |p| PortSpec::new(format!("{}:{}", c + 1, p.name), p.label))
            })
            .collect()
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let (ins, mut outs) = io.split();
        for copy in 0..self.copies {
            for ch in 0..self.channels {
                outs.get(copy * self.channels + ch)
                    .copy_from_slice(ins.get(ch));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Db, SampleRate};

    fn ctx(frames: usize) -> ProcessContext {
        ProcessContext {
            frames,
            max_frames: frames,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        }
    }

    #[test]
    fn mixer_sums_buses_channel_by_channel() {
        let mut m = MixerNode::new(2, 2);
        assert_eq!(m.inputs().len(), 4);
        assert_eq!(m.inputs()[2].name, "2:FL");
        assert_eq!(m.outputs().len(), 2);
        assert_eq!(m.input_index(1, 1), 3);
        let ins: Vec<Box<[f32]>> = vec![
            vec![1.0; 4].into(),
            vec![2.0; 4].into(),
            vec![10.0; 4].into(),
            vec![20.0; 4].into(),
        ];
        let conn = [true; 4];
        let mut outs: Vec<Box<[f32]>> = vec![vec![0.0; 4].into(), vec![0.0; 4].into()];
        m.process(&ctx(4), &mut NodeIo::new(&ins, &conn, &mut outs, 4));
        assert_eq!(&outs[0][..], &[11.0; 4]);
        assert_eq!(&outs[1][..], &[22.0; 4]);
        // Gain de bus avec rampe : bus 2 à −∞ → sur un cycle la sortie descend vers 1.0.
        m.control().bus_gains[1].set_db(Db::NEG_INF);
        m.process(&ctx(4), &mut NodeIo::new(&ins, &conn, &mut outs, 4));
        assert_eq!(&outs[0][..], &[8.5, 6.0, 3.5, 1.0]);
        m.process(&ctx(4), &mut NodeIo::new(&ins, &conn, &mut outs, 4));
        assert_eq!(&outs[0][..], &[1.0; 4]);
    }

    #[test]
    fn empty_mixer_outputs_silence() {
        let mut m = MixerNode::new(0, 1);
        let mut outs: Vec<Box<[f32]>> = vec![vec![5.0; 4].into()];
        m.process(&ctx(4), &mut NodeIo::new(&[], &[], &mut outs, 4));
        assert_eq!(&outs[0][..], &[0.0; 4]);
    }

    #[test]
    fn splitter_duplicates_channel_by_channel() {
        let mut s = SplitterNode::new(2, 3);
        assert_eq!(s.outputs().len(), 6);
        assert_eq!(s.outputs()[5].name, "3:FR");
        assert_eq!(s.output_index(2, 1), 5);
        let ins: Vec<Box<[f32]>> = vec![vec![1.0; 4].into(), vec![2.0; 4].into()];
        let conn = [true; 2];
        let mut outs: Vec<Box<[f32]>> = (0..6).map(|_| vec![0.0; 4].into()).collect();
        s.process(&ctx(4), &mut NodeIo::new(&ins, &conn, &mut outs, 4));
        for c in 0..3 {
            assert_eq!(&outs[2 * c][..], &[1.0; 4]);
            assert_eq!(&outs[2 * c + 1][..], &[2.0; 4]);
        }
    }
}
