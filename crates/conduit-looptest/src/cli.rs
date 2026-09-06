//! Définition `clap` des options de `conduit-looptest`.

use clap::Parser;
use conduit_core::types::SampleRate;

use crate::analysis::{AnalysisOptions, SineSpec, Tolerances};

/// Test de boucle du pilote Conduit : joue un sinus sur un endpoint de rendu,
/// capture ce qui revient, vérifie fréquence, continuité de phase et absence de
/// trous.
#[derive(Debug, Parser)]
#[command(name = "conduit-looptest", version, about, long_about = None)]
pub struct Args {
    /// Endpoint de rendu : identifiant exact ou fragment de nom (défaut : le
    /// câble `Conduit 1`).
    #[arg(long)]
    pub render: Option<String>,
    /// Endpoint de capture : identifiant, fragment de nom, ou `none` pour ne pas
    /// capturer (défaut : le câble `Conduit 1`).
    #[arg(long)]
    pub capture: Option<String>,
    /// Fréquence du sinus, en hertz.
    #[arg(long, default_value_t = 440.0)]
    pub freq: f64,
    /// Fréquence d'échantillonnage, en hertz.
    #[arg(long, default_value_t = 48_000)]
    pub rate: u32,
    /// Nombre de canaux.
    #[arg(long, default_value_t = 2)]
    pub channels: usize,
    /// Durée d'une passe, en secondes.
    #[arg(long, default_value_t = 2.0)]
    pub seconds: f64,
    /// Trames par rappel demandées au backend.
    #[arg(long, default_value_t = 480)]
    pub block: usize,
    /// Amplitude crête du sinus (0 à 1).
    #[arg(long, default_value_t = 0.5)]
    pub amplitude: f64,
    /// Nombre de passes (le critère de la ROADMAP est 10).
    #[arg(long, default_value_t = 1)]
    pub repeat: usize,
    /// Marge jetée après la détection du signal, en millisecondes.
    #[arg(long = "skip-ms", default_value_t = 100.0)]
    pub skip_ms: f64,
    /// Borne supérieure du seuil de saut de phase, en radians.
    #[arg(long = "phase-tolerance", default_value_t = 0.25)]
    pub phase_tolerance: f64,
    /// Joue seulement, sans ouvrir la capture (équivaut à `--capture none`).
    #[arg(long = "no-capture")]
    pub no_capture: bool,
    /// Capture en écho : au lieu d'ouvrir l'endpoint de capture, prélève le
    /// mélange sur l'endpoint de `--render` lui-même, avant le pilote. Dit si le
    /// moteur audio délivre quelque chose.
    #[arg(long)]
    pub loopback: bool,
    /// Sortie JSON.
    #[arg(long)]
    pub json: bool,
    /// Liste les endpoints et sort.
    #[arg(long)]
    pub list: bool,
    /// Affiche le volume et l'état de coupure : avec `--list`, pour chaque
    /// endpoint listé ; sinon, pour ceux que la mesure va utiliser.
    #[arg(long = "show-volume")]
    pub show_volume: bool,
    /// Règle le volume maître (0 à 1) des endpoints de `--render` et `--capture`,
    /// affiche le résultat, et sort sans rien mesurer.
    #[arg(long = "set-volume", value_name = "0..1")]
    pub set_volume: Option<f32>,
    /// Rétablit le son des endpoints de `--render` et `--capture` (annule la
    /// coupure), affiche le résultat, et sort sans rien mesurer.
    #[arg(long)]
    pub unmute: bool,
    /// Test de l'outil lui-même : aucun périphérique n'est ouvert, la boucle est
    /// simulée en mémoire.
    #[arg(long = "self-test", hide = true)]
    pub self_test: bool,
    /// Avec `--self-test` : retire la trame indiquée (par défaut celle du milieu),
    /// pour prouver que le détecteur de discontinuité la voit.
    #[arg(long = "inject-glitch", hide = true, value_name = "TRAME")]
    pub inject_glitch: Option<Option<usize>>,
}

impl Args {
    /// Sinus décrit par les options.
    pub fn spec(&self) -> SineSpec {
        SineSpec {
            freq_hz: self.freq,
            amplitude: self.amplitude,
            sample_rate: self.rate,
            channels: self.channels,
        }
    }

    /// Réglages de l'analyse.
    pub fn analysis_options(&self) -> AnalysisOptions {
        AnalysisOptions {
            phase_tolerance_rad: self.phase_tolerance,
            ..AnalysisOptions::default()
        }
    }

    /// Tolérances du verdict (celles de la ROADMAP : rien de perdu, rien de
    /// dupliqué).
    pub fn tolerances(&self) -> Tolerances {
        Tolerances::default()
    }

    /// Trames de signal jetées après la détection du début.
    pub fn skip_frames(&self) -> usize {
        self.skip_frames_at(self.rate)
    }

    /// Comme [`Self::skip_frames`], mais à la fréquence réellement obtenue : en
    /// `--loopback` le flux prend le format de mixage de l'endpoint, qui n'est pas
    /// forcément `--rate`.
    pub fn skip_frames_at(&self, rate: u32) -> usize {
        (self.skip_ms.max(0.0) * f64::from(rate) / 1000.0).round() as usize
    }

    /// Trames d'une passe.
    pub fn frames(&self) -> usize {
        (self.seconds * f64::from(self.rate)).round() as usize
    }

    /// Vrai si la capture ne doit pas être ouverte.
    pub fn capture_disabled(&self) -> bool {
        self.no_capture || self.capture.as_deref() == Some("none")
    }

    /// Vrai si l'outil doit **régler** un volume au lieu de mesurer
    /// (`--set-volume`, `--unmute`).
    ///
    /// C'est une action à part entière, comme `--list` : elle applique, affiche le
    /// résultat et sort. Rien n'est joué — un réglage de volume ne doit pas avoir
    /// pour effet de bord d'émettre du son.
    pub fn adjusts_volume(&self) -> bool {
        self.set_volume.is_some() || self.unmute
    }

    /// Trame à retirer en `--self-test`, si `--inject-glitch` a été donné (sans
    /// valeur : la trame du milieu de la passe).
    pub fn glitch_frame(&self) -> Option<usize> {
        self.inject_glitch
            .map(|frame| frame.unwrap_or_else(|| self.frames() / 2))
    }

    /// Vérifie la cohérence des options ; le message est destiné à l'utilisateur.
    pub fn validate(&self) -> Result<SampleRate, String> {
        let rate = SampleRate::new(self.rate).ok_or_else(|| {
            format!(
                "--rate {} hors de [{}, {}] Hz",
                self.rate,
                SampleRate::MIN,
                SampleRate::MAX
            )
        })?;
        if self.channels == 0 || self.channels > 32 {
            return Err(format!("--channels {} hors de [1, 32]", self.channels));
        }
        if !(self.freq > 0.0 && self.freq < f64::from(self.rate) / 2.0) {
            return Err(format!(
                "--freq {} doit être dans ]0, {}[ Hz (moitié de --rate)",
                self.freq,
                f64::from(self.rate) / 2.0
            ));
        }
        if !(self.amplitude > 0.0 && self.amplitude <= 1.0) {
            return Err(format!(
                "--amplitude {} doit être dans ]0, 1]",
                self.amplitude
            ));
        }
        if !(self.seconds > 0.1 && self.seconds <= 600.0) {
            return Err(format!(
                "--seconds {} doit être dans ]0,1, 600]",
                self.seconds
            ));
        }
        if self.block == 0 || self.block > 65_536 {
            return Err(format!("--block {} hors de [1, 65536]", self.block));
        }
        if self.repeat == 0 {
            return Err("--repeat doit valoir au moins 1".to_string());
        }
        if self.phase_tolerance <= 0.0 {
            return Err("--phase-tolerance doit être strictement positive".to_string());
        }
        if let Some(volume) = self.set_volume {
            if !(0.0..=1.0).contains(&volume) {
                return Err(format!("--set-volume {volume} doit être dans [0, 1]"));
            }
        }
        if self.adjusts_volume() {
            if self.list {
                return Err(
                    "--set-volume et --unmute règlent les endpoints de --render et --capture ; \
                     --list ne fait qu'énumérer. Gardez l'un des deux (--list --show-volume \
                     affiche les volumes sans rien changer)"
                        .to_string(),
                );
            }
            if self.self_test {
                return Err(
                    "--set-volume et --unmute demandent un vrai endpoint : --self-test n'en \
                     ouvre aucun"
                        .to_string(),
                );
            }
        }
        if self.loopback {
            if self.capture.is_some() {
                return Err(
                    "--loopback prélève le mélange sur l'endpoint de --render : --capture n'a \
                     rien à désigner. Retirez-le (ou retirez --loopback pour la boucle \
                     habituelle)"
                        .to_string(),
                );
            }
            if self.no_capture {
                return Err(
                    "--loopback est une capture : --no-capture le contredit. Gardez l'un des deux"
                        .to_string(),
                );
            }
            if self.self_test {
                return Err(
                    "--loopback demande un vrai endpoint de rendu : --self-test n'en ouvre aucun"
                        .to_string(),
                );
            }
        }
        Ok(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        let mut all = vec!["conduit-looptest"];
        all.extend_from_slice(args);
        Args::try_parse_from(all).expect("analyse des options")
    }

    #[test]
    fn defauts_conformes_a_la_roadmap() {
        let a = parse(&[]);
        assert_eq!(a.freq, 440.0);
        assert_eq!(a.rate, 48_000);
        assert_eq!(a.channels, 2);
        assert_eq!(a.seconds, 2.0);
        assert_eq!(a.block, 480);
        assert_eq!(a.amplitude, 0.5);
        assert_eq!(a.repeat, 1);
        assert_eq!(a.skip_frames(), 4_800);
        assert_eq!(a.frames(), 96_000);
        assert!(!a.capture_disabled());
        assert_eq!(a.validate().unwrap(), SampleRate::HZ_48000);
    }

    #[test]
    fn capture_none_equivaut_a_no_capture() {
        assert!(parse(&["--capture", "none"]).capture_disabled());
        assert!(parse(&["--no-capture"]).capture_disabled());
    }

    #[test]
    fn options_incoherentes_refusees() {
        assert!(parse(&["--rate", "100"]).validate().is_err());
        assert!(parse(&["--channels", "0"]).validate().is_err());
        assert!(parse(&["--freq", "40000"]).validate().is_err());
        assert!(parse(&["--amplitude", "2"]).validate().is_err());
        assert!(parse(&["--seconds", "0"]).validate().is_err());
        assert!(parse(&["--block", "0"]).validate().is_err());
        assert!(parse(&["--repeat", "0"]).validate().is_err());
        assert!(parse(&["--phase-tolerance", "0"]).validate().is_err());
    }

    #[test]
    fn spec_et_options_suivent_les_arguments() {
        let a = parse(&[
            "--freq",
            "997",
            "--amplitude",
            "0.2",
            "--phase-tolerance",
            "0.1",
        ]);
        let spec = a.spec();
        assert_eq!(spec.freq_hz, 997.0);
        assert_eq!(spec.amplitude, 0.2);
        assert_eq!(a.analysis_options().phase_tolerance_rad, 0.1);
        assert_eq!(a.tolerances().max_phase_breaks, 0);
    }

    #[test]
    fn glitch_avec_ou_sans_valeur() {
        assert_eq!(parse(&[]).glitch_frame(), None);
        assert_eq!(parse(&["--inject-glitch"]).glitch_frame(), Some(48_000));
        assert_eq!(
            parse(&["--inject-glitch", "1234"]).glitch_frame(),
            Some(1_234)
        );
    }

    #[test]
    fn echo_incompatible_avec_capture_et_self_test() {
        assert!(parse(&["--loopback"]).validate().is_ok());
        for (args, attendu) in [
            (vec!["--loopback", "--capture", "Conduit 1"], "--capture"),
            (vec!["--loopback", "--capture", "none"], "--capture"),
            (vec!["--loopback", "--no-capture"], "--no-capture"),
            (vec!["--loopback", "--self-test"], "--self-test"),
        ] {
            let err = parse(&args).validate().expect_err(&format!("{args:?}"));
            assert!(err.contains(attendu), "{err}");
        }
    }

    #[test]
    fn les_trames_jetees_suivent_la_frequence_effective() {
        let a = parse(&["--skip-ms", "100"]);
        assert_eq!(a.skip_frames(), 4_800);
        assert_eq!(a.skip_frames_at(44_100), 4_410);
        assert_eq!(a.skip_frames_at(96_000), 9_600);
    }

    #[test]
    fn le_volume_a_regler_reste_dans_ses_bornes() {
        assert!(parse(&["--set-volume", "0"]).validate().is_ok());
        assert!(parse(&["--set-volume", "1"]).validate().is_ok());
        assert!(parse(&["--set-volume", "0.5"]).validate().is_ok());
        // `--set-volume=-0.1` et non `--set-volume -0.1` : clap prendrait la valeur
        // négative pour une option courte inconnue.
        for hors in ["--set-volume=-0.1", "--set-volume=1.5", "--set-volume=42"] {
            let err = parse(&[hors]).validate().expect_err(hors);
            assert!(err.contains("--set-volume"), "{err}");
        }
        assert!(parse(&["--set-volume", "nan"]).validate().is_err());
    }

    #[test]
    fn regler_le_volume_est_une_action_a_part() {
        assert!(!parse(&[]).adjusts_volume());
        assert!(!parse(&["--show-volume"]).adjusts_volume());
        assert!(parse(&["--unmute"]).adjusts_volume());
        assert!(parse(&["--set-volume", "0.5"]).adjusts_volume());
        // `--show-volume` n'est qu'un affichage : il se combine avec tout.
        assert!(parse(&["--list", "--show-volume"]).validate().is_ok());
        for (args, attendu) in [
            (vec!["--list", "--unmute"], "--list"),
            (vec!["--list", "--set-volume", "0.5"], "--list"),
            (vec!["--self-test", "--unmute"], "--self-test"),
            (vec!["--self-test", "--set-volume", "0.5"], "--self-test"),
        ] {
            let err = parse(&args).validate().expect_err(&format!("{args:?}"));
            assert!(err.contains(attendu), "{err}");
        }
    }

    #[test]
    fn verbe_inconnu_refuse() {
        assert!(Args::try_parse_from(["conduit-looptest", "--inconnu"]).is_err());
    }
}
