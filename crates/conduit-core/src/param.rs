//! Paramètres numériques partagés, modifiables hors temps réel et lus sans verrou.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Paramètre `f32` atomique.
#[derive(Debug)]
pub struct Param {
    bits: AtomicU32,
}

impl Param {
    /// Crée avec une valeur initiale.
    pub const fn new(value: f32) -> Self {
        Self {
            bits: AtomicU32::new(value.to_bits()),
        }
    }

    /// Lit la valeur. Temps réel : oui.
    #[inline]
    pub fn get(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    /// Écrit la valeur.
    #[inline]
    pub fn set(&self, value: f32) {
        self.bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

impl Default for Param {
    fn default() -> Self {
        Self::new(0.0)
    }
}

/// Paramètre booléen atomique.
#[derive(Debug, Default)]
pub struct Flag(AtomicBool);

impl Flag {
    /// Crée avec une valeur initiale.
    pub const fn new(value: bool) -> Self {
        Self(AtomicBool::new(value))
    }

    /// Lit. Temps réel : oui.
    #[inline]
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Écrit.
    #[inline]
    pub fn set(&self, value: bool) {
        self.0.store(value, Ordering::Relaxed);
    }
}

/// Lissage exponentiel d'un paramètre côté fil audio (évite les clics).
///
/// `coeff` proche de 1 = lent. Le lissage s'arrête (valeur exacte) quand l'écart
/// devient négligeable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoothed {
    current: f32,
    coeff: f32,
}

impl Smoothed {
    /// Constante de temps typique : 5 ms.
    pub const DEFAULT_TIME_S: f32 = 0.005;

    /// Crée à une valeur initiale avec une constante de temps en secondes.
    pub fn new(initial: f32, time_s: f32, sample_rate: f32) -> Self {
        Self {
            current: initial,
            coeff: Self::coeff_for(time_s, sample_rate),
        }
    }

    /// Coefficient de lissage pour une constante de temps.
    pub fn coeff_for(time_s: f32, sample_rate: f32) -> f32 {
        if time_s <= 0.0 || sample_rate <= 0.0 {
            0.0
        } else {
            (-1.0 / (time_s * sample_rate)).exp()
        }
    }

    /// Valeur courante.
    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Force la valeur (sans lissage).
    pub fn reset(&mut self, value: f32) {
        self.current = value;
    }

    /// Avance d'un échantillon vers `target` et retourne la nouvelle valeur.
    #[inline]
    pub fn next(&mut self, target: f32) -> f32 {
        let diff = target - self.current;
        // Seuil absolu + relatif : au-delà de la granularité f32, sinon la valeur
        // pourrait rester bloquée à un ulp de la cible.
        if diff.abs() <= 1e-5 + 1e-6 * target.abs() {
            self.current = target;
        } else {
            self.current = target - diff * self.coeff;
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_and_flag_roundtrip() {
        let p = Param::new(1.5);
        assert_eq!(p.get(), 1.5);
        p.set(-2.0);
        assert_eq!(p.get(), -2.0);
        assert_eq!(Param::default().get(), 0.0);
        let f = Flag::new(true);
        assert!(f.get());
        f.set(false);
        assert!(!f.get());
        assert!(!Flag::default().get());
    }

    #[test]
    fn smoothed_converges_monotonically() {
        let mut s = Smoothed::new(0.0, 0.001, 48_000.0);
        let mut prev = 0.0;
        for _ in 0..1000 {
            let v = s.next(1.0);
            assert!(v >= prev && v <= 1.0);
            prev = v;
        }
        assert_eq!(s.current(), 1.0);
        s.reset(0.5);
        assert_eq!(s.current(), 0.5);
        let mut instant = Smoothed::new(0.0, 0.0, 48_000.0);
        assert_eq!(instant.next(3.0), 3.0);
    }
}
