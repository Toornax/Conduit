//! Boucle à verrouillage de délai (DLL) : estime la correction de ratio d'horloge à
//! partir de l'erreur de remplissage d'un tampon.
//!
//! Régulateur PI de second ordre sur l'erreur de remplissage préfiltrée, avec
//! anti-emballement. Modèle : le remplissage `x` (trames) évolue selon
//! `dx/dt = d + g·u` où `d` est la dérive inconnue, `g` la fréquence du côté régulé
//! et `u` la correction relative appliquée au ratio. Avec `u = −(kp·e + ki·∫e)`, la
//! boucle fermée est `ë + g·kp·ė + g·ki·e = 0` : `kp = 2ζω/g`, `ki = ω²/g`.

/// Paramètres d'une [`Dll`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DllConfig {
    /// Bande passante de la boucle en hertz (ω = 2π·bw). Plus haut = plus réactif,
    /// plus sensible au bruit de mesure.
    pub bandwidth_hz: f64,
    /// Amortissement ζ (1 = critique).
    pub damping: f64,
    /// Correction relative maximale (`0.01` = ± 1 %).
    pub max_correction: f64,
    /// Constante de temps du préfiltre de l'erreur, en secondes (0 = aucun).
    pub prefilter_s: f64,
    /// Seuil (trames) sous lequel l'erreur filtrée est considérée verrouillée.
    pub lock_threshold: f64,
}

impl Default for DllConfig {
    fn default() -> Self {
        Self {
            bandwidth_hz: 0.2,
            damping: 1.0,
            max_correction: 0.01,
            prefilter_s: 0.1,
            lock_threshold: 64.0,
        }
    }
}

/// État de la boucle.
#[derive(Debug, Clone, PartialEq)]
pub struct Dll {
    config: DllConfig,
    kp: f64,
    ki: f64,
    alpha: f64,
    period_s: f64,
    filtered: f64,
    integral: f64,
    correction: f64,
    updates: u64,
    locked_updates: u64,
}

impl Dll {
    /// Crée une boucle.
    ///
    /// - `regulated_rate_hz` : fréquence du côté dont on ajuste la consommation (`g`) ;
    /// - `period_s` : intervalle entre deux appels à [`update`](Self::update).
    pub fn new(config: DllConfig, regulated_rate_hz: f64, period_s: f64) -> Self {
        let omega = 2.0 * core::f64::consts::PI * config.bandwidth_hz.max(1e-3);
        let g = regulated_rate_hz.max(1.0);
        let alpha = if config.prefilter_s <= 0.0 {
            1.0
        } else {
            1.0 - (-period_s / config.prefilter_s).exp()
        };
        Self {
            config,
            kp: 2.0 * config.damping * omega / g,
            ki: omega * omega / g,
            alpha,
            period_s,
            filtered: 0.0,
            integral: 0.0,
            correction: 0.0,
            updates: 0,
            locked_updates: 0,
        }
    }

    /// Paramètres.
    pub fn config(&self) -> &DllConfig {
        &self.config
    }

    /// Remet la boucle à zéro (après un xrun ou un changement de cible).
    pub fn reset(&mut self) {
        self.filtered = 0.0;
        self.integral = 0.0;
        self.correction = 0.0;
        self.updates = 0;
        self.locked_updates = 0;
    }

    /// Met à jour avec l'erreur de remplissage (`remplissage − cible`, en trames) et
    /// retourne le multiplicateur de ratio à appliquer (`1 − u`, dans
    /// `[1 − max, 1 + max]`). Un remplissage excessif donne un multiplicateur < 1
    /// (consommer plus vite / produire moins). Temps réel : oui.
    pub fn update(&mut self, error_frames: f64) -> f64 {
        if self.updates == 0 {
            self.filtered = error_frames;
        } else {
            self.filtered += self.alpha * (error_frames - self.filtered);
        }
        self.updates += 1;
        let max = self.config.max_correction;
        let proportional = self.kp * self.filtered;
        let candidate = self.integral + self.ki * self.filtered * self.period_s;
        // Anti-emballement : on n'intègre pas dans le sens de la saturation.
        let u_unclamped = proportional + candidate;
        if u_unclamped.abs() <= max || (u_unclamped > 0.0) != (self.filtered > 0.0) {
            self.integral = candidate;
        }
        self.correction = (proportional + self.integral).clamp(-max, max);
        if self.filtered.abs() < self.config.lock_threshold {
            self.locked_updates += 1;
        } else {
            self.locked_updates = 0;
        }
        1.0 - self.correction
    }

    /// Dernière correction relative `u` (positive = remplissage trop haut).
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// Erreur filtrée courante (trames).
    pub fn filtered_error(&self) -> f64 {
        self.filtered
    }

    /// Dérive estimée en parties par million (composante intégrale).
    pub fn drift_ppm(&self) -> f64 {
        self.integral * 1e6
    }

    /// Vrai si l'erreur filtrée est restée sous le seuil pendant au moins 0,5 s.
    pub fn is_locked(&self) -> bool {
        self.locked_updates as f64 * self.period_s >= 0.5
    }

    /// Nombre de mises à jour.
    pub fn updates(&self) -> u64 {
        self.updates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::XorShift32;

    /// Modèle : un périphérique écrit à `g·(1 + drift)` trames/s ; le graphe consomme
    /// `quantum / ratio` trames d'entrée par période, `ratio = 1 − u`. La mesure du
    /// remplissage est bruitée (phase entre écriture par blocs et lecture).
    fn simulate(
        drift_ppm: f64,
        jitter_frames: f64,
        seconds: f64,
        cfg: DllConfig,
    ) -> (f64, Vec<f64>, Dll) {
        let g = 48_000.0;
        let quantum = 256.0;
        let period = quantum / g;
        let target = 512.0;
        let mut dll = Dll::new(cfg, g, period);
        let mut fill = target;
        let mut rng = XorShift32::new(1);
        let mut history = Vec::new();
        let steps = (seconds / period) as usize;
        let mut mult = 1.0;
        for _ in 0..steps {
            fill += g * period * (1.0 + drift_ppm * 1e-6);
            fill -= quantum / mult;
            let noise = f64::from(rng.next_f32()) * jitter_frames;
            mult = dll.update(fill + noise - target);
            history.push(fill - target);
        }
        (fill - target, history, dll)
    }

    #[test]
    fn converges_within_two_seconds_for_1000ppm_drift() {
        for drift in [1000.0, -1000.0, 300.0, -50.0] {
            let (_, hist, dll) = simulate(drift, 128.0, 20.0, DllConfig::default());
            let period = 256.0 / 48_000.0;
            let from = (2.0 / period) as usize;
            let worst = hist[from..].iter().fold(0.0f64, |m, e| m.max(e.abs()));
            assert!(
                worst < 256.0,
                "dérive {drift} ppm : erreur max après 2 s = {worst}"
            );
            assert!(dll.is_locked(), "dérive {drift} ppm : non verrouillée");
            assert!(
                (dll.drift_ppm() - drift).abs() < 100.0,
                "dérive estimée {} vs {drift}",
                dll.drift_ppm()
            );
        }
    }

    #[test]
    fn stays_stable_over_long_run_and_never_diverges() {
        let (final_err, hist, _) = simulate(700.0, 128.0, 120.0, DllConfig::default());
        assert!(final_err.abs() < 128.0, "erreur finale {final_err}");
        let tail = &hist[hist.len() / 2..];
        let worst = tail.iter().fold(0.0f64, |m, e| m.max(e.abs()));
        assert!(worst < 200.0, "pire erreur en régime établi {worst}");
    }

    #[test]
    fn correction_is_clamped_with_anti_windup() {
        let cfg = DllConfig {
            max_correction: 0.001,
            ..Default::default()
        };
        // Dérive de 5000 ppm : hors de portée (max 1000 ppm) → saturation propre.
        let (_, _, dll) = simulate(5000.0, 0.0, 5.0, cfg);
        assert!((dll.correction() - (-0.001)).abs() < 1e-12 || dll.correction().abs() <= 0.001);
        assert!(dll.correction().abs() <= 0.001);
        // L'intégrale n'a pas explosé : elle reste bornée près de la saturation.
        assert!(dll.integral.abs() < 0.01, "intégrale {}", dll.integral);
    }

    #[test]
    fn reset_and_accessors() {
        let mut dll = Dll::new(DllConfig::default(), 48_000.0, 256.0 / 48_000.0);
        assert_eq!(dll.updates(), 0);
        dll.update(100.0);
        assert!(dll.filtered_error() > 0.0);
        assert!(dll.correction() > 0.0);
        assert!(!dll.is_locked());
        dll.reset();
        assert_eq!(dll.updates(), 0);
        assert_eq!(dll.correction(), 0.0);
        assert_eq!(dll.config().damping, 1.0);
        let no_filter = Dll::new(
            DllConfig {
                prefilter_s: 0.0,
                ..Default::default()
            },
            48_000.0,
            0.005,
        );
        assert_eq!(no_filter.alpha, 1.0);
    }
}
