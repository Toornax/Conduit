//! Paramètres de gain partagés entre le fil de gestion et le fil audio.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::types::{Db, Gain};

/// Gain contrôlable hors temps réel, lu sans verrou par le fil audio.
///
/// Le fil audio lit la cible via [`target`](Self::target) et applique une rampe
/// linéaire sur un cycle (voir [`Ramp`]) : aucun changement n'est appliqué en
/// marche d'escalier (F-13).
#[derive(Debug)]
pub struct GainParam {
    /// Gain linéaire cible (bits d'un `f32`).
    linear_bits: AtomicU32,
    muted: AtomicBool,
    /// Dernier gain effectivement appliqué par le fil audio (bits d'un `f32`).
    /// Sert à initialiser la rampe quand le graphe est recompilé.
    current_bits: AtomicU32,
}

impl Default for GainParam {
    fn default() -> Self {
        Self::new(Gain::UNITY)
    }
}

impl GainParam {
    /// Crée un paramètre avec un gain initial, non muet, déjà « en place » (pas de fondu
    /// à la première application).
    pub fn new(gain: Gain) -> Self {
        Self {
            linear_bits: AtomicU32::new(gain.get().to_bits()),
            muted: AtomicBool::new(false),
            current_bits: AtomicU32::new(gain.get().to_bits()),
        }
    }

    /// Crée un paramètre dont la première application part du silence : le fil audio
    /// fait un fondu entrant sur un cycle (utile pour un nouveau lien).
    pub fn new_faded_in(gain: Gain) -> Self {
        let p = Self::new(gain);
        p.current_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
        p
    }

    /// Gain effectivement appliqué à la fin du dernier cycle.
    pub fn current(&self) -> f32 {
        f32::from_bits(self.current_bits.load(Ordering::Relaxed))
    }

    /// Enregistre le gain appliqué en fin de cycle. Temps réel (appelé par l'exécuteur).
    #[inline]
    pub fn store_current(&self, current: f32) {
        self.current_bits
            .store(current.to_bits(), Ordering::Relaxed);
    }

    /// Fixe le gain (linéaire).
    pub fn set(&self, gain: Gain) {
        self.linear_bits
            .store(gain.get().to_bits(), Ordering::Relaxed);
    }

    /// Fixe le gain en dB.
    pub fn set_db(&self, db: Db) {
        self.set(db.to_gain());
    }

    /// Gain réglé (indépendamment du muet).
    pub fn gain(&self) -> Gain {
        Gain::new(f32::from_bits(self.linear_bits.load(Ordering::Relaxed)))
    }

    /// Gain réglé en dB.
    pub fn db(&self) -> Db {
        self.gain().to_db()
    }

    /// Active ou désactive le muet.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// État du muet.
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Gain effectif cible : 0 si muet, sinon le gain réglé. Temps réel.
    #[inline]
    pub fn target(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.linear_bits.load(Ordering::Relaxed))
        }
    }
}

/// Rampe linéaire d'un gain sur un cycle. État local au fil audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    current: f32,
}

impl Ramp {
    /// Rampe démarrant à `initial` (typiquement la cible, pour éviter un fondu au démarrage).
    pub const fn new(initial: f32) -> Self {
        Self { current: initial }
    }

    /// Gain courant (fin du dernier cycle).
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Prépare le cycle : retourne le plan d'application pour `frames` trames vers `target`,
    /// et avance l'état à `target`.
    #[inline]
    pub fn step(&mut self, target: f32, frames: usize) -> RampPlan {
        let start = self.current;
        self.current = target;
        if start == target || frames == 0 {
            RampPlan::Constant(target)
        } else {
            RampPlan::Linear {
                start,
                step: (target - start) / frames as f32,
            }
        }
    }
}

/// Plan d'application d'un gain sur un cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RampPlan {
    /// Gain constant sur tout le cycle.
    Constant(f32),
    /// Gain variant linéairement : trame `i` reçoit `start + step * (i + 1)`.
    Linear {
        /// Gain en fin de cycle précédent.
        start: f32,
        /// Incrément par trame.
        step: f32,
    },
}

impl RampPlan {
    /// `dst[i] = src[i] * g(i)`.
    #[inline]
    pub fn copy(self, src: &[f32], dst: &mut [f32]) {
        match self {
            RampPlan::Constant(1.0) => dst.copy_from_slice(src),
            RampPlan::Constant(0.0) => dst.fill(0.0),
            RampPlan::Constant(g) => {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = s * g;
                }
            }
            RampPlan::Linear { start, step } => {
                let mut g = start;
                for (d, s) in dst.iter_mut().zip(src) {
                    g += step;
                    *d = s * g;
                }
            }
        }
    }

    /// `dst[i] += src[i] * g(i)`.
    #[inline]
    pub fn add(self, src: &[f32], dst: &mut [f32]) {
        match self {
            RampPlan::Constant(1.0) => {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d += s;
                }
            }
            RampPlan::Constant(0.0) => {}
            RampPlan::Constant(g) => {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d += s * g;
                }
            }
            RampPlan::Linear { start, step } => {
                let mut g = start;
                for (d, s) in dst.iter_mut().zip(src) {
                    g += step;
                    *d += s * g;
                }
            }
        }
    }

    /// `buf[i] *= g(i)`.
    #[inline]
    pub fn apply(self, buf: &mut [f32]) {
        match self {
            RampPlan::Constant(1.0) => {}
            RampPlan::Constant(0.0) => buf.fill(0.0),
            RampPlan::Constant(g) => {
                for x in buf {
                    *x *= g;
                }
            }
            RampPlan::Linear { start, step } => {
                let mut g = start;
                for x in buf {
                    g += step;
                    *x *= g;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_roundtrip_and_mute() {
        let p = GainParam::default();
        assert_eq!(p.target(), 1.0);
        p.set_db(Db::new(-6.0));
        assert!((p.target() - 0.501).abs() < 1e-3);
        assert!((p.db().get() + 6.0).abs() < 1e-3);
        p.set_muted(true);
        assert!(p.is_muted());
        assert_eq!(p.target(), 0.0);
        assert!(
            (p.gain().get() - 0.501).abs() < 1e-3,
            "le gain réglé survit au muet"
        );
        p.set_muted(false);
        assert!((p.target() - 0.501).abs() < 1e-3);
        assert_eq!(
            p.current(),
            1.0,
            "current n'est mis à jour que par le fil audio"
        );
        p.store_current(0.25);
        assert_eq!(p.current(), 0.25);
        let faded = GainParam::new_faded_in(Gain::UNITY);
        assert_eq!(faded.current(), 0.0);
        assert_eq!(faded.target(), 1.0);
    }

    #[test]
    fn ramp_is_constant_when_target_unchanged() {
        let mut r = Ramp::new(0.5);
        assert_eq!(r.step(0.5, 64), RampPlan::Constant(0.5));
        assert_eq!(r.step(0.5, 0), RampPlan::Constant(0.5));
    }

    #[test]
    fn ramp_reaches_target_exactly_at_end_of_cycle() {
        let mut r = Ramp::new(0.0);
        let plan = r.step(1.0, 4);
        let src = [1.0; 4];
        let mut dst = [0.0; 4];
        plan.copy(&src, &mut dst);
        assert_eq!(dst, [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(r.current(), 1.0);
        let mut dst2 = [0.0; 4];
        r.step(1.0, 4).copy(&src, &mut dst2);
        assert_eq!(dst2, [1.0; 4]);
    }

    #[test]
    fn ramp_has_bounded_derivative() {
        // Saut de gain 0 → 1 sur un signal constant : la sortie ne doit jamais
        // varier de plus de 1/frames entre deux échantillons.
        let frames = 256;
        let mut r = Ramp::new(0.0);
        let src = vec![1.0; frames];
        let mut dst = vec![0.0; frames];
        r.step(1.0, frames).copy(&src, &mut dst);
        let max_step = dst
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step <= 1.0 / frames as f32 + 1e-6,
            "pas maximal {max_step}"
        );
        assert!((dst[frames - 1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn plan_add_and_apply() {
        let src = [2.0; 4];
        let mut dst = [1.0; 4];
        RampPlan::Constant(1.0).add(&src, &mut dst);
        assert_eq!(dst, [3.0; 4]);
        RampPlan::Constant(0.0).add(&src, &mut dst);
        assert_eq!(dst, [3.0; 4]);
        RampPlan::Constant(0.5).add(&src, &mut dst);
        assert_eq!(dst, [4.0; 4]);
        RampPlan::Linear {
            start: 0.0,
            step: 0.25,
        }
        .add(&src, &mut dst);
        assert_eq!(dst, [4.5, 5.0, 5.5, 6.0]);

        let mut buf = [1.0; 4];
        RampPlan::Constant(1.0).apply(&mut buf);
        assert_eq!(buf, [1.0; 4]);
        RampPlan::Constant(2.0).apply(&mut buf);
        assert_eq!(buf, [2.0; 4]);
        RampPlan::Linear {
            start: 1.0,
            step: -0.25,
        }
        .apply(&mut buf);
        assert_eq!(buf, [1.5, 1.0, 0.5, 0.0]);
        RampPlan::Constant(0.0).apply(&mut buf);
        assert_eq!(buf, [0.0; 4]);
        let mut dst = [9.0; 4];
        RampPlan::Constant(0.0).copy(&src, &mut dst);
        assert_eq!(dst, [0.0; 4]);
        RampPlan::Constant(0.5).copy(&src, &mut dst);
        assert_eq!(dst, [1.0; 4]);
    }
}
