//! Contrôle des câbles virtuels de la plateforme.

use core::fmt;
use core::str::FromStr;

use conduit_core::types::{ChannelCount, SampleRate};

use crate::device::DeviceId;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Identifiant d'un câble (numéro stable, 1 = « Conduit 1 »).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableId(pub u32);

impl fmt::Display for CableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Conduit {}", self.0)
    }
}

// ---------------------------------------------------------------------------------
// Le format d'un câble, portable.
// ---------------------------------------------------------------------------------

/// Profondeur d'échantillon d'un câble.
///
/// # Pourquoi un type d'ici et non `conduit_kmd_core::ring::SampleFormat`
///
/// Ce crate est la **couche partagée** des trois plateformes : il porte le trait
/// [`CableControl`] que servent le dorsal WASAPI, le dorsal PipeWire et le dorsal
/// CoreAudio, et il ne dépend pas — et ne doit pas dépendre — de `conduit-kmd-core`, qui
/// est le contrat d'**un** pilote Windows. Un câble PipeWire a une profondeur sans que
/// `CableFormat<n>` existe nulle part sur la machine.
///
/// La traduction vers l'encodage `REG_DWORD` du pilote vit donc là où les deux mondes se
/// touchent, et nulle part ailleurs : `conduit_helper::controle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "snake_case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SampleDepth {
    /// PCM signé 16 bits.
    Pcm16,
    /// PCM signé 24 bits (conteneur de trois octets).
    Pcm24,
    /// Flottant 32 bits, plage nominale [−1, 1]. Le défaut du moteur audio de Windows.
    #[default]
    F32,
}

impl SampleDepth {
    /// Les trois profondeurs, dans l'ordre croissant de précision.
    pub const ALL: [Self; 3] = [Self::Pcm16, Self::Pcm24, Self::F32];

    /// Le jeton que [`FromStr`] accepte et que la CLI affiche : `pcm16`, `pcm24`, `f32`.
    #[must_use]
    pub const fn jeton(self) -> &'static str {
        match self {
            Self::Pcm16 => "pcm16",
            Self::Pcm24 => "pcm24",
            Self::F32 => "f32",
        }
    }
}

impl fmt::Display for SampleDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pcm16 => "PCM 16",
            Self::Pcm24 => "PCM 24",
            Self::F32 => "float 32",
        })
    }
}

/// Ce qu'un texte de format n'a pas su donner. Chaque variante porte le **texte reçu**,
/// pour que le message dise ce qui a été lu et pas seulement ce qui était attendu.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CableFormatParseError {
    /// La forme générale n'est pas `fréquence:profondeur:canaux`.
    #[error(
        "format « {0} » mal formé : attendu « fréquence:profondeur:canaux », \
         par exemple « 48000:f32:2 »"
    )]
    Forme(String),
    /// La fréquence n'est pas un entier, ou elle est hors des bornes du dépôt.
    #[error(
        "fréquence « {0} » invalide : attendu un entier de {min} à {max} Hz \
         (44100, 48000 ou 96000 sous Windows)",
        min = SampleRate::MIN,
        max = SampleRate::MAX
    )]
    Frequence(String),
    /// La profondeur n'est aucun des trois jetons.
    #[error("profondeur « {0} » inconnue : attendu pcm16, pcm24 ou f32")]
    Profondeur(String),
    /// Les canaux ne sont pas un entier de 1 à 8.
    #[error("canaux « {0} » invalides : attendu un entier de 1 à {max}", max = ChannelCount::MAX)]
    Canaux(String),
}

impl FromStr for SampleDepth {
    type Err = CableFormatParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // La casse est ignorée : « F32 » et « f32 » désignent la même chose pour qui tape
        // une commande, et rien ne se gagne à refuser la seconde.
        let jeton = s.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|d| d.jeton() == jeton)
            .ok_or_else(|| CableFormatParseError::Profondeur(s.trim().to_owned()))
    }
}

/// Le format d'un câble : fréquence, profondeur, canaux.
///
/// C'est **le** type que la chaîne client transporte, du `conduitctl cable set-format` au
/// contrôle de la plateforme. Sous Windows il finit encodé dans le `REG_DWORD`
/// `CableFormat<n>` du devnode ; ailleurs, il décrit simplement les endpoints que le
/// dorsal publie.
///
/// Le texte de [`FromStr`] est `fréquence:profondeur:canaux` — `48000:f32:2` —, le même
/// pour la ligne de commande et pour le TOML, parce qu'il n'y a aucune raison qu'un
/// utilisateur apprenne deux orthographes du même réglage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableFormat {
    /// Fréquence d'échantillonnage.
    pub sample_rate: SampleRate,
    /// Profondeur d'échantillon.
    pub depth: SampleDepth,
    /// Nombre de canaux.
    pub channels: ChannelCount,
}

impl Default for CableFormat {
    /// **48 kHz, float 32, stéréo** : le format d'un câble neuf.
    ///
    /// La même valeur que `conduit_kmd_core::config::CABLE_FORMAT_DEFAULT`, que l'INF
    /// écrit pour les seize câbles à l'installation — mais écrite ici avec les types
    /// d'ici, puisque ce crate ne voit pas celui-là. Le test
    /// `conduit_helper::controle::le_defaut_portable_est_celui_du_pilote` tient les deux
    /// d'accord.
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::HZ_48000,
            depth: SampleDepth::F32,
            channels: ChannelCount::STEREO,
        }
    }
}

impl fmt::Display for CableFormat {
    /// « 48 kHz float 32, 2 canaux ».
    ///
    /// Les canaux sont écrits en chiffres et non par le `Display` de [`ChannelCount`] —
    /// qui dirait « stéréo » — parce que c'est un **format** qu'on décrit : à côté d'une
    /// fréquence et d'une profondeur, un nombre se compare, « stéréo » se traduit.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.channels.get();
        let mot = if n == 1 { "canal" } else { "canaux" };
        write!(f, "{} {}, {n} {mot}", self.sample_rate, self.depth)
    }
}

impl FromStr for CableFormat {
    type Err = CableFormatParseError;

    /// `48000:f32:2`.
    ///
    /// Les bornes sont celles du dépôt ([`SampleRate::MIN`]..=[`SampleRate::MAX`], 1 à 8
    /// canaux), **pas** celles du pilote Windows : ce crate est la couche portable, et un
    /// câble PipeWire à 22 050 Hz n'a rien d'illégal. Ce qui restreint aux trois
    /// fréquences du contrat KS, c'est la conversion de `conduit_helper::controle`, qui
    /// refuse avec un message nommant les trois — et la CLI, qui les annonce dans son
    /// aide.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let texte = s.trim();
        let mut champs = texte.split(':');
        let (Some(rate), Some(depth), Some(channels), None) =
            (champs.next(), champs.next(), champs.next(), champs.next())
        else {
            return Err(CableFormatParseError::Forme(texte.to_owned()));
        };
        let sample_rate = rate
            .trim()
            .parse::<u32>()
            .ok()
            .and_then(SampleRate::new)
            .ok_or_else(|| CableFormatParseError::Frequence(rate.trim().to_owned()))?;
        let depth = depth.parse::<SampleDepth>()?;
        let channels = channels
            .trim()
            .parse::<u8>()
            .ok()
            .and_then(ChannelCount::new)
            .ok_or_else(|| CableFormatParseError::Canaux(channels.trim().to_owned()))?;
        Ok(Self {
            sample_rate,
            depth,
            channels,
        })
    }
}

/// Demande de création ou de modification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableSpec {
    /// Nom OS souhaité (`None` = `Conduit N`).
    pub name: Option<String>,
    /// Canaux.
    ///
    /// **Ignoré quand [`Self::format`] est `Some`** : le format porte déjà ses canaux, et
    /// les faire dire par deux champs ferait une contradiction possible. C'est
    /// `format.channels` qui compte alors.
    pub channels: ChannelCount,
    /// Format à appliquer **avant** l'activation, ou `None` pour prendre le câble tel
    /// qu'il est.
    ///
    /// `None` est le comportement d'avant M1b-05 : on connecte le câble sans toucher à sa
    /// clé matérielle. `Some` demande la seule séquence qui produise un endpoint au bon
    /// format — écrire `CableFormat<n>`, redémarrer le devnode, **puis** activer — parce
    /// que le format d'un endpoint audio est figé à sa création et qu'un redémarrage du
    /// périphérique ne le déplace pas (mesuré en M1b-05).
    #[cfg_attr(feature = "serde", serde(default))]
    pub format: Option<CableFormat>,
}

impl Default for CableSpec {
    fn default() -> Self {
        Self {
            name: None,
            channels: ChannelCount::STEREO,
            format: None,
        }
    }
}

/// Description d'un câble existant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableInfo {
    /// Identifiant.
    pub id: CableId,
    /// Nom OS.
    pub name: String,
    /// Canaux — **raccourci sur `format.channels`**, jamais une seconde vérité.
    ///
    /// Le champ précède [`Self::format`] et lui survit le temps que ses appelants
    /// migrent : la GUI, les règles d'auto-connexion et la table de `conduitctl cable
    /// list` le lisent encore. Les implémentations du trait doivent le tenir **égal** à
    /// `format.channels` ; les tests l'assertent, et la dette « supprimer `channels` » est
    /// ouverte.
    pub channels: ChannelCount,
    /// Format servi par ce câble.
    ///
    /// Non optionnel : depuis M1b-05 chaque câble a le sien, et depuis le lot A1 la
    /// réponse `lister` du service porte la table des seize — le format est donc
    /// **toujours** connu. Un mot nul (clé matérielle illisible, câble hors réserve) se
    /// replie sur [`CableFormat::default`], comme les canaux se repliaient auparavant,
    /// mais c'est désormais le cas exceptionnel et non l'ordinaire.
    pub format: CableFormat,
    /// Vrai si actif (visible des applications).
    pub active: bool,
    /// Périphérique de rendu (les applications y jouent).
    pub render: DeviceId,
    /// Périphérique de capture (les applications y lisent).
    pub capture: DeviceId,
}

/// Erreurs de contrôle des câbles. Les messages disent quoi faire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CableError {
    /// Nombre maximal de câbles atteint.
    #[error("limite de {max} câbles atteinte : supprimez un câble avant d'en créer un autre")]
    LimitReached {
        /// Limite de la plateforme.
        max: usize,
    },
    /// Câble inconnu.
    #[error("câble inconnu : {0}")]
    NotFound(CableId),
    /// Droits insuffisants.
    #[error("droits insuffisants : {0}")]
    PermissionDenied(String),
    /// Opération non supportée par cette plateforme.
    #[error("non supporté sur cette plateforme : {0}")]
    Unsupported(String),
    /// Nom refusé.
    #[error("nom invalide : {0}")]
    InvalidName(String),
    /// Le service ou le démon dont dépend le contrôle des câbles ne répond pas.
    ///
    /// Distinct de [`Self::Driver`] : le pilote n'a rien refusé, on n'a pas pu lui
    /// parler. Sous Windows c'est le service d'assistance `ConduitHelper` qui manque
    /// (M1b-20) ; le message dit alors quoi installer.
    #[error("service indisponible : {0}")]
    Unavailable(String),
    /// Erreur du pilote ou de l'OS.
    #[error("erreur du pilote : {0}")]
    Driver(String),
}

/// Longueur maximale d'un nom de câble (caractères).
pub const MAX_CABLE_NAME_LEN: usize = 64;

/// Valide un nom de câble : non vide, ≤ [`MAX_CABLE_NAME_LEN`] caractères,
/// imprimable, sans caractère de contrôle ni `/ \ : * ? " < > |`.
pub fn validate_cable_name(name: &str) -> Result<(), CableError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CableError::InvalidName("le nom est vide".into()));
    }
    if name.chars().count() > MAX_CABLE_NAME_LEN {
        return Err(CableError::InvalidName(format!(
            "plus de {MAX_CABLE_NAME_LEN} caractères"
        )));
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_control() || "/\\:*?\"<>|".contains(*c))
    {
        return Err(CableError::InvalidName(format!("caractère interdit {c:?}")));
    }
    Ok(())
}

/// Contrôle des câbles d'une plateforme.
pub trait CableControl: fmt::Debug {
    /// Nombre maximal de câbles.
    fn max_cables(&self) -> usize;

    /// Câbles existants, actifs ou non.
    fn list(&self) -> Result<Vec<CableInfo>, CableError>;

    /// Crée (ou active) un câble.
    fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError>;

    /// Supprime (ou désactive) un câble.
    fn remove(&mut self, id: CableId) -> Result<(), CableError>;

    /// Change le nombre de canaux (peut réactiver le câble : court silence).
    fn set_channels(
        &mut self,
        id: CableId,
        channels: ChannelCount,
    ) -> Result<CableInfo, CableError>;

    /// Change le **format** du câble : fréquence, profondeur, canaux.
    ///
    /// # Pas d'implémentation par défaut, et c'est voulu
    ///
    /// Un défaut qui rendrait [`CableError::Unsupported`] ferait taire le compilateur le
    /// jour où une plateforme neuve arrive, et l'utilisateur découvrirait le trou à
    /// l'usage. Chaque implémentation doit **se prononcer** : appliquer, ou refuser en
    /// disant pourquoi.
    ///
    /// # Le câble doit être inactif
    ///
    /// Le format d'un endpoint audio est figé à sa création (mesuré en M1b-05, sous
    /// Windows) : régler celui d'un câble connecté n'aurait aucun effet visible. Les
    /// implémentations refusent alors en nommant la séquence — désactiver, régler,
    /// réactiver.
    fn set_format(&mut self, id: CableId, format: CableFormat) -> Result<CableInfo, CableError>;

    /// Renomme côté OS.
    fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError>;

    /// Câble par identifiant.
    fn get(&self, id: CableId) -> Result<CableInfo, CableError> {
        self.list()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or(CableError::NotFound(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(validate_cable_name("Musique").is_ok());
        assert!(validate_cable_name("Conduit 1").is_ok());
        assert!(matches!(
            validate_cable_name("  "),
            Err(CableError::InvalidName(_))
        ));
        assert!(matches!(
            validate_cable_name("a/b"),
            Err(CableError::InvalidName(_))
        ));
        assert!(matches!(
            validate_cable_name("a\nb"),
            Err(CableError::InvalidName(_))
        ));
        let long = "x".repeat(MAX_CABLE_NAME_LEN + 1);
        assert!(matches!(
            validate_cable_name(&long),
            Err(CableError::InvalidName(_))
        ));
        assert!(validate_cable_name(&"é".repeat(MAX_CABLE_NAME_LEN)).is_ok());
    }

    #[test]
    fn display_and_errors() {
        assert_eq!(CableId(3).to_string(), "Conduit 3");
        assert!(CableError::LimitReached { max: 16 }
            .to_string()
            .contains("16"));
        assert!(CableError::NotFound(CableId(9))
            .to_string()
            .contains("Conduit 9"));
        // Un service absent ne se confond pas avec un refus du pilote : ce sont deux
        // conduites différentes pour l'utilisateur (installer, ou rapporter un bogue).
        let indisponible = CableError::Unavailable("le service n'est pas démarré".into());
        assert!(indisponible.to_string().starts_with("service indisponible"));
        assert_ne!(
            indisponible,
            CableError::Driver("le service n'est pas démarré".into())
        );
        assert_eq!(CableSpec::default().channels, ChannelCount::STEREO);
        // Un `CableSpec` neuf ne demande aucun format : c'est le comportement d'avant
        // M1b-05, celui qui connecte le câble sans toucher à sa clé matérielle.
        assert_eq!(CableSpec::default().format, None);
    }

    /// Le défaut portable est celui qu'on annonce partout : 48 kHz, float 32, stéréo.
    #[test]
    fn le_defaut_est_48k_float32_stereo() {
        let d = CableFormat::default();
        assert_eq!(d.sample_rate, SampleRate::HZ_48000);
        assert_eq!(d.depth, SampleDepth::F32);
        assert_eq!(d.channels, ChannelCount::STEREO);
        assert_eq!(d.to_string(), "48 kHz float 32, 2 canaux");
        assert_eq!(SampleDepth::default(), SampleDepth::F32);
    }

    /// `Display` en table : la fréquence, la profondeur, et le singulier de « canal ».
    #[test]
    fn display_du_format_en_table() {
        let cas = [
            (48_000, SampleDepth::F32, 2, "48 kHz float 32, 2 canaux"),
            (44_100, SampleDepth::Pcm16, 1, "44100 Hz PCM 16, 1 canal"),
            (96_000, SampleDepth::Pcm24, 8, "96 kHz PCM 24, 8 canaux"),
        ];
        for (hz, depth, canaux, attendu) in cas {
            let f = CableFormat {
                sample_rate: SampleRate::new(hz).unwrap(),
                depth,
                channels: ChannelCount::new(canaux).unwrap(),
            };
            assert_eq!(f.to_string(), attendu);
        }
        for d in SampleDepth::ALL {
            assert!(!d.to_string().is_empty(), "{d:?}");
            // Le jeton et le libellé ne se confondent pas : l'un se tape, l'autre se lit.
            assert_ne!(d.jeton(), d.to_string());
        }
    }

    /// `FromStr` en table : ce qui passe, et ce qui est refusé en nommant le champ.
    #[test]
    fn from_str_du_format_en_table() {
        assert_eq!(
            "48000:f32:2".parse::<CableFormat>().unwrap(),
            CableFormat::default()
        );
        // La casse et les espaces de bord ne changent rien.
        assert_eq!(
            "  48000:F32:2 ".parse::<CableFormat>().unwrap(),
            CableFormat::default()
        );
        assert_eq!(
            "44100:pcm24:6".parse::<CableFormat>().unwrap(),
            CableFormat {
                sample_rate: SampleRate::HZ_44100,
                depth: SampleDepth::Pcm24,
                channels: ChannelCount::new(6).unwrap(),
            }
        );

        // Chaque refus nomme **son** champ, pas « format invalide ».
        assert!(matches!(
            "48000:f32".parse::<CableFormat>(),
            Err(CableFormatParseError::Forme(_))
        ));
        assert!(matches!(
            "48000:f32:2:3".parse::<CableFormat>(),
            Err(CableFormatParseError::Forme(_))
        ));
        assert!(matches!(
            "".parse::<CableFormat>(),
            Err(CableFormatParseError::Forme(_))
        ));
        assert!(matches!(
            "quarante:f32:2".parse::<CableFormat>(),
            Err(CableFormatParseError::Frequence(_))
        ));
        // Hors des bornes du dépôt, pas seulement hors des trois du pilote.
        assert!(matches!(
            "1:f32:2".parse::<CableFormat>(),
            Err(CableFormatParseError::Frequence(_))
        ));
        assert!(matches!(
            "48000:double:2".parse::<CableFormat>(),
            Err(CableFormatParseError::Profondeur(_))
        ));
        assert!(matches!(
            "48000:f32:9".parse::<CableFormat>(),
            Err(CableFormatParseError::Canaux(_))
        ));
        assert!(matches!(
            "48000:f32:0".parse::<CableFormat>(),
            Err(CableFormatParseError::Canaux(_))
        ));
        // Le message porte le texte reçu : sans lui, l'utilisateur relit sa ligne sans
        // savoir lequel des trois champs on lui reproche.
        let erreur = "48000:double:2".parse::<CableFormat>().unwrap_err();
        assert!(erreur.to_string().contains("double"), "{erreur}");
        assert!(erreur.to_string().contains("pcm16"), "{erreur}");

        // Ce que la couche portable accepte et que le pilote Windows refusera : la
        // frontière est chez `conduit_helper::controle`, pas ici.
        assert!("22050:f32:2".parse::<CableFormat>().is_ok());
    }

    /// L'aller-retour `Display`/`FromStr` n'est **pas** l'identité, et le texte des deux
    /// sens l'est : `to_string` est fait pour l'œil, le jeton pour la ligne de commande.
    #[test]
    fn l_aller_retour_du_format() {
        for hz in [44_100, 48_000, 96_000] {
            for depth in SampleDepth::ALL {
                for canaux in 1..=ChannelCount::MAX {
                    let f = CableFormat {
                        sample_rate: SampleRate::new(hz).unwrap(),
                        depth,
                        channels: ChannelCount::new(canaux).unwrap(),
                    };
                    let texte = format!("{hz}:{}:{canaux}", depth.jeton());
                    assert_eq!(texte.parse::<CableFormat>().unwrap(), f, "{texte}");
                    assert_eq!(depth.jeton().parse::<SampleDepth>().unwrap(), depth);
                }
            }
        }
    }
}
