//! Configuration TOML (SPEC §5.7) : lecture, validation, valeurs par défaut.

use std::path::Path;

use conduit_backend::cable::{CableFormat, SampleDepth};
use conduit_core::types::{ChannelCount, Quantum, SampleRate};
use conduit_protocol::api::DriverChoice;
use serde::{Deserialize, Serialize};

use crate::autoconnect::AutoConnectRule;

/// Erreurs de configuration, avec le remède.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Lecture impossible.
    #[error("impossible de lire le fichier de configuration : {0}")]
    Io(#[from] std::io::Error),
    /// TOML invalide.
    #[error("configuration invalide : {0}")]
    Parse(#[from] toml::de::Error),
    /// Valeur hors bornes.
    #[error("configuration invalide : [{section}] {field} = {value} : {reason}")]
    Invalid {
        /// Section TOML.
        section: &'static str,
        /// Champ.
        field: &'static str,
        /// Valeur reçue.
        value: String,
        /// Explication et remède.
        reason: String,
    },
}

/// Section `[engine]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSection {
    /// Fréquence en Hz.
    pub sample_rate: u32,
    /// Quantum en trames.
    pub quantum: usize,
    /// `"auto"`, `"internal"` ou identifiant de périphérique (`"<backend>:<id>"`).
    pub driver: String,
}

impl Default for EngineSection {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            quantum: 256,
            driver: "auto".into(),
        }
    }
}

/// Entrée `[[cable]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CableSection {
    /// Numéro (1 = « Conduit 1 »).
    pub id: u32,
    /// Alias affiché dans Conduit.
    #[serde(default)]
    pub alias: Option<String>,
    /// Canaux (défaut 2).
    #[serde(default = "default_channels")]
    pub channels: u8,
    /// Fréquence en Hz : 44100, 48000 ou 96000 sous Windows.
    ///
    /// Absente, la fréquence du câble n'est **pas touchée** : une configuration qui ne
    /// parle pas d'un réglage ne doit pas le ramener au défaut à chaque démarrage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    /// Profondeur : `pcm16`, `pcm24` ou `f32`. Absente, elle n'est pas touchée non plus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
}

fn default_channels() -> u8 {
    2
}

impl CableSection {
    /// La fréquence de la section, déjà validée par [`Config::validate`].
    ///
    /// Rend `None` aussi quand la valeur est hors bornes : la validation a lieu au
    /// chargement, et cette lecture n'a pas à échouer une seconde fois.
    #[must_use]
    pub fn rate(&self) -> Option<SampleRate> {
        self.rate.and_then(SampleRate::new)
    }

    /// La profondeur de la section, déjà validée par [`Config::validate`].
    #[must_use]
    pub fn depth(&self) -> Option<SampleDepth> {
        self.depth.as_deref().and_then(|d| d.parse().ok())
    }

    /// Le format que cette section demande, sachant celui que le câble sert aujourd'hui.
    ///
    /// Les champs absents gardent leur valeur **courante** — c'est la même règle que
    /// `conduitctl cable set-format`, et pour la même raison : un fichier qui ne mentionne
    /// pas la fréquence n'a pas demandé à en changer. `channels`, lui, a toujours une
    /// valeur (2 par défaut) : la section déclare l'état voulu du câble, et c'est ce qui
    /// permet au démon de dire qu'il diverge.
    #[must_use]
    pub fn format(&self, courant: CableFormat) -> CableFormat {
        CableFormat {
            sample_rate: self.rate().unwrap_or(courant.sample_rate),
            depth: self.depth().unwrap_or(courant.depth),
            channels: ChannelCount::new(self.channels).unwrap_or(courant.channels),
        }
    }
}

/// Section `[log]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSection {
    /// Niveau (`error`, `warn`, `info`, `debug`, `trace`) ou filtre `tracing`.
    pub level: String,
    /// Écrire aussi dans un fichier tournant du répertoire de données.
    pub file: bool,
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: true,
        }
    }
}

/// Configuration complète.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Moteur.
    pub engine: EngineSection,
    /// Câbles à créer au démarrage.
    #[serde(rename = "cable")]
    pub cables: Vec<CableSection>,
    /// Règles d'auto-connexion.
    #[serde(rename = "autoconnect")]
    pub autoconnect: Vec<AutoConnectRule>,
    /// Journalisation.
    pub log: LogSection,
}

impl Config {
    /// Configuration par défaut de SPEC §1.2 : deux câbles stéréo.
    pub fn with_default_cables() -> Self {
        Self {
            // Ni `rate` ni `depth` : la configuration par défaut ne demande aucun format,
            // donc aucun démarrage de démon ne redémarre un périphérique pour rien.
            cables: vec![
                CableSection {
                    id: 1,
                    alias: None,
                    channels: 2,
                    rate: None,
                    depth: None,
                },
                CableSection {
                    id: 2,
                    alias: None,
                    channels: 2,
                    rate: None,
                    depth: None,
                },
            ],
            ..Default::default()
        }
    }

    /// Lit et valide un fichier. Un fichier absent donne la configuration par défaut
    /// (avec deux câbles).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::with_default_cables()),
            Err(e) => Err(e.into()),
        }
    }

    /// Analyse et valide un texte TOML.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Sérialise en TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("configuration sérialisable")
    }

    /// Valide les bornes.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |section, field, value: String, reason: String| ConfigError::Invalid {
            section,
            field,
            value,
            reason,
        };
        if SampleRate::new(self.engine.sample_rate).is_none() {
            return Err(invalid(
                "engine",
                "sample_rate",
                self.engine.sample_rate.to_string(),
                format!(
                    "attendu entre {} et {} Hz (44100, 48000 ou 96000 conseillés)",
                    SampleRate::MIN,
                    SampleRate::MAX
                ),
            ));
        }
        if Quantum::new(self.engine.quantum).is_none() {
            return Err(invalid(
                "engine",
                "quantum",
                self.engine.quantum.to_string(),
                format!(
                    "attendu une puissance de deux entre {} et {}",
                    Quantum::MIN,
                    Quantum::MAX
                ),
            ));
        }
        if self.engine.driver.trim().is_empty() {
            return Err(invalid(
                "engine",
                "driver",
                String::new(),
                "attendu \"auto\", \"internal\" ou \"<backend>:<identifiant>\"".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.cables {
            if c.id == 0 {
                return Err(invalid(
                    "cable",
                    "id",
                    "0".into(),
                    "les câbles sont numérotés à partir de 1".into(),
                ));
            }
            if !seen.insert(c.id) {
                return Err(invalid(
                    "cable",
                    "id",
                    c.id.to_string(),
                    "numéro utilisé deux fois".into(),
                ));
            }
            if ChannelCount::new(c.channels).is_none() {
                return Err(invalid(
                    "cable",
                    "channels",
                    c.channels.to_string(),
                    format!("attendu entre 1 et {}", ChannelCount::MAX),
                ));
            }
            // Les deux champs de format sont validés **au chargement**, comme tout le
            // reste : une faute de frappe dans un TOML doit se voir au démarrage du démon,
            // avec la ligne fautive, et non quinze secondes plus tard dans le journal.
            if let Some(hz) = c.rate {
                if SampleRate::new(hz).is_none() {
                    return Err(invalid(
                        "cable",
                        "rate",
                        hz.to_string(),
                        format!(
                            "attendu un entier de {} à {} Hz (44100, 48000 ou 96000 sous \
                             Windows, les seules que le pilote Conduit serve)",
                            SampleRate::MIN,
                            SampleRate::MAX
                        ),
                    ));
                }
            }
            if let Some(depth) = &c.depth {
                if depth.parse::<SampleDepth>().is_err() {
                    return Err(invalid(
                        "cable",
                        "depth",
                        depth.clone(),
                        "attendu pcm16, pcm24 ou f32".into(),
                    ));
                }
            }
            if let Some(a) = &c.alias {
                conduit_backend::cable::validate_cable_name(a)
                    .map_err(|e| invalid("cable", "alias", a.clone(), e.to_string()))?;
            }
        }
        for (i, r) in self.autoconnect.iter().enumerate() {
            r.validate().map_err(|reason| {
                invalid(
                    "autoconnect",
                    "match/target",
                    format!("règle {}", i + 1),
                    reason,
                )
            })?;
        }
        let level = self.log.level.trim();
        if level.is_empty() {
            return Err(invalid(
                "log",
                "level",
                String::new(),
                "attendu error, warn, info, debug ou trace".into(),
            ));
        }
        Ok(())
    }

    /// Fréquence validée.
    pub fn sample_rate(&self) -> SampleRate {
        SampleRate::new(self.engine.sample_rate).unwrap_or_default()
    }

    /// Quantum validé.
    pub fn quantum(&self) -> Quantum {
        Quantum::new(self.engine.quantum).unwrap_or_default()
    }

    /// Choix de pilote.
    pub fn driver_choice(&self) -> DriverChoice {
        match self.engine.driver.trim() {
            "auto" => DriverChoice::Auto,
            "internal" => DriverChoice::Internal,
            other => {
                // "<backend>:<id>" : on ignore le backend (un seul par démon).
                let id = other.split_once(':').map(|(_, id)| id).unwrap_or(other);
                DriverChoice::Device { id: id.into() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[engine]
sample_rate = 48000
quantum     = 256
driver      = "auto"

[[cable]]
id       = 1
alias    = "Musique"
channels = 2

[[cable]]
id       = 2
alias    = "Micro traité"
channels = 1
rate     = 96000
depth    = "pcm24"

[[autoconnect]]
match  = { cable = 1, direction = "capture" }
target = { node = "Haut-parleurs", ports = ["FL", "FR"] }

[log]
level = "info"
"#;

    #[test]
    fn spec_example_parses() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.engine.sample_rate, 48_000);
        assert_eq!(cfg.cables.len(), 2);
        assert_eq!(cfg.cables[1].channels, 1);
        assert_eq!(cfg.cables[0].alias.as_deref(), Some("Musique"));
        assert_eq!(cfg.autoconnect.len(), 1);
        assert_eq!(cfg.autoconnect[0].target.ports, vec!["FL", "FR"]);
        assert_eq!(cfg.driver_choice(), DriverChoice::Auto);
        assert_eq!(cfg.quantum().get(), 256);
        let back = Config::parse(&cfg.to_toml()).unwrap();
        assert_eq!(back, cfg);
    }

    /// Le format d'une section : ce qu'elle dit, et ce qu'elle laisse au câble.
    ///
    /// Le second point est celui qui compte : une section muette sur la fréquence ne doit
    /// pas ramener en 48 kHz un câble réglé en 96 à chaque démarrage du démon.
    #[test]
    fn le_format_d_une_section_complete_le_courant() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let courant = CableFormat::default();

        // Section muette sur le format : seuls les canaux sont déclarés.
        let un = &cfg.cables[0];
        assert_eq!(un.rate(), None);
        assert_eq!(un.depth(), None);
        assert_eq!(un.format(courant), courant, "rien n'est demandé");

        // Section qui déclare les trois champs.
        let deux = &cfg.cables[1];
        assert_eq!(deux.rate(), SampleRate::new(96_000));
        assert_eq!(deux.depth(), Some(SampleDepth::Pcm24));
        let voulu = deux.format(courant);
        assert_eq!(voulu.sample_rate, SampleRate::HZ_96000);
        assert_eq!(voulu.depth, SampleDepth::Pcm24);
        assert_eq!(voulu.channels, ChannelCount::MONO);

        // Un câble déjà en 96 kHz PCM 24 que la première section décrit : elle ne
        // demande que la stéréo, et ne touche pas au reste.
        let garde = un.format(voulu);
        assert_eq!(garde.sample_rate, SampleRate::HZ_96000);
        assert_eq!(garde.depth, SampleDepth::Pcm24);
        assert_eq!(garde.channels, ChannelCount::STEREO);
    }

    /// Un `rate` ou un `depth` seul se désérialise, et l'autre champ reste absent.
    #[test]
    fn les_champs_de_format_sont_facultatifs_un_par_un() {
        let cfg = Config::parse("[[cable]]\nid = 1\nrate = 44100").unwrap();
        assert_eq!(cfg.cables[0].rate(), SampleRate::new(44_100));
        assert_eq!(cfg.cables[0].depth(), None);
        assert_eq!(cfg.cables[0].channels, 2, "le défaut reste le défaut");

        let cfg = Config::parse("[[cable]]\nid = 1\ndepth = \"pcm16\"").unwrap();
        assert_eq!(cfg.cables[0].rate(), None);
        assert_eq!(cfg.cables[0].depth(), Some(SampleDepth::Pcm16));

        // Et l'aller-retour TOML n'invente pas les champs absents. (« rate » nu : le
        // `sample_rate` de la section [engine] en contient les lettres.)
        let toml = cfg.to_toml();
        assert!(!toml.contains("\nrate ="), "{toml}");
        assert_eq!(Config::parse(&toml).unwrap(), cfg);
    }

    #[test]
    fn defaults_and_missing_file() {
        let cfg = Config::parse("").unwrap();
        assert_eq!(cfg, Config::default());
        assert!(cfg.cables.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(cfg.cables.len(), 2, "deux câbles par défaut (F-01)");
        assert_eq!(cfg.log.level, "info");
        assert!(cfg.log.file);
    }

    #[test]
    fn invalid_values_give_actionable_messages() {
        let e = Config::parse("[engine]\nsample_rate = 12")
            .unwrap_err()
            .to_string();
        assert!(e.contains("sample_rate") && e.contains("44100"), "{e}");
        let e = Config::parse("[engine]\nquantum = 300")
            .unwrap_err()
            .to_string();
        assert!(e.contains("puissance de deux"), "{e}");
        let e = Config::parse("[[cable]]\nid = 1\nchannels = 9")
            .unwrap_err()
            .to_string();
        assert!(e.contains("channels") && e.contains("entre 1 et 8"), "{e}");
        let e = Config::parse("[[cable]]\nid = 1\n[[cable]]\nid = 1")
            .unwrap_err()
            .to_string();
        assert!(e.contains("deux fois"), "{e}");
        let e = Config::parse("[[cable]]\nid = 0").unwrap_err().to_string();
        assert!(e.contains("à partir de 1"), "{e}");
        let e = Config::parse("[[cable]]\nid = 1\nalias = \"a/b\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("caractère interdit"), "{e}");
        // Les deux champs de format : le message nomme le domaine, pas seulement le refus.
        // Le domaine du champ est celui du dépôt ; les trois fréquences du pilote Windows
        // sont **nommées** dans le message, mais c'est `conduit_helper::controle` qui les
        // fait respecter — un câble PipeWire à 22 050 Hz n'a rien d'illégal.
        let e = Config::parse("[[cable]]\nid = 1\nrate = 12")
            .unwrap_err()
            .to_string();
        assert!(e.contains("rate") && e.contains("96000"), "{e}");
        let e = Config::parse("[[cable]]\nid = 1\ndepth = \"double\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("depth") && e.contains("pcm24"), "{e}");
        let e = Config::parse("[engine]\ndriver = \"\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("auto"), "{e}");
        let e = Config::parse("[log]\nlevel = \" \"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("level"), "{e}");
        let e = Config::parse("[engine]\nsample_rate = \"quarante\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("invalide"), "{e}");
        let e = Config::parse("[[autoconnect]]\nmatch = {}\ntarget = { node = \"x\", ports = [] }")
            .unwrap_err()
            .to_string();
        assert!(e.contains("autoconnect"), "{e}");
    }

    #[test]
    fn driver_choice_parsing() {
        let mut cfg = Config::default();
        cfg.engine.driver = "internal".into();
        assert_eq!(cfg.driver_choice(), DriverChoice::Internal);
        cfg.engine.driver = "null:null:casque".into();
        assert_eq!(
            cfg.driver_choice(),
            DriverChoice::Device {
                id: "null:casque".into()
            }
        );
        cfg.engine.driver = "casque".into();
        assert_eq!(
            cfg.driver_choice(),
            DriverChoice::Device {
                id: "casque".into()
            }
        );
    }
}
