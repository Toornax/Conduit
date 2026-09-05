//! Rééchantillonneur sinc fenêtré polyphase à ratio continûment variable.
//!
//! Le ratio (`sortie / entrée`) peut changer à chaque échantillon sans discontinuité :
//! seul le pas d'avancement dans le flux d'entrée varie. Les coefficients sont
//! précalculés dans une table de phases avec interpolation linéaire entre phases.
//! Aucune allocation après construction.

use core::f64::consts::PI;

/// Qualité (longueur du filtre).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ResampleQuality {
    /// 16 coefficients : faible latence, usage voix.
    Fast,
    /// 64 coefficients : usage général (défaut).
    #[default]
    Normal,
    /// 128 coefficients : haute fidélité.
    Best,
}

impl ResampleQuality {
    /// Nombre de coefficients (pair).
    pub fn taps(self) -> usize {
        match self {
            ResampleQuality::Fast => 16,
            ResampleQuality::Normal => 64,
            ResampleQuality::Best => 128,
        }
    }

    /// Nombre de phases de la table.
    pub fn phases(self) -> usize {
        match self {
            ResampleQuality::Fast => 128,
            ResampleQuality::Normal => 512,
            ResampleQuality::Best => 1024,
        }
    }
}

/// Rééchantillonneur multicanal sur trames entrelacées.
#[derive(Debug, Clone)]
pub struct Resampler {
    channels: usize,
    taps: usize,
    half: usize,
    phases: usize,
    /// `(phases + 1) × taps` coefficients, somme unitaire par phase.
    table: Vec<f32>,
    /// Ligne à retard entrelacée, `capacity` trames.
    buf: Vec<f32>,
    capacity: usize,
    /// Trames valides dans `buf`.
    len: usize,
    /// Position fractionnaire (en trames de `buf`) du prochain échantillon de sortie.
    pos: f64,
    /// Trames d'entrée par trame de sortie (`1 / ratio`).
    step: f64,
    nominal_ratio: f64,
}

impl Resampler {
    /// Crée un rééchantillonneur.
    ///
    /// - `nominal_ratio` = fréquence de sortie / fréquence d'entrée ; sert à choisir
    ///   la fréquence de coupure. Le ratio effectif part de cette valeur.
    /// - `max_block` : nombre maximal de trames de sortie demandées en une fois,
    ///   dimensionne la ligne à retard.
    pub fn new(
        channels: usize,
        nominal_ratio: f64,
        quality: ResampleQuality,
        max_block: usize,
    ) -> Self {
        assert!(channels > 0, "au moins un canal");
        assert!(
            nominal_ratio > 0.0 && nominal_ratio.is_finite(),
            "ratio invalide"
        );
        let taps = quality.taps();
        let half = taps / 2;
        let phases = quality.phases();
        let table = build_table(taps, phases, cutoff_for(nominal_ratio));
        // Entrée nécessaire pour un bloc de sortie (ratio ≤ 1 %) + historique + marge.
        let input_per_block = (max_block as f64 / nominal_ratio * 1.05).ceil() as usize + 1;
        let capacity = taps + 2 * input_per_block + 16;
        let mut r = Self {
            channels,
            taps,
            half,
            phases,
            table,
            buf: vec![0.0; capacity * channels],
            capacity,
            len: 0,
            pos: 0.0,
            step: 1.0 / nominal_ratio,
            nominal_ratio,
        };
        r.reset();
        r
    }

    /// Nombre de canaux.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Nombre de coefficients.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Latence intrinsèque du filtre, en trames d'entrée.
    pub fn latency(&self) -> usize {
        self.half
    }

    /// Ratio nominal.
    pub fn nominal_ratio(&self) -> f64 {
        self.nominal_ratio
    }

    /// Ratio effectif courant (sortie / entrée).
    pub fn ratio(&self) -> f64 {
        1.0 / self.step
    }

    /// Change le ratio effectif. Temps réel : oui. Borné à ± 10 % du nominal.
    pub fn set_ratio(&mut self, ratio: f64) {
        let r = ratio.clamp(self.nominal_ratio * 0.9, self.nominal_ratio * 1.1);
        self.step = 1.0 / r;
    }

    /// Vide la ligne à retard et repositionne : le premier échantillon de sortie
    /// correspondra à la première trame d'entrée poussée.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        // Historique nul devant la première trame réelle.
        self.len = self.half - 1;
        self.pos = (self.half - 1) as f64;
        self.step = 1.0 / self.nominal_ratio;
    }

    /// Trames d'entrée pouvant encore être poussées.
    pub fn free(&self) -> usize {
        self.capacity - self.len
    }

    /// Trames d'entrée en attente dans la ligne à retard (non encore consommées,
    /// au-delà de la position courante).
    pub fn pending(&self) -> f64 {
        self.len as f64 - self.pos
    }

    /// Nombre de trames de sortie productibles maintenant.
    pub fn available(&self) -> usize {
        // Il faut floor(pos) + half < len pour chaque sortie.
        let mut n = 0;
        let mut p = self.pos;
        while (p.floor() as usize) + self.half < self.len {
            n += 1;
            p += self.step;
        }
        n
    }

    /// Trames d'entrée à pousser pour pouvoir produire `out_frames` trames.
    pub fn input_needed(&self, out_frames: usize) -> usize {
        if out_frames == 0 {
            return 0;
        }
        let last = self.pos + (out_frames - 1) as f64 * self.step;
        let required_len = last.floor() as usize + self.half + 1;
        required_len.saturating_sub(self.len)
    }

    /// Pousse des trames entrelacées ; retourne le nombre acceptées. Temps réel : oui.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        let frames = (interleaved.len() / self.channels).min(self.free());
        let n = frames * self.channels;
        let start = self.len * self.channels;
        self.buf[start..start + n].copy_from_slice(&interleaved[..n]);
        self.len += frames;
        frames
    }

    /// Pousse `frames` trames de silence ; retourne le nombre acceptées. Temps réel : oui.
    pub fn push_silence(&mut self, frames: usize) -> usize {
        let frames = frames.min(self.free());
        let start = self.len * self.channels;
        self.buf[start..start + frames * self.channels].fill(0.0);
        self.len += frames;
        frames
    }

    /// Produit autant de trames que possible dans `out` (entrelacé) ; retourne le
    /// nombre produit. Temps réel : oui.
    pub fn pull(&mut self, out: &mut [f32]) -> usize {
        let ch = self.channels;
        let max_frames = out.len() / ch;
        let mut produced = 0;
        while produced < max_frames {
            let idx = self.pos.floor() as usize;
            if idx + self.half >= self.len {
                break;
            }
            let frac = self.pos - idx as f64;
            let phase_f = frac * self.phases as f64;
            let phase = phase_f.floor() as usize;
            let t = (phase_f - phase as f64) as f32;
            let row0 = &self.table[phase * self.taps..(phase + 1) * self.taps];
            let row1 = &self.table[(phase + 1) * self.taps..(phase + 2) * self.taps];
            let first = (idx + 1 - self.half) * ch;
            let window = &self.buf[first..first + self.taps * ch];
            let dst = &mut out[produced * ch..(produced + 1) * ch];
            dst.fill(0.0);
            for (k, (&c0, &c1)) in row0.iter().zip(row1).enumerate() {
                let c = c0 + (c1 - c0) * t;
                let frame = &window[k * ch..(k + 1) * ch];
                for (d, &x) in dst.iter_mut().zip(frame) {
                    *d += x * c;
                }
            }
            self.pos += self.step;
            produced += 1;
        }
        self.compact();
        produced
    }

    /// Décale la ligne à retard pour ne garder que l'historique nécessaire.
    fn compact(&mut self) {
        let idx = self.pos.floor() as usize;
        let keep_from = idx.saturating_sub(self.half - 1);
        if keep_from == 0 {
            return;
        }
        let ch = self.channels;
        self.buf.copy_within(keep_from * ch..self.len * ch, 0);
        self.len -= keep_from;
        self.pos -= keep_from as f64;
    }
}

/// Fréquence de coupure (cycles par trame d'entrée) pour un ratio donné.
fn cutoff_for(ratio: f64) -> f64 {
    0.5 * ratio.min(1.0) * 0.92
}

fn blackman_harris(u: f64) -> f64 {
    let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
    a0 - a1 * (2.0 * PI * u).cos() + a2 * (4.0 * PI * u).cos() - a3 * (6.0 * PI * u).cos()
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn build_table(taps: usize, phases: usize, cutoff: f64) -> Vec<f32> {
    let half = (taps / 2) as f64;
    let mut table = vec![0.0f32; (phases + 1) * taps];
    let mut row = vec![0.0f64; taps];
    for q in 0..=phases {
        let frac = q as f64 / phases as f64;
        let mut sum = 0.0;
        for (k, r) in row.iter_mut().enumerate() {
            // Distance entre l'échantillon d'entrée k et le point d'interpolation.
            let d = (k as f64 - half + 1.0) - frac;
            let u = (d + half) / taps as f64;
            *r = 2.0 * cutoff * sinc(2.0 * cutoff * d) * blackman_harris(u);
            sum += *r;
        }
        for (k, r) in row.iter().enumerate() {
            table[q * taps + k] = (r / sum) as f32;
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: f64, n: usize, offset_frames: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * (i as f64 + offset_frames) / rate).sin() as f32)
            .collect()
    }

    /// Rééchantillonne intégralement `input` (entrelacé) et retourne la sortie.
    fn run(r: &mut Resampler, input: &[f32], block: usize) -> Vec<f32> {
        let ch = r.channels();
        let frames = input.len() / ch;
        let mut out = Vec::new();
        let mut pos = 0;
        let mut tmp = vec![0.0f32; block * ch];
        loop {
            let need = r.input_needed(block);
            if need > 0 {
                if pos >= frames {
                    break;
                }
                let n = need.min(frames - pos).min(r.free());
                assert!(n > 0, "ligne à retard pleine");
                assert_eq!(r.push(&input[pos * ch..(pos + n) * ch]), n);
                pos += n;
                continue;
            }
            let got = r.pull(&mut tmp);
            assert_eq!(got, block);
            out.extend_from_slice(&tmp);
        }
        out
    }

    fn snr_db(signal: &[f32], reference: &[f32]) -> f64 {
        let mut s = 0.0f64;
        let mut e = 0.0f64;
        for (&x, &r) in signal.iter().zip(reference) {
            s += (r as f64).powi(2);
            e += (x as f64 - r as f64).powi(2);
        }
        10.0 * (s / e.max(1e-30)).log10()
    }

    #[test]
    fn unity_ratio_is_near_transparent_with_known_delay() {
        let mut r = Resampler::new(1, 1.0, ResampleQuality::Normal, 64);
        assert_eq!(r.latency(), 32);
        let input = sine(1000.0, 48_000.0, 4096, 0.0);
        let out = run(&mut r, &input, 64);
        // La sortie n correspond à l'entrée n (le filtre est centré, pas de décalage).
        let n = out.len().min(input.len()) - 64;
        let snr = snr_db(&out[64..n], &input[64..n]);
        assert!(snr > 100.0, "SNR ratio 1 : {snr:.1} dB");
    }

    #[test]
    fn fixed_ratio_48k_to_44k1_has_high_snr() {
        let (fin, fout) = (48_000.0, 44_100.0);
        let ratio = fout / fin;
        let mut r = Resampler::new(1, ratio, ResampleQuality::Normal, 256);
        let input = sine(1000.0, fin, 48_000, 0.0);
        let out = run(&mut r, &input, 256);
        assert!(out.len() > 40_000);
        // Sortie n ↔ temps n / fout.
        let reference: Vec<f32> = (0..out.len())
            .map(|i| (2.0 * PI * 1000.0 * i as f64 / fout).sin() as f32)
            .collect();
        let skip = 256;
        let n = out.len() - skip;
        let snr = snr_db(&out[skip..n], &reference[skip..n]);
        assert!(snr >= 90.0, "SNR 48k→44.1k : {snr:.1} dB");
    }

    #[test]
    fn upsampling_44k1_to_48k_and_best_quality() {
        let (fin, fout) = (44_100.0, 48_000.0);
        let mut r = Resampler::new(2, fout / fin, ResampleQuality::Best, 128);
        let mono = sine(5000.0, fin, 22_050, 0.0);
        let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, -x]).collect();
        let out = run(&mut r, &stereo, 128);
        let frames = out.len() / 2;
        let left: Vec<f32> = (0..frames).map(|i| out[2 * i]).collect();
        let right: Vec<f32> = (0..frames).map(|i| out[2 * i + 1]).collect();
        let reference: Vec<f32> = (0..frames)
            .map(|i| (2.0 * PI * 5000.0 * i as f64 / fout).sin() as f32)
            .collect();
        let skip = 256;
        let snr = snr_db(&left[skip..frames - skip], &reference[skip..frames - skip]);
        assert!(snr >= 90.0, "SNR 44.1k→48k : {snr:.1} dB");
        let neg: Vec<f32> = reference.iter().map(|x| -x).collect();
        assert!(snr_db(&right[skip..frames - skip], &neg[skip..frames - skip]) >= 90.0);
    }

    #[test]
    fn fast_quality_still_reasonable() {
        let mut r = Resampler::new(1, 1.0, ResampleQuality::Fast, 64);
        let input = sine(1000.0, 48_000.0, 4096, 0.0);
        let out = run(&mut r, &input, 64);
        let n = out.len().min(input.len()) - 64;
        assert!(snr_db(&out[64..n], &input[64..n]) > 60.0);
        assert_eq!(ResampleQuality::Fast.taps(), 16);
    }

    #[test]
    fn ratio_change_of_100ppm_has_no_discontinuity() {
        let mut r = Resampler::new(1, 1.0, ResampleQuality::Normal, 64);
        let input = sine(1000.0, 48_000.0, 8192, 0.0);
        let mut out = Vec::new();
        let mut pos = 0;
        let mut tmp = vec![0.0f32; 64];
        let mut cycles = 0;
        loop {
            let need = r.input_needed(64);
            if need > 0 {
                if pos + need > input.len() {
                    break;
                }
                r.push(&input[pos..pos + need]);
                pos += need;
                continue;
            }
            if cycles == 40 {
                r.set_ratio(1.0 + 100e-6);
            }
            if cycles == 80 {
                r.set_ratio(1.0 - 100e-6);
            }
            r.pull(&mut tmp);
            out.extend_from_slice(&tmp);
            cycles += 1;
        }
        // Dérivée bornée par ω·A d'un sinus à 1 kHz (± marge) : aucun saut.
        let max_step = 2.0 * PI * 1000.0 / 48_000.0 * 1.02;
        let worst = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs() as f64)
            .fold(0.0, f64::max);
        assert!(worst <= max_step, "saut {worst} > {max_step}");
        assert!((r.ratio() - (1.0 - 100e-6)).abs() < 1e-12);
    }

    #[test]
    fn ratio_is_clamped_and_reset_restores() {
        let mut r = Resampler::new(1, 1.0, ResampleQuality::Fast, 32);
        r.set_ratio(5.0);
        assert!((r.ratio() - 1.1).abs() < 1e-12);
        r.set_ratio(0.1);
        assert!((r.ratio() - 0.9).abs() < 1e-12);
        r.push(&[1.0; 20]);
        assert_eq!(r.free(), r.capacity - 20 - (r.half - 1));
        r.reset();
        assert_eq!(r.ratio(), 1.0);
        assert_eq!(r.pending(), 0.0);
        assert_eq!(r.available(), 0);
        assert_eq!(r.nominal_ratio(), 1.0);
        assert_eq!(r.channels(), 1);
        assert_eq!(r.taps(), 16);
        assert_eq!(r.push_silence(4), 4);
        assert_eq!(r.input_needed(0), 0);
    }

    #[test]
    fn available_and_input_needed_are_consistent() {
        let mut r = Resampler::new(1, 1.0, ResampleQuality::Fast, 32);
        let need = r.input_needed(32);
        assert_eq!(r.available(), 0);
        r.push(&vec![0.5; need - 1]);
        assert_eq!(r.available(), 31);
        assert_eq!(r.input_needed(32), 1);
        r.push(&[0.5]);
        assert_eq!(r.available(), 32);
        let mut out = [0.0; 32];
        assert_eq!(r.pull(&mut out), 32);
        assert_eq!(r.available(), 0);
        // Le silence initial puis le régime établi à 0,5 (DC gain unitaire).
        assert!((out[31] - 0.5).abs() < 1e-4, "{}", out[31]);
    }

    #[test]
    fn push_is_bounded_by_capacity() {
        let mut r = Resampler::new(2, 1.0, ResampleQuality::Fast, 8);
        let big = vec![0.0; 10_000];
        let accepted = r.push(&big);
        assert_eq!(accepted, r.free() + accepted - r.free());
        assert_eq!(r.free(), 0);
        assert_eq!(r.push(&[1.0, 2.0]), 0);
    }
}
