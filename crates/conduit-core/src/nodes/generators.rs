//! Générateurs : silence, sinus, bruit blanc et rose.

use std::sync::Arc;

use crate::dsp::XorShift32;
use crate::node::{Node, NodeIo, PortSpec, ProcessContext};
use crate::param::{Param, Smoothed};
use crate::types::SampleRate;

/// Nœud produisant du silence sur `channels` sorties.
#[derive(Debug, Clone)]
pub struct SilenceNode {
    channels: usize,
}

impl SilenceNode {
    /// Crée.
    pub fn new(channels: usize) -> Self {
        Self { channels }
    }
}

impl Node for SilenceNode {
    fn type_name(&self) -> &'static str {
        "silence"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        vec![]
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        io.silence_outputs();
    }
}

/// Commandes d'un [`SineNode`].
#[derive(Debug)]
pub struct SineControl {
    /// Fréquence en Hz.
    pub frequency: Param,
    /// Amplitude linéaire (crête).
    pub amplitude: Param,
}

/// Générateur sinusoïdal, même signal sur toutes les sorties.
///
/// Fréquence et amplitude sont modifiables à chaud ; l'amplitude est lissée,
/// la phase est continue lors d'un changement de fréquence.
#[derive(Debug)]
pub struct SineNode {
    channels: usize,
    ctrl: Arc<SineControl>,
    phase: f64,
    sample_rate: f64,
    amp: Smoothed,
}

impl SineNode {
    /// Crée un générateur.
    pub fn new(frequency: f32, amplitude: f32, channels: usize) -> Self {
        Self {
            channels,
            ctrl: Arc::new(SineControl {
                frequency: Param::new(frequency),
                amplitude: Param::new(amplitude),
            }),
            phase: 0.0,
            sample_rate: 48_000.0,
            amp: Smoothed::new(amplitude, Smoothed::DEFAULT_TIME_S, 48_000.0),
        }
    }

    /// Poignée de commande.
    pub fn control(&self) -> Arc<SineControl> {
        Arc::clone(&self.ctrl)
    }
}

impl Node for SineNode {
    fn type_name(&self) -> &'static str {
        "sine"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        vec![]
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn prepare(&mut self, sample_rate: SampleRate, _: usize) {
        self.sample_rate = sample_rate.as_f64();
        self.amp = Smoothed::new(
            self.ctrl.amplitude.get(),
            Smoothed::DEFAULT_TIME_S,
            sample_rate.hz() as f32,
        );
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let step = f64::from(self.ctrl.frequency.get()) / self.sample_rate;
        let target = self.ctrl.amplitude.get();
        let frames = io.frames();
        let (_, mut outs) = io.split();
        if outs.is_empty() {
            self.phase = (self.phase + step * frames as f64).fract();
            return;
        }
        for i in 0..frames {
            let v = (self.phase * core::f64::consts::TAU).sin() as f32 * self.amp.next(target);
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            for o in outs.iter_mut() {
                o[i] = v;
            }
        }
    }
    fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// Couleur de bruit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoiseColor {
    /// Spectre plat.
    White,
    /// −3 dB/octave (énergie égale par octave).
    #[default]
    Pink,
}

/// Commandes d'un [`NoiseNode`].
#[derive(Debug)]
pub struct NoiseControl {
    /// Amplitude linéaire (crête approximative).
    pub amplitude: Param,
}

/// Générateur de bruit, échantillons indépendants par sortie.
///
/// Le bruit rose est obtenu par le filtre de Paul Kellet (précision ≈ ±0,05 dB
/// sur 20 Hz – 20 kHz).
#[derive(Debug)]
pub struct NoiseNode {
    channels: usize,
    color: NoiseColor,
    ctrl: Arc<NoiseControl>,
    rng: XorShift32,
    pink: Vec<[f32; 7]>,
    amp: Smoothed,
}

impl NoiseNode {
    /// Crée un générateur.
    pub fn new(color: NoiseColor, amplitude: f32, channels: usize) -> Self {
        Self {
            channels,
            color,
            ctrl: Arc::new(NoiseControl {
                amplitude: Param::new(amplitude),
            }),
            rng: XorShift32::default(),
            pink: vec![[0.0; 7]; channels],
            amp: Smoothed::new(amplitude, Smoothed::DEFAULT_TIME_S, 48_000.0),
        }
    }

    /// Fixe la graine du générateur (déterminisme en test).
    pub fn with_seed(mut self, seed: u32) -> Self {
        self.rng = XorShift32::new(seed);
        self
    }

    /// Poignée de commande.
    pub fn control(&self) -> Arc<NoiseControl> {
        Arc::clone(&self.ctrl)
    }

    #[inline]
    #[allow(clippy::excessive_precision)] // coefficients publiés de P. Kellet
    fn pink_sample(state: &mut [f32; 7], white: f32) -> f32 {
        state[0] = 0.99886 * state[0] + white * 0.0555179;
        state[1] = 0.99332 * state[1] + white * 0.0750759;
        state[2] = 0.96900 * state[2] + white * 0.1538520;
        state[3] = 0.86650 * state[3] + white * 0.3104856;
        state[4] = 0.55000 * state[4] + white * 0.5329522;
        state[5] = -0.7616 * state[5] - white * 0.0168980;
        let pink = state[0]
            + state[1]
            + state[2]
            + state[3]
            + state[4]
            + state[5]
            + state[6]
            + white * 0.5362;
        state[6] = white * 0.115926;
        pink * 0.11
    }
}

impl Node for NoiseNode {
    fn type_name(&self) -> &'static str {
        match self.color {
            NoiseColor::White => "white-noise",
            NoiseColor::Pink => "pink-noise",
        }
    }
    fn inputs(&self) -> Vec<PortSpec> {
        vec![]
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn prepare(&mut self, sample_rate: SampleRate, _: usize) {
        self.amp = Smoothed::new(
            self.ctrl.amplitude.get(),
            Smoothed::DEFAULT_TIME_S,
            sample_rate.hz() as f32,
        );
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        let target = self.ctrl.amplitude.get();
        let frames = io.frames();
        let (_, mut outs) = io.split();
        for i in 0..frames {
            let a = self.amp.next(target);
            for (ch, o) in outs.iter_mut().enumerate() {
                let white = self.rng.next_f32();
                o[i] = a * match self.color {
                    NoiseColor::White => white,
                    NoiseColor::Pink => Self::pink_sample(&mut self.pink[ch], white),
                };
            }
        }
    }
    fn reset(&mut self) {
        for s in &mut self.pink {
            *s = [0.0; 7];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use crate::graph::GraphBuilder;
    use crate::types::Quantum;

    const SR: f32 = 48_000.0;

    /// Exécute un nœud générateur seul et récupère `frames` trames de sa sortie 0.
    fn render(node: Box<dyn Node>, frames: usize) -> Vec<f32> {
        let q = 256;
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(q).unwrap());
        let id = b.add_node(node, "g");
        let mut ex = Executor::standalone(b.compile());
        let mut out = Vec::with_capacity(frames);
        while out.len() < frames {
            ex.run(q);
            let g = ex.graph().unwrap();
            out.extend_from_slice(g.output_buffer(g.output_index(id, 0).unwrap()));
        }
        out.truncate(frames);
        out
    }

    fn zero_crossings(s: &[f32]) -> usize {
        s.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    /// Puissance dans une bande via DFT naïve (test seulement).
    fn band_power(s: &[f32], lo_hz: f32, hi_hz: f32) -> f64 {
        let n = s.len();
        let df = SR / n as f32;
        let (k0, k1) = ((lo_hz / df) as usize, (hi_hz / df) as usize);
        let mut p = 0.0f64;
        for k in k0..k1 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            let w = -2.0 * core::f64::consts::PI * k as f64 / n as f64;
            for (i, &x) in s.iter().enumerate() {
                let (si, co) = (w * i as f64).sin_cos();
                re += x as f64 * co;
                im += x as f64 * si;
            }
            p += re * re + im * im;
        }
        p
    }

    #[test]
    fn silence_is_silent() {
        let out = render(Box::new(SilenceNode::new(2)), 512);
        assert!(out.iter().all(|&x| x == 0.0));
        assert_eq!(SilenceNode::new(3).outputs().len(), 3);
    }

    #[test]
    fn sine_frequency_and_amplitude() {
        let out = render(Box::new(SineNode::new(1000.0, 0.5, 1)), 48_000);
        // 1 kHz sur 1 s : 2000 passages par zéro (± 2 pour les bords).
        let zc = zero_crossings(&out);
        assert!((1998..=2002).contains(&zc), "passages par zéro : {zc}");
        let peak = out[1000..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 1e-3, "crête {peak}");
        assert!(out[0].abs() < 1e-6, "démarre à phase 0");
    }

    #[test]
    fn sine_phase_is_continuous_across_cycles() {
        let out = render(Box::new(SineNode::new(440.0, 1.0, 2)), 4096);
        let step = 440.0 / SR * core::f32::consts::TAU;
        // Dérivée bornée par ω·A (pas de saut aux frontières de cycle de 256).
        let max_delta = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_delta <= step * 1.01, "delta max {max_delta} > {step}");
    }

    #[test]
    fn sine_control_changes_apply_smoothly() {
        let node = SineNode::new(100.0, 1.0, 1);
        let ctrl = node.control();
        let q = 256;
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(q).unwrap());
        let id = b.add_node(Box::new(node), "s");
        let mut ex = Executor::standalone(b.compile());
        ex.run(q);
        ctrl.amplitude.set(0.0);
        ctrl.frequency.set(2000.0);
        let mut out = Vec::new();
        for _ in 0..8 {
            ex.run(q);
            let g = ex.graph().unwrap();
            out.extend_from_slice(g.output_buffer(g.output_index(id, 0).unwrap()));
        }
        // Le lissage (5 ms ≈ 240 échantillons) rend la décroissance progressive.
        assert!(out[0].abs() > 0.0 || out[1].abs() > 0.0);
        assert!(out[out.len() - 1].abs() < 1e-3);
        let max_delta = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_delta < 0.3, "pas de saut brutal : {max_delta}");
    }

    #[test]
    fn white_noise_is_flat_and_pink_noise_falls_3db_per_octave() {
        let n = 4096;
        let white = render(
            Box::new(NoiseNode::new(NoiseColor::White, 1.0, 1).with_seed(7)),
            n,
        );
        let pink = render(
            Box::new(NoiseNode::new(NoiseColor::Pink, 1.0, 1).with_seed(7)),
            n,
        );
        assert!(white.iter().all(|x| x.abs() <= 1.0));
        assert!(white.iter().any(|x| x.abs() > 0.5));
        // Bandes d'une octave : 500–1000, 1000–2000, 2000–4000, 4000–8000 Hz.
        let bands = [
            (500.0, 1000.0),
            (1000.0, 2000.0),
            (2000.0, 4000.0),
            (4000.0, 8000.0),
        ];
        let wp: Vec<f64> = bands
            .iter()
            .map(|&(l, h)| band_power(&white, l, h))
            .collect();
        let pp: Vec<f64> = bands
            .iter()
            .map(|&(l, h)| band_power(&pink, l, h))
            .collect();
        // Blanc : la puissance double par octave (largeur double). Rose : constante.
        for w in wp.windows(2) {
            let ratio_db = 10.0 * (w[1] / w[0]).log10();
            assert!(
                (ratio_db - 3.0).abs() < 1.5,
                "blanc : {ratio_db} dB par octave"
            );
        }
        for w in pp.windows(2) {
            let ratio_db = 10.0 * (w[1] / w[0]).log10();
            assert!(ratio_db.abs() < 1.5, "rose : {ratio_db} dB par octave");
        }
        assert_eq!(
            NoiseNode::new(NoiseColor::Pink, 1.0, 2).type_name(),
            "pink-noise"
        );
        assert_eq!(
            NoiseNode::new(NoiseColor::White, 1.0, 2).type_name(),
            "white-noise"
        );
    }

    #[test]
    fn noise_channels_are_independent_and_reset_works() {
        let mut node = NoiseNode::new(NoiseColor::Pink, 1.0, 2).with_seed(3);
        node.prepare(SampleRate::HZ_48000, 64);
        let mut outs: Vec<Box<[f32]>> = vec![vec![0.0; 64].into(), vec![0.0; 64].into()];
        let ctx = ProcessContext {
            frames: 64,
            max_frames: 64,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        };
        node.process(&ctx, &mut NodeIo::new(&[], &[], &mut outs, 64));
        assert_ne!(outs[0], outs[1]);
        node.reset();
        assert!(node.pink.iter().all(|s| s.iter().all(|&x| x == 0.0)));
        node.control().amplitude.set(0.0);
        assert_eq!(node.control().amplitude.get(), 0.0);
    }
}
