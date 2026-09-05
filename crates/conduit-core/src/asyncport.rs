//! Port asynchrone : relie un flux à horloge étrangère (périphérique) au graphe.
//!
//! Un port se compose d'un tampon circulaire, d'un [`Resampler`] à ratio variable et
//! d'une [`Dll`] qui asservit ce ratio au remplissage du tampon. Deux sens :
//!
//! - **entrée** ([`input_port`]) : le périphérique écrit ([`DeviceWriter`]), le
//!   graphe lit ([`GraphReader`]) ;
//! - **sortie** ([`output_port`]) : le graphe écrit ([`GraphWriter`]), le
//!   périphérique lit ([`DeviceReader`]).
//!
//! Le rééchantillonnage et la DLL tournent toujours côté graphe. Les xruns sont
//! comptés dans [`AsyncStats`], jamais silencieux. Aucune allocation après
//! construction.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::dsp::dll::{Dll, DllConfig};
use crate::dsp::resampler::{ResampleQuality, Resampler};
use crate::ring::{RingBuffer, RingConsumer, RingProducer};
use crate::types::SampleRate;

/// Configuration d'un port asynchrone.
#[derive(Debug, Clone, PartialEq)]
pub struct AsyncPortConfig {
    /// Canaux (trames entrelacées).
    pub channels: usize,
    /// Fréquence du graphe.
    pub graph_rate: SampleRate,
    /// Fréquence nominale du périphérique.
    pub device_rate: SampleRate,
    /// Trames par cycle du graphe.
    pub quantum: usize,
    /// Trames maximales par rappel du périphérique.
    pub device_block: usize,
    /// Remplissage visé du tampon, en trames périphérique. `None` = automatique
    /// (`device_block + quantum équivalent + marge`).
    pub target_fill: Option<usize>,
    /// Qualité du rééchantillonnage.
    pub quality: ResampleQuality,
    /// Paramètres de la DLL.
    pub dll: DllConfig,
}

impl AsyncPortConfig {
    /// Configuration par défaut pour un périphérique à la même fréquence que le graphe.
    pub fn new(channels: usize, rate: SampleRate, quantum: usize) -> Self {
        Self {
            channels,
            graph_rate: rate,
            device_rate: rate,
            quantum,
            device_block: quantum,
            target_fill: None,
            quality: ResampleQuality::Normal,
            dll: DllConfig::default(),
        }
    }

    /// Ratio nominal graphe / périphérique.
    fn ratio_graph_per_device(&self) -> f64 {
        self.graph_rate.as_f64() / self.device_rate.as_f64()
    }

    /// Quantum exprimé en trames périphérique (arrondi supérieur).
    fn quantum_in_device_frames(&self) -> usize {
        (self.quantum as f64 / self.ratio_graph_per_device()).ceil() as usize
    }

    /// Remplissage cible effectif.
    pub fn effective_target_fill(&self) -> usize {
        self.target_fill
            .unwrap_or_else(|| self.device_block + self.quantum_in_device_frames() + 32)
    }

    /// Paramètres DLL effectifs : le seuil de verrouillage est au moins la moitié du
    /// bruit de mesure (un bloc périphérique + un quantum).
    fn effective_dll(&self) -> DllConfig {
        let noise = (self.device_block + self.quantum_in_device_frames()) as f64 / 2.0;
        DllConfig {
            lock_threshold: self.dll.lock_threshold.max(noise),
            ..self.dll
        }
    }

    fn ring_capacity(&self) -> usize {
        4 * self.effective_target_fill()
            + 2 * self.device_block
            + 2 * self.quantum_in_device_frames()
    }
}

/// Statistiques partagées d'un port, lisibles hors temps réel.
#[derive(Debug, Default)]
pub struct AsyncStats {
    underruns: AtomicU64,
    overruns: AtomicU64,
    fill: AtomicU32,
    ratio_bits: AtomicU64,
    locked: AtomicBool,
    running: AtomicBool,
    cycles: AtomicU64,
}

impl AsyncStats {
    /// Sous-alimentations (lecture sans données suffisantes).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    /// Débordements (écriture refusée faute de place).
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }
    /// Total des xruns.
    pub fn xruns(&self) -> u64 {
        self.underruns() + self.overruns()
    }
    /// Dernier remplissage mesuré (trames périphérique).
    pub fn fill(&self) -> u32 {
        self.fill.load(Ordering::Relaxed)
    }
    /// Ratio de rééchantillonnage courant.
    pub fn ratio(&self) -> f64 {
        f64::from_bits(self.ratio_bits.load(Ordering::Relaxed))
    }
    /// Vrai si la DLL est verrouillée.
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
    /// Vrai si le port est en régime (préremplissage terminé).
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
    /// Cycles graphe traités.
    pub fn cycles(&self) -> u64 {
        self.cycles.load(Ordering::Relaxed)
    }
    /// Remet les compteurs de xruns à zéro.
    pub fn reset_xruns(&self) {
        self.underruns.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
    }
}

/// Côté périphérique d'un port d'entrée : écrit les trames capturées.
#[derive(Debug)]
pub struct DeviceWriter {
    ring: RingProducer<f32>,
    channels: usize,
    stats: Arc<AsyncStats>,
}

impl DeviceWriter {
    /// Écrit des trames entrelacées. Retourne le nombre de trames acceptées ; si
    /// inférieur, un débordement est compté. Temps réel : oui.
    pub fn write(&mut self, interleaved: &[f32]) -> usize {
        let frames = interleaved.len() / self.channels;
        let free = self.ring.slots() / self.channels;
        let n = frames.min(free);
        self.ring.write(&interleaved[..n * self.channels]);
        if n < frames {
            self.stats.overruns.fetch_add(1, Ordering::Relaxed);
        }
        n
    }

    /// Trames actuellement dans le tampon.
    pub fn fill(&self) -> usize {
        self.ring.len() / self.channels
    }

    /// Statistiques.
    pub fn stats(&self) -> Arc<AsyncStats> {
        Arc::clone(&self.stats)
    }
}

/// Côté graphe d'un port d'entrée : lit des trames au rythme du graphe.
#[derive(Debug)]
pub struct GraphReader {
    ring: RingConsumer<f32>,
    resampler: Resampler,
    dll: Dll,
    scratch: Box<[f32]>,
    channels: usize,
    target: usize,
    nominal_ratio: f64,
    running: bool,
    stats: Arc<AsyncStats>,
}

impl GraphReader {
    /// Remplit `out` (entrelacé, `n × channels`) avec exactement `n` trames. Retourne
    /// `true` si de l'audio réel a été fourni, `false` si silence (préremplissage ou
    /// sous-alimentation). Temps réel : oui.
    pub fn read(&mut self, out: &mut [f32]) -> bool {
        let frames = out.len() / self.channels;
        self.stats.cycles.fetch_add(1, Ordering::Relaxed);
        let ring_frames = self.ring.len() / self.channels;
        if !self.running {
            if ring_frames < self.target {
                self.publish(ring_frames);
                out[..frames * self.channels].fill(0.0);
                return false;
            }
            self.running = true;
            self.stats.running.store(true, Ordering::Relaxed);
        }
        let mut need = self.resampler.input_needed(frames);
        let mut underrun = false;
        while need > 0 {
            let chunk = need.min(self.scratch.len() / self.channels);
            let got = self.ring.read(&mut self.scratch[..chunk * self.channels]) / self.channels;
            if got == 0 {
                underrun = true;
                break;
            }
            self.resampler.push(&self.scratch[..got * self.channels]);
            need -= got;
        }
        if underrun {
            self.stats.underruns.fetch_add(1, Ordering::Relaxed);
            self.resampler.reset();
            self.dll.reset();
            self.running = false;
            self.stats.running.store(false, Ordering::Relaxed);
            out[..frames * self.channels].fill(0.0);
            self.publish(0);
            return false;
        }
        let produced = self.resampler.pull(&mut out[..frames * self.channels]);
        debug_assert_eq!(produced, frames);
        let fill = self.ring.len() as f64 / self.channels as f64 + self.resampler.pending();
        let mult = self.dll.update(fill - self.target as f64);
        self.resampler.set_ratio(self.nominal_ratio * mult);
        self.publish(fill as usize);
        true
    }

    fn publish(&self, fill: usize) {
        self.stats.fill.store(fill as u32, Ordering::Relaxed);
        self.stats
            .ratio_bits
            .store(self.resampler.ratio().to_bits(), Ordering::Relaxed);
        self.stats
            .locked
            .store(self.dll.is_locked(), Ordering::Relaxed);
    }

    /// Statistiques.
    pub fn stats(&self) -> Arc<AsyncStats> {
        Arc::clone(&self.stats)
    }

    /// Nombre de canaux.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Accès à la DLL (diagnostic).
    pub fn dll(&self) -> &Dll {
        &self.dll
    }
}

/// Crée un port d'entrée (périphérique → graphe).
pub fn input_port(cfg: &AsyncPortConfig) -> (DeviceWriter, GraphReader) {
    let ch = cfg.channels;
    let (p, c) = RingBuffer::with_capacity(cfg.ring_capacity() * ch);
    let stats = Arc::new(AsyncStats::default());
    let nominal = cfg.ratio_graph_per_device();
    let period = cfg.quantum as f64 / cfg.graph_rate.as_f64();
    stats.ratio_bits.store(nominal.to_bits(), Ordering::Relaxed);
    let reader = GraphReader {
        ring: c,
        resampler: Resampler::new(ch, nominal, cfg.quality, cfg.quantum),
        dll: Dll::new(cfg.effective_dll(), cfg.device_rate.as_f64(), period),
        scratch: vec![0.0; cfg.device_block.max(cfg.quantum) * ch].into_boxed_slice(),
        channels: ch,
        target: cfg.effective_target_fill(),
        nominal_ratio: nominal,
        running: false,
        stats: Arc::clone(&stats),
    };
    (
        DeviceWriter {
            ring: p,
            channels: ch,
            stats,
        },
        reader,
    )
}

/// Côté graphe d'un port de sortie : écrit des trames au rythme du graphe.
#[derive(Debug)]
pub struct GraphWriter {
    ring: RingProducer<f32>,
    resampler: Resampler,
    dll: Dll,
    scratch: Box<[f32]>,
    channels: usize,
    target: usize,
    nominal_ratio: f64,
    stats: Arc<AsyncStats>,
}

impl GraphWriter {
    /// Écrit des trames entrelacées produites par le graphe. Retourne `false` si un
    /// débordement s'est produit. Temps réel : oui.
    pub fn write(&mut self, interleaved: &[f32]) -> bool {
        self.stats.cycles.fetch_add(1, Ordering::Relaxed);
        let ring_frames = self.ring.len() as f64 / self.channels as f64;
        let pending_dev = self.resampler.pending() * self.resampler.ratio();
        let fill = ring_frames + pending_dev;
        let mult = self.dll.update(fill - self.target as f64);
        self.resampler.set_ratio(self.nominal_ratio * mult);
        let mut ok = true;
        let mut offset = 0;
        while offset < interleaved.len() {
            let accepted = self.resampler.push(&interleaved[offset..]) * self.channels;
            offset += accepted;
            let produced = self.resampler.pull(&mut self.scratch) * self.channels;
            if produced > 0 && !self.ring.write_all(&self.scratch[..produced]) {
                self.stats.overruns.fetch_add(1, Ordering::Relaxed);
                ok = false;
            }
            if accepted == 0 && produced == 0 {
                // Ligne à retard pleine et rien à produire : impossible en pratique.
                break;
            }
        }
        self.stats.fill.store(fill as u32, Ordering::Relaxed);
        self.stats
            .ratio_bits
            .store(self.resampler.ratio().to_bits(), Ordering::Relaxed);
        self.stats
            .locked
            .store(self.dll.is_locked(), Ordering::Relaxed);
        ok
    }

    /// Statistiques.
    pub fn stats(&self) -> Arc<AsyncStats> {
        Arc::clone(&self.stats)
    }

    /// Accès à la DLL (diagnostic).
    pub fn dll(&self) -> &Dll {
        &self.dll
    }
}

/// Côté périphérique d'un port de sortie : lit les trames à rendre.
#[derive(Debug)]
pub struct DeviceReader {
    ring: RingConsumer<f32>,
    channels: usize,
    target: usize,
    running: bool,
    stats: Arc<AsyncStats>,
}

impl DeviceReader {
    /// Remplit `out` (entrelacé). Retourne `true` si de l'audio réel a été fourni.
    /// Silence pendant le préremplissage et en cas de sous-alimentation (comptée).
    /// Temps réel : oui.
    pub fn read(&mut self, out: &mut [f32]) -> bool {
        let frames = out.len() / self.channels;
        let avail = self.ring.len() / self.channels;
        if !self.running {
            if avail < self.target {
                out.fill(0.0);
                return false;
            }
            self.running = true;
            self.stats.running.store(true, Ordering::Relaxed);
        }
        if avail < frames {
            self.stats.underruns.fetch_add(1, Ordering::Relaxed);
            self.ring.skip(avail * self.channels);
            out.fill(0.0);
            self.running = false;
            self.stats.running.store(false, Ordering::Relaxed);
            return false;
        }
        self.ring.read(&mut out[..frames * self.channels]);
        true
    }

    /// Trames disponibles.
    pub fn fill(&self) -> usize {
        self.ring.len() / self.channels
    }

    /// Statistiques.
    pub fn stats(&self) -> Arc<AsyncStats> {
        Arc::clone(&self.stats)
    }
}

/// Crée un port de sortie (graphe → périphérique).
pub fn output_port(cfg: &AsyncPortConfig) -> (GraphWriter, DeviceReader) {
    let ch = cfg.channels;
    let (p, c) = RingBuffer::with_capacity(cfg.ring_capacity() * ch);
    let stats = Arc::new(AsyncStats::default());
    // Ratio sortie/entrée du rééchantillonneur : périphérique / graphe.
    let nominal = 1.0 / cfg.ratio_graph_per_device();
    let period = cfg.quantum as f64 / cfg.graph_rate.as_f64();
    let max_out = (cfg.quantum as f64 * nominal).ceil() as usize + 2;
    stats.ratio_bits.store(nominal.to_bits(), Ordering::Relaxed);
    let writer = GraphWriter {
        ring: p,
        resampler: Resampler::new(ch, nominal, cfg.quality, cfg.quantum),
        dll: Dll::new(cfg.effective_dll(), cfg.device_rate.as_f64(), period),
        scratch: vec![0.0; max_out * ch].into_boxed_slice(),
        channels: ch,
        target: cfg.effective_target_fill(),
        nominal_ratio: nominal,
        stats: Arc::clone(&stats),
    };
    let reader = DeviceReader {
        ring: c,
        channels: ch,
        target: cfg.effective_target_fill(),
        running: false,
        stats,
    };
    (writer, reader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::XorShift32;

    const GRAPH_RATE: f64 = 48_000.0;
    const QUANTUM: usize = 256;

    fn zero_crossings(s: &[f32]) -> usize {
        s.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    /// Simulation à deux horloges : le périphérique (dérive `drift_ppm`, gigue
    /// `jitter` en fraction de sa période, blocs `dev_block`) écrit un sinus 1 kHz ;
    /// le graphe lit `QUANTUM` trames par cycle. Retourne la sortie graphe et les stats.
    fn simulate_input(
        drift_ppm: f64,
        jitter: f64,
        dev_block: usize,
        device_rate: f64,
        seconds: f64,
    ) -> (Vec<f32>, Arc<AsyncStats>, GraphReader) {
        let cfg = AsyncPortConfig {
            channels: 2,
            graph_rate: SampleRate::HZ_48000,
            device_rate: SampleRate::new(device_rate as u32).unwrap(),
            quantum: QUANTUM,
            device_block: dev_block,
            target_fill: None,
            quality: ResampleQuality::Normal,
            dll: DllConfig::default(),
        };
        let (mut writer, mut reader) = input_port(&cfg);
        let stats = reader.stats();
        let f_dev = device_rate * (1.0 + drift_ppm * 1e-6);
        let dev_period = dev_block as f64 / f_dev;
        let graph_period = QUANTUM as f64 / GRAPH_RATE;
        let mut rng = XorShift32::new(9);
        let mut dev_frame = 0u64;
        let mut dev_block_buf = vec![0.0f32; dev_block * 2];
        let mut graph_out = vec![0.0f32; QUANTUM * 2];
        let mut out = Vec::new();
        let (mut k, mut j) = (0u64, 0u64);
        let mut next_dev = 0.0;
        loop {
            let t_graph = j as f64 * graph_period;
            if t_graph > seconds {
                break;
            }
            if next_dev <= t_graph {
                for i in 0..dev_block {
                    let v = (2.0 * core::f64::consts::PI * 1000.0 * (dev_frame + i as u64) as f64
                        / device_rate)
                        .sin() as f32;
                    dev_block_buf[2 * i] = v;
                    dev_block_buf[2 * i + 1] = -v;
                }
                dev_frame += dev_block as u64;
                writer.write(&dev_block_buf);
                k += 1;
                next_dev = k as f64 * dev_period + f64::from(rng.next_f32()) * jitter * dev_period;
                continue;
            }
            reader.read(&mut graph_out);
            out.extend(graph_out.iter().step_by(2));
            j += 1;
        }
        (out, stats, reader)
    }

    fn check_output(out: &[f32], drift_ppm: f64, seconds: f64, stats: &AsyncStats) {
        assert_eq!(stats.underruns(), 0, "sous-alimentations");
        assert_eq!(stats.overruns(), 0, "débordements");
        assert!(stats.is_running());
        assert!(stats.is_locked(), "DLL non verrouillée");
        // Après 3 s : continuité de phase (dérivée bornée par ω·A à 1 kHz + marge).
        let start = (3.0 * GRAPH_RATE) as usize;
        let tail = &out[start..];
        let expected_freq = 1000.0 * (1.0 + drift_ppm * 1e-6);
        let max_step = 2.0 * core::f64::consts::PI * expected_freq / GRAPH_RATE * 1.05;
        let worst = tail
            .windows(2)
            .map(|w| (w[1] - w[0]).abs() as f64)
            .fold(0.0, f64::max);
        assert!(worst <= max_step, "discontinuité {worst} > {max_step}");
        let peak = tail.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 1.0).abs() < 0.02, "crête {peak}");
        // Fréquence : le sinus du périphérique arrive à sa vitesse réelle.
        let zc = zero_crossings(tail) as f64;
        let measured = zc / 2.0 / (tail.len() as f64 / GRAPH_RATE);
        let tol = expected_freq * 2e-4 + 2.0 / (seconds - 3.0);
        assert!(
            (measured - expected_freq).abs() < tol,
            "fréquence {measured} vs {expected_freq} (± {tol})"
        );
    }

    #[test]
    fn input_port_absorbs_positive_drift_with_jitter() {
        let (out, stats, reader) = simulate_input(1000.0, 0.3, 256, 48_000.0, 12.0);
        check_output(&out, 1000.0, 12.0, &stats);
        // L'estimation intégrale converge lentement (quelques constantes de temps) :
        // on vérifie le signe et l'ordre de grandeur ; la fréquence mesurée ci-dessus
        // prouve que le ratio effectif est juste.
        let est = reader.dll().drift_ppm();
        assert!(est > 500.0 && est < 2000.0, "dérive estimée {est}");
        assert!(
            stats.ratio() < 1.0,
            "ratio {} : le périphérique va plus vite, on consomme plus",
            stats.ratio()
        );
        assert!(stats.cycles() > 0);
    }

    #[test]
    fn input_port_absorbs_negative_drift_with_odd_block_size() {
        let (out, stats, _) = simulate_input(-1000.0, 0.5, 240, 48_000.0, 12.0);
        check_output(&out, -1000.0, 12.0, &stats);
        assert!(stats.ratio() > 1.0);
    }

    #[test]
    fn input_port_resamples_44k1_device() {
        let (out, stats, _) = simulate_input(200.0, 0.2, 441, 44_100.0, 10.0);
        check_output(&out, 200.0, 10.0, &stats);
        assert!((stats.ratio() - 48_000.0 / 44_100.0).abs() < 0.002);
    }

    #[test]
    #[ignore = "long : 1 h simulée, lancer en release"]
    fn input_port_one_hour_without_xrun() {
        let (out, stats, _) = simulate_input(800.0, 0.4, 256, 48_000.0, 3600.0);
        check_output(&out, 800.0, 3600.0, &stats);
    }

    #[test]
    fn input_port_reports_underrun_and_recovers() {
        let cfg = AsyncPortConfig::new(1, SampleRate::HZ_48000, QUANTUM);
        let (mut writer, mut reader) = input_port(&cfg);
        let stats = reader.stats();
        let block = vec![0.25f32; QUANTUM];
        let mut out = vec![0.0f32; QUANTUM];
        // Préremplissage : silence sans xrun.
        assert!(!reader.read(&mut out));
        assert_eq!(stats.underruns(), 0);
        for _ in 0..4 {
            assert_eq!(writer.write(&block), QUANTUM);
        }
        assert!(reader.read(&mut out));
        assert!(stats.is_running());
        // Le périphérique s'arrête : sous-alimentation comptée une fois, puis silence.
        let mut real = 0;
        for _ in 0..8 {
            if reader.read(&mut out) {
                real += 1;
            }
        }
        assert!(real < 8);
        assert_eq!(stats.underruns(), 1);
        assert!(!stats.is_running());
        assert!(out.iter().all(|&x| x == 0.0));
        // Reprise : après préremplissage, l'audio revient.
        for _ in 0..4 {
            writer.write(&block);
        }
        assert!(reader.read(&mut out));
        assert!(stats.is_running());
        assert!((out[QUANTUM - 1] - 0.25).abs() < 1e-3);
        assert_eq!(reader.channels(), 1);
        stats.reset_xruns();
        assert_eq!(stats.xruns(), 0);
    }

    #[test]
    fn device_writer_counts_overrun_when_graph_stalls() {
        let cfg = AsyncPortConfig::new(2, SampleRate::HZ_48000, QUANTUM);
        let (mut writer, _reader) = input_port(&cfg);
        let block = vec![0.0f32; QUANTUM * 2];
        let mut total = 0;
        for _ in 0..200 {
            total += writer.write(&block);
        }
        assert!(total < 200 * QUANTUM);
        assert!(writer.stats().overruns() > 0);
        assert_eq!(writer.fill(), total);
    }

    /// Simulation sens sortie : le graphe écrit un sinus, le périphérique lit.
    fn simulate_output(
        drift_ppm: f64,
        jitter: f64,
        dev_block: usize,
        device_rate: f64,
        seconds: f64,
    ) -> (Vec<f32>, Arc<AsyncStats>) {
        let cfg = AsyncPortConfig {
            channels: 1,
            graph_rate: SampleRate::HZ_48000,
            device_rate: SampleRate::new(device_rate as u32).unwrap(),
            quantum: QUANTUM,
            device_block: dev_block,
            target_fill: None,
            quality: ResampleQuality::Normal,
            dll: DllConfig::default(),
        };
        let (mut writer, mut reader) = output_port(&cfg);
        let stats = writer.stats();
        let f_dev = device_rate * (1.0 + drift_ppm * 1e-6);
        let dev_period = dev_block as f64 / f_dev;
        let graph_period = QUANTUM as f64 / GRAPH_RATE;
        let mut rng = XorShift32::new(5);
        let mut graph_frame = 0u64;
        let mut graph_buf = vec![0.0f32; QUANTUM];
        let mut dev_buf = vec![0.0f32; dev_block];
        let mut out = Vec::new();
        let (mut k, mut j) = (0u64, 0u64);
        let mut next_dev = 0.0;
        loop {
            let t_graph = j as f64 * graph_period;
            if t_graph > seconds {
                break;
            }
            if next_dev <= t_graph {
                reader.read(&mut dev_buf);
                out.extend_from_slice(&dev_buf);
                k += 1;
                next_dev = k as f64 * dev_period + f64::from(rng.next_f32()) * jitter * dev_period;
                continue;
            }
            for (i, x) in graph_buf.iter_mut().enumerate() {
                *x = (2.0 * core::f64::consts::PI * 1000.0 * (graph_frame + i as u64) as f64
                    / GRAPH_RATE)
                    .sin() as f32;
            }
            graph_frame += QUANTUM as u64;
            writer.write(&graph_buf);
            j += 1;
        }
        (out, stats)
    }

    #[test]
    fn output_port_absorbs_drift_both_ways() {
        for (drift, block, rate) in [
            (1000.0, 256, 48_000.0),
            (-1000.0, 200, 48_000.0),
            (300.0, 441, 44_100.0),
        ] {
            let (out, stats) = simulate_output(drift, 0.3, block, rate, 12.0);
            assert_eq!(stats.underruns(), 0, "dérive {drift}");
            assert_eq!(stats.overruns(), 0, "dérive {drift}");
            assert!(stats.is_locked(), "dérive {drift} : DLL non verrouillée");
            let start = (3.0 * rate) as usize;
            let tail = &out[start..];
            // Dans l'horloge du périphérique, le sinus du graphe apparaît à 1000 / (1 + dérive).
            // En temps réel, le sinus du graphe reste à 1000 Hz : le rééchantillonneur
            // compense la dérive du périphérique.
            let expected = 1000.0;
            let max_step = 2.0 * core::f64::consts::PI * expected / rate * 1.05;
            let worst = tail
                .windows(2)
                .map(|w| (w[1] - w[0]).abs() as f64)
                .fold(0.0, f64::max);
            assert!(
                worst <= max_step,
                "dérive {drift} : discontinuité {worst} > {max_step}"
            );
            let true_seconds = tail.len() as f64 / (rate * (1.0 + drift * 1e-6));
            let measured = zero_crossings(tail) as f64 / 2.0 / true_seconds;
            assert!(
                (measured - expected).abs() < expected * 3e-4 + 0.5,
                "dérive {drift} : {measured} vs {expected}"
            );
        }
    }

    #[test]
    fn device_reader_prefills_then_reports_underrun() {
        let cfg = AsyncPortConfig::new(1, SampleRate::HZ_48000, QUANTUM);
        let (mut writer, mut reader) = output_port(&cfg);
        let stats = reader.stats();
        let mut out = vec![1.0f32; 128];
        assert!(!reader.read(&mut out));
        assert!(out.iter().all(|&x| x == 0.0));
        assert_eq!(stats.underruns(), 0);
        let block = vec![0.5f32; QUANTUM];
        for _ in 0..4 {
            assert!(writer.write(&block));
        }
        assert!(reader.read(&mut out));
        assert!(stats.is_running());
        assert!(reader.fill() > 0);
        while reader.read(&mut out) {}
        assert_eq!(stats.underruns(), 1);
        assert!(!stats.is_running());
        assert_eq!(stats.fill(), stats.fill());
        assert!(writer.dll().updates() > 0);
    }

    #[test]
    fn graph_writer_counts_overrun_when_device_stalls() {
        let cfg = AsyncPortConfig::new(1, SampleRate::HZ_48000, QUANTUM);
        let (mut writer, _reader) = output_port(&cfg);
        let block = vec![0.0f32; QUANTUM];
        let mut ok = true;
        for _ in 0..200 {
            ok &= writer.write(&block);
        }
        assert!(!ok);
        assert!(writer.stats().overruns() > 0);
    }

    #[test]
    fn config_defaults() {
        let cfg = AsyncPortConfig::new(2, SampleRate::HZ_48000, 256);
        assert_eq!(cfg.effective_target_fill(), 256 + 256 + 32);
        let c2 = AsyncPortConfig {
            target_fill: Some(100),
            ..cfg.clone()
        };
        assert_eq!(c2.effective_target_fill(), 100);
        assert!(cfg.ring_capacity() > 4 * cfg.effective_target_fill());
    }
}
