//! Filtre biquad (formules du « Audio EQ Cookbook » de R. Bristow-Johnson).

use core::f64::consts::PI;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Type de filtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum FilterKind {
    /// Passe-bas (pente 12 dB/oct).
    LowPass,
    /// Passe-haut.
    HighPass,
    /// Passe-bande (gain 0 dB au centre).
    BandPass,
    /// Coupe-bande.
    Notch,
    /// Passe-tout (phase seulement).
    AllPass,
    /// Cloche (égaliseur paramétrique).
    #[default]
    Peaking,
    /// Plateau grave.
    LowShelf,
    /// Plateau aigu.
    HighShelf,
}

impl FilterKind {
    /// Tous les types, dans l'ordre de déclaration.
    pub const ALL: [FilterKind; 8] = [
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::BandPass,
        FilterKind::Notch,
        FilterKind::AllPass,
        FilterKind::Peaking,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
    ];

    /// Vrai si le gain en dB a un effet pour ce type.
    pub fn uses_gain(self) -> bool {
        matches!(
            self,
            FilterKind::Peaking | FilterKind::LowShelf | FilterKind::HighShelf
        )
    }

    /// Index stable (pour un stockage atomique).
    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|&k| k == self).unwrap_or(0) as u8
    }

    /// Inverse de [`index`](Self::index).
    pub fn from_index(i: u8) -> FilterKind {
        Self::ALL.get(i as usize).copied().unwrap_or_default()
    }
}

/// Coefficients normalisés (`a0 = 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    /// Numérateur.
    pub b0: f32,
    /// Numérateur.
    pub b1: f32,
    /// Numérateur.
    pub b2: f32,
    /// Dénominateur (sans `a0`).
    pub a1: f32,
    /// Dénominateur (sans `a0`).
    pub a2: f32,
}

impl Default for BiquadCoeffs {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl BiquadCoeffs {
    /// Filtre transparent.
    pub const IDENTITY: BiquadCoeffs = BiquadCoeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// Calcule les coefficients. Temps réel : oui (quelques fonctions trigonométriques,
    /// pas d'allocation).
    ///
    /// - `frequency` est bornée dans `[1, 0,49 × sample_rate]` (au-delà, les pôles
    ///   s'approchent trop du cercle unité pour la précision `f32`) ;
    /// - `q` est borné dans `[0.01, 100]` ;
    /// - `gain_db` n'est utilisé que si [`FilterKind::uses_gain`].
    pub fn compute(
        kind: FilterKind,
        sample_rate: f32,
        frequency: f32,
        q: f32,
        gain_db: f32,
    ) -> Self {
        let sr = f64::from(sample_rate.max(1.0));
        let f0 = f64::from(frequency).clamp(1.0, 0.49 * sr);
        let q = f64::from(q).clamp(0.01, 100.0);
        let a = 10f64.powf(f64::from(gain_db) / 40.0);
        let w0 = 2.0 * PI * f0 / sr;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterKind::LowPass => {
                let b1 = 1.0 - cos_w0;
                (
                    b1 / 2.0,
                    b1,
                    b1 / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            FilterKind::HighPass => {
                let t = 1.0 + cos_w0;
                (
                    t / 2.0,
                    -t,
                    t / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha),
            FilterKind::Notch => (
                1.0,
                -2.0 * cos_w0,
                1.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            FilterKind::AllPass => (
                1.0 - alpha,
                -2.0 * cos_w0,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            FilterKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos_w0,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos_w0,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let sqrt_a = a.sqrt();
                let k = 2.0 * sqrt_a * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 - k),
                    (a + 1.0) + (a - 1.0) * cos_w0 + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                    (a + 1.0) + (a - 1.0) * cos_w0 - k,
                )
            }
            FilterKind::HighShelf => {
                let sqrt_a = a.sqrt();
                let k = 2.0 * sqrt_a * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 - k),
                    (a + 1.0) - (a - 1.0) * cos_w0 + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                    (a + 1.0) - (a - 1.0) * cos_w0 - k,
                )
            }
        };
        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }

    /// Réponse en amplitude à `frequency` (linéaire).
    pub fn magnitude(&self, sample_rate: f32, frequency: f32) -> f32 {
        let w = 2.0 * PI * f64::from(frequency) / f64::from(sample_rate);
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (b0, b1, b2) = (f64::from(self.b0), f64::from(self.b1), f64::from(self.b2));
        let (a1, a2) = (f64::from(self.a1), f64::from(self.a2));
        // H(e^jw) = (b0 + b1 e^-jw + b2 e^-2jw) / (1 + a1 e^-jw + a2 e^-2jw)
        let num_re = b0 + b1 * c1 + b2 * c2;
        let num_im = -(b1 * s1 + b2 * s2);
        let den_re = 1.0 + a1 * c1 + a2 * c2;
        let den_im = -(a1 * s1 + a2 * s2);
        ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).sqrt() as f32
    }

    /// Réponse en amplitude en dB.
    pub fn magnitude_db(&self, sample_rate: f32, frequency: f32) -> f32 {
        super::to_dbfs(self.magnitude(sample_rate, frequency))
    }

    /// Vrai si les pôles sont dans le cercle unité.
    pub fn is_stable(&self) -> bool {
        self.a2.abs() < 1.0 && self.a1.abs() < 1.0 + self.a2
    }
}

/// État d'un biquad (forme directe II transposée), un canal.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Biquad {
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// État initial nul.
    pub const fn new() -> Self {
        Self { z1: 0.0, z2: 0.0 }
    }

    /// Remet l'état à zéro.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Filtre un échantillon. Temps réel : oui.
    #[inline]
    pub fn tick(&mut self, c: &BiquadCoeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Filtre un bloc en place. Temps réel : oui.
    pub fn process(&mut self, c: &BiquadCoeffs, buf: &mut [f32]) {
        for x in buf {
            *x = self.tick(c, *x);
        }
        self.flush_denormals();
    }

    /// Filtre `src` vers `dst`. Temps réel : oui.
    pub fn process_to(&mut self, c: &BiquadCoeffs, src: &[f32], dst: &mut [f32]) {
        for (d, &x) in dst.iter_mut().zip(src) {
            *d = self.tick(c, x);
        }
        self.flush_denormals();
    }

    #[inline]
    fn flush_denormals(&mut self) {
        // Évite les dénormalisés qui ralentissent le calcul quand le signal s'éteint.
        if self.z1.abs() < 1e-20 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-20 {
            self.z2 = 0.0;
        }
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

    /// Amplitude crête en régime établi (seconde moitié du signal).
    fn steady_peak(buf: &[f32]) -> f32 {
        buf[buf.len() / 2..]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    }

    #[test]
    fn identity_is_transparent() {
        let mut f = Biquad::new();
        let src = sine(1000.0, 256);
        let mut dst = vec![0.0; 256];
        f.process_to(&BiquadCoeffs::IDENTITY, &src, &mut dst);
        assert_eq!(src, dst);
        assert_eq!(BiquadCoeffs::default(), BiquadCoeffs::IDENTITY);
        assert!(BiquadCoeffs::IDENTITY.is_stable());
    }

    #[test]
    fn lowpass_passes_low_and_cuts_high() {
        let c = BiquadCoeffs::compute(
            FilterKind::LowPass,
            SR,
            1000.0,
            core::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!(c.is_stable());
        let mut low = sine(100.0, 4800);
        Biquad::new().process(&c, &mut low);
        assert!(
            (steady_peak(&low) - 1.0).abs() < 0.02,
            "100 Hz : {}",
            steady_peak(&low)
        );
        let mut high = sine(10_000.0, 4800);
        Biquad::new().process(&c, &mut high);
        assert!(steady_peak(&high) < 0.02, "10 kHz : {}", steady_peak(&high));
        // Réponse théorique : −3 dB à fc pour Q = 1/√2.
        assert!((c.magnitude_db(SR, 1000.0) + 3.0).abs() < 0.1);
        assert!(c.magnitude_db(SR, 10_000.0) < -35.0);
    }

    #[test]
    fn highpass_mirrors_lowpass() {
        let c = BiquadCoeffs::compute(
            FilterKind::HighPass,
            SR,
            1000.0,
            core::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!(c.magnitude_db(SR, 100.0) < -35.0);
        assert!(c.magnitude_db(SR, 10_000.0).abs() < 0.1);
    }

    #[test]
    fn peaking_boosts_at_center_only() {
        let c = BiquadCoeffs::compute(FilterKind::Peaking, SR, 1000.0, 1.0, 6.0);
        assert!((c.magnitude_db(SR, 1000.0) - 6.0).abs() < 0.05);
        assert!(c.magnitude_db(SR, 50.0).abs() < 0.2);
        assert!(c.magnitude_db(SR, 15_000.0).abs() < 0.3);
        let mut s = sine(1000.0, 9600);
        Biquad::new().process(&c, &mut s);
        assert!(
            (steady_peak(&s) - 1.995).abs() < 0.03,
            "+6 dB ≈ ×2 : {}",
            steady_peak(&s)
        );
        let cut = BiquadCoeffs::compute(FilterKind::Peaking, SR, 1000.0, 1.0, -12.0);
        assert!((cut.magnitude_db(SR, 1000.0) + 12.0).abs() < 0.05);
    }

    #[test]
    fn shelves_change_one_side_only() {
        let ls = BiquadCoeffs::compute(FilterKind::LowShelf, SR, 300.0, 0.7, 6.0);
        assert!((ls.magnitude_db(SR, 20.0) - 6.0).abs() < 0.2);
        assert!(ls.magnitude_db(SR, 10_000.0).abs() < 0.2);
        let hs = BiquadCoeffs::compute(FilterKind::HighShelf, SR, 3000.0, 0.7, -6.0);
        assert!((hs.magnitude_db(SR, 20_000.0) + 6.0).abs() < 0.3);
        assert!(hs.magnitude_db(SR, 50.0).abs() < 0.2);
    }

    #[test]
    fn notch_bandpass_allpass() {
        let n = BiquadCoeffs::compute(FilterKind::Notch, SR, 1000.0, 5.0, 0.0);
        assert!(n.magnitude_db(SR, 1000.0) < -40.0);
        assert!(n.magnitude_db(SR, 100.0).abs() < 0.2);
        let bp = BiquadCoeffs::compute(FilterKind::BandPass, SR, 1000.0, 5.0, 0.0);
        assert!(bp.magnitude_db(SR, 1000.0).abs() < 0.05);
        assert!(bp.magnitude_db(SR, 100.0) < -20.0);
        let ap = BiquadCoeffs::compute(FilterKind::AllPass, SR, 1000.0, 1.0, 0.0);
        for f in [50.0, 1000.0, 12_000.0] {
            assert!(ap.magnitude_db(SR, f).abs() < 0.01);
        }
    }

    #[test]
    fn parameters_are_clamped_and_stable() {
        for kind in FilterKind::ALL {
            for (f, q, g) in [
                (0.0, 0.0, 0.0),
                (1e9, 1e9, 24.0),
                (20.0, 0.01, -24.0),
                (23_999.0, 100.0, 24.0),
            ] {
                let c = BiquadCoeffs::compute(kind, SR, f, q, g);
                assert!(c.is_stable(), "{kind:?} f={f} q={q} g={g} : {c:?}");
                assert!(c.b0.is_finite() && c.a1.is_finite() && c.a2.is_finite());
            }
        }
    }

    #[test]
    fn kind_index_roundtrip() {
        for k in FilterKind::ALL {
            assert_eq!(FilterKind::from_index(k.index()), k);
        }
        assert_eq!(FilterKind::from_index(200), FilterKind::default());
        assert!(FilterKind::Peaking.uses_gain());
        assert!(!FilterKind::LowPass.uses_gain());
    }

    #[test]
    fn silence_after_signal_stays_finite_and_decays() {
        let c = BiquadCoeffs::compute(FilterKind::LowPass, SR, 50.0, 10.0, 0.0);
        let mut f = Biquad::new();
        let mut s = sine(50.0, 4800);
        f.process(&c, &mut s);
        let mut z = vec![0.0; 480_000];
        f.process(&c, &mut z);
        assert!(z.iter().all(|x| x.is_finite()));
        assert!(z[z.len() - 1].abs() < 1e-6);
        f.reset();
        assert_eq!(f, Biquad::default());
    }
}
