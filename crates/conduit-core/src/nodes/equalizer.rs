//! Égaliseur paramétrique multi-bandes, paramètres réglables à chaud.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use crate::dsp::{Biquad, BiquadCoeffs, FilterKind};
use crate::node::{Node, NodeIo, PortSpec, ProcessContext};
use crate::param::{Flag, Param};
use crate::types::SampleRate;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Réglage d'une bande.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EqBand {
    /// Type de filtre.
    pub kind: FilterKind,
    /// Fréquence centrale ou de coupure, en Hz.
    pub frequency: f32,
    /// Facteur de qualité.
    pub q: f32,
    /// Gain en dB (cloche et plateaux).
    pub gain_db: f32,
    /// Bande active.
    pub enabled: bool,
}

impl EqBand {
    /// Cloche.
    pub fn peaking(frequency: f32, q: f32, gain_db: f32) -> Self {
        Self {
            kind: FilterKind::Peaking,
            frequency,
            q,
            gain_db,
            enabled: true,
        }
    }
    /// Plateau grave.
    pub fn low_shelf(frequency: f32, gain_db: f32) -> Self {
        Self {
            kind: FilterKind::LowShelf,
            frequency,
            q: 0.707,
            gain_db,
            enabled: true,
        }
    }
    /// Plateau aigu.
    pub fn high_shelf(frequency: f32, gain_db: f32) -> Self {
        Self {
            kind: FilterKind::HighShelf,
            frequency,
            q: 0.707,
            gain_db,
            enabled: true,
        }
    }
    /// Passe-bas.
    pub fn low_pass(frequency: f32, q: f32) -> Self {
        Self {
            kind: FilterKind::LowPass,
            frequency,
            q,
            gain_db: 0.0,
            enabled: true,
        }
    }
    /// Passe-haut.
    pub fn high_pass(frequency: f32, q: f32) -> Self {
        Self {
            kind: FilterKind::HighPass,
            frequency,
            q,
            gain_db: 0.0,
            enabled: true,
        }
    }
    /// Bande désactivée.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

impl Default for EqBand {
    fn default() -> Self {
        Self::peaking(1000.0, 1.0, 0.0)
    }
}

/// Commande d'une bande : chaque champ est modifiable à chaud. Après modification,
/// appeler [`commit`](Self::commit) pour que le fil audio recalcule le filtre.
#[derive(Debug)]
pub struct EqBandControl {
    kind: AtomicU8,
    /// Fréquence (Hz).
    pub frequency: Param,
    /// Facteur Q.
    pub q: Param,
    /// Gain (dB).
    pub gain_db: Param,
    /// Bande active.
    pub enabled: Flag,
    version: AtomicU32,
}

impl EqBandControl {
    fn new(band: EqBand) -> Self {
        Self {
            kind: AtomicU8::new(band.kind.index()),
            frequency: Param::new(band.frequency),
            q: Param::new(band.q),
            gain_db: Param::new(band.gain_db),
            enabled: Flag::new(band.enabled),
            version: AtomicU32::new(1),
        }
    }

    /// Type de filtre.
    pub fn kind(&self) -> FilterKind {
        FilterKind::from_index(self.kind.load(Ordering::Relaxed))
    }

    /// Change le type de filtre.
    pub fn set_kind(&self, kind: FilterKind) {
        self.kind.store(kind.index(), Ordering::Relaxed);
    }

    /// Applique un réglage complet et le valide.
    pub fn set(&self, band: EqBand) {
        self.set_kind(band.kind);
        self.frequency.set(band.frequency);
        self.q.set(band.q);
        self.gain_db.set(band.gain_db);
        self.enabled.set(band.enabled);
        self.commit();
    }

    /// Réglage courant.
    pub fn get(&self) -> EqBand {
        EqBand {
            kind: self.kind(),
            frequency: self.frequency.get(),
            q: self.q.get(),
            gain_db: self.gain_db.get(),
            enabled: self.enabled.get(),
        }
    }

    /// Signale au fil audio que les paramètres ont changé.
    pub fn commit(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }

    fn version(&self) -> u32 {
        self.version.load(Ordering::Acquire)
    }
}

/// Égaliseur : `bands` biquads en série, appliqués identiquement à `channels` canaux.
#[derive(Debug)]
pub struct EqualizerNode {
    channels: usize,
    controls: Vec<Arc<EqBandControl>>,
    coeffs: Vec<BiquadCoeffs>,
    seen: Vec<u32>,
    /// `states[band * channels + channel]`.
    states: Vec<Biquad>,
    sample_rate: f32,
}

impl EqualizerNode {
    /// Crée un égaliseur avec des bandes initiales.
    pub fn new(channels: usize, bands: &[EqBand]) -> Self {
        Self {
            channels,
            controls: bands
                .iter()
                .map(|&b| Arc::new(EqBandControl::new(b)))
                .collect(),
            coeffs: vec![BiquadCoeffs::IDENTITY; bands.len()],
            seen: vec![0; bands.len()],
            states: vec![Biquad::new(); bands.len() * channels],
            sample_rate: 48_000.0,
        }
    }

    /// Commandes des bandes, dans l'ordre.
    pub fn controls(&self) -> Vec<Arc<EqBandControl>> {
        self.controls.clone()
    }

    /// Commande d'une bande.
    pub fn band(&self, i: usize) -> Arc<EqBandControl> {
        Arc::clone(&self.controls[i])
    }

    /// Nombre de bandes.
    pub fn bands(&self) -> usize {
        self.controls.len()
    }

    /// Réponse en amplitude totale à une fréquence (dB), d'après les coefficients
    /// courants. Hors temps réel (calculé depuis les commandes).
    pub fn response_db(&self, frequency: f32) -> f32 {
        self.controls
            .iter()
            .filter(|c| c.enabled.get())
            .map(|c| {
                let b = c.get();
                BiquadCoeffs::compute(b.kind, self.sample_rate, b.frequency, b.q, b.gain_db)
                    .magnitude_db(self.sample_rate, frequency)
            })
            .sum()
    }

    /// Recalcule les coefficients des bandes modifiées. Temps réel : oui.
    fn refresh(&mut self) {
        for (i, c) in self.controls.iter().enumerate() {
            let v = c.version();
            if v != self.seen[i] {
                self.seen[i] = v;
                let b = c.get();
                self.coeffs[i] = if b.enabled {
                    BiquadCoeffs::compute(b.kind, self.sample_rate, b.frequency, b.q, b.gain_db)
                } else {
                    BiquadCoeffs::IDENTITY
                };
            }
        }
    }
}

impl Node for EqualizerNode {
    fn type_name(&self) -> &'static str {
        "equalizer"
    }
    fn inputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn outputs(&self) -> Vec<PortSpec> {
        PortSpec::layout(self.channels)
    }
    fn prepare(&mut self, sample_rate: SampleRate, _: usize) {
        self.sample_rate = sample_rate.hz() as f32;
        self.seen.iter_mut().for_each(|s| *s = 0);
        self.refresh();
    }
    fn process(&mut self, _: &ProcessContext, io: &mut NodeIo<'_>) {
        self.refresh();
        for ch in 0..self.channels {
            let (inp, out) = io.in_out(ch, ch);
            out.copy_from_slice(inp);
            for (band, c) in self.coeffs.iter().enumerate() {
                if *c == BiquadCoeffs::IDENTITY {
                    continue;
                }
                self.states[band * self.channels + ch].process(c, out);
            }
        }
    }
    fn reset(&mut self) {
        self.states.iter_mut().for_each(Biquad::reset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * core::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect()
    }

    fn steady_peak(buf: &[f32]) -> f32 {
        buf[buf.len() / 2..]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn run(node: &mut EqualizerNode, freq: f32, n: usize) -> Vec<f32> {
        let ins: Vec<Box<[f32]>> = (0..node.channels).map(|_| sine(freq, n).into()).collect();
        let conn = vec![true; node.channels];
        let mut outs: Vec<Box<[f32]>> = (0..node.channels).map(|_| vec![0.0; n].into()).collect();
        let ctx = ProcessContext {
            frames: n,
            max_frames: n,
            sample_rate: SampleRate::HZ_48000,
            position: 0,
            cycle: 0,
        };
        node.process(&ctx, &mut NodeIo::new(&ins, &conn, &mut outs, n));
        outs[node.channels - 1].to_vec()
    }

    #[test]
    fn empty_eq_is_transparent() {
        let mut eq = EqualizerNode::new(2, &[]);
        eq.prepare(SampleRate::HZ_48000, 256);
        let out = run(&mut eq, 1000.0, 256);
        assert_eq!(out, sine(1000.0, 256));
        assert_eq!(eq.bands(), 0);
        assert_eq!(eq.response_db(1000.0), 0.0);
    }

    #[test]
    fn peaking_band_boosts_and_low_pass_cuts() {
        let mut eq = EqualizerNode::new(
            2,
            &[
                EqBand::peaking(1000.0, 1.0, 6.0),
                EqBand::low_pass(4000.0, 0.707),
            ],
        );
        eq.prepare(SampleRate::HZ_48000, 9600);
        let at_1k = steady_peak(&run(&mut eq, 1000.0, 9600));
        assert!((at_1k - 1.9).abs() < 0.1, "1 kHz : {at_1k}");
        eq.reset();
        let at_16k = steady_peak(&run(&mut eq, 16_000.0, 9600));
        assert!(at_16k < 0.1, "16 kHz : {at_16k}");
        assert!((eq.response_db(1000.0) - 5.9).abs() < 0.3);
        assert!(eq.response_db(16_000.0) < -20.0);
    }

    #[test]
    fn hot_parameter_change_applies_after_commit() {
        let mut eq = EqualizerNode::new(1, &[EqBand::peaking(1000.0, 1.0, 0.0)]);
        eq.prepare(SampleRate::HZ_48000, 4800);
        let flat = steady_peak(&run(&mut eq, 1000.0, 4800));
        assert!((flat - 1.0).abs() < 0.01);
        let ctrl = eq.band(0);
        ctrl.gain_db.set(-12.0);
        // Sans commit, rien ne change.
        let still = steady_peak(&run(&mut eq, 1000.0, 4800));
        assert!((still - 1.0).abs() < 0.01);
        ctrl.commit();
        let cut = steady_peak(&run(&mut eq, 1000.0, 4800));
        assert!((cut - 0.25).abs() < 0.02, "−12 dB : {cut}");
        // Désactivation.
        ctrl.enabled.set(false);
        ctrl.commit();
        let back = steady_peak(&run(&mut eq, 1000.0, 4800));
        assert!((back - 1.0).abs() < 0.01);
        // set() complet.
        ctrl.set(EqBand::high_pass(2000.0, 0.707));
        assert_eq!(ctrl.kind(), FilterKind::HighPass);
        assert_eq!(ctrl.get().frequency, 2000.0);
        let hp = steady_peak(&run(&mut eq, 100.0, 4800));
        assert!(hp < 0.02, "passe-haut à 100 Hz : {hp}");
        assert_eq!(eq.controls().len(), 1);
    }

    #[test]
    fn band_constructors() {
        assert_eq!(EqBand::low_shelf(200.0, 3.0).kind, FilterKind::LowShelf);
        assert_eq!(EqBand::high_shelf(8000.0, -3.0).kind, FilterKind::HighShelf);
        assert!(!EqBand::default().disabled().enabled);
        assert_eq!(EqBand::default().frequency, 1000.0);
    }
}
