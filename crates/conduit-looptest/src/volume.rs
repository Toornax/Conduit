//! Ce que l'outil dit du volume d'un endpoint.
//!
//! Le relevé lui-même est propre à Windows (`conduit_backend_wasapi::
//! EndpointVolumeControl`, module `loopback`) ; **tout ce qui s'imprime est ici**,
//! sans plateforme, pour que les messages se testent partout — ils comptent autant
//! que la mesure.
//!
//! Un endpoint coupé ou à zéro rend toute la chaîne muette. C'est banal, invisible
//! depuis le pilote, et cela ressemble trait pour trait à une panne du pilote :
//! l'outil doit donc le nommer **avant** la mesure, et dire quelle option le
//! corrige.

#![forbid(unsafe_code)]

use crate::analysis::fr;

/// Volume maître et coupure d'un endpoint, tels que l'outil les affiche.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// Volume maître scalaire, 0 à 1 (la position du curseur du mélangeur).
    pub scalar: f32,
    /// Vrai si l'endpoint est coupé.
    pub muted: bool,
}

impl Level {
    /// Vrai si rien ne peut sortir de cet endpoint : coupé, ou volume nul.
    pub fn is_silent(&self) -> bool {
        self.muted || self.scalar <= 0.0
    }

    /// Volume en pourcentage entier, arrondi.
    pub fn percent(&self) -> u32 {
        (f64::from(self.scalar).clamp(0.0, 1.0) * 100.0).round() as u32
    }

    /// « 75 % (0,750), non coupé ».
    pub fn describe(&self) -> String {
        format!(
            "{} % ({}), {}",
            self.percent(),
            fr(f64::from(self.scalar), 3),
            if self.muted { "coupé" } else { "non coupé" }
        )
    }
}

/// Ce que l'outil a pu savoir du volume d'un endpoint.
///
/// « Pas de contrôle » et « illisible » sont distingués à dessein : le premier est
/// une réponse (l'endpoint n'a pas de mélangeur, son silence ne prouve rien), le
/// second est une panne de l'outil, qu'il ne faut pas maquiller en réponse.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Volume et coupure relevés.
    Known(Level),
    /// L'endpoint n'expose pas de contrôle de volume.
    NoControl,
    /// La lecture a échoué ; le message dit pourquoi.
    Unreadable(String),
}

/// Ce que l'outil a relevé sur un endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// Rôle dans la mesure, tel qu'il s'imprime : « rendu » ou « capture ».
    pub role: String,
    /// Nom convivial de l'endpoint.
    pub name: String,
    /// Le volume, ou ce qui en tient lieu.
    pub state: State,
}

impl Reading {
    /// Relevé d'un endpoint.
    pub fn new(role: impl Into<String>, name: impl Into<String>, state: State) -> Self {
        Self {
            role: role.into(),
            name: name.into(),
            state,
        }
    }

    /// Le volume, ou la raison de son absence : ce que `--show-volume` affiche.
    pub fn describe(&self) -> String {
        match &self.state {
            State::Known(level) => level.describe(),
            State::NoControl => "pas de contrôle de volume sur cet endpoint".to_string(),
            State::Unreadable(why) => format!("volume illisible ({why})"),
        }
    }

    /// « rendu « Conduit 1 » : 75 % (0,750), non coupé ».
    pub fn line(&self) -> String {
        format!("{} « {} » : {}", self.role, self.name, self.describe())
    }

    /// Avertissement à imprimer avant la mesure si cet endpoint ne peut rien
    /// laisser passer ; `None` quand il n'y a rien à dire.
    ///
    /// Le message nomme la cause **et** l'option qui la corrige : un avertissement
    /// qui laisse chercher ne vaut pas mieux que pas d'avertissement.
    pub fn warning(&self) -> Option<String> {
        let State::Known(level) = &self.state else {
            return None;
        };
        if !level.is_silent() {
            return None;
        }
        let (cause, fix) = match (level.muted, level.scalar <= 0.0) {
            (true, true) => ("coupé (muet) et volume à 0 %", "--set-volume 0.5 --unmute"),
            (true, false) => ("coupé (muet)", "--unmute"),
            _ => ("volume à 0 %", "--set-volume 0.5"),
        };
        // Le nom de l'endpoint, seule partie de longueur inconnue, finit la première
        // ligne : les suivantes sont fixes et tiennent dans 80 colonnes, la largeur
        // de la console où on lira ce message.
        Some(format!(
            "ATTENTION : {} « {} » — {cause}\n\
             \x20   Une mesure ne peut alors qu'être silencieuse, et ce serait le\n\
             \x20   volume de l'endpoint, pas le pilote. Corrigez-le sur ce même\n\
             \x20   endpoint, puis relancez :\n\
             \x20       conduit-looptest {fix} …",
            self.role, self.name
        ))
    }
}

/// Bloc « volume » des endpoints que la mesure va utiliser (`--show-volume`) ;
/// `None` s'il n'y a rien à montrer.
pub fn levels_block(readings: &[Reading]) -> Option<String> {
    let mut lines = readings.iter().map(Reading::line);
    let first = lines.next()?;
    let mut out = format!("volume  : {first}");
    for line in lines {
        out.push_str("\n          ");
        out.push_str(&line);
    }
    Some(out)
}

/// Les avertissements de tous les endpoints relevés, dans l'ordre.
pub fn warnings(readings: &[Reading]) -> Vec<String> {
    readings.iter().filter_map(Reading::warning).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(role: &str, scalar: f32, muted: bool) -> Reading {
        Reading::new(role, "Conduit 1", State::Known(Level { scalar, muted }))
    }

    #[test]
    fn le_volume_se_decrit_en_pourcentage_et_en_scalaire() {
        assert_eq!(
            Level {
                scalar: 0.75,
                muted: false
            }
            .describe(),
            "75 % (0,750), non coupé"
        );
        assert_eq!(
            Level {
                scalar: 0.0,
                muted: true
            }
            .describe(),
            "0 % (0,000), coupé"
        );
        assert_eq!(
            Level {
                scalar: 1.0,
                muted: false
            }
            .describe(),
            "100 % (1,000), non coupé"
        );
    }

    #[test]
    fn le_silence_vient_de_la_coupure_ou_du_zero() {
        let silent = |scalar, muted| Level { scalar, muted }.is_silent();
        assert!(silent(0.0, false));
        assert!(silent(1.0, true));
        assert!(!silent(0.01, false));
    }

    #[test]
    fn sans_controle_de_volume_l_outil_le_dit_et_n_avertit_pas() {
        let reading = Reading::new("rendu", "Écran HDMI", State::NoControl);
        assert!(reading.describe().contains("pas de contrôle"));
        assert_eq!(reading.warning(), None);
    }

    /// Une lecture en panne ne doit pas se faire passer pour une réponse : l'outil
    /// dit qu'il n'a pas su lire, et n'accuse pas un volume qu'il n'a pas vu.
    #[test]
    fn une_lecture_en_panne_le_dit_et_n_avertit_pas() {
        let reading = Reading::new(
            "capture",
            "Conduit 1",
            State::Unreadable("le service audio ne répond pas".to_string()),
        );
        assert!(reading.describe().contains("illisible"), "{reading:?}");
        assert!(
            reading
                .describe()
                .contains("le service audio ne répond pas"),
            "{reading:?}"
        );
        assert_eq!(reading.warning(), None);
    }

    #[test]
    fn un_endpoint_audible_n_avertit_pas() {
        assert_eq!(reading("rendu", 0.5, false).warning(), None);
        assert_eq!(reading("capture", 1.0, false).warning(), None);
    }

    #[test]
    fn chaque_avertissement_nomme_la_cause_et_l_option_qui_corrige() {
        let coupe = reading("rendu", 0.5, true).warning().expect("coupé");
        assert!(coupe.contains("coupé (muet)"), "{coupe}");
        assert!(coupe.contains("--unmute"), "{coupe}");
        assert!(!coupe.contains("--set-volume"), "{coupe}");

        let zero = reading("capture", 0.0, false).warning().expect("zéro");
        assert!(zero.contains("volume à 0 %"), "{zero}");
        assert!(zero.contains("--set-volume 0.5"), "{zero}");
        assert!(!zero.contains("--unmute"), "{zero}");

        let deux = reading("rendu", 0.0, true).warning().expect("les deux");
        assert!(deux.contains("--set-volume 0.5 --unmute"), "{deux}");

        // Le message doit rester lisible là où on le lit : une console de VM.
        for message in [&coupe, &zero, &deux] {
            for ligne in message.lines() {
                assert!(ligne.chars().count() <= 80, "ligne trop longue : {ligne}");
            }
            assert!(message.contains("pas le pilote"), "{message}");
        }
    }

    #[test]
    fn le_bloc_aligne_les_endpoints_et_disparait_s_il_est_vide() {
        assert_eq!(levels_block(&[]), None);
        let bloc = levels_block(&[reading("rendu", 1.0, false), reading("capture", 0.0, true)])
            .expect("bloc");
        let lignes: Vec<&str> = bloc.lines().collect();
        assert_eq!(lignes.len(), 2);
        assert_eq!(
            lignes[0],
            "volume  : rendu « Conduit 1 » : 100 % (1,000), non coupé"
        );
        assert_eq!(
            lignes[1],
            "          capture « Conduit 1 » : 0 % (0,000), coupé"
        );
    }

    #[test]
    fn seuls_les_endpoints_muets_avertissent() {
        let readings = [
            reading("rendu", 1.0, false),
            reading("capture", 0.0, false),
            Reading::new("rendu", "Sans mélangeur", State::NoControl),
        ];
        let warnings = warnings(&readings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("capture"), "{}", warnings[0]);
    }
}
