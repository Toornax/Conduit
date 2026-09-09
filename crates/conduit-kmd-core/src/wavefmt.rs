//! Négociation de format (`IMiniport::DataRangeIntersection`) et description du
//! `WAVEFORMATEX[TENSIBLE]` à rendre (`docs/driver-design.md` §5.4).
//!
//! Deux choses, aussi pures l'une que l'autre :
//!
//! - **l'intersection** de deux plages audio ([`intersect`]) — celle que le client soumet
//!   et une des nôtres —, qui rend le format retenu ou dit ce qui n'a pas croisé
//!   ([`NoMatch`]) ;
//! - **la forme** du descripteur à écrire ([`WaveFormat`]) : `WAVEFORMATEX` simple ou
//!   `WAVEFORMATEXTENSIBLE`, avec le masque de haut-parleurs qui va avec le nombre de
//!   canaux ([`speaker_mask`]).
//!
//! Rien ici ne connaît les types du WDK : `conduit-kmd` traduit les `KSDATARANGE` en
//! [`AudioRange`] et recopie le [`WaveFormat`] dans un `KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]`.
//!
//! # Pourquoi ce module existe
//!
//! Le pilote laissait `DataRangeIntersection` au défaut (`STATUS_NOT_IMPLEMENTED`), ce qui
//! confie l'intersection au **gestionnaire par défaut de PortCls**. Sa documentation
//! (« Default Data-Intersection Handlers ») énonce trois limites, dans ces termes : *only
//! PCM data formats*, *only mono and stereo audio streams*, et « *it does not support any
//! format containing a WAVEFORMATEXTENSIBLE structure, which is needed, for example, to
//! specify the channel mask for a format with more than two channels* ». Un câble à trois
//! canaux ou plus ne pouvait donc **pas** obtenir d'endpoint, et nos plages flottantes
//! n'étaient jamais retenues. C'est ce que ce module remplace.
//!
//! # Ce que le gestionnaire par défaut fait, et où nous nous en écartons
//!
//! La même page décrit la règle de choix : « *the port driver's default handler always
//! selects the highest value in each parameter's region of intersection* ». [`intersect`]
//! la suit — la plus haute fréquence, la plus haute profondeur — parce que c'est ce que le
//! moteur audio attend d'un gestionnaire.
//!
//! Il y a **une** divergence, et elle est délibérée : le nombre de canaux. Une
//! `KSDATARANGE_AUDIO` n'a pas de `MinimumChannels`, seulement `MaximumChannels` ; une
//! plage à six canaux dit donc littéralement « un à six ». Or un câble Conduit ne sait
//! servir que **son** compte de canaux : [`crate::ring::copy_frames`] ne remappe rien
//! ([`RingError::ChannelMismatch`](crate::ring::RingError::ChannelMismatch)). Rendre un
//! format stéréo depuis une plage à six canaux ferait donc proposer au moteur audio une
//! combinaison que `NewStream` refuserait ensuite. [`intersect`] exige que la plus haute
//! valeur de l'intersection soit **exactement** la nôtre, et refuse sinon
//! ([`NoMatch::Canaux`]).

use core::cmp::{max, min};
use core::fmt;

use crate::format::{RequestedFormat, SampleKind};
use crate::ring::FrameLayout;

// ---------------------------------------------------------------------------------
// Tailles et étiquettes des structures de `mmreg.h` / `ksmedia.h`.
//
// Recopiées ici parce que ce crate ne dépend d'aucun en-tête ; `conduit-kmd` les relie aux
// types générés par des assertions `const` (`size_of::<KSDATAFORMAT_WAVEFORMATEX>()`…), si
// bien qu'une divergence casserait la compilation du pilote au lieu de produire un
// descripteur tronqué.
// ---------------------------------------------------------------------------------

/// `sizeof(KSDATAFORMAT)` sur x64.
pub const KSDATAFORMAT_BYTES: u32 = 64;
/// `sizeof(WAVEFORMATEX)` : la structure est `packed`, `cbSize` compris.
pub const WAVEFORMATEX_BYTES: u32 = 18;
/// `cbSize` d'un `WAVEFORMATEXTENSIBLE` : `Samples`, `dwChannelMask`, `SubFormat`.
pub const WAVEFORMATEXTENSIBLE_CB_SIZE: u16 = 22;
/// `sizeof(KSDATAFORMAT_WAVEFORMATEX)` : 64 + 18.
pub const KSDATAFORMAT_WAVEFORMATEX_BYTES: u32 = 82;
/// `sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE)` : 64 + 18 + 22.
pub const KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES: u32 = 104;

/// `WAVE_FORMAT_PCM` (`mmreg.h`).
pub const WAVE_FORMAT_PCM: u16 = 0x0001;
/// `WAVE_FORMAT_IEEE_FLOAT` (`mmreg.h`).
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// `WAVE_FORMAT_EXTENSIBLE` (`mmreg.h`).
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

// Les deux tailles composées se déduisent bien des deux tailles élémentaires.
const _: () = {
    assert!(
        KSDATAFORMAT_WAVEFORMATEX_BYTES == KSDATAFORMAT_BYTES.saturating_add(WAVEFORMATEX_BYTES)
    );
    assert!(
        KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES
            == KSDATAFORMAT_WAVEFORMATEX_BYTES.saturating_add(WAVEFORMATEXTENSIBLE_CB_SIZE as u32)
    );
};

// ---------------------------------------------------------------------------------
// Masques de haut-parleurs (`KSAUDIO_SPEAKER_*`, ksmedia.h).
// ---------------------------------------------------------------------------------

/// `SPEAKER_FRONT_LEFT`.
pub const SPEAKER_FRONT_LEFT: u32 = 0x0000_0001;
/// `SPEAKER_FRONT_RIGHT`.
pub const SPEAKER_FRONT_RIGHT: u32 = 0x0000_0002;
/// `SPEAKER_FRONT_CENTER`.
pub const SPEAKER_FRONT_CENTER: u32 = 0x0000_0004;
/// `SPEAKER_LOW_FREQUENCY`.
pub const SPEAKER_LOW_FREQUENCY: u32 = 0x0000_0008;
/// `SPEAKER_BACK_LEFT`.
pub const SPEAKER_BACK_LEFT: u32 = 0x0000_0010;
/// `SPEAKER_BACK_RIGHT`.
pub const SPEAKER_BACK_RIGHT: u32 = 0x0000_0020;
/// `SPEAKER_SIDE_LEFT`.
pub const SPEAKER_SIDE_LEFT: u32 = 0x0000_0200;
/// `SPEAKER_SIDE_RIGHT`.
pub const SPEAKER_SIDE_RIGHT: u32 = 0x0000_0400;

/// `KSAUDIO_SPEAKER_MONO` : le canal unique va au centre.
pub const KSAUDIO_SPEAKER_MONO: u32 = SPEAKER_FRONT_CENTER;
/// `KSAUDIO_SPEAKER_STEREO`.
pub const KSAUDIO_SPEAKER_STEREO: u32 = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT;
/// `KSAUDIO_SPEAKER_2POINT1`.
pub const KSAUDIO_SPEAKER_2POINT1: u32 = KSAUDIO_SPEAKER_STEREO | SPEAKER_LOW_FREQUENCY;
/// `KSAUDIO_SPEAKER_QUAD`.
pub const KSAUDIO_SPEAKER_QUAD: u32 =
    KSAUDIO_SPEAKER_STEREO | SPEAKER_BACK_LEFT | SPEAKER_BACK_RIGHT;
/// `KSAUDIO_SPEAKER_5POINT0` (avant + latéraux).
pub const KSAUDIO_SPEAKER_5POINT0: u32 =
    KSAUDIO_SPEAKER_STEREO | SPEAKER_FRONT_CENTER | SPEAKER_SIDE_LEFT | SPEAKER_SIDE_RIGHT;
/// `KSAUDIO_SPEAKER_5POINT1`.
pub const KSAUDIO_SPEAKER_5POINT1: u32 = KSAUDIO_SPEAKER_STEREO
    | SPEAKER_FRONT_CENTER
    | SPEAKER_LOW_FREQUENCY
    | SPEAKER_BACK_LEFT
    | SPEAKER_BACK_RIGHT;
/// `KSAUDIO_SPEAKER_7POINT0`.
pub const KSAUDIO_SPEAKER_7POINT0: u32 =
    KSAUDIO_SPEAKER_QUAD | SPEAKER_FRONT_CENTER | SPEAKER_SIDE_LEFT | SPEAKER_SIDE_RIGHT;
/// `KSAUDIO_SPEAKER_7POINT1_SURROUND`.
pub const KSAUDIO_SPEAKER_7POINT1_SURROUND: u32 = KSAUDIO_SPEAKER_7POINT0 | SPEAKER_LOW_FREQUENCY;

/// Le masque `KSAUDIO_SPEAKER_*` de `channels` canaux, ou `None` hors de `1..=8`.
///
/// C'est la disposition la plus courante à ce compte-là ; les comptes impairs sans
/// disposition standard — 3, 5, 7 — prennent celle que Windows nomme sans le « point »
/// manquant. Aucune n'est fausse, toutes sont conventionnelles.
///
/// **L'invariant qui compte** est vérifié plus bas à la compilation : un masque porte
/// exactement autant de bits à 1 que le format a de canaux. Un `dwChannelMask` qui ne
/// compte pas ses canaux est refusé par le moteur audio, et c'est précisément le genre de
/// faute qu'un `WAVEFORMATEXTENSIBLE` mal rempli produit sans rien dire.
#[must_use]
pub const fn speaker_mask(channels: u8) -> Option<u32> {
    match channels {
        1 => Some(KSAUDIO_SPEAKER_MONO),
        2 => Some(KSAUDIO_SPEAKER_STEREO),
        3 => Some(KSAUDIO_SPEAKER_2POINT1),
        4 => Some(KSAUDIO_SPEAKER_QUAD),
        5 => Some(KSAUDIO_SPEAKER_5POINT0),
        6 => Some(KSAUDIO_SPEAKER_5POINT1),
        7 => Some(KSAUDIO_SPEAKER_7POINT0),
        8 => Some(KSAUDIO_SPEAKER_7POINT1_SURROUND),
        _ => None,
    }
}

// Autant de bits à 1 que de canaux, sur les huit comptes qu'un câble peut porter ; et rien
// au-delà. C'est l'assertion que `topo.rs` portait sur sa propre table : elle vit ici
// désormais, et le pilote relie les deux (voir `conduit_kmd::topo`).
const _: () = {
    let mut channels: u8 = 1;
    while channels <= FrameLayout::MAX_CHANNELS {
        let masque = match speaker_mask(channels) {
            Some(m) => m,
            None => 0,
        };
        assert!(
            masque.count_ones() == channels as u32,
            "un masque de haut-parleurs ne compte pas ses canaux"
        );
        channels = channels.wrapping_add(1);
    }
    assert!(FrameLayout::MAX_CHANNELS == 8);
    assert!(speaker_mask(0).is_none());
    assert!(speaker_mask(9).is_none());
    assert!(speaker_mask(u8::MAX).is_none());
};

// ---------------------------------------------------------------------------------
// Les plages, et leur intersection.
// ---------------------------------------------------------------------------------

/// Les bornes d'une `KSDATARANGE_AUDIO`, sans les types du WDK.
///
/// `kind` vaut `None` pour un **joker** : `KSDATAFORMAT_SUBTYPE_WILDCARD` est
/// `GUID_NULL` (`ks.h`), et un client a le droit de soumettre une plage qui n'impose
/// aucun sous-format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRange {
    /// Famille d'échantillons imposée, ou `None` pour un joker.
    pub kind: Option<SampleKind>,
    /// `MaximumChannels` : le compte de canaux le plus élevé que la plage couvre.
    pub max_channels: u32,
    /// `MinimumBitsPerSample`.
    pub min_bits: u32,
    /// `MaximumBitsPerSample`.
    pub max_bits: u32,
    /// `MinimumSampleFrequency`.
    pub min_rate: u32,
    /// `MaximumSampleFrequency`.
    pub max_rate: u32,
}

impl AudioRange {
    /// La plage qui n'impose rien : ce que vaut une `KSDATARANGE` simple (64 octets, sans
    /// les champs audio) soumise par un client.
    ///
    /// Un client peut légitimement décrire sa demande par une `KSDATARANGE` nue avec des
    /// jokers partout ; lire au-delà de ce que `FormatSize` annonce serait une lecture hors
    /// objet. Toutes les bornes sont donc ouvertes, et c'est **notre** plage qui décide.
    pub const WILDCARD: Self = Self {
        kind: None,
        max_channels: u32::MAX,
        min_bits: 0,
        max_bits: u32::MAX,
        min_rate: 0,
        max_rate: u32::MAX,
    };

    /// Une plage **ponctuelle** : la forme de toutes les nôtres
    /// (`descriptors::audio_range`), bornes confondues pour la profondeur comme pour la
    /// fréquence.
    #[must_use]
    pub const fn punctual(kind: SampleKind, channels: u32, bits: u32, rate: u32) -> Self {
        Self {
            kind: Some(kind),
            max_channels: channels,
            min_bits: bits,
            max_bits: bits,
            min_rate: rate,
            max_rate: rate,
        }
    }
}

/// Pourquoi deux plages ne se croisent pas — et c'est **la** information à consigner : un
/// `STATUS_NO_MATCH` rendu sans la dire est exactement le genre de panne muette qui donne
/// « aucun format » dans le panneau de son sans une ligne de journal.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoMatch {
    /// Les sous-formats sont incompatibles (le client veut du PCM, la plage est flottante,
    /// ou l'inverse). **Attendu et bénin** : PortCls interroge nos trois plages l'une après
    /// l'autre, et un client PCM en refusera toujours au moins une.
    SousType,
    /// Le client ne monte pas jusqu'au compte de canaux du câble (voir l'en-tête de module
    /// sur la divergence assumée avec le gestionnaire par défaut).
    Canaux {
        /// `MaximumChannels` du client.
        demandes: u32,
        /// `MaximumChannels` de notre plage : ce qu'il faut accepter, ni plus ni moins.
        declares: u32,
    },
    /// Les intervalles de profondeur ne se recouvrent pas.
    Bits {
        /// `(minimum, maximum)` du client.
        demandes: (u32, u32),
        /// `(minimum, maximum)` de notre plage.
        declares: (u32, u32),
    },
    /// Les intervalles de fréquence ne se recouvrent pas.
    Frequence {
        /// `(minimum, maximum)` du client.
        demandes: (u32, u32),
        /// `(minimum, maximum)` de notre plage.
        declares: (u32, u32),
    },
    /// L'intersection existe mais aucun `WAVEFORMATEX[TENSIBLE]` ne la décrit (profondeur
    /// qui n'est pas un multiple d'octet, canaux hors de `1..=8`, débits qui débordent).
    Indescriptible,
    /// L'intersection se décrit, mais [`crate::format::validate`] ne la reconnaît pas dans
    /// les formats du câble : le proposer reviendrait à annoncer ce que `NewStream`
    /// refuserait ensuite.
    ///
    /// **Inatteignable** tant que les plages déclarées et [`crate::format::cable_formats`]
    /// disent la même chose, ce que les tests d'ici vérifient sur les 24 variantes. Le
    /// pilote garde tout de même le contrôle : c'est la seule chose qui relie les deux
    /// portes de Windows à l'exécution.
    HorsCatalogue,
}

impl NoMatch {
    /// Vrai si le refus porte sur les **nombres** (canaux, profondeur, fréquence) et non
    /// sur le sous-format.
    ///
    /// C'est le tri qui rend le journal exploitable : un refus de sous-type est le
    /// fonctionnement normal (PortCls propose nos trois plages à un client qui n'en veut
    /// qu'une famille), un refus numérique dit que Windows a demandé un format que le câble
    /// ne sert pas — et c'est cela qu'il faut lire quand un endpoint n'a « aucun format ».
    #[must_use]
    pub const fn est_numerique(self) -> bool {
        !matches!(self, Self::SousType)
    }
}

/// Les messages sont **courts à dessein** : ils partent dans la chaîne d'insertion d'une
/// entrée du journal d'événements, plafonnée à environ quatre-vingts caractères une fois le
/// préfixe posé (voir `conduit_kmd::eventlog`). Au-delà, la fin serait tronquée — donc les
/// bornes, c'est-à-dire l'information.
impl fmt::Display for NoMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SousType => f.write_str("sous-formats incompatibles"),
            Self::Canaux { demandes, declares } => write!(
                f,
                "canaux : le client monte à {demandes}, le câble en sert {declares}"
            ),
            Self::Bits {
                demandes: (dmin, dmax),
                declares: (nmin, nmax),
            } => write!(f, "bits : client {dmin}-{dmax}, câble {nmin}-{nmax}"),
            Self::Frequence {
                demandes: (dmin, dmax),
                declares: (nmin, nmax),
            } => write!(f, "Hz : client {dmin}-{dmax}, câble {nmin}-{nmax}"),
            Self::Indescriptible => f.write_str("intersection indescriptible en WAVEFORMATEX"),
            Self::HorsCatalogue => {
                f.write_str("format négocié hors des formats déclarés par le câble")
            }
        }
    }
}

/// L'intersection de la plage `client` et de la nôtre, `mine` : le format de meilleure
/// qualité que les deux acceptent.
///
/// Règle de choix : **la plus haute valeur de chaque paramètre**, comme le gestionnaire par
/// défaut de PortCls (voir l'en-tête de module), à ceci près que le nombre de canaux doit
/// être exactement celui de notre plage.
///
/// `mine.kind` à `None` (un joker de **notre** côté) est refusé : nos plages sont toujours
/// typées, et une plage sans sous-type ne dirait pas quel `SubFormat` écrire.
///
/// # Erreurs
///
/// [`NoMatch`], qui nomme le paramètre qui n'a pas croisé.
pub fn intersect(client: &AudioRange, mine: &AudioRange) -> Result<WaveFormat, NoMatch> {
    let kind = match (client.kind, mine.kind) {
        (_, None) => return Err(NoMatch::SousType),
        (None, Some(k)) => k,
        (Some(demande), Some(k)) if demande == k => k,
        _ => return Err(NoMatch::SousType),
    };
    if matches!(kind, SampleKind::Other) {
        return Err(NoMatch::SousType);
    }
    // Canaux : la valeur la plus haute de l'intersection est `min(client, nous)`, et elle
    // doit valoir la nôtre — sinon nous proposerions un format que `NewStream` refuserait.
    if client.max_channels < mine.max_channels {
        return Err(NoMatch::Canaux {
            demandes: client.max_channels,
            declares: mine.max_channels,
        });
    }
    let channels = mine.max_channels;

    let bits_min = max(client.min_bits, mine.min_bits);
    let bits_max = min(client.max_bits, mine.max_bits);
    if bits_min > bits_max {
        return Err(NoMatch::Bits {
            demandes: (client.min_bits, client.max_bits),
            declares: (mine.min_bits, mine.max_bits),
        });
    }
    let rate_min = max(client.min_rate, mine.min_rate);
    let rate_max = min(client.max_rate, mine.max_rate);
    if rate_min > rate_max {
        return Err(NoMatch::Frequence {
            demandes: (client.min_rate, client.max_rate),
            declares: (mine.min_rate, mine.max_rate),
        });
    }

    WaveFormat::new(kind, channels, bits_max, rate_max).ok_or(NoMatch::Indescriptible)
}

// ---------------------------------------------------------------------------------
// La forme du descripteur rendu.
// ---------------------------------------------------------------------------------

/// Le format retenu, et tout ce qu'il faut pour écrire son
/// `KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]`.
///
/// Ne se construit que par [`WaveFormat::new`], qui refuse tout ce qui ne se décrit pas :
/// les champs dérivés (alignement de bloc, débit moyen, masque de haut-parleurs) sont donc
/// toujours cohérents entre eux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormat {
    kind: SampleKind,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    block_align: u16,
    avg_bytes_per_sec: u32,
    channel_mask: u32,
    extensible: bool,
}

/// Vrai si le format **exige** un `WAVEFORMATEXTENSIBLE` plutôt qu'un `WAVEFORMATEX`.
///
/// La documentation (« Extensible Wave-Format Descriptors ») donne les deux raisons, et
/// elles sont exactement celles-ci : `WAVEFORMATEX` « *can adequately support only mono and
/// (two-channel) stereo streams* », et « *WAVEFORMATEX is sufficient for describing formats
/// with sample sizes of 8 or 16 bits, but WAVEFORMATEXTENSIBLE is necessary to adequately
/// describe formats with a sample precision of greater than 16 bits* ».
///
/// Au-delà de deux canaux, la forme étendue est aussi la seule qui puisse porter le
/// `dwChannelMask` — c'est ce que la page du gestionnaire par défaut désigne comme sa
/// limite décisive.
#[must_use]
pub const fn needs_extensible(channels: u32, bits: u32) -> bool {
    channels > 2 || bits > 16
}

impl WaveFormat {
    /// Le format de `channels` canaux à `rate` Hz sur `bits` bits, ou `None` s'il ne se
    /// décrit pas.
    ///
    /// Refusé : une famille [`SampleKind::Other`], zéro canal ou plus de
    /// [`FrameLayout::MAX_CHANNELS`], une profondeur nulle, non multiple de huit ou
    /// au-delà de 32 bits, un flottant qui ne ferait pas 32 bits, une fréquence nulle, et
    /// tout ce qui ferait déborder l'alignement de bloc (`u16`) ou le débit moyen (`u32`).
    ///
    /// La borne haute de 32 bits n'est pas une limite du format mais celle du **câble** :
    /// [`crate::ring::SampleFormat`] ne connaît que F32, PCM24 et I16, et proposer autre
    /// chose reviendrait à annoncer un format que `NewStream` refuserait.
    #[must_use]
    pub fn new(kind: SampleKind, channels: u32, bits: u32, rate: u32) -> Option<Self> {
        if matches!(kind, SampleKind::Other) || rate == 0 {
            return None;
        }
        let channels8 = u8::try_from(channels).ok()?;
        if channels8 == 0 || channels8 > FrameLayout::MAX_CHANNELS {
            return None;
        }
        if bits == 0 || bits > 32 || bits.checked_rem(8)? != 0 {
            return None;
        }
        // Un `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` de 16 ou 24 bits n'existe pas : le refuser
        // ici évite d'annoncer un format que `copy_frames` ne saurait pas lire.
        if matches!(kind, SampleKind::Float) && bits != 32 {
            return None;
        }
        let bits16 = u16::try_from(bits).ok()?;
        let channels16 = u16::from(channels8);
        let block_align = bits16
            .checked_div(8)
            .and_then(|octets| octets.checked_mul(channels16))?;
        if block_align == 0 {
            return None;
        }
        let avg_bytes_per_sec = rate.checked_mul(u32::from(block_align))?;
        let extensible = needs_extensible(channels, bits);
        let channel_mask = if extensible {
            speaker_mask(channels8)?
        } else {
            0
        };
        Some(Self {
            kind,
            channels: channels16,
            sample_rate: rate,
            bits_per_sample: bits16,
            block_align,
            avg_bytes_per_sec,
            channel_mask,
            extensible,
        })
    }

    /// Famille d'échantillons (jamais [`SampleKind::Other`]).
    #[must_use]
    pub const fn kind(self) -> SampleKind {
        self.kind
    }

    /// `nChannels`.
    #[must_use]
    pub const fn channels(self) -> u16 {
        self.channels
    }

    /// `nSamplesPerSec`.
    #[must_use]
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// `wBitsPerSample` : la taille du conteneur.
    #[must_use]
    pub const fn bits_per_sample(self) -> u16 {
        self.bits_per_sample
    }

    /// `Samples.wValidBitsPerSample` : toujours la taille du conteneur.
    ///
    /// Un câble ne sert que des échantillons pleins — c'est la règle de
    /// [`SupportedFormat::accepts`](crate::format::SupportedFormat::accepts), qui refuse le
    /// PCM « 24 dans un conteneur de 32 ». Annoncer autre chose ici reviendrait à proposer
    /// un format que `NewStream` refuserait.
    #[must_use]
    pub const fn valid_bits(self) -> u16 {
        self.bits_per_sample
    }

    /// `nBlockAlign` : les octets d'une trame.
    #[must_use]
    pub const fn block_align(self) -> u16 {
        self.block_align
    }

    /// `nAvgBytesPerSec`.
    #[must_use]
    pub const fn avg_bytes_per_sec(self) -> u32 {
        self.avg_bytes_per_sec
    }

    /// `dwChannelMask` de la forme étendue ; 0 pour la forme simple, qui n'a pas ce champ.
    #[must_use]
    pub const fn channel_mask(self) -> u32 {
        self.channel_mask
    }

    /// Vrai si le descripteur doit être un `WAVEFORMATEXTENSIBLE`.
    #[must_use]
    pub const fn is_extensible(self) -> bool {
        self.extensible
    }

    /// `wFormatTag` : `WAVE_FORMAT_EXTENSIBLE` pour la forme étendue, sinon l'étiquette de
    /// la famille.
    #[must_use]
    pub const fn format_tag(self) -> u16 {
        if self.extensible {
            WAVE_FORMAT_EXTENSIBLE
        } else {
            match self.kind {
                SampleKind::Float => WAVE_FORMAT_IEEE_FLOAT,
                SampleKind::Pcm | SampleKind::Other => WAVE_FORMAT_PCM,
            }
        }
    }

    /// `cbSize` : 22 pour la forme étendue, 0 pour la forme simple.
    #[must_use]
    pub const fn cb_size(self) -> u16 {
        if self.extensible {
            WAVEFORMATEXTENSIBLE_CB_SIZE
        } else {
            0
        }
    }

    /// `KSDATAFORMAT::FormatSize` du descripteur complet : 104 ou 82 octets.
    ///
    /// C'est aussi la taille que `DataRangeIntersection` écrit dans
    /// `ResultantFormatLength`, et donc celle qu'il faut avoir dans le tampon.
    #[must_use]
    pub const fn format_size(self) -> u32 {
        if self.extensible {
            KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES
        } else {
            KSDATAFORMAT_WAVEFORMATEX_BYTES
        }
    }

    /// `KSDATAFORMAT::SampleSize` : les octets d'une trame, comme SYSVAD le renseigne.
    #[must_use]
    pub const fn sample_size(self) -> u32 {
        self.block_align as u32
    }

    /// Le même format, sous la forme que [`crate::format::validate`] confronte à la liste
    /// des formats du câble.
    ///
    /// C'est par là que le pilote vérifie qu'il ne propose **jamais** un format que
    /// `NewStream` refuserait ensuite : la négociation et l'ouverture de flux passent par
    /// la même liste.
    #[must_use]
    pub const fn requested(self) -> RequestedFormat {
        RequestedFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits_per_sample: self.bits_per_sample,
            valid_bits: self.valid_bits(),
            kind: self.kind,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::format::{
        cable_formats, validate, RATE_44100, RATE_48000, RATE_96000, SAMPLE_DEPTHS, SAMPLE_RATES,
    };
    use crate::ring::SampleFormat;
    use proptest::prelude::*;
    use std::string::ToString;

    /// La famille d'échantillons d'une profondeur, comme `descriptors::subtype_of`.
    fn famille(depth: SampleFormat) -> SampleKind {
        if matches!(depth, SampleFormat::F32) {
            SampleKind::Float
        } else {
            SampleKind::Pcm
        }
    }

    /// Les trois plages que déclare un câble réglé sur `(rate, channels)` — exactement
    /// celles que `descriptors::variant_ranges` construit.
    fn plages_du_cable(rate: u32, channels: u32) -> [AudioRange; 3] {
        SAMPLE_DEPTHS.map(|depth| {
            AudioRange::punctual(famille(depth), channels, depth.bits_per_sample(), rate)
        })
    }

    #[test]
    fn masques_de_haut_parleurs() {
        assert_eq!(speaker_mask(1), Some(0x4));
        assert_eq!(speaker_mask(2), Some(0x3));
        assert_eq!(speaker_mask(3), Some(0xB));
        assert_eq!(speaker_mask(4), Some(0x33));
        assert_eq!(speaker_mask(5), Some(0x607));
        assert_eq!(speaker_mask(6), Some(0x3F));
        assert_eq!(speaker_mask(7), Some(0x637));
        assert_eq!(speaker_mask(8), Some(0x63F));
        assert_eq!(speaker_mask(0), None);
        assert_eq!(speaker_mask(9), None);
        for channels in 1u8..=8 {
            assert_eq!(
                speaker_mask(channels).unwrap().count_ones(),
                u32::from(channels),
                "{channels} canaux"
            );
        }
    }

    /// Le seuil de la forme étendue, sur les deux axes que la documentation nomme.
    #[test]
    fn seuil_de_la_forme_etendue() {
        assert!(!needs_extensible(1, 16));
        assert!(!needs_extensible(2, 16));
        // Plus de deux canaux : il faut un `dwChannelMask`.
        assert!(needs_extensible(3, 16));
        assert!(needs_extensible(8, 16));
        // Plus de seize bits : il faut séparer conteneur et bits significatifs.
        assert!(needs_extensible(1, 24));
        assert!(needs_extensible(2, 32));
        // Et les tailles suivent.
        let simple = WaveFormat::new(SampleKind::Pcm, 2, 16, RATE_48000).unwrap();
        assert!(!simple.is_extensible());
        assert_eq!(simple.format_size(), 82);
        assert_eq!(simple.cb_size(), 0);
        assert_eq!(simple.channel_mask(), 0);
        assert_eq!(simple.format_tag(), WAVE_FORMAT_PCM);
        assert_eq!(simple.block_align(), 4);
        assert_eq!(simple.avg_bytes_per_sec(), 192_000);
        assert_eq!(simple.sample_size(), 4);

        let etendu = WaveFormat::new(SampleKind::Float, 6, 32, RATE_96000).unwrap();
        assert!(etendu.is_extensible());
        assert_eq!(etendu.format_size(), 104);
        assert_eq!(etendu.cb_size(), 22);
        assert_eq!(etendu.channel_mask(), KSAUDIO_SPEAKER_5POINT1);
        assert_eq!(etendu.format_tag(), WAVE_FORMAT_EXTENSIBLE);
        assert_eq!(etendu.block_align(), 24);
        assert_eq!(etendu.avg_bytes_per_sec(), 96_000 * 24);

        // Un flottant stéréo est étendu par sa profondeur, pas par ses canaux, et garde
        // quand même son masque : c'est la forme qui le porte.
        let flottant = WaveFormat::new(SampleKind::Float, 2, 32, RATE_44100).unwrap();
        assert!(flottant.is_extensible());
        assert_eq!(flottant.channel_mask(), KSAUDIO_SPEAKER_STEREO);
    }

    #[test]
    fn formats_refuses() {
        // Familles et profondeurs impossibles.
        assert!(WaveFormat::new(SampleKind::Other, 2, 16, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Float, 2, 16, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Float, 2, 24, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Pcm, 2, 0, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Pcm, 2, 20, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Pcm, 2, 40, RATE_48000).is_none());
        // Canaux hors du domaine d'un câble.
        assert!(WaveFormat::new(SampleKind::Pcm, 0, 16, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Pcm, 9, 16, RATE_48000).is_none());
        assert!(WaveFormat::new(SampleKind::Pcm, u32::MAX, 16, RATE_48000).is_none());
        // Fréquence nulle, et débit moyen qui déborde.
        assert!(WaveFormat::new(SampleKind::Pcm, 2, 16, 0).is_none());
        assert!(WaveFormat::new(SampleKind::Pcm, 8, 32, u32::MAX).is_none());
    }

    /// Le cas nominal : un client qui n'impose rien obtient, pour chacune de nos trois
    /// plages, exactement le format de cette plage.
    #[test]
    fn un_client_joker_obtient_notre_plage() {
        for (rate, channels) in [(RATE_44100, 2u8), (RATE_48000, 1), (RATE_96000, 6)] {
            let cable = cable_formats(rate, channels);
            for plage in plages_du_cable(rate, u32::from(channels)) {
                let format = intersect(&AudioRange::WILDCARD, &plage).unwrap();
                assert_eq!(format.sample_rate(), rate);
                assert_eq!(format.channels(), u16::from(channels));
                assert_eq!(u32::from(format.bits_per_sample()), plage.max_bits);
                // Et le format proposé est un de ceux que `NewStream` acceptera.
                assert!(
                    validate(&format.requested(), &cable).is_ok(),
                    "{rate} Hz, {channels} canaux : format proposé mais refusé par validate"
                );
            }
        }
    }

    /// Ce que le refus dit, cas par cas.
    #[test]
    fn table_des_refus() {
        let notre = AudioRange::punctual(SampleKind::Pcm, 2, 16, RATE_48000);

        // Sous-type : le client veut du flottant, la plage est entière.
        let client = AudioRange {
            kind: Some(SampleKind::Float),
            ..AudioRange::WILDCARD
        };
        assert_eq!(intersect(&client, &notre), Err(NoMatch::SousType));
        assert!(!NoMatch::SousType.est_numerique());

        // Notre plage ne peut pas être un joker.
        let joker = AudioRange {
            kind: None,
            ..notre
        };
        assert_eq!(
            intersect(&AudioRange::WILDCARD, &joker),
            Err(NoMatch::SousType)
        );

        // Canaux : le client plafonne à 1, le câble en sert 2.
        let client = AudioRange {
            max_channels: 1,
            ..AudioRange::WILDCARD
        };
        assert_eq!(
            intersect(&client, &notre),
            Err(NoMatch::Canaux {
                demandes: 1,
                declares: 2
            })
        );
        // Le client qui monte plus haut que le câble obtient le compte du câble.
        let client = AudioRange {
            max_channels: 8,
            ..AudioRange::WILDCARD
        };
        assert_eq!(intersect(&client, &notre).unwrap().channels(), 2);

        // Profondeur disjointe.
        let client = AudioRange {
            min_bits: 24,
            max_bits: 32,
            ..AudioRange::WILDCARD
        };
        assert_eq!(
            intersect(&client, &notre),
            Err(NoMatch::Bits {
                demandes: (24, 32),
                declares: (16, 16)
            })
        );

        // Fréquence disjointe : c'est la forme qu'aurait un client plafonné à 48 kHz face
        // à un câble à 96 — l'hypothèse à vérifier en machine (voir le rapport de M1b-21).
        let quatre_vingt_seize = AudioRange::punctual(SampleKind::Pcm, 2, 16, RATE_96000);
        let client = AudioRange {
            min_rate: 8_000,
            max_rate: 48_000,
            ..AudioRange::WILDCARD
        };
        assert_eq!(
            intersect(&client, &quatre_vingt_seize),
            Err(NoMatch::Frequence {
                demandes: (8_000, 48_000),
                declares: (96_000, 96_000)
            })
        );

        for refus in [
            NoMatch::Canaux {
                demandes: 1,
                declares: 2,
            },
            NoMatch::Bits {
                demandes: (24, 32),
                declares: (16, 16),
            },
            NoMatch::Frequence {
                demandes: (8_000, 48_000),
                declares: (96_000, 96_000),
            },
            NoMatch::Indescriptible,
            NoMatch::HorsCatalogue,
        ] {
            assert!(refus.est_numerique(), "{refus:?}");
        }
    }

    /// Les messages sont en français, nomment les deux côtés, et tiennent dans ce que la
    /// chaîne d'insertion du journal d'événements peut porter (environ quatre-vingts
    /// caractères, préfixe et nom du filtre déduits).
    #[test]
    fn les_refus_se_lisent_et_tiennent_dans_le_journal() {
        let cas = [
            NoMatch::SousType,
            NoMatch::Canaux {
                demandes: 1,
                declares: 6,
            },
            NoMatch::Bits {
                demandes: (8, 16),
                declares: (24, 24),
            },
            NoMatch::Frequence {
                demandes: (8_000, 192_000),
                declares: (96_000, 96_000),
            },
            NoMatch::Indescriptible,
            NoMatch::HorsCatalogue,
        ];
        for refus in cas {
            let texte = refus.to_string();
            assert!(texte.chars().count() <= 66, "trop long : {texte}");
            assert!(!texte.is_empty());
        }
        assert!(cas[0].to_string().contains("sous-formats"));
        assert!(cas[1].to_string().contains("en sert 6"));
        assert!(cas[2].to_string().contains("24-24"));
        assert!(cas[3].to_string().contains("96000-96000"));
        assert!(cas[4].to_string().contains("WAVEFORMATEX"));
        assert!(cas[5].to_string().contains("hors des formats"));
    }

    /// « La plus haute valeur de chaque paramètre » : sur une plage large des deux côtés,
    /// le résultat est bien le maximum de l'intersection.
    #[test]
    fn le_plus_haut_de_chaque_parametre() {
        let notre = AudioRange {
            kind: Some(SampleKind::Pcm),
            max_channels: 2,
            min_bits: 8,
            max_bits: 24,
            min_rate: 8_000,
            max_rate: 96_000,
        };
        let client = AudioRange {
            kind: None,
            max_channels: 8,
            min_bits: 16,
            max_bits: 32,
            min_rate: 44_100,
            max_rate: 48_000,
        };
        let format = intersect(&client, &notre).unwrap();
        assert_eq!(format.bits_per_sample(), 24);
        assert_eq!(format.sample_rate(), 48_000);
        assert_eq!(format.channels(), 2);
    }

    /// **La matrice entière** : chacune des 24 variantes, chacune de ses trois plages, face
    /// à un client joker — le format proposé est toujours accepté par `validate` sur la
    /// liste du même câble. C'est l'invariant qui empêche de proposer au moteur audio ce
    /// que `NewStream` refuserait.
    #[test]
    fn aucune_variante_ne_propose_ce_que_new_stream_refuserait() {
        for rate in SAMPLE_RATES {
            for channels in 1u8..=FrameLayout::MAX_CHANNELS {
                let cable = cable_formats(rate, channels);
                let mut vus = 0;
                for plage in plages_du_cable(rate, u32::from(channels)) {
                    let format = intersect(&AudioRange::WILDCARD, &plage).unwrap();
                    assert!(
                        validate(&format.requested(), &cable).is_ok(),
                        "{rate} Hz, {channels} canaux, {plage:?}"
                    );
                    // La forme suit la règle documentée, sans exception.
                    assert_eq!(
                        format.is_extensible(),
                        channels > 2 || format.bits_per_sample() > 16
                    );
                    if format.is_extensible() {
                        assert_eq!(
                            format.channel_mask().count_ones(),
                            u32::from(channels),
                            "masque incohérent"
                        );
                    } else {
                        assert_eq!(format.channel_mask(), 0);
                    }
                    vus += 1;
                }
                assert_eq!(vus, 3);
            }
        }
    }

    proptest! {
        /// Un format construit est toujours cohérent : taille 82 ou 104 selon la forme,
        /// trame égale au produit canaux × octets, débit égal au produit trame × fréquence,
        /// masque qui compte ses canaux (ou nul).
        #[test]
        fn wave_format_est_coherent(
            channels in 1u32..=8,
            depth in 0usize..3,
            rate in 1u32..=384_000,
        ) {
            let format = SAMPLE_DEPTHS[depth];
            let bits = format.bits_per_sample();
            let wf = WaveFormat::new(famille(format), channels, bits, rate).unwrap();
            prop_assert_eq!(u32::from(wf.channels()), channels);
            prop_assert_eq!(wf.sample_rate(), rate);
            prop_assert_eq!(u32::from(wf.bits_per_sample()), bits);
            prop_assert_eq!(wf.valid_bits(), wf.bits_per_sample());
            prop_assert_eq!(u32::from(wf.block_align()), channels * bits / 8);
            prop_assert_eq!(wf.avg_bytes_per_sec(), rate * u32::from(wf.block_align()));
            prop_assert_eq!(wf.sample_size(), u32::from(wf.block_align()));
            prop_assert_eq!(wf.is_extensible(), channels > 2 || bits > 16);
            if wf.is_extensible() {
                prop_assert_eq!(wf.format_size(), 104);
                prop_assert_eq!(wf.cb_size(), 22);
                prop_assert_eq!(wf.format_tag(), WAVE_FORMAT_EXTENSIBLE);
                prop_assert_eq!(wf.channel_mask().count_ones(), channels);
            } else {
                prop_assert_eq!(wf.format_size(), 82);
                prop_assert_eq!(wf.cb_size(), 0);
                prop_assert_eq!(wf.channel_mask(), 0);
                prop_assert_ne!(wf.format_tag(), WAVE_FORMAT_EXTENSIBLE);
            }
        }

        /// L'intersection ne rend **jamais** un format hors des bornes des deux plages, et
        /// son nombre de canaux est toujours celui que le câble déclare.
        #[test]
        fn intersect_reste_dans_les_deux_plages(
            notre_channels in 1u32..=8,
            depth in 0usize..3,
            notre_rate in prop_oneof![Just(RATE_44100), Just(RATE_48000), Just(RATE_96000)],
            client_channels in 0u32..=16,
            client_bits in (0u32..=64, 0u32..=64),
            client_rate in (0u32..=200_000, 0u32..=200_000),
            client_kind in prop_oneof![
                Just(None),
                Just(Some(SampleKind::Pcm)),
                Just(Some(SampleKind::Float)),
                Just(Some(SampleKind::Other)),
            ],
        ) {
            let depth = SAMPLE_DEPTHS[depth];
            let notre_kind = famille(depth);
            let bits = depth.bits_per_sample();
            let notre = AudioRange::punctual(notre_kind, notre_channels, bits, notre_rate);
            let client = AudioRange {
                kind: client_kind,
                max_channels: client_channels,
                min_bits: client_bits.0.min(client_bits.1),
                max_bits: client_bits.0.max(client_bits.1),
                min_rate: client_rate.0.min(client_rate.1),
                max_rate: client_rate.0.max(client_rate.1),
            };
            match intersect(&client, &notre) {
                Ok(wf) => {
                    prop_assert_eq!(u32::from(wf.channels()), notre_channels);
                    prop_assert!(client.max_channels >= notre_channels);
                    prop_assert_eq!(u32::from(wf.bits_per_sample()), bits);
                    prop_assert!(client.min_bits <= bits && bits <= client.max_bits);
                    prop_assert_eq!(wf.sample_rate(), notre_rate);
                    prop_assert!(client.min_rate <= notre_rate && notre_rate <= client.max_rate);
                    prop_assert_eq!(wf.kind(), notre_kind);
                    // Et le format proposé est servable par ce câble.
                    let cable = cable_formats(notre_rate, u8::try_from(notre_channels).unwrap());
                    prop_assert!(validate(&wf.requested(), &cable).is_ok());
                }
                Err(refus) => {
                    // Le refus est justifié par au moins une borne réellement disjointe.
                    let sous_type = match client.kind {
                        None => false,
                        Some(k) => k != notre_kind,
                    };
                    let canaux = client.max_channels < notre_channels;
                    let bits_ko = client.min_bits > bits || client.max_bits < bits;
                    let rate_ko = client.min_rate > notre_rate || client.max_rate < notre_rate;
                    prop_assert!(
                        sous_type || canaux || bits_ko || rate_ko,
                        "refus {refus:?} sans borne disjointe"
                    );
                }
            }
        }
    }
}
