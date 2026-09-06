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
    /// Sortie JSON.
    #[arg(long)]
    pub json: bool,
    /// Liste les endpoints et sort.
    #[arg(long)]
    pub list: bool,
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
        (self.skip_ms.max(0.0) * f64::from(self.rate) / 1000.0).round() as usize
    }

    /// Trames d'une passe.
    pub fn frames(&self) -> usize {
        (self.seconds * f64::from(self.rate)).round() as usize
    }

    /// Vrai si la capture ne doit pas être ouverte.
    pub fn capture_disabled(&self) -> bool {
        self.no_capture || self.capture.as_deref() == Some("none")
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
    fn verbe_inconnu_refuse() {
        assert!(Args::try_parse_from(["conduit-looptest", "--inconnu"]).is_err());
    }
}
