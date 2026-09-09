//! Définition `clap` des options de `conduit-looptest`.

use clap::{Parser, ValueEnum};
use conduit_core::types::SampleRate;
// Le nombre de câbles adressables vient du **contrat** partagé avec le pilote, pas
// d'un 16 recopié ici : c'est la même constante que celle qui borne `CableState::cable`.
use conduit_kmd_core::config::CABLE_MAX;

use crate::analysis::{AnalysisOptions, SineSpec, Tolerances};

/// L'état de connexion à écrire dans un câble (`--cable-set`).
///
/// Les deux valeurs sont sans accent : elles se tapent à la ligne de commande.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EtatCable {
    /// Câble connecté : ses deux endpoints apparaissent.
    Connecte,
    /// Câble déconnecté : ses deux endpoints se rangent sous « Périphériques
    /// déconnectés ».
    Deconnecte,
}

impl EtatCable {
    /// Vrai pour [`EtatCable::Connecte`].
    #[must_use]
    pub const fn connecte(self) -> bool {
        matches!(self, Self::Connecte)
    }

    /// Le mot à afficher.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecte => "connecté",
            Self::Deconnecte => "déconnecté",
        }
    }
}

/// Le côté du câble dont on ouvre le filtre de topologie (`--cable-cote`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CoteCable {
    /// `TopoRender<n>` : le côté où les applications jouent.
    Rendu,
    /// `TopoCapture<n>` : le côté où les applications lisent.
    Capture,
}

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
    /// Affiche le volume, l'état de coupure et la plage en décibels (minimum,
    /// maximum, pas) : avec `--list`, pour chaque endpoint listé ; sinon, pour
    /// ceux que la mesure va utiliser. Ne fait que lire, n'ouvre aucun flux.
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
    /// Lit et affiche l'état des seize câbles par le jeu de propriétés KS privé du
    /// pilote, plus la version du contrat qu'il sert. Ne change rien.
    #[arg(long = "cable-etat")]
    pub cable_etat: bool,
    /// Lit et affiche les compteurs de la boucle locale (propriété
    /// `KSPROPERTY_CONDUIT_COUNTERS`) : ticks, trames copiées, silences par cause,
    /// ticks jetés, débordements. Les seize câbles par défaut, ou le seul câble de
    /// `--cable N`. C'est le seul moyen de relever ces compteurs **sans débogueur
    /// noyau**, dont l'attachement fausse la mesure de transport. Ne change rien et
    /// n'ouvre aucun flux.
    #[arg(long = "cable-compteurs")]
    pub cable_compteurs: bool,
    /// Affiche l'état de SeLoadDriverPrivilege dans le jeton de ce processus (absent /
    /// présent mais désactivé / actif) et sort. N'écrit rien, n'arme rien, et ne
    /// demande ni le pilote ni un câble : c'est le diagnostic qui sépare « mauvais
    /// compte » de « bogue du pilote » quand une écriture est refusée en 1314.
    #[arg(long = "cable-privilege")]
    pub cable_privilege: bool,
    /// Câble visé par `--cable-set` et `--cable-invalide` : son numéro affiché, de 1
    /// à 16 (« Conduit 1 » est le câble 1).
    #[arg(long = "cable", value_name = "1..16")]
    pub cable: Option<u32>,
    /// Écrit l'état de connexion du câble `--cable` (propriété
    /// `KSPROPERTY_CONDUIT_CABLE_STATE`). L'état est affiché avant et après.
    #[arg(long = "cable-set", value_name = "connecte|deconnecte")]
    pub cable_set: Option<EtatCable>,
    /// Avec `--cable-set` : chronomètre le délai entre l'écriture et l'apparition (ou
    /// la disparition) des endpoints MMDevice du câble. C'est le critère F-01,
    /// « endpoint visible en moins d'une seconde, sans PnP ».
    #[arg(long = "cable-chrono")]
    pub cable_chrono: bool,
    /// Attente maximale du chronomètre, en millisecondes.
    #[arg(
        long = "cable-chrono-max-ms",
        default_value_t = 3_000,
        value_name = "MS"
    )]
    pub cable_chrono_max_ms: u64,
    /// Envoie au câble `--cable` la batterie d'entrées **volontairement invalides**
    /// (15 octets, 17 octets, champ réservé non nul, connected = 2, index d'un autre
    /// câble, index hors domaine) et affiche le code d'erreur Win32 de chaque refus.
    #[arg(long = "cable-invalide")]
    pub cable_invalide: bool,
    /// Côté du câble dont on ouvre le filtre de topologie : les deux portent la même
    /// propriété, en changer sert à vérifier qu'ils s'accordent.
    #[arg(
        long = "cable-cote",
        default_value = "rendu",
        value_name = "rendu|capture"
    )]
    pub cable_cote: CoteCable,
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

    /// Vrai si l'outil doit parler au **jeu de propriétés KS privé** du pilote au
    /// lieu de mesurer une boucle (`--cable-etat`, `--cable-compteurs`, `--cable-set`,
    /// `--cable-invalide`), ou simplement relever l'état du privilège d'écriture
    /// (`--cable-privilege`).
    ///
    /// C'est une action à part entière, comme `--list` et `--set-volume` : elle
    /// s'exécute, affiche son compte rendu et sort. Aucun flux n'est ouvert, aucun
    /// son n'est émis — configurer un câble ne doit pas avoir cet effet de bord.
    ///
    /// Les cinq se combinent dans un seul appel, et s'exécutent dans cet ordre :
    /// état du privilège, lecture de l'état des câbles, relevé des compteurs,
    /// écriture, batterie d'entrées invalides.
    pub fn controls_cable(&self) -> bool {
        self.cable_privilege
            || self.cable_etat
            || self.cable_compteurs
            || self.cable_set.is_some()
            || self.cable_invalide
    }

    /// Vrai si l'action demandée va **écrire** sur le pilote (`--cable-set`,
    /// `--cable-invalide`) : c'est ce qui décide s'il faut armer le privilège.
    pub fn writes_cable(&self) -> bool {
        self.cable_set.is_some() || self.cable_invalide
    }

    /// Le numéro de câble visé par `--cable-set` et `--cable-invalide`, une fois
    /// [`Self::validate`] passé.
    pub fn cable_vise(&self) -> Option<u32> {
        self.cable
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
        self.validate_cable()?;
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

    /// Cohérence des options du jeu de propriétés (`--cable-*`).
    ///
    /// Séparée de [`Self::validate`] pour rester lisible : ces options forment une
    /// action à part, qui n'a rien à voir avec la mesure de boucle.
    fn validate_cable(&self) -> Result<(), String> {
        if let Some(cable) = self.cable {
            if cable == 0 || cable > CABLE_MAX {
                return Err(format!(
                    "--cable {cable} hors de [1, {CABLE_MAX}] : « Conduit 1 » est le câble 1"
                ));
            }
        }
        // Le câble n'a **pas** de défaut pour les actions qui écrivent : déconnecter
        // « Conduit 1 » par omission serait la mauvaise surprise à ne pas offrir.
        if self.cable.is_none() && (self.cable_set.is_some() || self.cable_invalide) {
            return Err(
                "--cable-set et --cable-invalide visent un câble précis : ajoutez --cable N \
                 (1 à 16). --cable-etat, lui, les lit tous."
                    .to_string(),
            );
        }
        if self.cable_chrono && self.cable_set.is_none() {
            return Err(
                "--cable-chrono mesure le délai entre une écriture et l'endpoint qui suit : \
                 il demande --cable-set connecte ou --cable-set deconnecte"
                    .to_string(),
            );
        }
        if self.cable_chrono_max_ms == 0 || self.cable_chrono_max_ms > 60_000 {
            return Err(format!(
                "--cable-chrono-max-ms {} hors de [1, 60000]",
                self.cable_chrono_max_ms
            ));
        }
        if !self.controls_cable() {
            return Ok(());
        }
        for (interdit, option) in [
            (self.list, "--list"),
            (self.self_test, "--self-test"),
            (self.adjusts_volume(), "--set-volume / --unmute"),
            (self.loopback, "--loopback"),
        ] {
            if interdit {
                return Err(format!(
                    "les options --cable-* parlent au jeu de propriétés du pilote, affichent \
                     leur compte rendu et sortent : {option} fait autre chose. Gardez l'un \
                     des deux."
                ));
            }
        }
        Ok(())
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
    fn les_options_de_cable_forment_une_action_a_part() {
        assert!(!parse(&[]).controls_cable());
        assert!(parse(&["--cable-etat"]).controls_cable());
        assert!(parse(&["--cable-privilege"]).controls_cable());
        assert!(parse(&["--cable", "3", "--cable-set", "connecte"]).controls_cable());
        assert!(parse(&["--cable", "3", "--cable-invalide"]).controls_cable());
        assert!(parse(&["--cable-compteurs"]).controls_cable());
        // Les cinq se combinent dans un seul appel.
        let tout = parse(&[
            "--cable-privilege",
            "--cable-etat",
            "--cable-compteurs",
            "--cable",
            "2",
            "--cable-set",
            "deconnecte",
            "--cable-invalide",
        ]);
        assert!(tout.validate().is_ok());
        assert_eq!(tout.cable_vise(), Some(2));
        assert_eq!(tout.cable_set, Some(EtatCable::Deconnecte));
        assert!(tout.cable_compteurs);
    }

    /// `--cable-compteurs` est une **lecture** : il se suffit à lui-même, n'exige pas
    /// `--cable N`, n'arme aucun privilège, et exclut les actions qui font autre chose.
    ///
    /// La première moitié compte : exiger un câble ferait du relevé des seize une
    /// commande à écrire seize fois, et c'est le relevé complet qu'on veut après une
    /// passe.
    #[test]
    fn le_releve_des_compteurs_se_demande_seul_et_ne_prend_pas_le_privilege() {
        let seul = parse(&["--cable-compteurs"]);
        assert!(seul.validate().is_ok());
        assert_eq!(seul.cable_vise(), None);
        assert!(!seul.writes_cable());

        // Restreint à un câble quand on le demande, sans que la validation l'exige.
        let un = parse(&["--cable", "3", "--cable-compteurs"]);
        assert!(un.validate().is_ok());
        assert_eq!(un.cable, Some(3));

        for (args, attendu) in [
            (vec!["--cable-compteurs", "--list"], "--list"),
            (vec!["--cable-compteurs", "--self-test"], "--self-test"),
            (vec!["--cable-compteurs", "--loopback"], "--loopback"),
        ] {
            let err = parse(&args).validate().expect_err(&format!("{args:?}"));
            assert!(err.contains(attendu), "{err}");
        }
    }

    /// Seules les actions qui **écrivent** demandent d'armer le privilège : une
    /// lecture ne doit pas toucher au jeton du processus.
    #[test]
    fn seules_les_ecritures_demandent_le_privilege() {
        assert!(!parse(&[]).writes_cable());
        assert!(!parse(&["--cable-etat"]).writes_cable());
        assert!(!parse(&["--cable-privilege"]).writes_cable());
        assert!(!parse(&["--cable-compteurs"]).writes_cable());
        assert!(parse(&["--cable", "3", "--cable-set", "connecte"]).writes_cable());
        assert!(parse(&["--cable", "3", "--cable-invalide"]).writes_cable());
    }

    /// `--cable-privilege` ne vise aucun câble et n'écrit rien : il se suffit à
    /// lui-même, contrairement à `--cable-set`.
    #[test]
    fn l_etat_du_privilege_se_demande_seul() {
        let seul = parse(&["--cable-privilege"]);
        assert!(seul.validate().is_ok());
        assert_eq!(seul.cable_vise(), None);
        assert!(seul.cable_privilege);
        // Il reste une action `--cable-*` : il exclut les autres actions de l'outil.
        for (args, attendu) in [
            (vec!["--cable-privilege", "--list"], "--list"),
            (vec!["--cable-privilege", "--self-test"], "--self-test"),
            (vec!["--cable-privilege", "--loopback"], "--loopback"),
        ] {
            let err = parse(&args).validate().expect_err(&format!("{args:?}"));
            assert!(err.contains(attendu), "{err}");
        }
    }

    #[test]
    fn le_numero_de_cable_reste_dans_le_contrat() {
        assert!(parse(&["--cable", "1", "--cable-invalide"])
            .validate()
            .is_ok());
        assert!(parse(&["--cable", "16", "--cable-invalide"])
            .validate()
            .is_ok());
        for hors in ["0", "17", "99"] {
            let err = parse(&["--cable", hors, "--cable-invalide"])
                .validate()
                .expect_err(hors);
            assert!(err.contains("--cable"), "{err}");
        }
        // Écrire sans dire quel câble : refusé plutôt que retombé sur « Conduit 1 ».
        for args in [vec!["--cable-set", "deconnecte"], vec!["--cable-invalide"]] {
            let err = parse(&args).validate().expect_err(&format!("{args:?}"));
            assert!(err.contains("--cable N"), "{err}");
        }
        // Lire les seize, en revanche, ne vise personne.
        assert!(parse(&["--cable-etat"]).validate().is_ok());
    }

    #[test]
    fn le_chronometre_demande_une_ecriture() {
        let err = parse(&["--cable-etat", "--cable-chrono"])
            .validate()
            .expect_err("chrono sans écriture");
        assert!(err.contains("--cable-set"), "{err}");
        assert!(
            parse(&["--cable", "1", "--cable-set", "connecte", "--cable-chrono"])
                .validate()
                .is_ok()
        );
        for hors in ["0", "60001"] {
            let err = parse(&["--cable-etat", "--cable-chrono-max-ms", hors])
                .validate()
                .expect_err(hors);
            assert!(err.contains("--cable-chrono-max-ms"), "{err}");
        }
    }

    #[test]
    fn les_options_de_cable_excluent_les_autres_actions() {
        for (args, attendu) in [
            (vec!["--cable-etat", "--list"], "--list"),
            (vec!["--cable-etat", "--self-test"], "--self-test"),
            (vec!["--cable-etat", "--unmute"], "--set-volume"),
            (vec!["--cable-etat", "--set-volume", "0.5"], "--set-volume"),
            (vec!["--cable-etat", "--loopback"], "--loopback"),
        ] {
            let err = parse(&args).validate().expect_err(&format!("{args:?}"));
            assert!(err.contains(attendu), "{err}");
        }
    }

    #[test]
    fn le_cote_du_cable_a_un_defaut() {
        assert_eq!(parse(&[]).cable_cote, CoteCable::Rendu);
        assert_eq!(
            parse(&["--cable-cote", "capture"]).cable_cote,
            CoteCable::Capture
        );
        assert!(Args::try_parse_from(["conduit-looptest", "--cable-cote", "les-deux"]).is_err());
        assert!(EtatCable::Connecte.connecte());
        assert!(!EtatCable::Deconnecte.connecte());
        assert_eq!(EtatCable::Connecte.label(), "connecté");
        assert_eq!(EtatCable::Deconnecte.label(), "déconnecté");
    }

    #[test]
    fn verbe_inconnu_refuse() {
        assert!(Args::try_parse_from(["conduit-looptest", "--inconnu"]).is_err());
    }
}
