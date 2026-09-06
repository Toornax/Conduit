//! Ce qui se passe autour d'une passe : fabrication du signal simulé, découpe du
//! préambule, analyse, verdict. Rien ici ne dépend de la plateforme : la vraie
//! boucle WASAPI et `--self-test` passent par les mêmes fonctions.

#![forbid(unsafe_code)]

use serde::Serialize;

use crate::analysis::{self, Analysis, AnalysisOptions, SineSpec, Tolerances, Verdict};

/// Trames minimales à analyser après la découpe du préambule : sous ce seuil la
/// mesure de fréquence n'a plus de sens.
const MIN_ANALYSED_FRAMES: usize = 4_096;

/// Fraction de l'amplitude demandée à partir de laquelle on considère que le
/// signal est arrivé.
const ONSET_RATIO: f64 = 0.25;

/// Résultat d'une passe.
#[derive(Debug, Clone, Serialize)]
pub struct Pass {
    /// Numéro de passe, à partir de 1.
    pub index: usize,
    /// Trames reçues de la capture.
    pub captured_frames: usize,
    /// Trames jetées au début (préambule + marge `--skip-ms`).
    pub skipped_frames: usize,
    /// Trames jetées à la fin (queue de silence après l'arrêt du rendu).
    pub trimmed_frames: usize,
    /// Mesures.
    pub analysis: Analysis,
    /// Verdict.
    pub verdict: Verdict,
}

/// Pourquoi une passe n'a pas pu être jugée : ce n'est pas un échec du pilote,
/// mais un enregistrement inexploitable.
///
/// Le cas est **typé** et non un simple message, parce que l'appelant en fait deux
/// choses différentes : [`Unusable::NoSignal`] est celui qui appelle un diagnostic
/// (volume de l'endpoint, session Windows, écho), l'autre est une simple erreur de
/// réglage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// Rien n'a été entendu du tout : la question devient « pourquoi ? ».
    NoSignal(String),
    /// Il ne reste pas assez de trames après la découpe du préambule et de la queue.
    TooShort(String),
}

impl Unusable {
    /// Vrai pour [`Unusable::NoSignal`] : le cas qui mérite un diagnostic.
    pub fn is_no_signal(&self) -> bool {
        matches!(self, Self::NoSignal(_))
    }

    /// Le message destiné à l'utilisateur.
    pub fn message(&self) -> &str {
        match self {
            Self::NoSignal(message) | Self::TooShort(message) => message,
        }
    }
}

impl core::fmt::Display for Unusable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Unusable {}

/// Découpe le préambule de `recording` puis analyse et juge.
///
/// # Erreurs
///
/// [`Unusable`] si aucun signal n'a été trouvé ou s'il ne reste pas assez de
/// trames : c'est un problème d'environnement, pas un échec du pilote.
pub fn evaluate(
    index: usize,
    recording: &[f32],
    spec: &SineSpec,
    options: &AnalysisOptions,
    tolerances: &Tolerances,
    skip_frames: usize,
) -> Result<Pass, Unusable> {
    let channels = spec.channels.max(1);
    let captured_frames = recording.len() / channels;
    let onset_level = (spec.amplitude * ONSET_RATIO).max(options.silence_threshold() * 4.0) as f32;
    let onset =
        analysis::first_signal_frame(recording, channels, onset_level).ok_or_else(|| {
            Unusable::NoSignal(format!(
                "passe {index} : aucun signal au-dessus de {} dans les {captured_frames} trames \
                 capturées — la capture n'entend pas le rendu (les deux endpoints sont-ils les \
                 deux côtés du même câble ?)",
                analysis::fr(f64::from(onset_level), 4)
            ))
        })?;
    let start = onset + skip_frames;

    // Borne de fin, symétrique du préambule : le rendu s'arrête avant la capture, qui
    // enregistre encore un silence de queue de durée fixe (voir `last_signal_frame`).
    // Sans cette découpe, la queue se lit comme un trou et une rupture de phase, et fait
    // échouer des passes dont l'audio était parfait — constaté dans la VM le 2026-09-06.
    // La garde de 5 ms écarte en plus la décroissance partielle de la dernière trame utile.
    let tail_guard = (spec.sample_rate as usize / 200).max(1);
    let end = analysis::last_signal_frame(recording, channels, onset_level)
        .map_or(captured_frames, |last| {
            last.saturating_add(1).saturating_sub(tail_guard)
        })
        .min(captured_frames)
        .max(start);

    if end.saturating_sub(start) < MIN_ANALYSED_FRAMES {
        return Err(Unusable::TooShort(format!(
            "passe {index} : {} trames utiles seulement après la découpe du préambule et de \
             la queue ({start} jetées au début, {} à la fin, sur {captured_frames}) — \
             allongez --seconds ou baissez --skip-ms",
            end.saturating_sub(start),
            captured_frames.saturating_sub(end)
        )));
    }
    let region = &recording[start * channels..end * channels];
    let analysis = analysis::analyze_with(region, spec, options);
    let verdict = analysis::verify(&analysis, spec, tolerances);
    Ok(Pass {
        index,
        captured_frames,
        skipped_frames: start,
        trimmed_frames: captured_frames.saturating_sub(end),
        analysis,
        verdict,
    })
}

/// Fabrique l'enregistrement qu'une boucle parfaite rendrait : `preamble_frames`
/// de silence (le temps que le rendu démarre, c'est le décalage que la vraie
/// capture voit), puis le sinus.
///
/// `glitch` retire une trame à l'index indiqué **dans le sinus** : c'est
/// exactement ce que fait un pilote qui perd une trame, et c'est ce que
/// `--self-test --inject-glitch` sert à prouver.
pub fn simulate(
    spec: &SineSpec,
    frames: usize,
    preamble_frames: usize,
    glitch: Option<usize>,
) -> Vec<f32> {
    let channels = spec.channels.max(1);
    let mut recording = vec![0.0f32; preamble_frames * channels];
    recording.extend_from_slice(&analysis::generate(spec, frames, 0.0));
    if let Some(frame) = glitch {
        if frame < frames {
            let at = (preamble_frames + frame) * channels;
            recording.drain(at..at + channels);
        }
    }
    recording
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SineSpec {
        SineSpec {
            freq_hz: 440.0,
            amplitude: 0.5,
            sample_rate: 48_000,
            channels: 2,
        }
    }

    #[test]
    fn boucle_parfaite_acceptee() {
        let spec = spec();
        let recording = simulate(&spec, 48_000, 2_400, None);
        let pass = evaluate(
            1,
            &recording,
            &spec,
            &AnalysisOptions::default(),
            &Tolerances::default(),
            4_800,
        )
        .expect("évaluation");
        assert!(pass.verdict.ok, "{:?}", pass.verdict.reasons);
        assert_eq!(pass.captured_frames, 50_400);
        // Le préambule est retrouvé (2 400 trames) puis la marge est ajoutée.
        assert!(pass.skipped_frames.abs_diff(2_400 + 4_800) <= 8);
        assert_eq!(
            pass.analysis.frames,
            pass.captured_frames - pass.skipped_frames - pass.trimmed_frames
        );
        // Signal jusqu'au dernier échantillon : seule la garde de 5 ms est retirée, plus
        // au plus une demi-période de sinus, la dernière trame au-dessus du seuil
        // précédant la vraie fin d'autant (1,1 ms à 440 Hz).
        assert!(
            pass.trimmed_frames < 48_000 / 100,
            "{}",
            pass.trimmed_frames
        );
    }

    /// La queue de silence que la vraie capture enregistre après l'arrêt du rendu ne
    /// doit plus faire échouer la passe : c'est ce qui faisait échouer dix passes sur
    /// dix dans la VM le 2026-09-06, alors que l'audio traversait parfaitement.
    #[test]
    fn queue_de_silence_ignoree() {
        let spec = spec();
        let mut recording = simulate(&spec, 48_000, 2_400, None);
        // 100 ms de silence en fin d'enregistrement, la durée mesurée dans la VM.
        recording.extend(core::iter::repeat_n(0.0f32, 4_800 * spec.channels));
        let pass = evaluate(
            1,
            &recording,
            &spec,
            &AnalysisOptions::default(),
            &Tolerances::default(),
            4_800,
        )
        .expect("évaluation");
        assert!(pass.verdict.ok, "{:?}", pass.verdict.reasons);
        assert!(pass.analysis.gaps.is_empty(), "{:?}", pass.analysis.gaps);
        assert!(pass.trimmed_frames >= 4_800, "{}", pass.trimmed_frames);
    }

    #[test]
    fn trame_injectee_refusee() {
        let spec = spec();
        let recording = simulate(&spec, 48_000, 2_400, Some(24_000));
        let pass = evaluate(
            1,
            &recording,
            &spec,
            &AnalysisOptions::default(),
            &Tolerances::default(),
            4_800,
        )
        .expect("évaluation");
        assert!(!pass.verdict.ok);
        assert!(
            pass.verdict
                .reasons
                .iter()
                .any(|r| r.starts_with("continuité")),
            "{:?}",
            pass.verdict.reasons
        );
        assert!(!pass.analysis.phase_breaks.is_empty());
    }

    #[test]
    fn silence_total_signale_comme_environnement() {
        let spec = spec();
        let err = evaluate(
            3,
            &vec![0.0; 48_000 * 2],
            &spec,
            &AnalysisOptions::default(),
            &Tolerances::default(),
            4_800,
        )
        .expect_err("silence");
        assert!(err.is_no_signal(), "{err:?}");
        assert!(err.message().contains("aucun signal"), "{err}");
    }

    #[test]
    fn enregistrement_trop_court_signale() {
        let spec = spec();
        let recording = simulate(&spec, 5_000, 100, None);
        let err = evaluate(
            2,
            &recording,
            &spec,
            &AnalysisOptions::default(),
            &Tolerances::default(),
            4_800,
        )
        .expect_err("trop court");
        // Un enregistrement trop court est un réglage à corriger, pas un silence à
        // diagnostiquer : la distinction décide de ce que l'outil imprime ensuite.
        assert!(!err.is_no_signal(), "{err:?}");
        assert!(err.message().contains("trames utiles"), "{err}");
    }

    #[test]
    fn glitch_hors_bornes_ignore() {
        let spec = spec();
        let recording = simulate(&spec, 1_000, 0, Some(5_000));
        assert_eq!(recording.len(), 1_000 * 2);
    }
}
