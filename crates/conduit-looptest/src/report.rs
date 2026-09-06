//! Mise en forme des résultats : une ligne par passe, un résumé, ou un document
//! JSON pour une machine.

#![forbid(unsafe_code)]

use serde_json::{json, Value};

use crate::analysis::SineSpec;
use crate::pass::Pass;

pub use crate::analysis::fr;

/// Ligne (ou lignes, en cas d'échec) d'une passe.
pub fn pass_lines(pass: &Pass, total: usize) -> String {
    let a = &pass.analysis;
    let mut out = format!(
        "passe {}/{total} : {} Hz, amplitude {}, {} saut(s), {} trou(s) → {}",
        pass.index,
        fr(a.detected_hz, 2),
        fr(a.amplitude, 3),
        a.phase_breaks.len(),
        a.gaps.len(),
        if pass.verdict.ok { "OK" } else { "ÉCHEC" }
    );
    for reason in &pass.verdict.reasons {
        out.push_str("\n    - ");
        out.push_str(reason);
    }
    out
}

/// Résumé final.
pub fn summary_line(passes: &[Pass]) -> String {
    let total = passes.len();
    let ok = passes.iter().filter(|p| p.verdict.ok).count();
    if total == 0 {
        return "résumé : aucune passe".to_string();
    }
    let lo = passes
        .iter()
        .map(|p| p.analysis.detected_hz)
        .fold(f64::INFINITY, f64::min);
    let hi = passes
        .iter()
        .map(|p| p.analysis.detected_hz)
        .fold(f64::NEG_INFINITY, f64::max);
    let breaks: usize = passes.iter().map(|p| p.analysis.phase_breaks.len()).sum();
    let gap_frames: usize = passes.iter().map(|p| p.analysis.gap_frames()).sum();
    format!(
        "résumé : {ok}/{total} passe(s) OK — fréquence de {} à {} Hz, {breaks} saut(s), \
         {gap_frames} trame(s) de trou",
        fr(lo, 3),
        fr(hi, 3)
    )
}

/// Document JSON complet (`--json`).
pub fn json_document(spec: &SineSpec, passes: &[Pass]) -> Value {
    let ok = passes.iter().filter(|p| p.verdict.ok).count();
    json!({
        "outil": "conduit-looptest",
        "sinus": spec,
        "passes": passes,
        "resume": {
            "passes": passes.len(),
            "ok": ok,
            "echouees": passes.len() - ok,
            "verdict": if ok == passes.len() && !passes.is_empty() { "ok" } else { "echec" },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{AnalysisOptions, Tolerances};
    use crate::pass::{evaluate, simulate};

    fn spec() -> SineSpec {
        SineSpec {
            freq_hz: 440.0,
            amplitude: 0.5,
            sample_rate: 48_000,
            channels: 2,
        }
    }

    fn une_passe(glitch: Option<usize>) -> Pass {
        let spec = spec();
        let recording = simulate(&spec, 48_000, 2_400, glitch);
        evaluate(
            1,
            &recording,
            &spec,
            &AnalysisOptions::default(),
            &Tolerances::default(),
            4_800,
        )
        .expect("évaluation")
    }

    #[test]
    fn virgule_decimale() {
        assert_eq!(fr(440.0234, 2), "440,02");
        assert_eq!(fr(-1.5, 1), "-1,5");
        assert_eq!(fr(2.0, 0), "2");
    }

    #[test]
    fn ligne_de_passe_ok() {
        let line = pass_lines(&une_passe(None), 10);
        assert!(line.starts_with("passe 1/10 : 440,0"), "{line}");
        assert!(line.ends_with("→ OK"), "{line}");
        assert!(!line.contains('\n'), "{line}");
    }

    #[test]
    fn ligne_de_passe_en_echec_liste_les_raisons() {
        let line = pass_lines(&une_passe(Some(24_000)), 1);
        assert!(line.contains("→ ÉCHEC"), "{line}");
        assert!(line.contains("\n    - continuité"), "{line}");
    }

    #[test]
    fn resume_et_json() {
        let passes = vec![une_passe(None)];
        let summary = summary_line(&passes);
        assert!(summary.starts_with("résumé : 1/1 passe(s) OK"), "{summary}");
        let doc = json_document(&spec(), &passes);
        assert_eq!(doc["resume"]["verdict"], "ok");
        assert_eq!(doc["resume"]["ok"], 1);
        assert_eq!(doc["sinus"]["freq_hz"], 440.0);
        assert_eq!(doc["passes"][0]["verdict"]["ok"], true);
        assert!(doc["passes"][0]["analysis"]["detected_hz"].is_number());
        assert_eq!(summary_line(&[]), "résumé : aucune passe");
        assert_eq!(json_document(&spec(), &[])["resume"]["verdict"], "echec");
    }
}
