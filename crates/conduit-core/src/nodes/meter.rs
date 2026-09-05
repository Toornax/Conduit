//! VU-mètre : crête et RMS par canal, exportés par atomiques.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::node::{Node, NodeIo, PortSpec, ProcessContext};
use crate::types::SampleRate;

/// Mesure d'un canal.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterReading {
    /// Crête avec décroissance (linéaire, ≥ 0).
    pub peak: f32,
    /// RMS lissé (linéaire, ≥ 0).
    pub rms: f32,
    /// Crête maximale depuis la dernière remise à zéro.
    pub peak_hold: f32,
}

impl MeterReading {
    /// Crête en dBFS.
    pub fn peak_dbfs(&self) -> f32 {
        crate::dsp::to_dbfs(self.peak)
    }
    /// RMS en dBFS.
    pub fn rms_dbfs(&self) -> f32 {
        crate::dsp::to_dbfs(self.rms)
    }
}

#[derive(Debug, Default)]
struct ChannelAtomics {
    peak: AtomicU32,
    rms: AtomicU32,
    peak_hold: AtomicU32,
}

/// Valeurs publiées par un [`MeterNode`], lisibles hors temps réel.
#[derive(Debug)]
pub struct MeterShared {
    channels: Vec<ChannelAtomics>,
    /// Nombre de cycles traités (pour détecter l'activité).
    cycles: AtomicU64,
    /// Nombre d'échantillons ayant dépassé ±1,0.
    clips: AtomicU64,
}

impl MeterShared {
    fn new(channels: usize) -> Self {
        Self {
            channels: (0..channels).map(|_| ChannelAtomics::default()).collect(),
            cycles: AtomicU64::new(0),
            clips: AtomicU64::new(0),
        }
    }

    /// Nombre de canaux.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Mesure d'un canal.
    pub fn read(&self, ch: usize) -> MeterReading {
        let c = &self.channels[ch];
        MeterReading {
            peak: f32::from_bits(c.peak.load(Ordering::Relaxed)),
            rms: f32::from_bits(c.rms.load(Ordering::Relaxed)),
            peak_hold: f32::from_bits(c.peak_hold.load(Ordering::Relaxed)),
        }
    }

    /// Mesures de tous les canaux.
    pub fn read_all(&self) -> Vec<MeterReading> {
        (0..self.channels.len()).map(|c| self.read(c)).collect()
    }

    /// Remet à zéro les crêtes maintenues et le compteur d'écrêtage.
    pub fn reset_hold(&self) {
        for c in &self.channels {
            c.peak_hold.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
        self.clips.store(0, Ordering::Relaxed);
    }

    /// Cycles traités.
    pub fn cycles(&self) -> u64 {
        self.cycles.load(Ordering::Relaxed)
    }

    /// Échantillons écrêtés (|x| > 1) depuis la dernière remise à zéro.
    pub fn clips(&self) -> u64 {
        self.clips.load(Ordering::Relaxed)
    }
}

/// Nœud de mesure passe-plat : `channels` entrées copiées vers `channels` sorties,
/// avec crête (décroissance configurable) et RMS (fenêtre exponentielle).
#[derive(Debug)]
pub struct MeterNode {
    channels: usize,
    shared: Arc<MeterShared>,
    /// Temps de décroissance de la crête, en dB par seconde.
    peak_decay_db_per_s: f32,
    /// Constante de temps du RMS, en secondes.
    rms_window_s: f32,
    peak_coeff: f32,
    rms_coeff: f32,
    peaks: Vec<f32>,
    ms: Vec<f32>,
}

impl MeterNode {
    /// Décroissance de crête par défaut (20 dB/s).
    pub const DEFAULT_PEAK_DECAY_DB_PER_S: f32 = 20.0;
    /// Fenêtre RMS par défaut (300 ms).
    pub const DEFAULT_RMS_WINDOW_S: f32 = 0.3;

    /// Crée un VU-mètre.
    pub fn new(channels: usize) -> Self {
        let mut m = Self {
            channels,
            shared: Arc::new(MeterShared::new(channels)),
            peak_decay_db_per_s: Self::DEFAULT_PEAK_DECAY_DB_PER_S,
            rms_window_s: Self::DEFAULT_RMS_WINDOW_S,
            peak_coeff: 0.0,
            rms_coeff: 0.0,
            peaks: vec![0.0; channels],
            ms: vec![0.0; channels],
        };
        m.recompute(48_000.0, 256);
        m
    }

    /// Règle la décroissance de crête (dB/s) et la fenêtre RMS (s).
    pub fn with_ballistics(mut self, peak_decay_db_per_s: f32, rms_window_s: f32) -> Self {
        self.peak_decay_db_per_s = peak_decay_db_per_s.max(0.0);
        self.rms_window_s = rms_window_s.max(0.0);
        self.recompute(48_000.0, 256);
        self
    }

    /// Valeurs publiées.
    pub fn shared(&self) -> Arc<MeterShared> {
        Arc::clone(&self.shared)
    }

    fn recompute(&mut self, sample_rate: f32, block: usize) {
        // Décroissance appliquée par bloc : facteur par échantillon élevé à la puissance du bloc.
        let per_sample = 10f32.powf(-self.peak_decay_db_per_s / 20.0 / sample_rate);
        self.peak_coeff = per_sample.powi(block as i32);
        self.rms_coeff = if self.rms_window_s <= 0.0 {
            0.0
        } else {
            (-(block as f32) / (self.rms_window_s * sample_rate)).exp()
        };
    }
}

impl Node for MeterNode {
    fn type_name(&self) -> &'static str {
        "meter"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn prepare(&mut self, sample_rate: SampleRate, max_frames: usize) {
        self.recompute(sample_rate.hz() as f32, max_frames);
    }
    fn process(&mut self, ctx: &ProcessContext, io: &mut NodeIo<'_>) {
        let frames = io.frames();
        if frames == 0 {
            return;
        }
        // Les coefficients sont calibrés pour `max_frames` ; on corrige pour les cycles courts.
        let scale = frames as f32 / ctx.max_frames as f32;
        let peak_coeff = if scale == 1.0 {
            self.peak_coeff
        } else {
            self.peak_coeff.powf(scale)
        };
        let rms_coeff = if scale == 1.0 {
            self.rms_coeff
        } else {
            self.rms_coeff.powf(scale)
        };
        let mut clips = 0u64;
        for ch in 0..self.channels {
            let (inp, out) = io.in_out(ch, ch);
            out.copy_from_slice(inp);
            let mut block_peak = 0.0f32;
            let mut sum_sq = 0.0f32;
            for &x in inp {
                let a = x.abs();
                if a > block_peak {
                    block_peak = a;
                }
                sum_sq += x * x;
                if a > 1.0 {
                    clips += 1;
                }
            }
            let decayed = self.peaks[ch] * peak_coeff;
            self.peaks[ch] = if block_peak > decayed {
                block_peak
            } else {
                decayed
            };
            let block_ms = sum_sq / frames as f32;
            self.ms[ch] = block_ms + (self.ms[ch] - block_ms) * rms_coeff;
            let c = &self.shared.channels[ch];
            c.peak.store(self.peaks[ch].to_bits(), Ordering::Relaxed);
            c.rms.store(self.ms[ch].sqrt().to_bits(), Ordering::Relaxed);
            let hold = f32::from_bits(c.peak_hold.load(Ordering::Relaxed));
            if block_peak > hold {
                c.peak_hold.store(block_peak.to_bits(), Ordering::Relaxed);
            }
        }
        if clips > 0 {
            self.shared.clips.fetch_add(clips, Ordering::Relaxed);
        }
        self.shared.cycles.fetch_add(1, Ordering::Relaxed);
    }
    fn reset(&mut self) {
        self.peaks.iter_mut().for_each(|p| *p = 0.0);
        self.ms.iter_mut().for_each(|m| *m = 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use crate::graph::GraphBuilder;
    use crate::nodes::SineNode;
    use crate::types::Quantum;

    #[test]
    fn sine_of_known_amplitude_gives_expected_peak_and_rms() {
        let amp = 0.5;
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(256).unwrap());
        let s = b.add_node(Box::new(SineNode::new(1000.0, amp, 2)), "s");
        let meter = MeterNode::new(2);
        let shared = meter.shared();
        let m = b.add_node(Box::new(meter), "m");
        for ch in 0..2 {
            b.add_link(b.node(s).unwrap().output(ch), b.node(m).unwrap().input(ch))
                .unwrap();
        }
        let mut ex = Executor::standalone(b.compile());
        // 1 s : le RMS (fenêtre 300 ms) et la rampe d'entrée des liens sont établis.
        for _ in 0..(48_000 / 256) {
            ex.run(256);
        }
        for ch in 0..2 {
            let r = shared.read(ch);
            assert!((r.peak - amp).abs() < 0.01, "crête {}", r.peak);
            assert!((r.rms - amp / 2f32.sqrt()).abs() < 0.01, "rms {}", r.rms);
            assert!((r.peak_hold - amp).abs() < 0.01);
            assert!((r.peak_dbfs() + 6.02).abs() < 0.2);
            assert!((r.rms_dbfs() + 9.03).abs() < 0.2);
        }
        assert_eq!(shared.clips(), 0);
        assert_eq!(shared.cycles(), 48_000 / 256);
        assert_eq!(shared.channels(), 2);
        assert_eq!(shared.read_all().len(), 2);
        // Sortie = entrée (passe-plat).
        let g = ex.graph().unwrap();
        let out = g.output_buffer(g.output_index(m, 0).unwrap());
        let inp = g.output_buffer(g.output_index(s, 0).unwrap());
        assert_eq!(out, inp);
    }

    #[test]
    fn peak_decays_and_hold_stays_until_reset() {
        let mut meter = MeterNode::new(1).with_ballistics(60.0, 0.1);
        meter.prepare(SampleRate::HZ_48000, 480);
        let shared = meter.shared();
        let ctx = ProcessContext {
            frames: 480,
            max_frames: 480,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        };
        let loud: Vec<Box<[f32]>> = vec![vec![1.5; 480].into()];
        let quiet: Vec<Box<[f32]>> = vec![vec![0.0; 480].into()];
        let mut outs: Vec<Box<[f32]>> = vec![vec![0.0; 480].into()];
        meter.process(&ctx, &mut NodeIo::new(&loud, &[true], &mut outs, 480));
        assert_eq!(shared.read(0).peak, 1.5);
        assert_eq!(shared.clips(), 480);
        // 100 blocs de 10 ms = 1 s à −60 dB/s → crête ≈ 1,5 × 0,001.
        for _ in 0..100 {
            meter.process(&ctx, &mut NodeIo::new(&quiet, &[true], &mut outs, 480));
        }
        let r = shared.read(0);
        assert!((r.peak - 0.0015).abs() < 0.0002, "crête {}", r.peak);
        // RMS : fenêtre 100 ms, 1 s de silence → la moyenne quadratique a chuté de e^-10.
        assert!(r.rms > 1e-3 && r.rms < 5e-3, "rms {}", r.rms);
        assert_eq!(r.peak_hold, 1.5);
        shared.reset_hold();
        assert_eq!(shared.read(0).peak_hold, 0.0);
        assert_eq!(shared.clips(), 0);
        meter.reset();
        meter.process(&ctx, &mut NodeIo::new(&quiet, &[true], &mut outs, 480));
        assert_eq!(shared.read(0).peak, 0.0);
        assert_eq!(MeterReading::default().peak_dbfs(), f32::NEG_INFINITY);
    }

    #[test]
    fn short_cycles_decay_proportionally() {
        let mut meter = MeterNode::new(1).with_ballistics(20.0, 0.3);
        meter.prepare(SampleRate::HZ_48000, 512);
        let shared = meter.shared();
        let full = ProcessContext {
            frames: 512,
            max_frames: 512,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        };
        let half = ProcessContext {
            frames: 256,
            ..full
        };
        let loud: Vec<Box<[f32]>> = vec![vec![1.0; 512].into()];
        let quiet: Vec<Box<[f32]>> = vec![vec![0.0; 512].into()];
        let mut outs: Vec<Box<[f32]>> = vec![vec![0.0; 512].into()];
        meter.process(&full, &mut NodeIo::new(&loud, &[true], &mut outs, 512));
        meter.process(&half, &mut NodeIo::new(&quiet, &[true], &mut outs, 256));
        meter.process(&half, &mut NodeIo::new(&quiet, &[true], &mut outs, 256));
        let two_halves = shared.read(0).peak;
        meter.reset();
        meter.process(&full, &mut NodeIo::new(&loud, &[true], &mut outs, 512));
        meter.process(&full, &mut NodeIo::new(&quiet, &[true], &mut outs, 512));
        let one_full = shared.read(0).peak;
        assert!((two_halves - one_full).abs() < 1e-5);
        let empty = ProcessContext { frames: 0, ..full };
        meter.process(&empty, &mut NodeIo::new(&quiet, &[true], &mut outs, 0));
    }
}
