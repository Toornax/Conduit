//! Génération du sinus et analyse de ce qui revient de la boucle.
//!
//! Ce module ne connaît ni WASAPI ni aucun périphérique : il ne voit que des
//! `&[f32]` entrelacés. C'est là que vit toute l'intelligence de l'outil, et
//! c'est donc là que sont les tests.
//!
//! # Ce qui est mesuré
//!
//! - **Fréquence** ([`Analysis::detected_hz`]) : ajustement au sens des moindres
//!   carrés du modèle `a·cos(ωn) + b·sin(ωn)`. Pas de FFT : pour une pulsation
//!   donnée, les corrélations `Σ x·cos` et `Σ x·sin` se calculent en un passage,
//!   les sommes `Σcos²`, `Σsin²`, `Σcos·sin` ont une forme close (noyau de
//!   Dirichlet), et le système 2×2 donne `(a, b)`. La fréquence retenue est celle
//!   qui maximise l'énergie expliquée `a·Sxc + b·Sxs`, cherchée par raffinements
//!   successifs autour de la fréquence attendue (±5 %) : une grille sur un préfixe
//!   court d'abord (lobe large, pas de faux maximum), puis des grilles de plus en
//!   plus fines sur des préfixes de plus en plus longs, et enfin une section dorée
//!   sur le signal entier à l'intérieur du lobe principal.
//! - **Continuité de phase** ([`Analysis::phase_breaks`]) : le même ajustement, par
//!   blocs courts ([`AnalysisOptions::block_frames`], 128 trames par défaut) et
//!   avec une **référence de temps globale**. Pour un sinus continu la phase de
//!   chaque bloc est constante ; une trame perdue ou dupliquée décale tout ce qui
//!   suit et produit un saut de `ω` radians. La dérive lente (erreur résiduelle de
//!   fréquence, horloges qui ne sont pas la même) est retirée en soustrayant la
//!   **médiane** des écarts : ne reste que la discontinuité.
//! - **Trous** ([`Analysis::gaps`]) : suites d'au moins
//!   [`AnalysisOptions::min_gap_frames`] trames sous le seuil de silence, alors que
//!   le signal est censé être continu.
//! - **Amplitude**, **écrêtage** et **THD+N** ([`Analysis::thd_estimate`], part de
//!   l'énergie que le sinus ajusté n'explique pas).
//!
//! # Seuil de saut de phase
//!
//! Une seule trame perdue à 440 Hz et 48 kHz ne décale la phase que de
//! `2π·440/48000 ≈ 0,058 rad` : le seuil de 0,25 rad de
//! [`AnalysisOptions::phase_tolerance_rad`] la laisserait passer. Le seuil
//! **effectif** ([`Analysis::phase_threshold_rad`]) est donc le plus petit de la
//! tolérance demandée et de `0,3·ω` (borné en bas à 0,005 rad, très au-dessus du
//! bruit de mesure d'une boucle numérique). Le facteur 0,3 laisse de la marge au
//! bloc qui contient la coupure : son ajustement mélange les deux phases, chacun
//! des deux écarts ne vaut qu'une fraction du saut total.

#![forbid(unsafe_code)]

use std::f64::consts::TAU;

use serde::Serialize;

/// Sinus émis : ce que la boucle est censée rendre à l'identique.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SineSpec {
    /// Fréquence en hertz.
    pub freq_hz: f64,
    /// Amplitude crête, linéaire (0 à 1).
    pub amplitude: f64,
    /// Fréquence d'échantillonnage en hertz.
    pub sample_rate: u32,
    /// Nombre de canaux (le même signal sur chacun).
    pub channels: usize,
}

impl SineSpec {
    /// Pulsation en radians par trame.
    pub fn omega(&self) -> f64 {
        TAU * self.freq_hz / f64::from(self.sample_rate.max(1))
    }
}

/// Réglages de l'analyse. `Default` reprend les valeurs documentées du module.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AnalysisOptions {
    /// Trames par bloc d'estimation de phase.
    pub block_frames: usize,
    /// Borne supérieure du seuil de saut de phase, en radians.
    pub phase_tolerance_rad: f64,
    /// Longueur minimale d'un trou, en trames.
    pub min_gap_frames: usize,
    /// Seuil de silence en dBFS.
    pub silence_threshold_dbfs: f64,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            block_frames: 128,
            phase_tolerance_rad: 0.25,
            min_gap_frames: 8,
            silence_threshold_dbfs: -80.0,
        }
    }
}

impl AnalysisOptions {
    /// Seuil de silence en amplitude linéaire.
    pub fn silence_threshold(&self) -> f64 {
        10f64.powf(self.silence_threshold_dbfs / 20.0)
    }
}

/// Discontinuité de phase entre deux blocs consécutifs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PhaseBreak {
    /// Première trame du bloc où le saut est constaté.
    pub frame: usize,
    /// Écart de phase (dérive lente retirée), en radians, dans `]-π, π]`.
    pub delta_rad: f64,
}

/// Suite de trames silencieuses au milieu du signal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Gap {
    /// Première trame du trou.
    pub frame: usize,
    /// Longueur en trames.
    pub frames: usize,
}

/// Résultat de l'analyse d'un enregistrement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Analysis {
    /// Trames analysées.
    pub frames: usize,
    /// Fréquence mesurée en hertz.
    pub detected_hz: f64,
    /// Amplitude crête du sinus ajusté.
    pub amplitude: f64,
    /// Part de l'énergie que le sinus ajusté n'explique pas (distorsion + bruit).
    pub thd_estimate: f64,
    /// Sauts de phase constatés.
    pub phase_breaks: Vec<PhaseBreak>,
    /// Seuil de saut de phase effectivement appliqué, en radians.
    pub phase_threshold_rad: f64,
    /// Trous constatés.
    pub gaps: Vec<Gap>,
    /// Trames sous le seuil de silence (les passages par zéro du sinus en font
    /// partie : c'est `gaps` qui dit s'il y a un vrai trou).
    pub silence_frames: usize,
    /// Trames dont au moins un canal atteint `|x| ≥ 0,999`.
    pub clipped_frames: usize,
}

impl Analysis {
    /// Total des trames manquantes (somme des trous).
    pub fn gap_frames(&self) -> usize {
        self.gaps.iter().map(|g| g.frames).sum()
    }
}

/// Tolérances du verdict.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Tolerances {
    /// Écart de fréquence accepté, en parties par million.
    pub freq_ppm: f64,
    /// Amplitude minimale acceptée, en fraction de l'amplitude demandée.
    pub min_amplitude_ratio: f64,
    /// Sauts de phase acceptés.
    pub max_phase_breaks: usize,
    /// Trames de trou acceptées, toutes suites confondues.
    pub max_gap_frames: usize,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            freq_ppm: 200.0,
            min_amplitude_ratio: 0.5,
            max_phase_breaks: 0,
            max_gap_frames: 0,
        }
    }
}

/// Verdict d'une passe.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    /// Vrai si aucune raison de refus.
    pub ok: bool,
    /// Raisons du refus, en français, disant quoi regarder.
    pub reasons: Vec<String>,
}

/// Marge relative sur le seuil d'amplitude : l'amplitude doit le dépasser d'au
/// moins un pour mille. Sans elle, un signal exactement au seuil rendrait un
/// verdict tiré au sort par l'arrondi flottant.
const AMPLITUDE_MARGIN: f64 = 1e-3;

/// Plancher du seuil de saut de phase, en radians.
const PHASE_THRESHOLD_FLOOR: f64 = 0.005;

/// Fraction de l'avance de phase d'une trame utilisée comme seuil.
const PHASE_THRESHOLD_RATIO: f64 = 0.3;

/// Niveau à partir duquel une trame est comptée comme écrêtée.
const CLIP_LEVEL: f32 = 0.999;

/// Nombre à la française : virgule décimale. Tous les messages de l'outil
/// passent par là, ceux de [`verify`] compris.
pub fn fr(value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}").replace('.', ",")
}

/// Écrit `out.len() / channels` trames de sinus, en partant de `phase0`, et rend
/// la phase de la trame suivante (dans `[0, 2π[`).
///
/// N'alloue pas : c'est cette fonction qu'appelle le rappel de rendu.
pub fn fill(spec: &SineSpec, out: &mut [f32], phase0: f64) -> f64 {
    let channels = spec.channels.max(1);
    let step = spec.omega();
    let mut phase = phase0;
    for frame in out.chunks_exact_mut(channels) {
        let value = (spec.amplitude * phase.sin()) as f32;
        frame.fill(value);
        phase += step;
        if phase >= TAU {
            phase -= TAU;
        }
    }
    // Queue incomplète si `out` n'est pas un multiple du nombre de canaux.
    let used = (out.len() / channels) * channels;
    out[used..].fill(0.0);
    phase
}

/// Sinus entrelacé de `frames` trames, même signal sur tous les canaux.
pub fn generate(spec: &SineSpec, frames: usize, phase0: f64) -> Vec<f32> {
    let mut buffer = vec![0.0; frames * spec.channels.max(1)];
    fill(spec, &mut buffer, phase0);
    buffer
}

/// Analyse un enregistrement entrelacé avec les réglages par défaut.
pub fn analyze(samples: &[f32], spec: &SineSpec) -> Analysis {
    analyze_with(samples, spec, &AnalysisOptions::default())
}

/// Analyse un enregistrement entrelacé.
pub fn analyze_with(samples: &[f32], spec: &SineSpec, options: &AnalysisOptions) -> Analysis {
    let channels = spec.channels.max(1);
    let mono = to_mono(samples, channels);
    let frames = mono.len();
    let clipped_frames = samples
        .chunks_exact(channels)
        .filter(|frame| frame.iter().any(|s| s.abs() >= CLIP_LEVEL))
        .count();
    let silence = options.silence_threshold();
    let silence_frames = mono.iter().filter(|s| s.abs() < silence).count();
    let gaps = find_gaps(&mono, silence, options.min_gap_frames.max(1));

    let rate = f64::from(spec.sample_rate.max(1));
    let detected_hz = detect_frequency(&mono, rate, spec.freq_hz);
    let omega = TAU * detected_hz / rate;
    let fit = fit_at(&mono, omega, 0);
    let energy: f64 = mono.iter().map(|s| s * s).sum();
    let residual = (energy - fit.explained).max(0.0);
    let thd_estimate = if energy > 0.0 {
        (residual / energy).sqrt()
    } else {
        0.0
    };

    let phase_threshold_rad = phase_threshold(omega, options.phase_tolerance_rad);
    let phase_breaks = find_phase_breaks(
        &mono,
        omega,
        options.block_frames.max(8),
        phase_threshold_rad,
    );

    Analysis {
        frames,
        detected_hz,
        amplitude: fit.amplitude,
        thd_estimate,
        phase_breaks,
        phase_threshold_rad,
        gaps,
        silence_frames,
        clipped_frames,
    }
}

/// Confronte l'analyse aux tolérances.
pub fn verify(analysis: &Analysis, spec: &SineSpec, tol: &Tolerances) -> Verdict {
    let mut reasons = Vec::new();

    if spec.freq_hz > 0.0 {
        let ppm = (analysis.detected_hz - spec.freq_hz).abs() / spec.freq_hz * 1e6;
        if ppm > tol.freq_ppm {
            reasons.push(format!(
                "fréquence : {} Hz mesurés pour {} Hz demandés ({} ppm d'écart, {} accepté) — \
                 horloge du pilote, taille de tampon ou rééchantillonnage inattendu sur le chemin",
                fr(analysis.detected_hz, 3),
                fr(spec.freq_hz, 3),
                fr(ppm, 0),
                fr(tol.freq_ppm, 0)
            ));
        }
    }

    let floor = spec.amplitude * tol.min_amplitude_ratio;
    if analysis.amplitude <= floor * (1.0 + AMPLITUDE_MARGIN) {
        reasons.push(format!(
            "amplitude : {} mesurée pour {} demandée (plancher {}) — atténuation, canal muet ou \
             volume de l'endpoint ; vérifiez que la boucle est bien numérique",
            fr(analysis.amplitude, 4),
            fr(spec.amplitude, 4),
            fr(floor, 4)
        ));
    }

    if analysis.phase_breaks.len() > tol.max_phase_breaks {
        let first = analysis.phase_breaks[0];
        reasons.push(format!(
            "continuité : {} saut(s) de phase au-delà de {} rad ({} accepté(s)), le premier \
             à la trame {} de {} rad — trames perdues ou dupliquées dans la boucle \
             (position du tampon cyclique du pilote)",
            analysis.phase_breaks.len(),
            fr(analysis.phase_threshold_rad, 4),
            tol.max_phase_breaks,
            first.frame,
            fr(first.delta_rad, 4)
        ));
    }

    let gap_frames = analysis.gap_frames();
    if gap_frames > tol.max_gap_frames {
        let biggest = analysis
            .gaps
            .iter()
            .max_by_key(|g| g.frames)
            .copied()
            .unwrap_or(Gap {
                frame: 0,
                frames: 0,
            });
        reasons.push(format!(
            "trous : {} trou(s), {} trames au total ({} accepté(s)), le plus grand de {} trames \
             à la trame {} — le rendu n'alimente pas la capture (sous-alimentation du tampon, \
             rappel en retard)",
            analysis.gaps.len(),
            gap_frames,
            tol.max_gap_frames,
            biggest.frames,
            biggest.frame
        ));
    }

    if analysis.clipped_frames > 0 {
        reasons.push(format!(
            "écrêtage : {} trame(s) à |x| ≥ {} — baissez --amplitude ou le volume de l'endpoint, \
             la mesure de fréquence et de phase n'est plus fiable",
            analysis.clipped_frames,
            fr(f64::from(CLIP_LEVEL), 3)
        ));
    }

    Verdict {
        ok: reasons.is_empty(),
        reasons,
    }
}

/// Première trame où le signal dépasse `threshold`, s'il y en a une.
pub fn first_signal_frame(samples: &[f32], channels: usize, threshold: f32) -> Option<usize> {
    samples
        .chunks_exact(channels.max(1))
        .position(|frame| frame.iter().any(|s| s.abs() >= threshold))
}

/// **Dernière** trame où le signal dépasse `threshold`, s'il y en a une.
///
/// Symétrique de [`first_signal_frame`], et pour la même raison : un enregistrement
/// n'est propre ni à son début ni à sa fin. Le rendu s'arrête avant la capture, qui
/// enregistre encore un silence de queue — mesuré à 83 ms, 93 ms et 102 ms pour des
/// passes de 6 s, 3 s et 2 s dans la VM le 2026-09-06, donc une **durée fixe**, pas une
/// proportion. Analysée, cette queue se lit comme un trou et une rupture de phase, et
/// fait échouer le verdict alors que l'audio traversait parfaitement le câble.
pub fn last_signal_frame(samples: &[f32], channels: usize, threshold: f32) -> Option<usize> {
    let channels = channels.max(1);
    samples
        .chunks_exact(channels)
        .rposition(|frame| frame.iter().any(|s| s.abs() >= threshold))
}

// --- Interne -----------------------------------------------------------------

/// Moyenne des canaux, en `f64` : l'analyse travaille sur ce signal mono.
fn to_mono(samples: &[f32], channels: usize) -> Vec<f64> {
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().map(|s| f64::from(*s)).sum::<f64>() / channels as f64)
        .collect()
}

/// Seuil de saut de phase effectif.
fn phase_threshold(omega: f64, tolerance: f64) -> f64 {
    let from_frame = (PHASE_THRESHOLD_RATIO * omega.abs()).max(PHASE_THRESHOLD_FLOOR);
    from_frame.min(tolerance.abs())
}

/// Suites d'au moins `min_gap` trames sous `silence`.
fn find_gaps(mono: &[f64], silence: f64, min_gap: usize) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut start = None;
    for (i, value) in mono.iter().enumerate() {
        if value.abs() < silence {
            start.get_or_insert(i);
        } else if let Some(from) = start.take() {
            if i - from >= min_gap {
                gaps.push(Gap {
                    frame: from,
                    frames: i - from,
                });
            }
        }
    }
    if let Some(from) = start {
        if mono.len() - from >= min_gap {
            gaps.push(Gap {
                frame: from,
                frames: mono.len() - from,
            });
        }
    }
    gaps
}

/// Résultat de l'ajustement `a·cos(ωn) + b·sin(ωn)` sur une tranche.
#[derive(Debug, Clone, Copy)]
struct Fit {
    amplitude: f64,
    phase: f64,
    /// Énergie expliquée par le modèle : `a·Sxc + b·Sxs`.
    explained: f64,
}

/// Ajuste le modèle sur `x`, dont le premier échantillon est la trame `n0`.
fn fit_at(x: &[f64], omega: f64, n0: usize) -> Fit {
    let (sxc, sxs) = correlate(x, omega, n0);
    let n = x.len() as f64;
    let (c2, s2) = harmonic_sums(2.0 * omega, n0, x.len());
    let scc = 0.5 * (n + c2);
    let sss = 0.5 * (n - c2);
    let scs = 0.5 * s2;
    let det = scc * sss - scs * scs;
    let (a, b) = if det.abs() < 1e-9 {
        (0.0, 0.0)
    } else {
        ((sxc * sss - sxs * scs) / det, (sxs * scc - sxc * scs) / det)
    };
    Fit {
        amplitude: (a * a + b * b).sqrt(),
        phase: b.atan2(a),
        explained: a * sxc + b * sxs,
    }
}

/// `Σ x[i]·cos(ω(n0+i))` et `Σ x[i]·sin(ω(n0+i))`.
///
/// La rotation est incrémentale (six multiplications par échantillon) mais
/// ré-ancrée sur un `sin`/`cos` exact tous les [`ANCHOR`] échantillons : la
/// dérive de la récurrence ne s'accumule jamais.
fn correlate(x: &[f64], omega: f64, n0: usize) -> (f64, f64) {
    /// Échantillons entre deux ré-ancrages.
    const ANCHOR: usize = 512;
    let (cw, sw) = (omega.cos(), omega.sin());
    let mut sxc = 0.0;
    let mut sxs = 0.0;
    for (block, chunk) in x.chunks(ANCHOR).enumerate() {
        let base = omega * (n0 + block * ANCHOR) as f64;
        let mut c = base.cos();
        let mut s = base.sin();
        for value in chunk {
            sxc += value * c;
            sxs += value * s;
            let next_c = c * cw - s * sw;
            s = s * cw + c * sw;
            c = next_c;
        }
    }
    (sxc, sxs)
}

/// `Σ_{n=n0}^{n0+len-1} cos(θn)` et la somme des sinus (noyau de Dirichlet).
fn harmonic_sums(theta: f64, n0: usize, len: usize) -> (f64, f64) {
    if len == 0 {
        return (0.0, 0.0);
    }
    let half = theta / 2.0;
    let denom = half.sin();
    if denom.abs() < 1e-9 {
        // θ multiple de 2π : la forme close dégénère, on somme directement.
        let mut c = 0.0;
        let mut s = 0.0;
        for i in 0..len {
            let angle = theta * (n0 + i) as f64;
            c += angle.cos();
            s += angle.sin();
        }
        return (c, s);
    }
    let scale = (len as f64 * half).sin() / denom;
    let mid = theta * (n0 as f64 + (len as f64 - 1.0) / 2.0);
    (scale * mid.cos(), scale * mid.sin())
}

/// Fréquence qui maximise l'énergie expliquée, cherchée autour de `expected`.
fn detect_frequency(mono: &[f64], rate: f64, expected: f64) -> f64 {
    /// Longueur du premier préfixe analysé.
    const FIRST: usize = 1024;
    /// Facteur d'allongement du préfixe entre deux étapes.
    const GROWTH: usize = 4;
    if mono.len() < 32 || expected <= 0.0 || expected >= rate / 2.0 {
        return expected;
    }
    let mut lo = expected * 0.95;
    let mut hi = expected * 1.05;
    let mut len = FIRST.min(mono.len());
    loop {
        let prefix = &mono[..len];
        // Un pas d'un quart de lobe : le maximum vrai est à moins d'un pas du
        // meilleur point de la grille.
        let target = rate / (4.0 * len as f64);
        let points = (((hi - lo) / target).ceil() as usize + 1).clamp(5, 129);
        let step = (hi - lo) / (points - 1) as f64;
        let mut best = lo;
        let mut best_score = f64::NEG_INFINITY;
        for i in 0..points {
            let f = lo + step * i as f64;
            let score = fit_at(prefix, TAU * f / rate, 0).explained;
            if score > best_score {
                best_score = score;
                best = f;
            }
        }
        if len == mono.len() {
            // Le maximum est dans `best ± step`, soit un demi-lobe : la fonction y
            // est unimodale, la section dorée converge.
            return golden_max(mono, rate, best - step, best + step);
        }
        // Deux pas de marge : le lobe se resserre quand le préfixe s'allonge.
        lo = (best - 2.0 * step).max(expected * 0.9);
        hi = (best + 2.0 * step).min(expected * 1.1);
        len = (len * GROWTH).min(mono.len());
    }
}

/// Section dorée sur `[lo, hi]`, supposé contenir un unique maximum.
fn golden_max(mono: &[f64], rate: f64, lo: f64, hi: f64) -> f64 {
    /// Nombre d'itérations : réduit l'intervalle d'un facteur 4·10⁻⁷.
    const ITERS: usize = 30;
    let ratio = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (lo, hi);
    let mut c = b - ratio * (b - a);
    let mut d = a + ratio * (b - a);
    let score = |f: f64| fit_at(mono, TAU * f / rate, 0).explained;
    let (mut fc, mut fd) = (score(c), score(d));
    for _ in 0..ITERS {
        if fc > fd {
            b = d;
            d = c;
            fd = fc;
            c = b - ratio * (b - a);
            fc = score(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + ratio * (b - a);
            fd = score(d);
        }
    }
    (a + b) / 2.0
}

/// Sauts de phase entre blocs consécutifs, dérive lente retirée.
fn find_phase_breaks(mono: &[f64], omega: f64, block: usize, threshold: f64) -> Vec<PhaseBreak> {
    let blocks: Vec<(usize, f64)> = mono
        .chunks_exact(block)
        .enumerate()
        .map(|(k, chunk)| (k * block, fit_at(chunk, omega, k * block).phase))
        .collect();
    if blocks.len() < 3 {
        return Vec::new();
    }
    let deltas: Vec<f64> = blocks
        .windows(2)
        .map(|w| wrap_pi(w[1].1 - w[0].1))
        .collect();
    let drift = median(&deltas);
    deltas
        .iter()
        .zip(blocks.iter().skip(1))
        .filter_map(|(delta, (frame, _))| {
            let corrected = wrap_pi(delta - drift);
            (corrected.abs() > threshold).then_some(PhaseBreak {
                frame: *frame,
                delta_rad: corrected,
            })
        })
        .collect()
}

/// Ramène un angle dans `]-π, π]`.
fn wrap_pi(angle: f64) -> f64 {
    let mut a = angle % TAU;
    if a > std::f64::consts::PI {
        a -= TAU;
    } else if a <= -std::f64::consts::PI {
        a += TAU;
    }
    a
}

/// Médiane (valeur basse pour une longueur paire) ; `values` n'est pas vide.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(freq_hz: f64, sample_rate: u32) -> SineSpec {
        SineSpec {
            freq_hz,
            amplitude: 0.5,
            sample_rate,
            channels: 2,
        }
    }

    fn ppm(measured: f64, expected: f64) -> f64 {
        (measured - expected).abs() / expected * 1e6
    }

    /// Un demi-seconde de signal : assez pour toutes les mesures, assez court
    /// pour que les tests restent rapides en mode debug.
    fn frames(rate: u32) -> usize {
        rate as usize / 2
    }

    #[test]
    fn sinus_parfait_toutes_frequences() {
        for rate in [44_100u32, 48_000, 96_000] {
            for freq in [440.0, 997.0, 1000.0] {
                let s = spec(freq, rate);
                let samples = generate(&s, frames(rate), 0.0);
                let a = analyze(&samples, &s);
                assert!(
                    ppm(a.detected_hz, freq) < 50.0,
                    "{freq} Hz à {rate} Hz : {} Hz mesurés",
                    a.detected_hz
                );
                assert!((a.amplitude - 0.5).abs() < 1e-3, "{}", a.amplitude);
                assert!(a.thd_estimate < 1e-3, "{}", a.thd_estimate);
                assert!(a.phase_breaks.is_empty(), "{:?}", a.phase_breaks);
                assert!(a.gaps.is_empty(), "{:?}", a.gaps);
                assert_eq!(a.clipped_frames, 0);
                assert!(verify(&a, &s, &Tolerances::default()).ok);
            }
        }
    }

    #[test]
    fn phase_initiale_quelconque() {
        let s = spec(1000.0, 48_000);
        for phase0 in [0.0, 0.7, 2.5, 4.9] {
            let samples = generate(&s, frames(48_000), phase0);
            let a = analyze(&samples, &s);
            assert!(ppm(a.detected_hz, 1000.0) < 50.0, "{}", a.detected_hz);
            assert!(a.phase_breaks.is_empty(), "{phase0} : {:?}", a.phase_breaks);
        }
    }

    /// Une trame supprimée au milieu : c'est le cas « le pilote a perdu une trame ».
    #[test]
    fn trame_supprimee_donne_un_saut_de_phase() {
        let s = spec(440.0, 48_000);
        let n = frames(48_000);
        let mut samples = generate(&s, n, 0.0);
        let cut = n / 2;
        samples.drain(cut * s.channels..(cut + 1) * s.channels);
        let a = analyze(&samples, &s);
        assert!(!a.phase_breaks.is_empty(), "aucun saut détecté");
        assert!(
            a.phase_breaks.iter().any(|b| b.frame.abs_diff(cut) <= 256),
            "sauts au mauvais endroit : {:?} (coupure à {cut})",
            a.phase_breaks
        );
        assert!(a.gaps.is_empty(), "{:?}", a.gaps);
        let v = verify(&a, &s, &Tolerances::default());
        assert!(!v.ok);
        assert!(
            v.reasons.iter().any(|r| r.starts_with("continuité")),
            "{v:?}"
        );
    }

    /// Une trame dupliquée : le décalage est de signe opposé, il doit se voir aussi.
    #[test]
    fn trame_dupliquee_donne_un_saut_de_phase() {
        let s = spec(440.0, 48_000);
        let n = frames(48_000);
        let samples = generate(&s, n, 0.0);
        let cut = n / 2;
        let mut doubled = samples.clone();
        let frame: Vec<f32> = samples[cut * s.channels..(cut + 1) * s.channels].to_vec();
        for (i, value) in frame.into_iter().enumerate() {
            doubled.insert(cut * s.channels + i, value);
        }
        let a = analyze(&doubled, &s);
        assert!(!a.phase_breaks.is_empty(), "aucun saut détecté");
        assert!(
            a.phase_breaks.iter().any(|b| b.frame.abs_diff(cut) <= 256),
            "sauts au mauvais endroit : {:?}",
            a.phase_breaks
        );
    }

    #[test]
    fn silence_insere_donne_un_trou_et_des_sauts() {
        let s = spec(440.0, 48_000);
        let n = frames(48_000);
        let mut samples = generate(&s, n, 0.0);
        let cut = n / 2;
        let hole = 100;
        for _ in 0..hole * s.channels {
            samples.insert(cut * s.channels, 0.0);
        }
        let a = analyze(&samples, &s);
        assert_eq!(a.gaps.len(), 1, "{:?}", a.gaps);
        // Le trou peut déborder d'une trame ou deux : le sinus lui-même passe sous
        // le seuil de silence juste avant et juste après la coupure si elle tombe
        // près d'un passage par zéro.
        assert!(
            (hole..=hole + 2).contains(&a.gaps[0].frames),
            "{:?}",
            a.gaps
        );
        assert!(a.gaps[0].frame.abs_diff(cut) <= 2, "{:?}", a.gaps);
        assert!(!a.phase_breaks.is_empty());
        let v = verify(&a, &s, &Tolerances::default());
        assert!(!v.ok);
        assert!(v.reasons.iter().any(|r| r.starts_with("trous")), "{v:?}");
    }

    #[test]
    fn amplitude_de_moitie_refusee() {
        let s = spec(440.0, 48_000);
        let mut samples = generate(&s, frames(48_000), 0.0);
        for value in &mut samples {
            *value *= 0.5;
        }
        let a = analyze(&samples, &s);
        assert!((a.amplitude - 0.25).abs() < 1e-3, "{}", a.amplitude);
        let v = verify(&a, &s, &Tolerances::default());
        assert!(!v.ok);
        assert!(
            v.reasons.iter().any(|r| r.starts_with("amplitude")),
            "{v:?}"
        );
    }

    #[test]
    fn mauvaise_frequence_refusee() {
        let s = spec(440.0, 48_000);
        let joue = spec(441.0, 48_000);
        let samples = generate(&joue, frames(48_000), 0.0);
        let a = analyze(&samples, &s);
        assert!((a.detected_hz - 441.0).abs() < 0.05, "{}", a.detected_hz);
        let v = verify(&a, &s, &Tolerances::default());
        assert!(!v.ok);
        assert!(
            v.reasons.iter().any(|r| r.starts_with("fréquence")),
            "{v:?}"
        );
    }

    #[test]
    fn bruit_blanc_refuse() {
        let s = spec(440.0, 48_000);
        let n = frames(48_000);
        let mut state = 0x1234_5678u32;
        let mut samples = Vec::with_capacity(n * s.channels);
        for _ in 0..n {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let value = state as f32 / u32::MAX as f32 - 0.5;
            samples.push(value);
            samples.push(value);
        }
        let a = analyze(&samples, &s);
        assert!(a.thd_estimate > 0.9, "{}", a.thd_estimate);
        let v = verify(&a, &s, &Tolerances::default());
        assert!(!v.ok);
        assert!(
            v.reasons.iter().any(|r| r.starts_with("amplitude")),
            "{v:?}"
        );
    }

    #[test]
    fn ecretage_compte() {
        let s = SineSpec {
            amplitude: 1.5,
            ..spec(440.0, 48_000)
        };
        let samples: Vec<f32> = generate(&s, frames(48_000), 0.0)
            .into_iter()
            .map(|v| v.clamp(-1.0, 1.0))
            .collect();
        let a = analyze(&samples, &s);
        assert!(a.clipped_frames > 0);
        let v = verify(&a, &s, &Tolerances::default());
        assert!(!v.ok);
        assert!(v.reasons.iter().any(|r| r.starts_with("écrêtage")), "{v:?}");
    }

    #[test]
    fn seuil_de_phase_borne_par_la_tolerance() {
        // 8 kHz à 48 kHz : 0,3·ω dépasse 0,25, la tolérance demandée l'emporte.
        assert!((phase_threshold(TAU * 8_000.0 / 48_000.0, 0.25) - 0.25).abs() < 1e-12);
        // 440 Hz à 48 kHz : le seuil descend sous une demi-trame de phase.
        let omega = TAU * 440.0 / 48_000.0;
        let t = phase_threshold(omega, 0.25);
        assert!(t < omega / 2.0 && t > PHASE_THRESHOLD_FLOOR, "{t}");
        // Très basse fréquence : le plancher protège du bruit de mesure.
        assert!((phase_threshold(1e-4, 0.25) - PHASE_THRESHOLD_FLOOR).abs() < 1e-12);
    }

    #[test]
    fn generation_multicanale_et_queue() {
        let s = SineSpec {
            channels: 3,
            ..spec(1000.0, 48_000)
        };
        let mut out = vec![9.0f32; 3 * 4 + 2];
        let phase = fill(&s, &mut out, 0.0);
        assert!((0.0..TAU).contains(&phase));
        for frame in out[..12].chunks_exact(3) {
            assert_eq!(frame[0], frame[1]);
            assert_eq!(frame[1], frame[2]);
        }
        assert_eq!(&out[12..], &[0.0, 0.0]);
    }

    #[test]
    fn detection_du_debut_de_signal() {
        let s = spec(440.0, 48_000);
        let mut samples = vec![0.0f32; 100 * s.channels];
        samples.extend(generate(&s, 1000, 0.0));
        let first = first_signal_frame(&samples, s.channels, 0.1).unwrap();
        assert!((100..140).contains(&first), "{first}");
        assert_eq!(first_signal_frame(&[0.0; 8], 2, 0.1), None);
    }

    #[test]
    fn signal_trop_court_ne_panique_pas() {
        let s = spec(440.0, 48_000);
        let a = analyze(&[0.0; 8], &s);
        assert_eq!(a.frames, 4);
        assert!(a.phase_breaks.is_empty());
        let a = analyze(&[], &s);
        assert_eq!(a.frames, 0);
        assert_eq!(a.thd_estimate, 0.0);
    }

    #[test]
    fn medianes_et_repliement() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert!((wrap_pi(TAU + 0.5) - 0.5).abs() < 1e-12);
        assert!((wrap_pi(-TAU - 0.5) + 0.5).abs() < 1e-12);
        assert!((wrap_pi(std::f64::consts::PI) - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn sommes_harmoniques_contre_somme_directe() {
        for theta in [0.1, 1.0, 2.5, 6.0] {
            let (c, s) = harmonic_sums(theta, 7, 50);
            let mut dc = 0.0;
            let mut ds = 0.0;
            for n in 7..57 {
                dc += (theta * n as f64).cos();
                ds += (theta * n as f64).sin();
            }
            assert!((c - dc).abs() < 1e-9, "{theta} : {c} vs {dc}");
            assert!((s - ds).abs() < 1e-9, "{theta} : {s} vs {ds}");
        }
    }

    #[test]
    fn correlation_contre_somme_directe() {
        let x: Vec<f64> = (0..1300).map(|n| (0.001 * n as f64).sin()).collect();
        let omega = 0.13;
        let (sxc, sxs) = correlate(&x, omega, 11);
        let mut dc = 0.0;
        let mut ds = 0.0;
        for (i, v) in x.iter().enumerate() {
            dc += v * (omega * (11 + i) as f64).cos();
            ds += v * (omega * (11 + i) as f64).sin();
        }
        assert!((sxc - dc).abs() < 1e-8, "{sxc} vs {dc}");
        assert!((sxs - ds).abs() < 1e-8, "{sxs} vs {ds}");
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(24))]

        /// Un sinus propre, quelle que soit sa fréquence, son amplitude et sa
        /// phase dans les bornes : accepté, et la fréquence est à 100 ppm près.
        #[test]
        fn sinus_propre_toujours_accepte(
            freq in 200.0f64..4_000.0,
            amplitude in 0.1f64..0.9,
            phase0 in 0.0f64..TAU,
        ) {
            let s = SineSpec { freq_hz: freq, amplitude, sample_rate: 48_000, channels: 2 };
            let samples = generate(&s, 12_000, phase0);
            let a = analyze(&samples, &s);
            let ecart = (a.detected_hz - freq).abs() / freq * 1e6;
            proptest::prop_assert!(ecart < 100.0, "{} Hz mesurés pour {freq}", a.detected_hz);
            let v = verify(&a, &s, &Tolerances::default());
            proptest::prop_assert!(v.ok, "{:?}", v.reasons);
        }
    }
}
