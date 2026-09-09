//! Validation des formats (`docs/driver-design.md` §5.2, §5.4).
//!
//! Le moteur audio de Windows propose un `KSDATAFORMAT_WAVEFORMATEXTENSIBLE` à
//! `NewStream` / `AllocateAudioBuffer`. Le pilote en extrait les champs utiles dans
//! un [`RequestedFormat`] (le décodage de `wFormatTag`/`SubFormat` en
//! [`SampleKind`] se fait dans `conduit-kmd`, ce crate ne connaît pas les GUID) et
//! le confronte ici à la liste des formats supportés par le câble
//! ([`validate`]). La taille de tampon demandée par le moteur est arrondie à la
//! trame **vers le haut**, jamais réduite : au-delà de la borne haute, [`buffer_bytes`]
//! refuse plutôt que d'écrêter.
//!
//! Ce crate ne dépend pas de `conduit-core` : fréquences et comptes de canaux sont
//! des entiers simples.
//!
//! # La matrice de M1b-05, et pourquoi elle est asymétrique
//!
//! Un câble déclare **une** fréquence et **un** nombre de canaux — ceux que le registre
//! lui fixe ([`crate::config::CableFormat`]) — mais **trois** profondeurs, toujours les
//! mêmes ([`SAMPLE_DEPTHS`]). L'asymétrie n'est pas un compromis : elle est dictée par ce
//! que [`crate::ring::copy_frames`] sait faire. La copie **convertit** les profondeurs (les
//! neuf couples) et ne **rééchantillonne rien**, ni ne remappe les canaux
//! (`RingError::ChannelMismatch`). Fréquence et canaux doivent donc s'accorder entre les
//! deux bouts d'un câble ; la profondeur, non.
//!
//! D'où la règle du pilote : les deux endpoints d'un câble ne déclarent que la fréquence
//! et le nombre de canaux configurés. Windows ne peut pas les désaccorder, parce qu'on ne
//! lui propose rien d'autre — c'est plus sûr qu'une négociation qu'il faudrait surveiller,
//! et c'est ce qui garde le pilote léger (`docs/driver-design.md` §1).

use core::fmt;

use crate::ring::{FrameLayout, SampleFormat};

/// Famille d'échantillons annoncée par le format demandé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleKind {
    /// `KSDATAFORMAT_SUBTYPE_PCM` / `WAVE_FORMAT_PCM` : entiers signés.
    Pcm,
    /// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` / `WAVE_FORMAT_IEEE_FLOAT`.
    Float,
    /// Tout autre sous-format (jamais supporté).
    Other,
}

/// Format demandé par le moteur audio, extrait du `WAVEFORMATEXTENSIBLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestedFormat {
    /// `nSamplesPerSec`.
    pub sample_rate: u32,
    /// `nChannels`.
    pub channels: u16,
    /// `wBitsPerSample` : taille du conteneur.
    pub bits_per_sample: u16,
    /// `wValidBitsPerSample` : bits significatifs (égal au conteneur pour un
    /// `WAVEFORMATEX` simple).
    pub valid_bits: u16,
    /// Famille d'échantillons.
    pub kind: SampleKind,
}

/// Format que le câble accepte (une entrée de `KSDATARANGE_AUDIO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SupportedFormat {
    /// Fréquence d'échantillonnage en Hz.
    pub sample_rate: u32,
    /// Nombre de canaux (1 à 8).
    pub channels: u8,
    /// Format des échantillons.
    pub format: SampleFormat,
}

impl SupportedFormat {
    /// Disposition de trame correspondante ; `None` si `channels` est hors bornes.
    pub const fn layout(self) -> Option<FrameLayout> {
        FrameLayout::new(self.channels, self.format)
    }

    /// Vrai si `requested` décrit exactement ce format : même fréquence, même
    /// nombre de canaux, et F32 ⇔ `Float` 32/32 bits, PCM24 ⇔ `Pcm` 24/24 bits,
    /// I16 ⇔ `Pcm` 16/16 bits.
    ///
    /// # Le conteneur, pas seulement les bits significatifs
    ///
    /// `bits_per_sample` **et** `valid_bits` doivent valoir la taille du conteneur. Un
    /// PCM « 24 bits dans un conteneur de 32 » (`wBitsPerSample = 32`,
    /// `wValidBitsPerSample = 24`) est donc refusé, et c'est délibéré : notre PCM24 fait
    /// trois octets en mémoire ([`SampleFormat::Pcm24`]), et l'accepter reviendrait à lire
    /// le tampon avec un pas de 3 là où le moteur audio écrit avec un pas de 4 — un
    /// décalage qui grandit d'un octet par échantillon, c'est-à-dire du bruit. Le moteur
    /// se rabat alors sur un format que nous déclarons vraiment.
    ///
    /// # Une entrée dont la trame n'existe pas n'accepte rien (M1b-05)
    ///
    /// Le premier contrôle porte sur **l'entrée**, pas sur la demande : une
    /// [`SupportedFormat`] dont [`Self::layout`] vaut `None` — `channels == 0`, ou
    /// au-delà de [`FrameLayout::MAX_CHANNELS`] — ne décrit **aucune trame**. L'accepter
    /// rendrait à [`validate`] un format que l'appelant croirait servable et dont il ne
    /// saurait pas dimensionner la trame : `wave::open_stream` le passe tel quel à
    /// `stream::WaveStream::new`, qui doit alors traiter un `layout()` absent sur un
    /// format qu'on vient de lui dire supporté.
    ///
    /// Le trou était **inatteignable** tant que la liste supportée était une `const` à
    /// deux canaux (`M1A_FORMATS`) ; M1b-05 la construit à l'exécution
    /// ([`cable_formats`], depuis le registre), et il a fallu ce contrôle. Trouvé par le
    /// fuzzing de M1b-08, entrée déclenchante consignée dans le test
    /// `accepts_refuse_une_entree_sans_trame` : demande et entrée toutes deux à
    /// `channels: 0`, que l'égalité seule laissait passer.
    pub fn accepts(self, requested: &RequestedFormat) -> bool {
        if self.layout().is_none() {
            return false;
        }
        if requested.sample_rate != self.sample_rate
            || requested.channels != u16::from(self.channels)
        {
            return false;
        }
        let kind = match self.format {
            SampleFormat::F32 => SampleKind::Float,
            SampleFormat::Pcm24 | SampleFormat::I16 => SampleKind::Pcm,
        };
        let bits = self.format.bits_per_sample() as u16;
        requested.kind == kind && requested.bits_per_sample == bits && requested.valid_bits == bits
    }
}

/// Les trois fréquences d'échantillonnage qu'un câble peut porter (SPEC F-02, §5.4).
///
/// **Une seule est active à la fois**, celle que le registre fixe pour le câble : les deux
/// bouts d'un câble ne déclarent que celle-là. La raison est dans
/// [`crate::ring::copy_frames`] — il n'y a **aucun rééchantillonnage** dans le pilote, et
/// il n'y en aura pas (« tout ce qui peut être fait en espace utilisateur y sera fait ») :
/// deux bouts à deux fréquences différentes ne pourraient rien se transmettre. En
/// déclarer une seule est donc ce qui rend l'incohérence *impossible* plutôt que
/// détectable — Windows ne peut pas désaccorder ce qu'on ne lui propose pas.
pub const SAMPLE_RATES: [u32; 3] = [RATE_44100, RATE_48000, RATE_96000];

/// 44 100 Hz : la fréquence du disque compact, celle de la plupart des fichiers.
pub const RATE_44100: u32 = 44_100;
/// 48 000 Hz : la fréquence par défaut de Windows, et celle du câble par défaut.
pub const RATE_48000: u32 = 48_000;
/// 96 000 Hz : la fréquence des stations de travail audio.
pub const RATE_96000: u32 = 96_000;

/// Les trois profondeurs déclarées, **toutes les trois, toujours** (§5.4).
///
/// C'est l'exacte contrepartie de [`SAMPLE_RATES`] : la profondeur, elle, se convertit à
/// la volée ([`crate::ring::copy_frames`] traite les neuf couples), donc rien n'oblige les
/// deux bouts d'un câble à s'accorder dessus, donc rien n'oblige à en choisir une. C'est ce
/// qui fait qu'un câble a **3 fréquences × 8 canaux = 24** variantes de descripteurs, et
/// non 72.
///
/// L'ordre est celui de la préférence usuelle du moteur audio de Windows — le flottant
/// d'abord, l'entier 16 bits en dernier ressort. Rien dans la documentation ne promet que
/// l'ordre des `KSDATARANGE` compte, mais PortCls interroge le gestionnaire d'intersection
/// ([`crate::wavefmt`], M1b-21) une plage après l'autre : si l'ordre a un effet, c'est
/// celui-là, et il va dans le bon sens.
pub const SAMPLE_DEPTHS: [SampleFormat; 3] =
    [SampleFormat::F32, SampleFormat::Pcm24, SampleFormat::I16];

/// Nombre de formats qu'un câble déclare : une profondeur par entrée de [`SAMPLE_DEPTHS`].
pub const FORMATS_PER_CABLE: usize = SAMPLE_DEPTHS.len();

/// Les formats que déclare un câble réglé sur `sample_rate` et `channels` : les trois
/// profondeurs de [`SAMPLE_DEPTHS`], à cette fréquence et sur ce nombre de canaux.
///
/// Aucune validation ici — `sample_rate` et `channels` viennent d'une
/// `crate::config::CableFormat`, déjà écrêtée. Une valeur aberrante ne produirait qu'un
/// format que personne ne demande, jamais une panique : depuis M1b-08,
/// [`SupportedFormat::accepts`] refuse **de lui-même** une entrée dont
/// [`SupportedFormat::layout`] vaut `None`, si bien qu'une liste bâtie sur `channels == 0`
/// n'accepte plus rien plutôt que d'accepter un format sans trame.
#[must_use]
pub const fn cable_formats(sample_rate: u32, channels: u8) -> [SupportedFormat; FORMATS_PER_CABLE] {
    [
        SupportedFormat {
            sample_rate,
            channels,
            format: SampleFormat::F32,
        },
        SupportedFormat {
            sample_rate,
            channels,
            format: SampleFormat::Pcm24,
        },
        SupportedFormat {
            sample_rate,
            channels,
            format: SampleFormat::I16,
        },
    ]
}

/// Rang de `sample_rate` dans [`SAMPLE_RATES`], ou `None` si ce n'est pas une des trois.
///
/// C'est l'index que les tables de descripteurs du pilote emploient : une fréquence hors
/// liste n'a pas de variante, et l'appelant se replie sur celle du défaut. Écrit en
/// `match` sur les trois valeurs plutôt qu'en boucle sur [`SAMPLE_RATES`] : le crate
/// refuse l'indexation, et une assertion `const` plus bas relie les deux.
#[must_use]
pub const fn sample_rate_index(sample_rate: u32) -> Option<usize> {
    match sample_rate {
        RATE_44100 => Some(0),
        RATE_48000 => Some(1),
        RATE_96000 => Some(2),
        _ => None,
    }
}

/// La fréquence de rang `index` dans [`SAMPLE_RATES`], réciproque de
/// [`sample_rate_index`] ; `None` au-delà de la troisième.
#[must_use]
pub const fn sample_rate_at(index: usize) -> Option<u32> {
    match index {
        0 => Some(RATE_44100),
        1 => Some(RATE_48000),
        2 => Some(RATE_96000),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------
// La matrice des variantes de descripteurs (M1b-05, §5.4).
//
// Cette arithmétique **vivait dans `conduit_kmd::descriptors`**, c'est-à-dire dans le seul
// crate du dépôt qui ne se teste pas en mode utilisateur : ses seules vérifications étaient
// des assertions `const`, et une assertion `const` ne dit rien de ce qu'elle ne sait pas
// lire. Elle est ici depuis la correction de M1b-05 — pure, sans le moindre type du WDK,
// donc testable et fuzzable comme le reste du crate. Le pilote n'en garde que la
// construction des tables KS.
// ---------------------------------------------------------------------------------

/// Nombre maximal de canaux d'un câble (SPEC F-03), du type des index d'ici.
pub const MAX_CHANNELS_PER_CABLE: usize = FrameLayout::MAX_CHANNELS as usize;

/// Nombre de **variantes de descripteurs** d'un sens : une par couple (fréquence, canaux).
///
/// **24, et non 72.** La profondeur ne multiplie rien : les trois de [`SAMPLE_DEPTHS`] sont
/// déclarées dans toutes les variantes, parce que [`crate::ring::copy_frames`] les convertit
/// à la volée et que les deux bouts d'un câble n'ont donc pas à s'accorder dessus. Seuls la
/// fréquence et le nombre de canaux figent quelque chose (voir l'en-tête de module).
pub const VARIANT_COUNT: usize = SAMPLE_RATES.len().saturating_mul(MAX_CHANNELS_PER_CABLE);

/// L'index de variante du couple (`rate_index`, `channels`), ou `None` hors domaine.
///
/// Rangement : les huit comptes de canaux d'une fréquence sont contigus, ce qui rend la
/// table lisible dans un vidage mémoire — les entrées 0 à 7 sont le 44,1 kHz, 8 à 15 le
/// 48 kHz, 16 à 23 le 96 kHz. [`variant_rate`] et [`variant_channels`] en sont les
/// réciproques exactes, ce que les tests d'ici vérifient sur les 24.
#[must_use]
pub const fn variant_index(rate_index: usize, channels: u8) -> Option<usize> {
    if rate_index >= SAMPLE_RATES.len()
        || channels == 0
        || channels as usize > MAX_CHANNELS_PER_CABLE
    {
        return None;
    }
    // `rate_index < 3` et `channels ≤ 8` : le produit vaut au plus 23, sans débordement
    // possible. Les `checked_*` remplacent des opérateurs que les lints du crate refusent.
    match rate_index.checked_mul(MAX_CHANNELS_PER_CABLE) {
        Some(base) => base.checked_add((channels as usize).wrapping_sub(1)),
        None => None,
    }
}

/// L'index de variante du couple (`sample_rate`, `channels`), ou `None` si la fréquence
/// n'est pas une des trois de [`SAMPLE_RATES`] ou le compte de canaux hors de `1..=8`.
#[must_use]
pub const fn variant_of(sample_rate: u32, channels: u8) -> Option<usize> {
    match sample_rate_index(sample_rate) {
        Some(rate_index) => variant_index(rate_index, channels),
        None => None,
    }
}

/// La fréquence de la variante `variant`, en Hz, ou `None` au-delà de la dernière.
///
/// **`Option`, et pas un repli.** La version d'origine rendait 48 000 Hz hors domaine et
/// [`variant_channels`] y bouclait sur `1..=8` : deux replis muets, dans le module même dont
/// l'en-tête dit que son pire mode de panne est silencieux. Ici, hors domaine se dit.
#[must_use]
pub const fn variant_rate(variant: usize) -> Option<u32> {
    if variant >= VARIANT_COUNT {
        return None;
    }
    sample_rate_at(variant.wrapping_div(MAX_CHANNELS_PER_CABLE))
}

/// Le nombre de canaux de la variante `variant`, ou `None` au-delà de la dernière (voir
/// [`variant_rate`] sur l'absence de repli).
#[must_use]
pub const fn variant_channels(variant: usize) -> Option<u8> {
    if variant >= VARIANT_COUNT {
        return None;
    }
    // `variant < 24`, donc le reste est dans `0..8` et le `+ 1` dans `1..=8` : la
    // conversion ne perd rien.
    Some(variant.wrapping_rem(MAX_CHANNELS_PER_CABLE).wrapping_add(1) as u8)
}

// La matrice est bien celle qu'on annonce partout — 24 variantes, huit canaux, trois
// fréquences — et l'aller-retour index → (fréquence, canaux) → index est l'identité sur
// tout le domaine. C'est la vérification qui attrape un rangement décalé d'un cran : 24
// variantes toutes valides mais permutées donneraient un câble à six canaux servi en
// quatre, sans un mot. Écrite en `while` (le `for` est interdit en `const`).
const _: () = {
    assert!(MAX_CHANNELS_PER_CABLE == 8 && SAMPLE_RATES.len() == 3);
    assert!(VARIANT_COUNT == 24, "3 fréquences × 8 canaux, pas 72");
    let mut variant = 0;
    while variant < VARIANT_COUNT {
        let rate = match variant_rate(variant) {
            Some(hz) => hz,
            None => 0,
        };
        let channels = match variant_channels(variant) {
            Some(canaux) => canaux,
            None => 0,
        };
        assert!(matches!(variant_of(rate, channels), Some(i) if i == variant));
        variant = variant.wrapping_add(1);
    }
    // Hors domaine : aucune variante, jamais un index qu'on indexerait quand même.
    assert!(variant_index(SAMPLE_RATES.len(), 2).is_none());
    assert!(variant_index(0, 0).is_none());
    assert!(variant_index(0, 9).is_none());
    assert!(variant_rate(VARIANT_COUNT).is_none());
    assert!(variant_channels(VARIANT_COUNT).is_none());
    assert!(variant_of(0, 2).is_none());
    assert!(variant_of(RATE_48000, 0).is_none());
    // Le format d'un câble neuf : 48 kHz (rang 1) sur 2 canaux, donc 1 × 8 + 1 = 9.
    assert!(matches!(variant_of(RATE_48000, 2), Some(9)));
};

// Les deux écritures de la liste des fréquences disent la même chose. Sans cette
// assertion, ajouter une fréquence à `SAMPLE_RATES` sans toucher aux deux `match`
// donnerait une variante de descripteur que personne ne sait indexer — un câble réglé
// dessus se replierait en silence sur le défaut.
const _: () = {
    let [a, b, c] = SAMPLE_RATES;
    assert!(a == RATE_44100 && b == RATE_48000 && c == RATE_96000);
    assert!(matches!(sample_rate_index(RATE_44100), Some(0)));
    assert!(matches!(sample_rate_index(RATE_48000), Some(1)));
    assert!(matches!(sample_rate_index(RATE_96000), Some(2)));
    assert!(sample_rate_index(0).is_none());
    assert!(matches!(sample_rate_at(0), Some(RATE_44100)));
    assert!(matches!(sample_rate_at(1), Some(RATE_48000)));
    assert!(matches!(sample_rate_at(2), Some(RATE_96000)));
    assert!(sample_rate_at(SAMPLE_RATES.len()).is_none());
    // Les trois profondeurs, dans l'ordre annoncé, et leurs tailles de conteneur.
    let [f32_, pcm24, i16_] = SAMPLE_DEPTHS;
    assert!(matches!(f32_, SampleFormat::F32));
    assert!(matches!(pcm24, SampleFormat::Pcm24));
    assert!(matches!(i16_, SampleFormat::I16));
    assert!(f32_.bits_per_sample() == 32);
    assert!(pcm24.bits_per_sample() == 24);
    assert!(i16_.bits_per_sample() == 16);
    assert!(FORMATS_PER_CABLE == 3);
};

/// Erreur de [`validate`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// Aucun format supporté ne correspond à la demande.
    Unsupported,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => {
                "le format demandé n'est pas dans la liste des formats supportés par le câble"
            }
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for FormatError {}

/// Cherche dans `supported` le format qui correspond exactement à `requested`.
///
/// Correspondance : F32 ⇔ [`SampleKind::Float`] 32 bits sur 32 ; PCM24 ⇔
/// [`SampleKind::Pcm`] 24 bits sur 24 ; I16 ⇔ [`SampleKind::Pcm`] 16 bits sur 16 ;
/// fréquence et canaux égaux. Tout le reste (`Other`, bits valides partiels — dont le
/// PCM 24-dans-32, voir [`SupportedFormat::accepts`]) est [`FormatError::Unsupported`] :
/// le pilote répond `STATUS_INVALID_PARAMETER` et le moteur choisit un autre format.
///
/// IRQL : `PASSIVE_LEVEL` en pratique, mais aucune allocation ni panique.
pub fn validate(
    requested: &RequestedFormat,
    supported: &[SupportedFormat],
) -> Result<SupportedFormat, FormatError> {
    supported
        .iter()
        .copied()
        .find(|s| s.accepts(requested))
        .ok_or(FormatError::Unsupported)
}

/// Durée minimale du tampon cyclique, en millisecondes (§5.2).
pub const MIN_BUFFER_MS: u32 = 1;
/// Durée maximale du tampon cyclique, en millisecondes (§5.2).
///
/// **Garde-fou, pas politique de latence.** Tant que [`buffer_bytes`] écrêtait, cette
/// borne exprimait une préférence : au-delà on rabotait, et le flux se créait quand
/// même. Depuis que la fonction **refuse** au-delà (le contrat « jamais moins que
/// demandé, ou rien »), elle est devenue une *condition d'échec de création de flux* —
/// l'appelant traduit le refus en `STATUS_UNSUCCESSFUL`. Une borne serrée cesserait
/// donc de raboter la latence pour casser des applications : à 100 ms, une station de
/// travail audio en mode exclusif qui demande 200 ms de tampon — courant, la stabilité
/// y prime sur la latence — ne pourrait plus ouvrir de flux du tout, là où elle
/// fonctionnait auparavant, mal mais elle fonctionnait. La borne ne doit donc rejeter
/// que l'absurde.
///
/// 500 ms couvrent toute demande réaliste avec de la marge, et gardent la mémoire non
/// paginée bornée — c'est le seul point à ne pas perdre de vue. Au pire absolu : une
/// réserve de 16 câbles (M1b-02) fait 32 flux ; à 96 kHz sur 8 canaux en float32
/// (32 octets par trame), 500 ms font 48 000 trames, soit 1,5 Mio par flux et 48 Mio
/// pour la réserve entière.
pub const MAX_BUFFER_MS: u32 = 500;

/// Taille du tampon cyclique à allouer pour `AllocateAudioBuffer`.
///
/// **Contrat : jamais moins que demandé, ou rien.** La documentation de
/// `IMiniportWaveRTStream::AllocateAudioBuffer` est explicite — « The actual size
/// must be at least the requested size; otherwise, the Audio Session API (WASAPI)
/// audio engine won't use the buffer, and stream creation will fail. » Rendre un
/// tampon plus petit que la demande n'est donc pas une option : au-delà de la borne
/// haute, la fonction **refuse** (`None`), et l'appelant traduit ce refus en
/// `STATUS_UNSUCCESSFUL`.
///
/// `requested_bytes` (la taille demandée par le moteur audio) est arrondie **vers
/// le haut** au multiple de `frame_bytes`, puis remontée à [`MIN_BUFFER_MS`] de son
/// à `sample_rate` si elle est en dessous : le DPC (période 1 ms) ne saurait pas
/// suivre un tampon plus court. Au-delà de [`MAX_BUFFER_MS`] — demande absurde, mémoire
/// non paginée gaspillée — la demande est refusée plutôt qu'écrêtée.
///
/// Renvoie `None` si `frame_bytes == 0`, si `sample_rate == 0`, si les bornes sont
/// incohérentes (`sample_rate == 1` : 1 ms arrondi vers le haut fait 1 trame, 500 ms
/// arrondis vers le bas en font 0 — seule fréquence non nulle où le plancher passe
/// au-dessus du plafond), si la demande arrondie dépasse [`MAX_BUFFER_MS`] ou si la
/// taille retenue ne tient pas dans un `u32`.
pub fn buffer_bytes(requested_bytes: u32, frame_bytes: u32, sample_rate: u32) -> Option<u32> {
    buffer_bytes_with_floor(requested_bytes, frame_bytes, sample_rate, MIN_BUFFER_MS)
}

/// Comme [`buffer_bytes`], mais avec un **plancher réglable** : `floor_ms` millisecondes
/// de son au lieu de [`MIN_BUFFER_MS`].
///
/// C'est par là que le paramètre de registre `BufferMs` agit (M1b-01 le lisait, M1b-05
/// l'applique). Le plancher est la seule chose qu'il commande, et c'est bien ce que SPEC
/// §11.2 décrit — « tampon interne du pilote : 10 ms par défaut, réglable ». Un moteur
/// audio qui demande **plus** obtient ce qu'il demande, inchangé ; un moteur qui demande
/// **moins** obtient le plancher.
///
/// # Ce que régler ce plancher change vraiment
///
/// Le contrat de [`buffer_bytes`] tient — jamais moins que demandé, ou rien —, mais le
/// défaut de 10 ms est **dix fois** le plancher de 1 ms qui valait jusqu'ici. Une
/// application en mode exclusif qui demanderait 3 ms de tampon recevra donc 10 ms, et sa
/// latence s'en ressentira. C'est le comportement voulu (le plancher existe pour que le DPC
/// de copie ait de la marge), et c'est aussi ce qui rend le paramètre utile : un poste qui
/// veut 5 ms écrit `BufferMs = 5`.
///
/// `floor_ms` est écrêté à [`MAX_BUFFER_MS`] : un plancher au-dessus du plafond refuserait
/// toute allocation, ce qui n'est jamais ce qu'un administrateur voulait dire.
/// [`crate::params::sanitize`] l'a déjà ramené dans les bornes de toute façon ; l'écrêtage
/// ici est la ceinture qui rend la fonction sûre pour un appelant qui l'aurait oublié.
pub fn buffer_bytes_with_floor(
    requested_bytes: u32,
    frame_bytes: u32,
    sample_rate: u32,
    floor_ms: u32,
) -> Option<u32> {
    if frame_bytes == 0 || sample_rate == 0 {
        return None;
    }
    let floor_ms = floor_ms.clamp(MIN_BUFFER_MS, MAX_BUFFER_MS);
    let frame_bytes = u64::from(frame_bytes);
    let rate = u64::from(sample_rate);
    // ceil(rate × floor_ms / 1000) et floor(rate × MAX_MS / 1000), en u64 : pas de
    // débordement (u32 × 500 < 2^32 × 2^9 = 2^41, et le + 999 du ceil reste loin de 2^64).
    let min_frames = rate
        .checked_mul(u64::from(floor_ms))?
        .checked_add(999)?
        .checked_div(1_000)?;
    let max_frames = rate
        .checked_mul(u64::from(MAX_BUFFER_MS))?
        .checked_div(1_000)?;
    // Atteignable aux basses fréquences, et d'autant plus depuis que le plancher est
    // réglable : à 1 Hz min_frames = 1 et max_frames = 0 quel que soit `floor_ms`, et à
    // `floor_ms == MAX_BUFFER_MS` le ceil du plancher peut dépasser d'une trame le floor du
    // plafond. Dans les deux cas il n'existe aucune taille valide : refuser.
    if min_frames > max_frames {
        return None;
    }
    let requested_frames = u64::from(requested_bytes)
        .checked_add(frame_bytes.checked_sub(1)?)?
        .checked_div(frame_bytes)?;
    if requested_frames > max_frames {
        // Écrêter rendrait un tampon plus petit que demandé : refuser.
        return None;
    }
    let frames = requested_frames.max(min_frames);
    u32::try_from(frames.checked_mul(frame_bytes)?).ok()
}

/// Taille du tampon cyclique pour `AllocateBufferWithNotification` : comme
/// [`buffer_bytes`], puis arrondie **vers le haut** à un multiple de
/// `notification_count` trames, pour que chaque période de notification
/// (`bytes / notification_count`) soit un nombre entier de trames et que la fin du
/// tampon soit une frontière de période ([`crate::notify`]). L'arrondi peut dépasser
/// [`MAX_BUFFER_MS`] d'au plus `notification_count − 1` trames.
///
/// L'alignement ne peut que **remonter** la taille : le contrat de [`buffer_bytes`]
/// — jamais moins que demandé, ou rien — vaut donc aussi ici, refus de la borne
/// haute compris.
///
/// `None` dans les cas de [`buffer_bytes`], si `notification_count == 0`, ou si le
/// résultat ne tient pas dans un `u32`.
pub fn buffer_bytes_for_notifications(
    requested_bytes: u32,
    frame_bytes: u32,
    sample_rate: u32,
    notification_count: u32,
) -> Option<u32> {
    buffer_bytes_for_notifications_with_floor(
        requested_bytes,
        frame_bytes,
        sample_rate,
        notification_count,
        MIN_BUFFER_MS,
    )
}

/// Comme [`buffer_bytes_for_notifications`], avec le plancher réglable de
/// [`buffer_bytes_with_floor`] (paramètre de registre `BufferMs`).
pub fn buffer_bytes_for_notifications_with_floor(
    requested_bytes: u32,
    frame_bytes: u32,
    sample_rate: u32,
    notification_count: u32,
    floor_ms: u32,
) -> Option<u32> {
    let bytes = buffer_bytes_with_floor(requested_bytes, frame_bytes, sample_rate, floor_ms)?;
    // `frame_bytes ≠ 0` (vérifié par `buffer_bytes`) et `bytes` en est un multiple.
    let frames = bytes.checked_div(frame_bytes)?;
    let aligned = crate::notify::align_frames(frames, notification_count)?;
    aligned.checked_mul(frame_bytes)
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
    use proptest::prelude::*;
    use std::string::ToString;

    const fn req(
        sample_rate: u32,
        channels: u16,
        bits: u16,
        valid: u16,
        kind: SampleKind,
    ) -> RequestedFormat {
        RequestedFormat {
            sample_rate,
            channels,
            bits_per_sample: bits,
            valid_bits: valid,
            kind,
        }
    }

    /// Les formats du câble par défaut : 48 kHz stéréo, les trois profondeurs.
    const DEFAUT: [SupportedFormat; FORMATS_PER_CABLE] = cable_formats(48_000, 2);

    #[test]
    fn validate_table() {
        let [f32_, pcm24, i16_] = DEFAUT;
        let cases: [(RequestedFormat, Result<SupportedFormat, FormatError>); 14] = [
            (req(48_000, 2, 32, 32, SampleKind::Float), Ok(f32_)),
            (req(48_000, 2, 24, 24, SampleKind::Pcm), Ok(pcm24)),
            (req(48_000, 2, 16, 16, SampleKind::Pcm), Ok(i16_)),
            // Mauvaise fréquence : les deux autres du domaine, qu'un **autre** câble
            // pourrait servir mais pas celui-ci.
            (
                req(44_100, 2, 32, 32, SampleKind::Float),
                Err(FormatError::Unsupported),
            ),
            (
                req(96_000, 2, 16, 16, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            // Mauvais nombre de canaux.
            (
                req(48_000, 1, 32, 32, SampleKind::Float),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 6, 16, 16, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            // Famille et taille croisées.
            (
                req(48_000, 2, 32, 32, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 2, 16, 16, SampleKind::Float),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 2, 24, 24, SampleKind::Float),
                Err(FormatError::Unsupported),
            ),
            // **PCM 24 dans un conteneur de 32** : refusé, et c'est le cas qui compte
            // depuis M1b-05. L'accepter ferait lire le tampon avec un pas de 3 là où le
            // moteur écrit avec un pas de 4 (voir `SupportedFormat::accepts`).
            (
                req(48_000, 2, 32, 24, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 2, 32, 24, SampleKind::Float),
                Err(FormatError::Unsupported),
            ),
            // Bits valides partiels dans un conteneur de 24.
            (
                req(48_000, 2, 24, 20, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 2, 16, 16, SampleKind::Other),
                Err(FormatError::Unsupported),
            ),
        ];
        for (requested, expected) in cases {
            assert_eq!(validate(&requested, &DEFAUT), expected, "{requested:?}");
        }
        assert_eq!(
            validate(&req(48_000, 2, 32, 32, SampleKind::Float), &[]),
            Err(FormatError::Unsupported)
        );
    }

    /// Un câble réglé sur autre chose que le défaut n'accepte que **sa** fréquence et
    /// **son** nombre de canaux — c'est ce qui rend l'accord des deux bouts impossible à
    /// rater.
    #[test]
    fn un_cable_ne_declare_que_son_reglage() {
        let cable = cable_formats(96_000, 6);
        assert_eq!(cable.len(), FORMATS_PER_CABLE);
        for depth in SAMPLE_DEPTHS {
            let bits = depth.bits_per_sample() as u16;
            let kind = if matches!(depth, SampleFormat::F32) {
                SampleKind::Float
            } else {
                SampleKind::Pcm
            };
            // Les trois profondeurs passent, à sa fréquence et sur ses canaux.
            assert!(
                validate(&req(96_000, 6, bits, bits, kind), &cable).is_ok(),
                "{depth:?}"
            );
            // Les deux autres fréquences, non — même profondeur, mêmes canaux.
            for rate in [44_100, 48_000] {
                assert_eq!(
                    validate(&req(rate, 6, bits, bits, kind), &cable),
                    Err(FormatError::Unsupported),
                    "{depth:?} à {rate} Hz"
                );
            }
            // Les autres comptes de canaux, non plus.
            for channels in [1, 2, 5, 7, 8] {
                assert_eq!(
                    validate(&req(96_000, channels, bits, bits, kind), &cable),
                    Err(FormatError::Unsupported),
                    "{depth:?} sur {channels} canaux"
                );
            }
        }
    }

    /// Les 24 variantes de la matrice : trois fréquences, huit comptes de canaux, et
    /// **trois profondeurs chacune** — 72 formats déclarés, mais 24 jeux de descripteurs.
    #[test]
    fn la_matrice_fait_bien_vingt_quatre_variantes() {
        assert_eq!(SAMPLE_RATES.len(), 3);
        assert_eq!(SAMPLE_DEPTHS.len(), FORMATS_PER_CABLE);
        let mut variantes = 0usize;
        let mut formats = 0usize;
        for rate in SAMPLE_RATES {
            for channels in 1u8..=FrameLayout::MAX_CHANNELS {
                let jeu = cable_formats(rate, channels);
                variantes += 1;
                formats += jeu.len();
                let layout = jeu[0].layout().unwrap();
                assert_eq!(layout.channels(), channels);
                // Les trois profondeurs, et rien d'autre.
                let depths: [SampleFormat; 3] = [jeu[0].format, jeu[1].format, jeu[2].format];
                assert_eq!(depths, SAMPLE_DEPTHS, "{rate} Hz, {channels} canaux");
                for f in jeu {
                    assert_eq!(f.sample_rate, rate);
                    assert_eq!(f.channels, channels);
                }
            }
        }
        assert_eq!(variantes, 24);
        assert_eq!(formats, 72);
    }

    /// Rang et fréquence sont réciproques sur tout le domaine, et rien d'autre n'a de
    /// rang.
    #[test]
    fn rang_et_frequence_sont_reciproques() {
        for (i, rate) in SAMPLE_RATES.iter().enumerate() {
            assert_eq!(sample_rate_index(*rate), Some(i), "{rate} Hz");
            assert_eq!(sample_rate_at(i), Some(*rate), "rang {i}");
        }
        for rate in [
            0,
            1,
            8_000,
            44_099,
            44_101,
            48_001,
            96_001,
            192_000,
            u32::MAX,
        ] {
            assert_eq!(sample_rate_index(rate), None, "{rate} Hz");
        }
        assert_eq!(sample_rate_at(3), None);
        assert_eq!(sample_rate_at(usize::MAX), None);
    }

    #[test]
    fn supported_layout() {
        // 48 kHz stéréo : 8 octets par trame en F32, 6 en PCM24, 4 en I16.
        let [f32_, pcm24, i16_] = DEFAUT;
        assert_eq!(f32_.layout().unwrap().frame_bytes(), 8);
        assert_eq!(pcm24.layout().unwrap().frame_bytes(), 6);
        assert_eq!(i16_.layout().unwrap().frame_bytes(), 4);
        // Huit canaux en PCM24 : 24 octets, la seule trame de la matrice qui ne soit ni
        // une puissance de deux ni un multiple de 4.
        let huit = cable_formats(96_000, 8);
        assert_eq!(huit[1].layout().unwrap().frame_bytes(), 24);
        let bad = SupportedFormat {
            sample_rate: 48_000,
            channels: 0,
            format: SampleFormat::F32,
        };
        assert!(bad.layout().is_none());
        assert!(cable_formats(48_000, 9)[0].layout().is_none());
    }

    /// **Le trou du fuzzing de M1b-08**, corrigé avec M1b-05 : une entrée dont le nombre
    /// de canaux n'est pas représentable est refusée, au lieu d'être acceptée par la seule
    /// égalité des comptes.
    ///
    /// L'entrée déclenchante est celle que le fuzzer a consignée : demande et entrée
    /// toutes deux à `channels: 0`. Avant la correction, `validate` rendait `Ok` d'un
    /// format dont `layout()` vaut `None` — l'appelant croyait tenir un format servable et
    /// n'avait pas de trame.
    #[test]
    fn accepts_refuse_une_entree_sans_trame() {
        let sans_trame = SupportedFormat {
            sample_rate: 0,
            channels: 0,
            format: SampleFormat::F32,
        };
        assert!(sans_trame.layout().is_none());
        let demande = req(0, 0, 32, 32, SampleKind::Float);
        assert!(!sans_trame.accepts(&demande));
        assert_eq!(
            validate(&demande, &[sans_trame]),
            Err(FormatError::Unsupported)
        );

        // Le même trou par le haut : neuf canaux, au-delà de `FrameLayout::MAX_CHANNELS`.
        let trop = SupportedFormat {
            sample_rate: 48_000,
            channels: 9,
            format: SampleFormat::I16,
        };
        assert!(trop.layout().is_none());
        assert!(!trop.accepts(&req(48_000, 9, 16, 16, SampleKind::Pcm)));

        // Corollaire : toute liste rendue par `cable_formats` avec un compte aberrant
        // n'accepte plus rien, quel que soit ce qu'on lui demande.
        for canaux in [0u8, 9, 255] {
            for jeu in cable_formats(48_000, canaux) {
                assert!(jeu.layout().is_none(), "{canaux} canaux");
                let bits = jeu.format.bits_per_sample() as u16;
                let kind = if matches!(jeu.format, SampleFormat::F32) {
                    SampleKind::Float
                } else {
                    SampleKind::Pcm
                };
                assert!(!jeu.accepts(&req(48_000, u16::from(canaux), bits, bits, kind)));
            }
        }

        // Et rien n'a bougé pour un format servable : la garde ne mord que l'aberrant.
        let [f32_, ..] = DEFAUT;
        assert!(f32_.accepts(&req(48_000, 2, 32, 32, SampleKind::Float)));
    }

    /// Les 24 variantes : l'aller-retour est l'identité sur tout le domaine, et rien
    /// au-dehors n'a d'index. C'est la vérification que le pilote ne pouvait pas faire —
    /// `conduit-kmd` ne se teste pas en mode utilisateur.
    #[test]
    fn les_variantes_sont_reciproques_sur_tout_le_domaine() {
        assert_eq!(VARIANT_COUNT, 24);
        assert_eq!(MAX_CHANNELS_PER_CABLE, 8);
        let mut vues = std::vec::Vec::new();
        for (rang, rate) in SAMPLE_RATES.iter().enumerate() {
            for channels in 1u8..=8 {
                let variant = variant_index(rang, channels).unwrap();
                assert!(variant < VARIANT_COUNT);
                assert_eq!(variant_of(*rate, channels), Some(variant));
                assert_eq!(variant_rate(variant), Some(*rate));
                assert_eq!(variant_channels(variant), Some(channels));
                vues.push(variant);
            }
        }
        // Une variante et une seule par couple : la bijection, pas seulement l'injection.
        vues.sort_unstable();
        vues.dedup();
        assert_eq!(vues.len(), VARIANT_COUNT);

        // Le rangement annoncé : 0 à 7 le 44,1 kHz, 8 à 15 le 48 kHz, 16 à 23 le 96 kHz.
        assert_eq!(variant_of(RATE_44100, 1), Some(0));
        assert_eq!(variant_of(RATE_48000, 2), Some(9));
        assert_eq!(variant_of(RATE_96000, 8), Some(23));

        // Hors domaine, des deux côtés.
        assert_eq!(variant_index(3, 2), None);
        assert_eq!(variant_index(usize::MAX, 2), None);
        assert_eq!(variant_index(0, 0), None);
        assert_eq!(variant_index(0, 9), None);
        assert_eq!(variant_index(0, u8::MAX), None);
        assert_eq!(variant_of(44_101, 2), None);
        assert_eq!(variant_of(0, 2), None);
        assert_eq!(variant_of(RATE_48000, 0), None);
        assert_eq!(variant_rate(VARIANT_COUNT), None);
        assert_eq!(variant_rate(usize::MAX), None);
        assert_eq!(variant_channels(VARIANT_COUNT), None);
        assert_eq!(variant_channels(usize::MAX), None);
    }

    /// Chaque variante déclare exactement les trois profondeurs de **son** couple, et une
    /// demande faite au format d'une **autre** variante est refusée. C'est l'accord des
    /// deux bouts d'un câble, vérifié sur les 24 × 24 couples plutôt que sur un seul.
    #[test]
    fn chaque_variante_n_accepte_que_sa_ligne() {
        for variante in 0..VARIANT_COUNT {
            let rate = variant_rate(variante).unwrap();
            let channels = variant_channels(variante).unwrap();
            let jeu = cable_formats(rate, channels);
            for autre in 0..VARIANT_COUNT {
                let r = variant_rate(autre).unwrap();
                let c = variant_channels(autre).unwrap();
                for depth in SAMPLE_DEPTHS {
                    let bits = depth.bits_per_sample() as u16;
                    let kind = if matches!(depth, SampleFormat::F32) {
                        SampleKind::Float
                    } else {
                        SampleKind::Pcm
                    };
                    let demande = req(r, u16::from(c), bits, bits, kind);
                    assert_eq!(
                        validate(&demande, &jeu).is_ok(),
                        autre == variante,
                        "variante {variante} contre {autre}"
                    );
                }
            }
        }
    }

    #[test]
    fn error_display_is_french() {
        assert!(FormatError::Unsupported.to_string().contains("supportés"));
    }

    #[test]
    fn buffer_bytes_table() {
        // 48 kHz stéréo F32 : 8 octets/trame, plancher 48 trames (384 o), plafond
        // 24 000 trames (192 000 o).
        assert_eq!(buffer_bytes(0, 8, 48_000), Some(384));
        assert_eq!(buffer_bytes(1, 8, 48_000), Some(384));
        assert_eq!(buffer_bytes(384, 8, 48_000), Some(384));
        // 10 ms demandés par le moteur en mode partagé.
        assert_eq!(buffer_bytes(3_840, 8, 48_000), Some(3_840));
        // Arrondi vers le haut à la trame.
        assert_eq!(buffer_bytes(3_841, 8, 48_000), Some(3_848));
        assert_eq!(buffer_bytes(3_847, 8, 48_000), Some(3_848));
        // 200 ms en mode exclusif : accepté, alors que l'ancien plafond de 100 ms aurait
        // refusé la création du flux.
        assert_eq!(buffer_bytes(76_800, 8, 48_000), Some(76_800));
        // Plafond 500 ms : la demande exacte passe.
        assert_eq!(buffer_bytes(192_000, 8, 48_000), Some(192_000));
        // Au-delà : refus, jamais un tampon plus petit que demandé. `AllocateAudioBuffer`
        // exige « at least the requested size » ; un écrêtage ferait échouer la création
        // du flux côté moteur audio, sans que le pilote sache pourquoi.
        assert_eq!(buffer_bytes(192_001, 8, 48_000), None);
        assert_eq!(buffer_bytes(192_008, 8, 48_000), None);
        assert_eq!(buffer_bytes(u32::MAX, 8, 48_000), None);
        // 44,1 kHz I16 stéréo : 1 ms = 44,1 trames → 45 ; 500 ms = 22 050 trames.
        assert_eq!(buffer_bytes(0, 4, 44_100), Some(180));
        assert_eq!(buffer_bytes(88_200, 4, 44_100), Some(88_200));
        assert_eq!(buffer_bytes(88_201, 4, 44_100), None);
        assert_eq!(buffer_bytes(u32::MAX, 4, 44_100), None);
        // Cas invalides.
        assert_eq!(buffer_bytes(1_000, 0, 48_000), None);
        assert_eq!(buffer_bytes(1_000, 8, 0), None);
        // 1 Hz, seule fréquence non nulle aux bornes incohérentes : 1 ms = 1 trame (ceil)
        // > 500 ms = 0 trame (floor). Dès 2 Hz la garde ne se déclenche plus.
        assert_eq!(buffer_bytes(1_000, 8, 1), None);
        assert_eq!(buffer_bytes(8, 8, 2), Some(8));
        // u32::MAX Hz : la demande maximale tient dans les 500 ms (2 147 483 647 trames)
        // mais pas dans un u32 une fois multipliée par 32 octets ; la borne basse
        // (1 ms = 4 294 968 trames) tient encore.
        assert_eq!(buffer_bytes(u32::MAX, 32, u32::MAX), None);
        assert_eq!(buffer_bytes(1_000, 32, u32::MAX), Some(4_294_968 * 32));
    }

    /// Le plancher réglable (`BufferMs`, M1b-05) : il **remonte** les petites demandes et
    /// ne touche pas aux grandes.
    #[test]
    fn buffer_bytes_with_floor_table() {
        // 48 kHz stéréo F32, 8 octets/trame. Plancher par défaut du pilote : 10 ms, soit
        // 480 trames = 3 840 octets.
        assert_eq!(buffer_bytes_with_floor(0, 8, 48_000, 10), Some(3_840));
        assert_eq!(buffer_bytes_with_floor(1, 8, 48_000, 10), Some(3_840));
        // Une demande de 3 ms est remontée à 10 : c'est la conséquence assumée du
        // paramètre (voir la documentation de la fonction).
        assert_eq!(buffer_bytes_with_floor(1_152, 8, 48_000, 10), Some(3_840));
        // Une demande plus grande passe telle quelle, arrondie à la trame.
        assert_eq!(buffer_bytes_with_floor(7_680, 8, 48_000, 10), Some(7_680));
        assert_eq!(buffer_bytes_with_floor(7_681, 8, 48_000, 10), Some(7_688));
        // Plancher 1 ms : le comportement d'avant M1b-05, celui de `buffer_bytes`.
        assert_eq!(buffer_bytes_with_floor(0, 8, 48_000, 1), Some(384));
        assert_eq!(buffer_bytes(0, 8, 48_000), Some(384));
        assert_eq!(
            buffer_bytes_with_floor(1_152, 8, 48_000, 1),
            buffer_bytes(1_152, 8, 48_000)
        );
        // Plancher 5 ms (« réglable à 10 ms » sur le chemin complet, SPEC §5.6).
        assert_eq!(buffer_bytes_with_floor(0, 8, 48_000, 5), Some(1_920));
        // Plancher au plafond : 500 ms, exactement le maximum acceptable.
        assert_eq!(buffer_bytes_with_floor(0, 8, 48_000, 500), Some(192_000));
        // Plancher **au-dessus** du plafond : écrêté à 500 ms, jamais un refus général.
        assert_eq!(buffer_bytes_with_floor(0, 8, 48_000, 501), Some(192_000));
        assert_eq!(
            buffer_bytes_with_floor(0, 8, 48_000, u32::MAX),
            Some(192_000)
        );
        // Plancher nul : remonté à 1 ms, jamais un tampon vide.
        assert_eq!(buffer_bytes_with_floor(0, 8, 48_000, 0), Some(384));
        // Le refus de la borne haute est indépendant du plancher.
        assert_eq!(buffer_bytes_with_floor(192_001, 8, 48_000, 10), None);
        assert_eq!(buffer_bytes_with_floor(u32::MAX, 8, 48_000, 1), None);
        // 44,1 kHz I16 : 10 ms = 441 trames = 1 764 octets.
        assert_eq!(buffer_bytes_with_floor(0, 4, 44_100, 10), Some(1_764));
        // Les cas invalides restent invalides quel que soit le plancher.
        assert_eq!(buffer_bytes_with_floor(1_000, 0, 48_000, 10), None);
        assert_eq!(buffer_bytes_with_floor(1_000, 8, 0, 10), None);
        // Le plancher aligné sur les notifications hérite de tout cela.
        assert_eq!(
            buffer_bytes_for_notifications_with_floor(0, 8, 48_000, 2, 10),
            Some(3_840)
        );
        // 441 trames est **impair** : deux périodes de notification l'arrondissent à 442.
        // L'alignement ne peut que remonter la taille, jamais l'abaisser sous le plancher.
        assert_eq!(
            buffer_bytes_for_notifications_with_floor(0, 4, 44_100, 2, 10),
            Some(1_768)
        );
        assert_eq!(
            buffer_bytes_for_notifications_with_floor(0, 4, 44_100, 1, 10),
            Some(1_764)
        );
        assert_eq!(
            buffer_bytes_for_notifications(0, 4, 44_100, 2),
            buffer_bytes_for_notifications_with_floor(0, 4, 44_100, 2, MIN_BUFFER_MS)
        );
    }

    #[test]
    fn buffer_bytes_for_notifications_table() {
        // 48 kHz F32 stéréo, 10 ms : 480 trames, déjà pair.
        assert_eq!(
            buffer_bytes_for_notifications(3_840, 8, 48_000, 1),
            Some(3_840)
        );
        assert_eq!(
            buffer_bytes_for_notifications(3_840, 8, 48_000, 2),
            Some(3_840)
        );
        // 481 trames demandées : 482 avec deux notifications.
        assert_eq!(
            buffer_bytes_for_notifications(3_848, 8, 48_000, 1),
            Some(3_848)
        );
        assert_eq!(
            buffer_bytes_for_notifications(3_848, 8, 48_000, 2),
            Some(3_856)
        );
        // 44,1 kHz I16 : 1 ms = 45 trames → 46 avec deux notifications.
        assert_eq!(buffer_bytes_for_notifications(0, 4, 44_100, 2), Some(184));
        // Plafond 500 ms = 24 000 trames, pair : inchangé.
        assert_eq!(
            buffer_bytes_for_notifications(192_000, 8, 48_000, 2),
            Some(192_000)
        );
        // 44,1 kHz : 22 050 trames, pair.
        assert_eq!(
            buffer_bytes_for_notifications(88_200, 4, 44_100, 2),
            Some(88_200)
        );
        // Le refus de la borne haute est hérité de `buffer_bytes` : pas d'écrêtage ici
        // non plus.
        assert_eq!(buffer_bytes_for_notifications(192_001, 8, 48_000, 2), None);
        assert_eq!(buffer_bytes_for_notifications(u32::MAX, 8, 48_000, 2), None);
        assert_eq!(buffer_bytes_for_notifications(u32::MAX, 4, 44_100, 2), None);
        // Seul l'alignement peut dépasser les 500 ms, d'au plus `count − 1` trames :
        // 48 010 Hz, 4 octets/trame, plafond 24 005 trames (impair) → 24 006.
        assert_eq!(
            buffer_bytes_for_notifications(24_005 * 4, 4, 48_010, 2),
            Some(24_006 * 4)
        );
        assert_eq!(buffer_bytes_for_notifications(3_840, 8, 48_000, 0), None);
        assert_eq!(buffer_bytes_for_notifications(3_840, 0, 48_000, 1), None);
    }

    proptest! {
        /// Multiple de `frame_bytes × count`, au moins `buffer_bytes` (donc au moins la
        /// demande), moins de `count` trames au-dessus ; refuse exactement quand
        /// [`buffer_bytes`] refuse.
        #[test]
        fn buffer_bytes_for_notifications_aligned(
            requested in any::<u32>(),
            frame_bytes in 1u32..=32,
            sample_rate in 8_000u32..=384_000,
            count in 1u32..=2,
        ) {
            let base = buffer_bytes(requested, frame_bytes, sample_rate);
            let bytes = buffer_bytes_for_notifications(requested, frame_bytes, sample_rate, count);
            if let Some(base) = base {
                let bytes = bytes.unwrap();
                prop_assert_eq!(bytes % (frame_bytes * count), 0);
                prop_assert!(bytes >= base);
                prop_assert!(bytes >= requested, "moins que demandé : {bytes} < {requested}");
                prop_assert!(bytes - base < frame_bytes * count);
            } else {
                prop_assert!(bytes.is_none(), "aligné alors que buffer_bytes a refusé");
            }
        }

        /// Contrat : **jamais moins que demandé, ou rien**. Le résultat est un multiple
        /// de `frame_bytes`, au moins la demande et au moins 1 ms ; une demande au-delà
        /// de 500 ms est refusée, jamais rabotée.
        #[test]
        fn buffer_bytes_never_shrinks(
            requested in any::<u32>(),
            frame_bytes in 1u32..=32,
            sample_rate in 8_000u32..=384_000,
        ) {
            let rate = u64::from(sample_rate);
            let min = rate.div_ceil(1_000);
            let max = rate / 2;
            let requested_frames = u64::from(requested).div_ceil(u64::from(frame_bytes));
            match buffer_bytes(requested, frame_bytes, sample_rate) {
                Some(bytes) => {
                    prop_assert!(requested_frames <= max, "accepté au-delà de 500 ms");
                    prop_assert_eq!(bytes % frame_bytes, 0);
                    prop_assert!(bytes >= requested, "moins que demandé : {bytes} < {requested}");
                    let frames = u64::from(bytes / frame_bytes);
                    prop_assert!(frames * 1_000 >= rate, "moins de 1 ms : {frames} trames");
                    prop_assert_eq!(frames, requested_frames.max(min));
                }
                None => {
                    // Dans ces plages, le seul refus possible est le dépassement du
                    // plafond (les bornes sont cohérentes et les tailles tiennent dans
                    // un u32).
                    prop_assert!(requested_frames > max, "refus d'une demande sous les 500 ms");
                }
            }
        }

        /// Le plancher réglable ne casse aucune propriété de [`buffer_bytes`] : le
        /// résultat est un multiple de la trame, au moins la demande, au moins `floor_ms`
        /// de son, et le refus reste celui du plafond.
        #[test]
        fn buffer_bytes_with_floor_never_shrinks(
            requested in any::<u32>(),
            frame_bytes in 1u32..=32,
            sample_rate in 8_000u32..=384_000,
            floor_ms in any::<u32>(),
        ) {
            let rate = u64::from(sample_rate);
            let borne = floor_ms.clamp(MIN_BUFFER_MS, MAX_BUFFER_MS);
            let requested_frames = u64::from(requested).div_ceil(u64::from(frame_bytes));
            let max = rate * u64::from(MAX_BUFFER_MS) / 1_000;
            match buffer_bytes_with_floor(requested, frame_bytes, sample_rate, floor_ms) {
                Some(bytes) => {
                    prop_assert_eq!(bytes % frame_bytes, 0);
                    prop_assert!(bytes >= requested, "moins que demandé : {bytes} < {requested}");
                    let frames = u64::from(bytes / frame_bytes);
                    prop_assert!(
                        frames * 1_000 >= rate * u64::from(borne),
                        "sous le plancher de {borne} ms : {frames} trames"
                    );
                    prop_assert!(requested_frames <= max, "accepté au-delà de 500 ms");
                }
                None => {
                    // Dans ces plages, le seul refus est le dépassement du plafond : le
                    // plancher écrêté ne peut jamais passer au-dessus (à 8 kHz, 500 ms font
                    // 4 000 trames et le ceil du plancher au plus 4 000 aussi).
                    prop_assert!(requested_frames > max, "refus d'une demande sous les 500 ms");
                }
            }
        }

        /// Un format supporté s'accepte lui-même ; changer un champ le refuse — sur les
        /// **trois** profondeurs, y compris le conteneur élargi (`bits + 8`), qui est la
        /// forme du PCM 24-dans-32.
        #[test]
        fn supported_accepts_itself(
            sample_rate in 1u32..=384_000,
            channels in 1u8..=8,
            depth in 0usize..3,
        ) {
            let format = SAMPLE_DEPTHS[depth];
            let s = SupportedFormat { sample_rate, channels, format };
            let bits = format.bits_per_sample() as u16;
            let is_float = matches!(format, SampleFormat::F32);
            let kind = if is_float { SampleKind::Float } else { SampleKind::Pcm };
            let r = req(sample_rate, u16::from(channels), bits, bits, kind);
            prop_assert_eq!(validate(&r, &[s]), Ok(s));
            let other_kind = if is_float { SampleKind::Pcm } else { SampleKind::Float };
            let variants = [
                RequestedFormat { kind: other_kind, ..r },
                RequestedFormat { kind: SampleKind::Other, ..r },
                RequestedFormat { valid_bits: bits - 1, ..r },
                // Conteneur plus large, bits valides inchangés : la forme du PCM
                // 24-dans-32 que le moteur audio propose volontiers.
                RequestedFormat { bits_per_sample: bits + 8, ..r },
                RequestedFormat { sample_rate: sample_rate + 1, ..r },
                RequestedFormat { channels: u16::from(channels) + 1, ..r },
            ];
            for variant in variants {
                let refused = validate(&variant, &[s]).is_err();
                prop_assert!(refused, "{:?} accepté par {:?}", variant, s);
            }
        }

        /// Un câble ne reconnaît **que** son propre réglage : quelle que soit la demande,
        /// elle passe si et seulement si sa fréquence et ses canaux sont ceux du câble et
        /// que sa profondeur est une des trois, conteneur exact.
        #[test]
        fn un_cable_accepte_exactement_sa_ligne_de_la_matrice(
            rate_index in 0usize..3,
            channels in 1u8..=8,
            demande_rate in prop_oneof![
                Just(RATE_44100), Just(RATE_48000), Just(RATE_96000), 1u32..=384_000,
            ],
            demande_channels in 1u16..=16,
            bits in prop_oneof![Just(16u16), Just(24), Just(32), 1u16..=64],
            valid in prop_oneof![Just(16u16), Just(24), Just(32), 1u16..=64],
            float in any::<bool>(),
        ) {
            let rate = sample_rate_at(rate_index).unwrap();
            let jeu = cable_formats(rate, channels);
            let kind = if float { SampleKind::Float } else { SampleKind::Pcm };
            let r = req(demande_rate, demande_channels, bits, valid, kind);
            let profondeur_connue = bits == valid
                && ((float && bits == 32) || (!float && (bits == 24 || bits == 16)));
            let attendu = demande_rate == rate
                && demande_channels == u16::from(channels)
                && profondeur_connue;
            prop_assert_eq!(validate(&r, &jeu).is_ok(), attendu, "{:?}", r);
        }
    }
}
