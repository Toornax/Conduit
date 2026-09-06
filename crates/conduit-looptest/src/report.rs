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

/// Ce que signifie une capture en écho **qui entend** le sinus (`--loopback`).
///
/// C'est là toute la valeur du mode : le résultat brut ne vaut que par ce qu'il
/// permet de conclure, et la conclusion porte sur la moitié de la chaîne qu'on
/// n'était pas en train de regarder.
pub fn loopback_heard() -> String {
    "Ce que cela signifie : le moteur audio de Windows délivre bien un mélange vers\n\
     cet endpoint de rendu. La moitié « application → moteur audio » de la chaîne\n\
     est donc hors de cause. Si la capture du câble n'entend toujours rien, le\n\
     défaut est en aval de ce point : l'échange de données du pilote (copie\n\
     rendu → capture, positions du tampon cyclique, avance de copie).\n\
     À nuancer : l'écho prélève le mélange, donc aussi ce que jouent les autres\n\
     applications, et le volume de l'endpoint s'y applique — une amplitude basse\n\
     ne condamne rien à elle seule."
        .to_string()
}

/// Ce que signifie une capture en écho **silencieuse** (`--loopback`).
pub fn loopback_silent() -> String {
    "Ce que cela signifie : rien n'arrive jusqu'à cet endpoint de rendu. Le pilote\n\
     est hors de cause — il ne peut pas copier ce que le moteur audio ne lui donne\n\
     pas. Le défaut est en amont, à chercher dans cet ordre : volume de l'endpoint\n\
     et du mélangeur (muet ?), endpoint désactivé ou débranché, format par défaut\n\
     du périphérique (Paramètres > Son > Propriétés > Avancé), un autre programme\n\
     qui tient le périphérique en mode exclusif, ou le service audio en peine.\n\
     Vérification utile : relancer sans --loopback sur une vraie carte son, pour\n\
     voir si le rendu sort ailleurs."
        .to_string()
}

/// La **session Windows** du processus, et ce qu'elle implique pour la mesure.
///
/// À imprimer avec tout diagnostic de silence : un test lancé dans la session des
/// services (session 0 — un service, une tâche planifiée « même si l'utilisateur
/// n'est pas connecté », un agent d'exécution à distance) n'a pas l'audio de
/// l'utilisateur. Il ne mesure alors rien du tout, et tout ce qu'il conclurait du
/// pilote serait faux. C'est un piège coûteux, et il ne se voit nulle part ailleurs
/// dans la sortie.
///
/// `session` est `None` quand Windows a refusé de dire la session, et sur les
/// autres systèmes.
pub fn session_note(session: Option<u32>) -> String {
    match session {
        Some(0) => "session Windows : 0 — c'est la session des SERVICES, et c'est\n\
                    probablement toute l'explication. Aucun périphérique audio de\n\
                    l'utilisateur n'y est visible et rien de ce qui y est joué ne sort :\n\
                    la mesure n'y a aucun sens. Relancez l'outil depuis une session\n\
                    ouverte à l'écran — pour une tâche planifiée, « exécuter seulement\n\
                    si l'utilisateur est connecté »."
            .to_string(),
        Some(id) => format!(
            "session Windows : {id} — une session d'utilisateur, l'audio y existe. Ce\n\
             n'est donc pas la session qui explique le silence."
        ),
        None => "session Windows : inconnue. Vérifiez à la main que l'outil tourne dans\n\
                 une session d'utilisateur : la session 0, celle des services, n'a pas\n\
                 l'audio de l'utilisateur et la mesure n'y aurait aucun sens."
            .to_string(),
    }
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
    fn les_deux_lectures_de_l_echo_nomment_le_coupable() {
        let entendu = loopback_heard();
        assert!(entendu.contains("délivre bien"), "{entendu}");
        assert!(entendu.contains("pilote"), "{entendu}");
        let silence = loopback_silent();
        assert!(silence.contains("rien n'arrive"), "{silence}");
        assert!(silence.contains("hors de cause"), "{silence}");
        assert!(silence.contains("volume"), "{silence}");
        // Aucune ligne trop longue pour un terminal étroit : le message doit rester
        // lisible là où on l'utilise, une console de VM.
        for texte in [&entendu, &silence] {
            for ligne in texte.lines() {
                assert!(ligne.chars().count() <= 80, "ligne trop longue : {ligne}");
            }
        }
    }

    #[test]
    fn la_session_des_services_est_denoncee_les_autres_disculpees() {
        let services = session_note(Some(0));
        assert!(services.contains("SERVICES"), "{services}");
        assert!(services.contains("aucun sens"), "{services}");

        let utilisateur = session_note(Some(1));
        assert!(
            utilisateur.starts_with("session Windows : 1"),
            "{utilisateur}"
        );
        assert!(!utilisateur.contains("SERVICES"), "{utilisateur}");
        assert!(
            utilisateur.contains("n'est donc pas la session"),
            "{utilisateur}"
        );

        let inconnue = session_note(None);
        assert!(inconnue.contains("inconnue"), "{inconnue}");
        assert!(inconnue.contains("session 0"), "{inconnue}");

        for texte in [&services, &utilisateur, &inconnue] {
            for ligne in texte.lines() {
                assert!(ligne.chars().count() <= 80, "ligne trop longue : {ligne}");
            }
        }
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
