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

/// Écrêtage maximal de la correction de ratio d'un port, quelle que soit la
/// configuration demandée : 5e-4, soit ± 500 ppm.
///
/// Couvre toute dérive réelle entre deux horloges de matériel grand public (les
/// quartz usuels sont à ± 100 ppm au pire). Le rôle de ce plafond est de rendre
/// *inoffensive* une saturation provoquée par l'artefact de mesure décrit sur
/// [`AsyncPortConfig::effective_dll`] : à 500 ppm, la réserve ne se vide plus qu'à
/// 24 trames/s, donc une consigne de 768 trames tient 32 s, là où un artefact dure
/// moins d'une seconde. À 1 % (l'ancien défaut), elle se vidait à 485 trames/s et
/// l'anneau était à sec avant la fin de l'artefact.
pub const MAX_CORRECTION: f64 = 5e-4;

/// Bande passante maximale de la DLL d'un port : 0,02 Hz.
///
/// Dix fois moins que le défaut générique, donc `kp` dix fois plus petit
/// (5,24e-6 contre 5,24e-5). À écrêtage inchangé (1 %), il aurait fallu 1910 trames
/// d'erreur filtrée pour saturer, contre 191 auparavant — l'artefact de 480 trames
/// serait passé sous la butée. Mais l'écrêtage descend en même temps à
/// [`MAX_CORRECTION`], si bien que la butée est encore atteinte dès **96 trames**
/// d'erreur filtrée : un saut de 480 sature toujours la boucle. Ce que ce gain
/// réduit, c'est la vitesse à laquelle la boucle rejoint la butée et le temps qu'elle
/// y passe ; l'innocuité, elle, vient de [`MAX_CORRECTION`] — c'est ce que mesure
/// le test `dent_de_scie_ne_vide_plus_lanneau`.
///
/// Coût assumé : la boucle converge en une dizaine de secondes au démarrage au lieu
/// d'environ une. Les deux bouts partageant le même minuteur, il n'y a rien à
/// rattraper vite.
pub const MAX_BANDWIDTH_HZ: f64 = 0.02;

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

    /// Paramètres DLL effectifs. Trois garde-fous, tous imposés par le fait que le
    /// remplissage passé à la DLL est un **niveau instantané** (`GraphReader::read`,
    /// `GraphWriter::write`) et non une moyenne :
    ///
    /// - le seuil de verrouillage est au moins la moitié du bruit de mesure (un bloc
    ///   périphérique + un quantum) ;
    /// - l'écrêtage est plafonné à [`MAX_CORRECTION`] ;
    /// - la bande passante est plafonnée à [`MAX_BANDWIDTH_HZ`].
    ///
    /// Mesures du 2026-09-11 (boucle par câble, ROADMAP M1b-31) : rafales de
    /// sous-alimentations en capture toutes les ~2 min. Le producteur (capture
    /// WASAPI) verse 480 trames d'un coup par période de 10 ms et le consommateur les
    /// tire dos à dos dans le même réveil : le remplissage échantillonné une fois par
    /// cycle est une **dent de scie d'amplitude 480**, lue à sa propre fréquence
    /// (stroboscope). La phase des deux réveils dérive (~83 ppm mesurés) ; quand elle
    /// franchit la frontière du paquet, le remplissage mesuré saute de ±480 d'un cycle
    /// à l'autre. Avec les défauts génériques (`bandwidth_hz` 0,2 → `kp` ≈ 5,2e-5), ce
    /// saut donne `kp·480 = 0,025 > max_correction = 0,01` : la DLL saturait à
    /// **ratio 0,99000** (la butée, relevée telle quelle dans la série), la
    /// consommation accélérait de 1 % (−485 trames/s) et vidait l'anneau en moins
    /// d'une seconde — rafale, reset, et rebelote tant que la phase reste sur la
    /// frontière. Zéro débordement, zéro xrun au rendu : le signe colle. Les deux
    /// bouts partagent le minuteur du pilote, il n'existe aucune dérive réelle de
    /// cette taille ; 1 % d'écrêtage est absurde pour ce système.
    fn effective_dll(&self) -> DllConfig {
        let noise = (self.device_block + self.quantum_in_device_frames()) as f64 / 2.0;
        DllConfig {
            lock_threshold: self.dll.lock_threshold.max(noise),
            max_correction: self.dll.max_correction.min(MAX_CORRECTION),
            bandwidth_hz: self.dll.bandwidth_hz.min(MAX_BANDWIDTH_HZ),
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
///
/// Les extrêmes (`fill_min`/`fill_max`, `ratio_min`/`ratio_max`) sont cumulés depuis
/// la dernière remise à zéro : un relevé périodique du seul niveau courant ne peut
/// pas voir la dent de scie de la mesure de remplissage (cf.
/// [`AsyncPortConfig::effective_dll`]), qui est justement ce qu'il faut observer.
#[derive(Debug)]
pub struct AsyncStats {
    underruns: AtomicU64,
    overruns: AtomicU64,
    fill: AtomicU32,
    fill_min: AtomicU32,
    fill_max: AtomicU32,
    ratio_bits: AtomicU64,
    ratio_min_millionths: AtomicU64,
    ratio_max_millionths: AtomicU64,
    locked: AtomicBool,
    running: AtomicBool,
    cycles: AtomicU64,
}

impl Default for AsyncStats {
    fn default() -> Self {
        Self {
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            fill: AtomicU32::new(0),
            fill_min: AtomicU32::new(u32::MAX),
            fill_max: AtomicU32::new(0),
            ratio_bits: AtomicU64::new(0),
            ratio_min_millionths: AtomicU64::new(u64::MAX),
            ratio_max_millionths: AtomicU64::new(0),
            locked: AtomicBool::new(false),
            running: AtomicBool::new(false),
            cycles: AtomicU64::new(0),
        }
    }
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
    /// Remplissage minimal depuis la dernière remise à zéro (0 si aucune mesure).
    pub fn fill_min(&self) -> u32 {
        match self.fill_min.load(Ordering::Relaxed) {
            u32::MAX => 0,
            v => v,
        }
    }
    /// Remplissage maximal depuis la dernière remise à zéro.
    pub fn fill_max(&self) -> u32 {
        self.fill_max.load(Ordering::Relaxed)
    }
    /// Ratio de rééchantillonnage courant.
    pub fn ratio(&self) -> f64 {
        f64::from_bits(self.ratio_bits.load(Ordering::Relaxed))
    }
    /// Ratio minimal depuis la dernière remise à zéro, en millionièmes (0 si aucune
    /// mesure). Entier pour rester atomique sans verrou : 1_000_000 = ratio 1,0.
    pub fn ratio_min_millionths(&self) -> u64 {
        match self.ratio_min_millionths.load(Ordering::Relaxed) {
            u64::MAX => 0,
            v => v,
        }
    }
    /// Ratio maximal depuis la dernière remise à zéro, en millionièmes.
    pub fn ratio_max_millionths(&self) -> u64 {
        self.ratio_max_millionths.load(Ordering::Relaxed)
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

    /// Remet les extrêmes à zéro. À appeler après relevé, et une fois le flux en
    /// régime : le préremplissage passe par un remplissage nul, qui écraserait
    /// `fill_min` pour toute la vie du port.
    pub fn reset_extremes(&self) {
        self.fill_min.store(u32::MAX, Ordering::Relaxed);
        self.fill_max.store(0, Ordering::Relaxed);
        self.ratio_min_millionths.store(u64::MAX, Ordering::Relaxed);
        self.ratio_max_millionths.store(0, Ordering::Relaxed);
    }

    /// Publie l'état d'un cycle : niveau courant, ratio, verrouillage, et mise à jour
    /// des extrêmes. Temps réel : oui (atomiques relâchées, aucun verrou).
    fn record(&self, fill: usize, ratio: f64, locked: bool) {
        let fill = fill as u32;
        self.fill.store(fill, Ordering::Relaxed);
        self.fill_min.fetch_min(fill, Ordering::Relaxed);
        self.fill_max.fetch_max(fill, Ordering::Relaxed);
        self.ratio_bits.store(ratio.to_bits(), Ordering::Relaxed);
        let millionths = (ratio * 1e6).round().clamp(0.0, u64::MAX as f64) as u64;
        self.ratio_min_millionths
            .fetch_min(millionths, Ordering::Relaxed);
        self.ratio_max_millionths
            .fetch_max(millionths, Ordering::Relaxed);
        self.locked.store(locked, Ordering::Relaxed);
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
        self.stats
            .record(fill, self.resampler.ratio(), self.dll.is_locked());
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
        self.stats.record(
            fill.max(0.0) as usize,
            self.resampler.ratio(),
            self.dll.is_locked(),
        );
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

    /// Vérifie la sortie après `settle` secondes d'établissement. `settle` n'est plus
    /// une poignée de secondes depuis que [`MAX_BANDWIDTH_HZ`] ramène la bande passante
    /// de la boucle à 0,02 Hz : la convergence prend une dizaine de secondes, et c'est
    /// le prix assumé de l'insensibilité à la dent de scie de la mesure.
    fn check_output(out: &[f32], drift_ppm: f64, seconds: f64, settle: f64, stats: &AsyncStats) {
        assert_eq!(stats.underruns(), 0, "sous-alimentations");
        assert_eq!(stats.overruns(), 0, "débordements");
        assert!(stats.is_running());
        assert!(stats.is_locked(), "DLL non verrouillée");
        // Après `settle` : continuité de phase (dérivée bornée par ω·A à 1 kHz + marge).
        let start = (settle * GRAPH_RATE) as usize;
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
        let tol = expected_freq * 2e-4 + 2.0 / (seconds - settle);
        assert!(
            (measured - expected_freq).abs() < tol,
            "fréquence {measured} vs {expected_freq} (± {tol})"
        );
    }

    // Les dérives simulées ici tiennent dans l'enveloppe de [`MAX_CORRECTION`]
    // (± 500 ppm), qui est désormais tout ce que le port prétend rattraper : au-delà,
    // la boucle sature — par construction, et c'est ce qui rend la saturation
    // inoffensive (cf. `effective_dll`). Les quartz du matériel grand public tiennent
    // dans ± 100 ppm.

    #[test]
    fn input_port_absorbs_positive_drift_with_jitter() {
        let (out, stats, reader) = simulate_input(300.0, 0.3, 256, 48_000.0, 30.0);
        check_output(&out, 300.0, 30.0, 15.0, &stats);
        // L'estimation intégrale converge lentement (quelques constantes de temps) :
        // on vérifie le signe et l'ordre de grandeur ; la fréquence mesurée ci-dessus
        // prouve que le ratio effectif est juste.
        let est = reader.dll().drift_ppm();
        assert!(est > 150.0 && est < 450.0, "dérive estimée {est}");
        assert!(
            stats.ratio() < 1.0,
            "ratio {} : le périphérique va plus vite, on consomme plus",
            stats.ratio()
        );
        assert!(stats.cycles() > 0);
        // Les extrêmes ont bien été relevés, et le ratio n'a jamais quitté l'enveloppe.
        assert!(stats.fill_max() >= stats.fill_min());
        assert!(stats.fill_min() > 0, "remplissage minimal nul sans xrun");
        let (lo, hi) = (
            stats.ratio_min_millionths() as f64 / 1e6,
            stats.ratio_max_millionths() as f64 / 1e6,
        );
        assert!(
            lo >= 1.0 - MAX_CORRECTION && hi <= 1.0 + MAX_CORRECTION,
            "ratio hors de ± 500 ppm : {lo} … {hi}"
        );
    }

    #[test]
    fn input_port_absorbs_negative_drift_with_odd_block_size() {
        let (out, stats, _) = simulate_input(-300.0, 0.5, 240, 48_000.0, 30.0);
        check_output(&out, -300.0, 30.0, 15.0, &stats);
        assert!(stats.ratio() > 1.0);
    }

    #[test]
    fn input_port_resamples_44k1_device() {
        let (out, stats, _) = simulate_input(200.0, 0.2, 441, 44_100.0, 30.0);
        check_output(&out, 200.0, 30.0, 15.0, &stats);
        assert!((stats.ratio() - 48_000.0 / 44_100.0).abs() < 0.002);
    }

    #[test]
    #[ignore = "long : 1 h simulée, lancer en release"]
    fn input_port_one_hour_without_xrun() {
        let (out, stats, _) = simulate_input(400.0, 0.4, 256, 48_000.0, 3600.0);
        check_output(&out, 400.0, 3600.0, 15.0, &stats);
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
            (400.0, 256, 48_000.0),
            (-400.0, 200, 48_000.0),
            (300.0, 441, 44_100.0),
        ] {
            let (out, stats) = simulate_output(drift, 0.3, block, rate, 30.0);
            assert_eq!(stats.underruns(), 0, "dérive {drift}");
            assert_eq!(stats.overruns(), 0, "dérive {drift}");
            assert!(stats.is_locked(), "dérive {drift} : DLL non verrouillée");
            let start = (15.0 * rate) as usize;
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

    /// Configuration du câble mesuré le 2026-09-11 : capture WASAPI par paquets de
    /// 480 trames (10 ms), graphe à 256 trames de quantum, consigne 768 trames.
    fn cfg_cable_mesure() -> AsyncPortConfig {
        AsyncPortConfig {
            channels: 2,
            graph_rate: SampleRate::HZ_48000,
            device_rate: SampleRate::HZ_48000,
            quantum: 256,
            device_block: 480,
            target_fill: None,
            quality: ResampleQuality::Normal,
            dll: DllConfig::default(),
        }
    }

    /// Rejoue le défaut mesuré : le remplissage lu par le port est le **niveau
    /// instantané** de l'anneau, donc une dent de scie d'amplitude 480 (un paquet de
    /// capture) échantillonnée une fois par cycle ; la phase des deux réveils dérive
    /// (~83 ppm) et franchit la frontière du paquet à `t = 1 s`, ce qui fait sauter la
    /// mesure de +480 **sans qu'une seule trame n'ait été produite en plus**.
    ///
    /// Retourne `(ratio min, ratio max, creux minimal de l'anneau en trames)`. Le
    /// creux est le niveau vrai juste avant l'arrivée d'un paquet : c'est lui qui
    /// s'annule en sous-alimentation.
    fn rejoue_dent_de_scie(cfg: DllConfig, seconds: f64, saut: bool) -> (f64, f64, f64) {
        const FREQUENCE: f64 = 48_000.0;
        const PAQUET: f64 = 480.0;
        const QUANTUM_TEST: f64 = 256.0;
        let tic = PAQUET / FREQUENCE;
        let periode = QUANTUM_TEST / FREQUENCE;
        let consigne = cfg_cable_mesure().effective_target_fill() as f64;
        let mut dll = Dll::new(cfg, FREQUENCE, periode);
        let mut consomme = 0.0;
        let mut mult = 1.0;
        let (mut rmin, mut rmax) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut creux_min = f64::INFINITY;
        let cycles = (seconds / periode) as usize;
        for cycle in 0..cycles {
            let t = cycle as f64 * periode;
            let tics = (t / tic).floor();
            // Le creux : l'anneau tel qu'il est juste avant le paquet du tic courant.
            let creux = consigne + tics * PAQUET - consomme;
            // Le stroboscope : avant la frontière, la salve du graphe précède le
            // paquet du tic et voit le creux ; après, elle le suit et voit la crête.
            let strobe = if saut && t >= 1.0 { PAQUET } else { 0.0 };
            let mesure = creux + strobe;
            creux_min = creux_min.min(creux);
            consomme += QUANTUM_TEST / mult;
            mult = dll.update(mesure - consigne);
            rmin = rmin.min(mult);
            rmax = rmax.max(mult);
        }
        (rmin, rmax, creux_min)
    }

    #[test]
    fn dent_de_scie_ne_vide_plus_lanneau() {
        // Ancien réglage : les défauts génériques de la DLL, avec le seul garde-fou
        // d'alors (le seuil de verrouillage). C'est la panne mesurée.
        let ancien = DllConfig {
            lock_threshold: (480.0 + 256.0) / 2.0,
            ..DllConfig::default()
        };
        let (rmin, _, creux) = rejoue_dent_de_scie(ancien, 5.0, true);
        assert!(
            rmin <= 0.990_001,
            "ancien réglage : la DLL devrait saturer à la butée 0,99 ({rmin})"
        );
        // Une lecture qui tombe sur le creux réclame un quantum (256 trames) : sous ce
        // seuil elle repart à vide — c'est la rafale de sous-alimentations mesurée.
        assert!(
            creux < 256.0,
            "ancien réglage : l'anneau devrait se vider sous un quantum (creux {creux} trames)"
        );

        // Nouveau réglage : celui que le port applique réellement. Référence : la même
        // simulation sans le saut de phase, c'est-à-dire le creux structurel de la dent
        // de scie (consigne − un paquet). L'écart entre les deux est le coût de
        // l'artefact, et lui seul.
        let neuve = cfg_cable_mesure().effective_dll();
        let (_, _, creux_sans_saut) = rejoue_dent_de_scie(neuve, 5.0, false);
        let (rmin, rmax, creux) = rejoue_dent_de_scie(neuve, 5.0, true);
        assert!(
            rmin >= 1.0 - MAX_CORRECTION && rmax <= 1.0 + MAX_CORRECTION,
            "ratio hors de ± 500 ppm : {rmin} … {rmax}"
        );
        assert!(creux > 0.0, "l'anneau se vide : creux {creux} trames");
        // 4 s d'écrêtage à 500 ppm ne peuvent retirer que 4 × 24 = 96 trames ; on
        // tolère 130 pour le régime transitoire. L'ancien réglage en retirait 260.
        // Relevé de cette simulation : creux 225 trames contre 297 sans artefact, soit
        // 72 trames de réserve dépensées — et le ratio colle à la butée 0,999500. La
        // DLL sature donc toujours sur un saut de 480 trames (l'écrêtage cède dès
        // 96 trames d'erreur filtrée, kp = 5,24e-6) : ce que le réglage achète, ce
        // n'est pas l'absence de saturation, c'est son innocuité.
        let cout = creux_sans_saut - creux;
        assert!(
            cout < 130.0,
            "l'artefact coûte {cout} trames de réserve (creux {creux} contre {creux_sans_saut})"
        );
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
