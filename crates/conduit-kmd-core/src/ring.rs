//! Copie cyclique rendu → capture (`docs/driver-design.md` §5.3).
//!
//! Le DPC du câble copie, à chaque période, les trames comprises entre la dernière
//! position copiée et la position de rendu courante, du tampon cyclique du flux de
//! rendu vers celui du flux de capture. Les deux tampons ont des tailles et des
//! formats indépendants ([`FrameLayout`]) ; les positions sont des numéros de trame
//! absolus (ceux de [`crate::position`]) réduits ici modulo la taille du tampon.
//!
//! Invariants :
//!
//! - `count` ne dépasse jamais la taille du plus petit tampon : on ne recouvre pas
//!   plus d'un tour, sinon des trames seraient écrasées avant d'être lues ;
//! - aucune indexation non vérifiée : `chunks_exact`, `skip`, `take`, `zip` ;
//! - les échantillons sont en petit-boutiste (`from_le_bytes`/`to_le_bytes`), le
//!   seul ordre que Windows x86/ARM64 utilise pour l'audio.
//!
//! Formats (M1b-05) : F32, PCM24 et I16, dans les deux sens — les neuf couples sont
//! servis. [`SampleFormat`] reste `#[non_exhaustive]` : la matrice de M1b-05 est close,
//! mais rien n'oblige la suivante à l'être.
//!
//! # PCM24 casse un alignement que les deux autres donnaient gratuitement
//!
//! F32 et I16 font 4 et 2 octets ; toute trame de *n* canaux y est alignée sur la taille
//! d'un échantillon, et les tampons cycliques du moteur audio (multiples de la page) le
//! sont aussi. PCM24 fait **3** octets : une trame stéréo en fait 6, une trame à 5 canaux
//! en fait 15, et plus rien n'est une puissance de deux. Trois conséquences, toutes déjà
//! traitées ici : aucune lecture ne passe par un `u32` transtypé (les trois octets se
//! lisent et s'écrivent un par un, [`i24_from_le`]), la taille du tampon reste un multiple
//! **de la trame** et non de la page ([`frames_in`] le vérifie, `crate::format` l'arrondit),
//! et les conversions itèrent par `chunks_exact` sur des tailles constantes plutôt que par
//! indexation.
//!
//! IRQL : tout ce module est appelable à `DISPATCH_LEVEL`.

use core::fmt;
use core::num::NonZeroUsize;

/// Format d'un échantillon en mémoire.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    /// IEEE 754 simple précision, plage nominale [−1, 1].
    F32,
    /// PCM signé 24 bits, **conteneur de 3 octets** (et non 24 bits significatifs dans un
    /// conteneur de 32) : c'est ce que `KSDATARANGE_AUDIO` annonce en
    /// `Minimum/MaximumBitsPerSample = 24`, et c'est ce que le tampon contient.
    Pcm24,
    /// PCM signé 16 bits.
    I16,
}

impl SampleFormat {
    /// Taille d'un échantillon en octets.
    pub const fn bytes_per_sample(self) -> u32 {
        match self {
            Self::F32 => 4,
            Self::Pcm24 => 3,
            Self::I16 => 2,
        }
    }

    /// Taille du conteneur en **bits**, celle que `wBitsPerSample` et les bornes de
    /// `KSDATARANGE_AUDIO` portent.
    pub const fn bits_per_sample(self) -> u32 {
        self.bytes_per_sample().saturating_mul(8)
    }
}

/// Disposition d'une trame : canaux entrelacés d'un même format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameLayout {
    channels: u8,
    format: SampleFormat,
}

impl FrameLayout {
    /// Nombre maximal de canaux par câble (SPEC : 1 à 8).
    pub const MAX_CHANNELS: u8 = 8;

    /// Crée une disposition ; `None` si `channels` n'est pas dans `1..=8`.
    pub const fn new(channels: u8, format: SampleFormat) -> Option<Self> {
        if channels == 0 || channels > Self::MAX_CHANNELS {
            return None;
        }
        Some(Self { channels, format })
    }

    /// Nombre de canaux.
    pub const fn channels(self) -> u8 {
        self.channels
    }

    /// Format des échantillons.
    pub const fn format(self) -> SampleFormat {
        self.format
    }

    /// Taille d'une trame en octets (`channels × bytes_per_sample`, au plus 32).
    pub const fn frame_bytes(self) -> u32 {
        (self.channels as u32).saturating_mul(self.format.bytes_per_sample())
    }
}

/// Erreurs de [`copy_frames`] et [`silence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingError {
    /// Les nombres de canaux de la source et de la destination diffèrent.
    ChannelMismatch,
    /// La longueur d'un tampon n'est pas un multiple de sa taille de trame.
    Misaligned,
    /// `count` dépasse la capacité du plus petit tampon (plus d'un tour).
    CountTooLarge,
    /// Un tampon est vide.
    EmptyBuffer,
}

impl fmt::Display for RingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ChannelMismatch => {
                "les nombres de canaux de la source et de la destination diffèrent"
            }
            Self::Misaligned => "la longueur du tampon n'est pas un multiple de la taille de trame",
            Self::CountTooLarge => {
                "le nombre de trames à copier dépasse la capacité du plus petit tampon"
            }
            Self::EmptyBuffer => "le tampon est vide",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RingError {}

/// Copie `count` trames du tampon cyclique `src` vers le tampon cyclique `dst`.
///
/// La trame `k` (pour `k` dans `0..count`) est lue à l'indice
/// `(src_start_frame + k) mod src_frames` et écrite à l'indice
/// `(dst_start_frame + k) mod dst_frames`, avec conversion d'échantillons si les
/// formats diffèrent ([`i16_to_f32`], [`f32_to_i16`], [`i24_to_f32`], [`f32_to_i24`],
/// [`i16_to_i24`], [`i24_to_i16`]).
///
/// # Erreurs
///
/// Dans cet ordre : [`RingError::ChannelMismatch`] si les dispositions n'ont pas le
/// même nombre de canaux ; [`RingError::EmptyBuffer`] ou [`RingError::Misaligned`]
/// si un tampon est vide ou n'est pas un multiple entier de trames ;
/// [`RingError::CountTooLarge`] si `count` dépasse la taille en trames du plus petit
/// tampon. `count == 0` est accepté et ne fait rien.
///
/// IRQL : `<= DISPATCH_LEVEL`.
pub fn copy_frames(
    src: &[u8],
    src_layout: FrameLayout,
    src_start_frame: u64,
    dst: &mut [u8],
    dst_layout: FrameLayout,
    dst_start_frame: u64,
    count: u64,
) -> Result<(), RingError> {
    if src_layout.channels() != dst_layout.channels() {
        return Err(RingError::ChannelMismatch);
    }
    let src_frames = frames_in(src, src_layout)?;
    let dst_frames = frames_in(dst, dst_layout)?;
    let count = checked_count(count, src_frames.min(dst_frames))?;
    let si = wrap_index(src_start_frame, src_frames);
    let di = wrap_index(dst_start_frame, dst_frames);
    let sfb = frame_bytes_usize(src_layout);
    let dfb = frame_bytes_usize(dst_layout);

    // Un tour complet de la source à partir de `si` ; `count ≤ src_frames` suffit.
    let mut src_iter = src
        .chunks_exact(sfb)
        .skip(si)
        .chain(src.chunks_exact(sfb).take(si));
    // La destination en deux segments : de `di` jusqu'à la fin, puis depuis le début.
    let first = count.min(dst_frames.get().saturating_sub(di));
    let second = count.saturating_sub(first);
    let (src_format, dst_format) = (src_layout.format(), dst_layout.format());

    for (d, s) in dst
        .chunks_exact_mut(dfb)
        .skip(di)
        .take(first)
        .zip(src_iter.by_ref())
    {
        convert_frame(s, src_format, d, dst_format);
    }
    for (d, s) in dst.chunks_exact_mut(dfb).take(second).zip(src_iter) {
        convert_frame(s, src_format, d, dst_format);
    }
    Ok(())
}

/// Écrit `count` trames de silence dans le tampon cyclique `dst` à partir de
/// `start_frame` (même sémantique modulo que [`copy_frames`]).
///
/// Des octets nuls valent 0,0 en F32, 0 en PCM24 et 0 en I16 : le silence est le même
/// pour tous les formats supportés, ce qui vaut d'être vérifié plutôt que supposé (un
/// format à décalage — PCM non signé, µ-law — casserait cette propriété).
///
/// # Erreurs
///
/// [`RingError::EmptyBuffer`], [`RingError::Misaligned`], ou
/// [`RingError::CountTooLarge`] si `count` dépasse la taille du tampon.
///
/// IRQL : `<= DISPATCH_LEVEL`.
pub fn silence(
    dst: &mut [u8],
    layout: FrameLayout,
    start_frame: u64,
    count: u64,
) -> Result<(), RingError> {
    let dst_frames = frames_in(dst, layout)?;
    let count = checked_count(count, dst_frames)?;
    let di = wrap_index(start_frame, dst_frames);
    let fb = frame_bytes_usize(layout);
    let first = count.min(dst_frames.get().saturating_sub(di));
    let second = count.saturating_sub(first);
    for chunk in dst.chunks_exact_mut(fb).skip(di).take(first) {
        chunk.fill(0);
    }
    for chunk in dst.chunks_exact_mut(fb).take(second) {
        chunk.fill(0);
    }
    Ok(())
}

/// I16 → F32 : division par 32 768, si bien que −32 768 ↦ −1,0 exactement et
/// 32 767 ↦ 0,99997 (convention WASAPI et de la plupart des convertisseurs).
pub fn i16_to_f32(v: i16) -> f32 {
    f32::from(v) / 32_768.0
}

/// F32 → I16 : borné à [−1, 1] puis multiplié par 32 767 et arrondi au plus proche
/// (demi vers l'extérieur).
///
/// Choix documenté : le facteur 32 767 (et non 32 768) rend la conversion
/// symétrique — +1,0 et −1,0 donnent ±32 767 — et ne peut jamais déborder, au prix
/// d'un aller-retour F32 → I16 → F32 qui n'est pas l'identité : erreur au plus
/// `1,5 / 32 768` (un demi-pas de quantification plus l'écart d'échelle
/// 32 767 / 32 768). Les valeurs hors plage sont écrêtées ; `NaN` donne 0.
pub fn f32_to_i16(v: f32) -> i16 {
    // `clamp` renvoie NaN pour NaN, que `as i16` transforme en 0.
    let scaled = v.clamp(-1.0, 1.0) * 32_767.0;
    // Arrondi au plus proche sans `f32::round` (absent de `core` pour la MSRV) :
    // `as i16` tronque vers zéro et sature, d'où le ±0,5 selon le signe.
    if scaled >= 0.0 {
        (scaled + 0.5) as i16
    } else {
        (scaled - 0.5) as i16
    }
}

/// Valeur PCM24 la plus négative (−2^23), celle que −1,0 donne exactement.
pub const I24_MIN: i32 = -8_388_608;
/// Valeur PCM24 la plus positive (2^23 − 1).
pub const I24_MAX: i32 = 8_388_607;

/// PCM24 → F32 : division par 8 388 608, la transposition exacte de [`i16_to_f32`] —
/// [`I24_MIN`] ↦ −1,0 exactement, [`I24_MAX`] ↦ 0,999999881.
///
/// `v` est supposé dans `I24_MIN..=I24_MAX` ([`read_i24_le`] ne peut rien produire
/// d'autre) ; au-delà, le résultat sort simplement de [−1, 1], sans panique.
pub fn i24_to_f32(v: i32) -> f32 {
    v as f32 / 8_388_608.0
}

/// F32 → PCM24 : borné à [−1, 1] puis multiplié par 8 388 607 et arrondi au plus proche
/// (demi vers l'extérieur), **exactement** la convention de [`f32_to_i16`].
///
/// Le facteur [`I24_MAX`] (et non 2^23) rend la conversion symétrique et lui interdit de
/// déborder ; `NaN` donne 0, `clamp` le propageant et `as i32` le transformant en 0. Le
/// résultat est toujours dans `I24_MIN..=I24_MAX`, ce qu'un `f32` ne garantit pas
/// gratuitement — un `f32` ne représente pas tous les entiers de cette plage, et
/// 8 388 607,0 s'y arrondit à 8 388 607,0 mais 8 388 607,5 à 8 388 608,0. D'où l'écrêtage
/// final, qui n'est pas décoratif.
pub fn f32_to_i24(v: f32) -> i32 {
    let scaled = v.clamp(-1.0, 1.0) * 8_388_607.0;
    let arrondi = if scaled >= 0.0 {
        (scaled + 0.5) as i32
    } else {
        (scaled - 0.5) as i32
    };
    arrondi.clamp(I24_MIN, I24_MAX)
}

/// I16 → PCM24 : décalage de 8 bits, **exact et sans perte** (l'échelle des deux formats
/// entiers est la même puissance de deux).
///
/// C'est le seul couple de la matrice dont l'aller-retour est l'identité :
/// [`i24_to_i16`] rend bien la valeur de départ.
pub fn i16_to_i24(v: i16) -> i32 {
    i32::from(v).saturating_mul(256)
}

/// PCM24 → I16 : division par 256 **arrondie au plus proche**, puis écrêtée.
///
/// L'arrondi (le `+ 128` avant le décalage) plutôt qu'une troncature : sans lui, tout
/// échantillon négatif serait tiré d'un pas vers −∞, ce qui ajoute une composante continue
/// audible sur un signal de faible niveau. L'écrêtage ne mord que sur les 128 valeurs les
/// plus hautes de PCM24, que l'arrondi ferait déborder de 32 767.
pub fn i24_to_i16(v: i32) -> i16 {
    let arrondi = v.saturating_add(128) >> 8;
    arrondi.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Convertit une trame de `src` (format `src_format`) vers `dst` (format
/// `dst_format`). Les deux tranches ont exactement une trame du même nombre de
/// canaux (garanti par `chunks_exact` et la vérification des dispositions).
///
/// Les neuf couples sont écrits en toutes lettres : les trois identités par recopie
/// d'octets, les six conversions par [`convert_samples`], dont les deux tailles de
/// conteneur sont des paramètres `const`. Une taille fausse ne compilerait pas.
fn convert_frame(src: &[u8], src_format: SampleFormat, dst: &mut [u8], dst_format: SampleFormat) {
    use SampleFormat::{Pcm24, F32, I16};
    match (src_format, dst_format) {
        (F32, F32) | (Pcm24, Pcm24) | (I16, I16) => write_bytes(dst, src),
        (I16, F32) => convert_samples::<2, 4>(src, dst, |s| {
            i16_to_f32(i16::from_le_bytes(s)).to_le_bytes()
        }),
        (F32, I16) => convert_samples::<4, 2>(src, dst, |s| {
            f32_to_i16(f32::from_le_bytes(s)).to_le_bytes()
        }),
        (Pcm24, F32) => {
            convert_samples::<3, 4>(src, dst, |s| i24_to_f32(i24_from_le(s)).to_le_bytes());
        }
        (F32, Pcm24) => {
            convert_samples::<4, 3>(src, dst, |s| i24_to_le(f32_to_i24(f32::from_le_bytes(s))));
        }
        (I16, Pcm24) => {
            convert_samples::<2, 3>(src, dst, |s| i24_to_le(i16_to_i24(i16::from_le_bytes(s))));
        }
        (Pcm24, I16) => {
            convert_samples::<3, 2>(src, dst, |s| i24_to_i16(i24_from_le(s)).to_le_bytes());
        }
    }
}

/// Applique `convertir` à chaque échantillon de `src` (conteneur de `S` octets) vers
/// `dst` (conteneur de `D` octets), sur le nombre commun d'échantillons.
///
/// `chunks_exact` des deux côtés : aucune indexation, aucun reste, et `zip` s'arrête au
/// plus court — la trame étant garantie de même nombre de canaux par [`copy_frames`], les
/// deux comptes sont en fait égaux.
fn convert_samples<const S: usize, const D: usize>(
    src: &[u8],
    dst: &mut [u8],
    convertir: impl Fn([u8; S]) -> [u8; D],
) {
    for (s, d) in src.chunks_exact(S).zip(dst.chunks_exact_mut(D)) {
        // `chunks_exact(S)` ne rend que des tranches de `S` octets : le `Ok` est certain,
        // et le motif remplace un `unwrap` interdit par les lints du crate.
        if let Ok(octets) = <[u8; S]>::try_from(s) {
            write_bytes(d, &convertir(octets));
        }
    }
}

/// Lit un entier signé de 24 bits petit-boutiste, **avec extension de signe** : les trois
/// octets sont placés en poids fort d'un `i32` puis décalés arithmétiquement.
///
/// C'est le seul endroit du module où le signe ne se déduit pas du type : un
/// `i32::from_le_bytes([a, b, c, 0])` rendrait 16 777 215 pour −1.
fn i24_from_le([a, b, c]: [u8; 3]) -> i32 {
    i32::from_le_bytes([0, a, b, c]) >> 8
}

/// Écrit un entier signé de 24 bits en petit-boutiste : les trois octets de poids faible,
/// le quatrième (le signe étendu) étant redondant.
fn i24_to_le(v: i32) -> [u8; 3] {
    let [a, b, c, _] = v.to_le_bytes();
    [a, b, c]
}

/// Copie `src` dans `dst` octet par octet, sur la longueur commune (jamais de
/// panique, contrairement à `copy_from_slice`).
fn write_bytes(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = *s;
    }
}

/// Taille de trame en `usize` (u32 → usize sans perte sur toute cible 32/64 bits).
const fn frame_bytes_usize(layout: FrameLayout) -> usize {
    layout.frame_bytes() as usize
}

/// Nombre de trames d'un tampon, vérifié : non vide et multiple entier de trames.
fn frames_in(buf: &[u8], layout: FrameLayout) -> Result<NonZeroUsize, RingError> {
    if buf.is_empty() {
        return Err(RingError::EmptyBuffer);
    }
    let fb = frame_bytes_usize(layout);
    // `fb ≥ 2` par construction de `FrameLayout` : les deux `None` sont inatteignables.
    let rem = buf.len().checked_rem(fb).ok_or(RingError::Misaligned)?;
    if rem != 0 {
        return Err(RingError::Misaligned);
    }
    let frames = buf.len().checked_div(fb).ok_or(RingError::Misaligned)?;
    // `len ≥ fb` (non vide et multiple de `fb`) donc `frames ≥ 1`.
    NonZeroUsize::new(frames).ok_or(RingError::EmptyBuffer)
}

/// Convertit `count` en `usize` et vérifie qu'il ne dépasse pas `capacity` trames.
fn checked_count(count: u64, capacity: NonZeroUsize) -> Result<usize, RingError> {
    usize::try_from(count)
        .ok()
        .filter(|&c| c <= capacity.get())
        .ok_or(RingError::CountTooLarge)
}

/// Ramène un numéro de trame absolu dans `0..frames`.
fn wrap_index(frame: u64, frames: NonZeroUsize) -> usize {
    // `frames.get()` tient dans un u64 sur toute cible ≤ 64 bits ; `frames ≠ 0`
    // donc le reste existe, et il est `< frames` donc tient dans un usize.
    let rem = frame.checked_rem(frames.get() as u64).unwrap_or(0);
    usize::try_from(rem).unwrap_or(0)
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
    use std::vec::Vec;

    /// Lit un `i16` petit-boutiste depuis une tranche de deux octets.
    fn read_i16_le(bytes: &[u8]) -> i16 {
        <[u8; 2]>::try_from(bytes).map_or(0, i16::from_le_bytes)
    }

    /// Lit un `f32` petit-boutiste depuis une tranche de quatre octets.
    fn read_f32_le(bytes: &[u8]) -> f32 {
        <[u8; 4]>::try_from(bytes).map_or(0.0, f32::from_le_bytes)
    }

    /// Lit un PCM24 signé petit-boutiste depuis une tranche de trois octets.
    fn read_i24_le(bytes: &[u8]) -> i32 {
        <[u8; 3]>::try_from(bytes).map_or(0, i24_from_le)
    }

    /// Valeur de test « unique » pour la trame `frame`, canal `channel` : entier
    /// `frame × 8 + channel`, réduit modulo 1 024 et mis à l'échelle dans chaque format
    /// pour rester **positif, distinct, et distinct après toute conversion** — c'est cette
    /// dernière propriété qui donne leur force aux tests de recouvrement, une conversion
    /// qui écraserait deux trames voisines sur la même valeur les rendrait aveugles.
    ///
    /// D'où l'échelle : 1/1 024 en F32 (pas de 32 valeurs I16 et de 8 192 valeurs PCM24),
    /// 8 192 en PCM24 (pas de 32 valeurs I16), 1 en I16. Les fenêtres vérifiées font moins
    /// de 1 024 échantillons.
    fn sample_bytes(format: SampleFormat, frame: usize, channel: usize) -> Vec<u8> {
        let n = (frame * 8 + channel) % 1024;
        match format {
            SampleFormat::F32 => (n as f32 / 1024.0).to_le_bytes().to_vec(),
            SampleFormat::Pcm24 => i24_to_le(n as i32 * 8_192).to_vec(),
            SampleFormat::I16 => (n as i16).to_le_bytes().to_vec(),
        }
    }

    /// Même valeur que [`sample_bytes`] en format `src`, convertie en format `dst`
    /// par les fonctions publiques du module — les neuf couples.
    fn converted_sample_bytes(
        src: SampleFormat,
        dst: SampleFormat,
        frame: usize,
        channel: usize,
    ) -> Vec<u8> {
        use SampleFormat::{Pcm24, F32, I16};
        let raw = sample_bytes(src, frame, channel);
        match (src, dst) {
            (F32, F32) | (Pcm24, Pcm24) | (I16, I16) => raw,
            (I16, F32) => i16_to_f32(read_i16_le(&raw)).to_le_bytes().to_vec(),
            (F32, I16) => f32_to_i16(read_f32_le(&raw)).to_le_bytes().to_vec(),
            (Pcm24, F32) => i24_to_f32(read_i24_le(&raw)).to_le_bytes().to_vec(),
            (F32, Pcm24) => i24_to_le(f32_to_i24(read_f32_le(&raw))).to_vec(),
            (I16, Pcm24) => i24_to_le(i16_to_i24(read_i16_le(&raw))).to_vec(),
            (Pcm24, I16) => i24_to_i16(read_i24_le(&raw)).to_le_bytes().to_vec(),
        }
    }

    /// Tampon de `frames` trames rempli avec [`sample_bytes`].
    fn filled(layout: FrameLayout, frames: usize) -> Vec<u8> {
        (0..frames)
            .flat_map(|f| {
                (0..usize::from(layout.channels()))
                    .flat_map(move |c| sample_bytes(layout.format(), f, c))
            })
            .collect()
    }

    /// Sentinelle négative (jamais produite par [`sample_bytes`], qui ne rend que des
    /// valeurs positives ou nulles, dans les trois formats).
    fn sentinel(format: SampleFormat) -> Vec<u8> {
        match format {
            SampleFormat::F32 => (-5.0f32).to_le_bytes().to_vec(),
            SampleFormat::Pcm24 => i24_to_le(-5 * 8_192).to_vec(),
            SampleFormat::I16 => (-5i16).to_le_bytes().to_vec(),
        }
    }

    fn stereo(format: SampleFormat) -> FrameLayout {
        FrameLayout::new(2, format).unwrap()
    }

    fn any_format() -> impl Strategy<Value = SampleFormat> {
        prop_oneof![
            Just(SampleFormat::F32),
            Just(SampleFormat::Pcm24),
            Just(SampleFormat::I16)
        ]
    }

    #[test]
    fn layout_bounds() {
        assert!(FrameLayout::new(0, SampleFormat::F32).is_none());
        assert!(FrameLayout::new(9, SampleFormat::I16).is_none());
        let l = FrameLayout::new(8, SampleFormat::F32).unwrap();
        assert_eq!(l.channels(), 8);
        assert_eq!(l.format(), SampleFormat::F32);
        assert_eq!(l.frame_bytes(), 32);
        assert_eq!(stereo(SampleFormat::I16).frame_bytes(), 4);
        assert_eq!(SampleFormat::F32.bytes_per_sample(), 4);
        assert_eq!(SampleFormat::Pcm24.bytes_per_sample(), 3);
        assert_eq!(SampleFormat::I16.bytes_per_sample(), 2);
        assert_eq!(SampleFormat::F32.bits_per_sample(), 32);
        assert_eq!(SampleFormat::Pcm24.bits_per_sample(), 24);
        assert_eq!(SampleFormat::I16.bits_per_sample(), 16);
        // PCM24 casse les alignements que les deux autres donnaient : une trame à 5
        // canaux fait 15 octets, et rien n'y est une puissance de deux (voir l'en-tête).
        assert_eq!(stereo(SampleFormat::Pcm24).frame_bytes(), 6);
        assert_eq!(
            FrameLayout::new(5, SampleFormat::Pcm24)
                .unwrap()
                .frame_bytes(),
            15
        );
        assert_eq!(
            FrameLayout::new(8, SampleFormat::Pcm24)
                .unwrap()
                .frame_bytes(),
            24
        );
    }

    #[test]
    fn copy_wraps_both_sides() {
        // src : 6 trames, dst : 4 trames, stéréo I16 ; copie de 3 trames à partir de
        // src[5] vers dst[3] : src 5,0,1 → dst 3,0,1 ; dst[2] intact.
        let src_l = stereo(SampleFormat::I16);
        let src = filled(src_l, 6);
        let mut dst: Vec<u8> = (0..4)
            .flat_map(|_| sentinel(SampleFormat::I16).repeat(2))
            .collect();
        copy_frames(&src, src_l, 5, &mut dst, src_l, 3, 3).unwrap();
        let frame = |b: &[u8], i: usize| b[i * 4..(i + 1) * 4].to_vec();
        assert_eq!(frame(&dst, 3), frame(&src, 5));
        assert_eq!(frame(&dst, 0), frame(&src, 0));
        assert_eq!(frame(&dst, 1), frame(&src, 1));
        assert_eq!(frame(&dst, 2), sentinel(SampleFormat::I16).repeat(2));
    }

    #[test]
    fn copy_converts_i16_to_f32_and_back() {
        let src_l = stereo(SampleFormat::I16);
        let dst_l = stereo(SampleFormat::F32);
        let src: Vec<u8> = [i16::MIN, 0, 16_384, i16::MAX]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut dst = [0u8; 16];
        copy_frames(&src, src_l, 0, &mut dst, dst_l, 0, 2).unwrap();
        let out: Vec<f32> = dst.chunks_exact(4).map(read_f32_le).collect();
        assert_eq!(out, [-1.0, 0.0, 0.5, 32_767.0 / 32_768.0]);

        let mut back = [0u8; 8];
        copy_frames(&dst, dst_l, 0, &mut back, src_l, 0, 2).unwrap();
        let out: Vec<i16> = back.chunks_exact(2).map(read_i16_le).collect();
        // −1,0 ↦ −32 767 (symétrie), 0,5 ↦ 16 384, 32 767/32 768 ↦ 32 766.
        assert_eq!(out, [-32_767, 0, 16_384, 32_766]);
    }

    #[test]
    fn copy_zero_frames_is_noop() {
        let l = stereo(SampleFormat::F32);
        let src = filled(l, 3);
        let mut dst = [7u8; 24];
        copy_frames(&src, l, 100, &mut dst, l, 200, 0).unwrap();
        assert_eq!(dst, [7u8; 24]);
    }

    #[test]
    fn copy_errors() {
        let f32_2 = stereo(SampleFormat::F32);
        let f32_1 = FrameLayout::new(1, SampleFormat::F32).unwrap();
        let src = [0u8; 32];
        let mut dst = [0u8; 16];
        assert_eq!(
            copy_frames(&src, f32_2, 0, &mut dst, f32_1, 0, 1),
            Err(RingError::ChannelMismatch)
        );
        assert_eq!(
            copy_frames(&src[..30], f32_2, 0, &mut dst, f32_2, 0, 1),
            Err(RingError::Misaligned)
        );
        assert_eq!(
            copy_frames(&src, f32_2, 0, &mut dst[..12], f32_2, 0, 1),
            Err(RingError::Misaligned)
        );
        assert_eq!(
            copy_frames(&src[..0], f32_2, 0, &mut dst, f32_2, 0, 0),
            Err(RingError::EmptyBuffer)
        );
        assert_eq!(
            copy_frames(&src, f32_2, 0, &mut dst[..0], f32_2, 0, 0),
            Err(RingError::EmptyBuffer)
        );
        // src : 4 trames, dst : 2 trames → au plus 2.
        assert_eq!(
            copy_frames(&src, f32_2, 0, &mut dst, f32_2, 0, 3),
            Err(RingError::CountTooLarge)
        );
        assert_eq!(
            copy_frames(&src, f32_2, 0, &mut dst, f32_2, 0, u64::MAX),
            Err(RingError::CountTooLarge)
        );
        assert!(copy_frames(&src, f32_2, 0, &mut dst, f32_2, 0, 2).is_ok());
    }

    #[test]
    fn silence_wraps() {
        let l = stereo(SampleFormat::I16);
        let mut dst = [0xFFu8; 40]; // 10 trames
        silence(&mut dst, l, 8, 4).unwrap(); // trames 8, 9, 0, 1
        for (i, frame) in dst.chunks_exact(4).enumerate() {
            let expected = if matches!(i, 8 | 9 | 0 | 1) {
                [0u8; 4]
            } else {
                [0xFFu8; 4]
            };
            assert_eq!(frame, expected, "trame {i}");
        }
        assert_eq!(silence(&mut dst, l, 0, 11), Err(RingError::CountTooLarge));
        assert_eq!(silence(&mut dst[..0], l, 0, 0), Err(RingError::EmptyBuffer));
        assert_eq!(silence(&mut dst[..6], l, 0, 0), Err(RingError::Misaligned));
        assert!(silence(&mut dst, l, u64::MAX, 10).is_ok());
        assert!(dst.iter().all(|&b| b == 0));
    }

    #[test]
    fn conversion_extremes() {
        assert_eq!(f32_to_i16(1.0), 32_767);
        assert_eq!(f32_to_i16(-1.0), -32_767);
        assert_eq!(f32_to_i16(2.0), 32_767);
        assert_eq!(f32_to_i16(-2.0), -32_767);
        assert_eq!(f32_to_i16(f32::INFINITY), 32_767);
        assert_eq!(f32_to_i16(f32::NEG_INFINITY), -32_767);
        assert_eq!(f32_to_i16(f32::NAN), 0);
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(-0.0), 0);
        assert_eq!(f32_to_i16(0.5), 16_384); // 16 383,5 arrondi vers l'extérieur
        assert_eq!(f32_to_i16(-0.5), -16_384);
        assert_eq!(i16_to_f32(i16::MIN), -1.0);
        assert_eq!(i16_to_f32(0), 0.0);
        assert_eq!(i16_to_f32(i16::MAX), 32_767.0 / 32_768.0);
    }

    /// PCM24 aux bornes : mêmes conventions que I16, transposées à 24 bits.
    #[test]
    fn conversion_extremes_pcm24() {
        assert_eq!(f32_to_i24(1.0), I24_MAX);
        assert_eq!(f32_to_i24(-1.0), -I24_MAX);
        assert_eq!(f32_to_i24(2.0), I24_MAX);
        assert_eq!(f32_to_i24(-2.0), -I24_MAX);
        assert_eq!(f32_to_i24(f32::INFINITY), I24_MAX);
        assert_eq!(f32_to_i24(f32::NEG_INFINITY), -I24_MAX);
        assert_eq!(f32_to_i24(f32::NAN), 0);
        assert_eq!(f32_to_i24(0.0), 0);
        assert_eq!(f32_to_i24(-0.0), 0);
        assert_eq!(i24_to_f32(I24_MIN), -1.0);
        assert_eq!(i24_to_f32(0), 0.0);
        assert_eq!(i24_to_f32(I24_MAX), 8_388_607.0 / 8_388_608.0);

        // I16 ↔ PCM24 : le seul couple de la matrice dont l'aller-retour est l'identité.
        assert_eq!(i16_to_i24(i16::MIN), I24_MIN);
        assert_eq!(i16_to_i24(i16::MAX), 8_388_352);
        assert_eq!(i16_to_i24(0), 0);
        assert_eq!(i16_to_i24(1), 256);
        assert_eq!(i16_to_i24(-1), -256);
        assert_eq!(i24_to_i16(I24_MIN), i16::MIN);
        assert_eq!(i24_to_i16(I24_MAX), i16::MAX);
        assert_eq!(i24_to_i16(0), 0);
        // L'arrondi au plus proche, et non la troncature vers −∞ : sans le `+ 128`, tout
        // échantillon négatif serait tiré d'un pas et le signal gagnerait une composante
        // continue.
        assert_eq!(i24_to_i16(127), 0);
        assert_eq!(i24_to_i16(128), 1);
        assert_eq!(i24_to_i16(-128), 0);
        assert_eq!(i24_to_i16(-129), -1);
        // L'écrêtage haut ne mord que sur les 128 dernières valeurs de PCM24.
        assert_eq!(i24_to_i16(8_388_479), i16::MAX);
        assert_eq!(i24_to_i16(8_388_480), i16::MAX);
    }

    /// Les trois octets d'un PCM24 se relisent avec leur signe, dans les deux sens.
    #[test]
    fn pcm24_bytes_are_sign_extended() {
        // −1 : `FF FF FF`. Sans extension de signe, la lecture rendrait 16 777 215.
        assert_eq!(i24_to_le(-1), [0xFF, 0xFF, 0xFF]);
        assert_eq!(i24_from_le([0xFF, 0xFF, 0xFF]), -1);
        assert_eq!(i24_to_le(I24_MIN), [0x00, 0x00, 0x80]);
        assert_eq!(i24_from_le([0x00, 0x00, 0x80]), I24_MIN);
        assert_eq!(i24_to_le(I24_MAX), [0xFF, 0xFF, 0x7F]);
        assert_eq!(i24_from_le([0xFF, 0xFF, 0x7F]), I24_MAX);
        assert_eq!(i24_to_le(0), [0, 0, 0]);
        assert_eq!(i24_from_le([0, 0, 0]), 0);
        // Petit-boutiste : 0x00_0102 s'écrit `02 01 00`.
        assert_eq!(i24_to_le(0x0000_0102), [0x02, 0x01, 0x00]);
    }

    /// La copie convertit bien dans les six sens croisés, sur une trame connue.
    #[test]
    fn copy_converts_across_the_matrix() {
        use SampleFormat::{Pcm24, F32, I16};
        let couples = [
            (I16, F32),
            (F32, I16),
            (Pcm24, F32),
            (F32, Pcm24),
            (I16, Pcm24),
            (Pcm24, I16),
        ];
        for (src_format, dst_format) in couples {
            let src_l = stereo(src_format);
            let dst_l = stereo(dst_format);
            let src = filled(src_l, 4);
            let mut dst: Vec<u8> = (0..4 * 2).flat_map(|_| sentinel(dst_format)).collect();
            copy_frames(&src, src_l, 0, &mut dst, dst_l, 0, 4).unwrap();
            let attendu: Vec<u8> = (0..4)
                .flat_map(|f| {
                    (0..2).flat_map(move |c| converted_sample_bytes(src_format, dst_format, f, c))
                })
                .collect();
            assert_eq!(dst, attendu, "{src_format:?} → {dst_format:?}");
        }
    }

    /// Le silence est le même octet nul dans les trois formats : c'est ce qui permet à
    /// [`silence`] d'ignorer la disposition.
    #[test]
    fn le_silence_est_nul_dans_les_trois_formats() {
        assert_eq!(0.0f32.to_le_bytes(), [0, 0, 0, 0]);
        assert_eq!(0i16.to_le_bytes(), [0, 0]);
        assert_eq!(i24_to_le(0), [0, 0, 0]);
        assert_eq!(i24_from_le([0, 0, 0]), 0);
    }

    #[test]
    fn error_display_is_french() {
        assert_eq!(RingError::EmptyBuffer.to_string(), "le tampon est vide");
        assert!(RingError::ChannelMismatch.to_string().contains("canaux"));
        assert!(RingError::Misaligned.to_string().contains("multiple"));
        assert!(RingError::CountTooLarge.to_string().contains("dépasse"));
    }

    /// Simule le DPC : un producteur écrit un compteur de trames dans `src` (tour
    /// après tour), la copie suit avec les mêmes numéros de trame absolus, et `dst`
    /// doit contenir à tout moment les `dst_frames` dernières trames produites.
    fn run_loop(
        src_format: SampleFormat,
        dst_format: SampleFormat,
        channels: u8,
        src_frames: usize,
        dst_frames: usize,
        steps: &[usize],
    ) {
        let src_l = FrameLayout::new(channels, src_format).unwrap();
        let dst_l = FrameLayout::new(channels, dst_format).unwrap();
        let ch = usize::from(channels);
        let mut src = std::vec![0u8; src_frames * src_l.frame_bytes() as usize];
        let mut dst = std::vec![0u8; dst_frames * dst_l.frame_bytes() as usize];
        let mut produced: u64 = 0;
        let limit = src_frames.min(dst_frames);
        for &step in steps {
            let step = step.min(limit);
            // Production : trames `produced .. produced + step` dans `src`.
            for k in 0..step {
                let abs = produced as usize + k;
                let slot = abs % src_frames;
                let fb = src_l.frame_bytes() as usize;
                let bytes: Vec<u8> = (0..ch)
                    .flat_map(|c| sample_bytes(src_format, abs, c))
                    .collect();
                src[slot * fb..(slot + 1) * fb].copy_from_slice(&bytes);
            }
            copy_frames(
                &src,
                src_l,
                produced,
                &mut dst,
                dst_l,
                produced,
                step as u64,
            )
            .unwrap();
            produced += step as u64;
            // Vérification : les `min(produced, dst_frames)` dernières trames.
            let n = (produced as usize).min(dst_frames);
            for back in 0..n {
                let abs = produced as usize - 1 - back;
                let slot = abs % dst_frames;
                let fb = dst_l.frame_bytes() as usize;
                let expected: Vec<u8> = (0..ch)
                    .flat_map(|c| converted_sample_bytes(src_format, dst_format, abs, c))
                    .collect();
                assert_eq!(
                    &dst[slot * fb..(slot + 1) * fb],
                    &expected[..],
                    "trame absolue {abs} (emplacement {slot})"
                );
            }
        }
    }

    #[test]
    fn loop_deterministic() {
        run_loop(
            SampleFormat::F32,
            SampleFormat::F32,
            2,
            480,
            360,
            &[100; 50],
        );
        run_loop(
            SampleFormat::I16,
            SampleFormat::F32,
            2,
            360,
            480,
            &[7, 359, 1, 360, 200],
        );
        run_loop(
            SampleFormat::F32,
            SampleFormat::I16,
            1,
            5,
            5,
            &[5, 5, 3, 4, 5],
        );
        // PCM24 : trames de 3, 15 et 21 octets — aucune puissance de deux, aucun tampon
        // aligné sur la page, et c'est exactement le point de M1b-05 (voir l'en-tête).
        run_loop(
            SampleFormat::Pcm24,
            SampleFormat::Pcm24,
            5,
            441,
            333,
            &[100; 20],
        );
        run_loop(
            SampleFormat::F32,
            SampleFormat::Pcm24,
            7,
            97,
            97,
            &[1, 96, 97, 3],
        );
        run_loop(
            SampleFormat::Pcm24,
            SampleFormat::I16,
            1,
            5,
            5,
            &[5, 5, 3, 4, 5],
        );
    }

    proptest! {
        /// Aucune trame perdue ni dupliquée : une copie de `count` trames avec
        /// wrap-around des deux côtés écrit exactement les bonnes trames aux bons
        /// indices et rien d'autre.
        #[test]
        fn copy_exact_frames(
            src_format in any_format(),
            dst_format in any_format(),
            channels in 1u8..=8,
            src_frames in 1usize..=48,
            dst_frames in 1usize..=48,
            src_start in any::<u64>(),
            dst_start in any::<u64>(),
            count_ratio in 0.0f64..=1.0,
        ) {
            let src_l = FrameLayout::new(channels, src_format).unwrap();
            let dst_l = FrameLayout::new(channels, dst_format).unwrap();
            let ch = usize::from(channels);
            let src = filled(src_l, src_frames);
            let count = (count_ratio * src_frames.min(dst_frames) as f64).round() as usize;
            let mut dst: Vec<u8> = (0..dst_frames * ch)
                .flat_map(|_| sentinel(dst_format))
                .collect();
            copy_frames(&src, src_l, src_start, &mut dst, dst_l, dst_start, count as u64).unwrap();

            let mut expected = dst.clone();
            let fb = dst_l.frame_bytes() as usize;
            let mut untouched = std::vec![true; dst_frames];
            for k in 0..count {
                let s = ((src_start % src_frames as u64) as usize + k) % src_frames;
                let d = ((dst_start % dst_frames as u64) as usize + k) % dst_frames;
                let bytes: Vec<u8> = (0..ch)
                    .flat_map(|c| converted_sample_bytes(src_format, dst_format, s, c))
                    .collect();
                expected[d * fb..(d + 1) * fb].copy_from_slice(&bytes);
                untouched[d] = false;
            }
            for (d, &u) in untouched.iter().enumerate() {
                if u {
                    let sent: Vec<u8> = (0..ch).flat_map(|_| sentinel(dst_format)).collect();
                    expected[d * fb..(d + 1) * fb].copy_from_slice(&sent);
                }
            }
            prop_assert_eq!(dst, expected);
        }

        /// Plusieurs tours de boucle locale avec des pas variables.
        #[test]
        fn loop_many_turns(
            src_format in any_format(),
            dst_format in any_format(),
            channels in 1u8..=4,
            src_frames in 1usize..=64,
            dst_frames in 1usize..=64,
            steps in proptest::collection::vec(1usize..=70, 1..=40),
        ) {
            run_loop(src_format, dst_format, channels, src_frames, dst_frames, &steps);
        }

        /// Aller-retour F32 → I16 → F32 : erreur ≤ 1,5 / 32 768 (voir [`f32_to_i16`]).
        #[test]
        fn round_trip_f32_i16(x in -1.0f32..=1.0) {
            let y = i16_to_f32(f32_to_i16(x));
            prop_assert!((y - x).abs() <= 1.5 / 32_768.0 + f32::EPSILON, "{x} → {y}");
        }

        /// Aller-retour I16 → F32 → I16 : l'écart d'échelle 32 767/32 768 coûte au
        /// plus une unité.
        #[test]
        fn round_trip_i16_f32(v in any::<i16>()) {
            let w = f32_to_i16(i16_to_f32(v));
            prop_assert!((i32::from(w) - i32::from(v)).abs() <= 1, "{v} → {w}");
        }

        /// Hors plage : toujours écrêté, jamais de débordement.
        #[test]
        fn clamped_out_of_range(x in any::<f32>()) {
            let v = f32_to_i16(x);
            prop_assert!((-32_767..=32_767).contains(&v));
            if x >= 1.0 { prop_assert_eq!(v, 32_767); }
            if x <= -1.0 { prop_assert_eq!(v, -32_767); }
        }

        /// Aller-retour F32 → PCM24 → F32 : erreur ≤ 1,5 / 2^23, l'homologue exact de
        /// [`round_trip_f32_i16`] à 24 bits.
        #[test]
        fn round_trip_f32_i24(x in -1.0f32..=1.0) {
            let y = i24_to_f32(f32_to_i24(x));
            prop_assert!((y - x).abs() <= 1.5 / 8_388_608.0 + f32::EPSILON, "{x} → {y}");
        }

        /// **Aller-retour exact** I16 → PCM24 → I16 : les deux formats entiers partagent
        /// la même échelle en puissance de deux, la conversion ne perd donc rien. C'est le
        /// seul couple de la matrice dont on peut l'exiger.
        #[test]
        fn round_trip_i16_i24_est_exact(v in any::<i16>()) {
            prop_assert_eq!(i24_to_i16(i16_to_i24(v)), v);
        }

        /// PCM24 → I16 → PCM24 : la division par 256 coûte au plus un demi-pas, soit
        /// 128 unités PCM24, plus l'écrêtage des 128 valeurs hautes.
        #[test]
        fn round_trip_i24_i16(v in I24_MIN..=I24_MAX) {
            let w = i16_to_i24(i24_to_i16(v));
            prop_assert!((w - v).abs() <= 256, "{v} → {w}");
        }

        /// Hors plage en PCM24 : écrêtage garanti, et le résultat tient toujours dans les
        /// trois octets qu'[`i24_to_le`] écrit — sans quoi la valeur relue serait un
        /// tout autre nombre.
        #[test]
        fn clamped_out_of_range_i24(x in any::<f32>()) {
            let v = f32_to_i24(x);
            prop_assert!((-I24_MAX..=I24_MAX).contains(&v));
            prop_assert_eq!(i24_from_le(i24_to_le(v)), v);
            if x >= 1.0 { prop_assert_eq!(v, I24_MAX); }
            if x <= -1.0 { prop_assert_eq!(v, -I24_MAX); }
        }

        /// Les trois octets et l'entier se correspondent sur tout le domaine PCM24, et
        /// aucun entier hors domaine ne s'y faufile.
        #[test]
        fn pcm24_aller_retour_octets(v in I24_MIN..=I24_MAX) {
            prop_assert_eq!(i24_from_le(i24_to_le(v)), v);
        }

        /// Quels que soient les trois octets, la lecture reste dans le domaine PCM24 :
        /// c'est ce qui autorise [`i24_to_f32`] à supposer la plage sans la vérifier.
        #[test]
        fn pcm24_lu_reste_dans_le_domaine(octets in any::<[u8; 3]>()) {
            let v = i24_from_le(octets);
            prop_assert!((I24_MIN..=I24_MAX).contains(&v));
            prop_assert_eq!(i24_to_le(v), octets);
            prop_assert!((-1.0..=1.0).contains(&i24_to_f32(v)));
        }

        /// `silence` met à zéro exactement `count` trames.
        #[test]
        fn silence_exact(
            format in any_format(),
            channels in 1u8..=8,
            frames in 1usize..=48,
            start in any::<u64>(),
            count_ratio in 0.0f64..=1.0,
        ) {
            let l = FrameLayout::new(channels, format).unwrap();
            let fb = l.frame_bytes() as usize;
            let count = (count_ratio * frames as f64).round() as usize;
            let mut dst = std::vec![0xFFu8; frames * fb];
            silence(&mut dst, l, start, count as u64).unwrap();
            let first = (start % frames as u64) as usize;
            for (i, frame) in dst.chunks_exact(fb).enumerate() {
                let zeroed = (0..count).any(|k| (first + k) % frames == i);
                let all_zero = frame.iter().all(|&b| b == 0);
                prop_assert_eq!(all_zero, zeroed, "trame {}", i);
            }
        }
    }
}
