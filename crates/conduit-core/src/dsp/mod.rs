//! Briques de traitement du signal, sans allocation à l'exécution.

pub mod biquad;
pub mod dll;
pub mod resampler;

pub use biquad::{Biquad, BiquadCoeffs, FilterKind};
pub use dll::{Dll, DllConfig};
pub use resampler::{ResampleQuality, Resampler};

/// Générateur pseudo-aléatoire xorshift32, déterministe, sans allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XorShift32(u32);

impl XorShift32 {
    /// Crée avec une graine (0 est remplacé par une constante).
    pub const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    /// Prochain entier.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Prochain flottant uniforme dans [−1, 1).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        // 24 bits de mantisse → uniforme sur [0, 1), puis recentré.
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }
}

impl Default for XorShift32 {
    fn default() -> Self {
        Self::new(0x2545_F491)
    }
}

/// Convertit une amplitude linéaire en dBFS (−inf pour 0).
pub fn to_dbfs(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amplitude.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xorshift_is_deterministic_and_in_range() {
        let mut a = XorShift32::new(42);
        let mut b = XorShift32::new(42);
        for _ in 0..1000 {
            let x = a.next_f32();
            assert_eq!(x, b.next_f32());
            assert!((-1.0..1.0).contains(&x));
        }
        assert_ne!(XorShift32::new(0), XorShift32(0));
        let mut z = XorShift32::default();
        let mean: f32 = (0..100_000).map(|_| z.next_f32()).sum::<f32>() / 100_000.0;
        assert!(mean.abs() < 0.02, "moyenne {mean}");
    }

    #[test]
    fn dbfs() {
        assert_eq!(to_dbfs(1.0), 0.0);
        assert!((to_dbfs(0.5) + 6.02).abs() < 0.01);
        assert_eq!(to_dbfs(0.0), f32::NEG_INFINITY);
    }
}
