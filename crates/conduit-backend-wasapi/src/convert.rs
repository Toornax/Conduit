//! Conversion entre le `f32` du rappel Conduit et le format d'échantillon du
//! tampon WASAPI.
//!
//! En **mode partagé** le moteur audio convertit lui-même
//! (`AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`) et le tampon est toujours du float32 :
//! [`SampleType::F32`], aucune conversion. En **mode exclusif** il n'y a pas de
//! moteur entre nous et le pilote, donc pas d'`AUTOCONVERTPCM` : le matériel
//! impose son format, souvent de l'entier ([`SampleType::Pcm24In32`],
//! [`SampleType::Pcm24`], [`SampleType::Pcm16`]), et la conversion est **à notre
//! charge**, dans le fil du flux, sans allocation (le tampon intermédiaire `f32`
//! est celui de `stream::Worker::scratch`, alloué à l'ouverture).
//!
//! # Échelle et arrondi
//!
//! `f32 → entier` : `v × 2^(n−1)`, arrondi au plus proche (demi vers l'extérieur),
//! **écrêté** à `[−2^(n−1), 2^(n−1) − 1]` ; `NaN` donne 0.
//! `entier → f32` : `v / 2^(n−1)`.
//!
//! L'aller-retour `f32 → entier → f32` est donc exact à **un demi-pas** près, sauf
//! aux extrêmes où l'écrêtage coûte un pas entier (`+1,0` redonne
//! `32 767 / 32 768`), et le retour `entier → f32` ne sort jamais de `[−1, 1]`.
//!
//! C'est une convention **différente** de celle de `conduit_kmd_core::ring`
//! (`f32_to_i16` multiplie par 32 767, `i16_to_f32` divise par 32 768), et ce crate
//! ne dépend donc pas de `conduit-kmd-core`. Trois raisons :
//!
//! 1. `ring::copy_frames` est une copie **cyclique** entre deux tampons du pilote,
//!    adressée modulo, limitée à `FrameLayout::MAX_CHANNELS` (8) canaux et aux
//!    seuls formats F32 et I16 : ni le PCM 24 bits qu'exige le mode exclusif, ni le
//!    remplissage **linéaire** d'un tampon `GetBuffer` ne s'y expriment ;
//! 2. la convention symétrique ±32 767 du pilote vise un seul sens (vers le mixeur
//!    de Windows) ; ici les deux sens existent (rendu et capture) et l'aller-retour
//!    doit rester dans un pas ;
//! 3. `conduit-kmd-core` est la logique du **pilote noyau** : en faire une
//!    dépendance d'un backend utilisateur inverserait les couches pour deux
//!    fonctions scalaires.
//!
//! Rien ici n'alloue : les fonctions écrivent dans des tranches fournies par
//! l'appelant, en `chunks_exact`.

use core::fmt;

/// Format d'un échantillon dans le tampon WASAPI.
///
/// Les variantes entières ne servent qu'au mode exclusif ; le mode partagé est
/// toujours [`SampleType::F32`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleType {
    /// IEEE 754 simple précision : le format du rappel, copié tel quel.
    F32,
    /// PCM 24 bits significatifs **alignés à gauche** dans un conteneur de 32 bits
    /// (`wBitsPerSample = 32`, `wValidBitsPerSample = 24`) : le format que la
    /// plupart des pilotes exclusifs annoncent.
    Pcm24In32,
    /// PCM 24 bits **compactés** : trois octets par échantillon, petit-boutiste
    /// (`wBitsPerSample = wValidBitsPerSample = 24`).
    Pcm24,
    /// PCM signé 16 bits.
    Pcm16,
}

impl SampleType {
    /// Octets d'un échantillon dans le tampon (`wBitsPerSample / 8`).
    pub const fn bytes(self) -> usize {
        match self {
            Self::F32 | Self::Pcm24In32 => 4,
            Self::Pcm24 => 3,
            Self::Pcm16 => 2,
        }
    }

    /// Bits du conteneur (`wBitsPerSample`).
    pub const fn bits(self) -> u16 {
        match self {
            Self::F32 | Self::Pcm24In32 => 32,
            Self::Pcm24 => 24,
            Self::Pcm16 => 16,
        }
    }

    /// Bits significatifs (`wValidBitsPerSample`).
    pub const fn valid_bits(self) -> u16 {
        match self {
            Self::F32 => 32,
            Self::Pcm24In32 | Self::Pcm24 => 24,
            Self::Pcm16 => 16,
        }
    }

    /// Vrai si les échantillons sont des entiers : la conversion est à notre charge.
    pub const fn is_pcm(self) -> bool {
        !matches!(self, Self::F32)
    }

    /// Octets d'une trame de `channels` canaux entrelacés (`nBlockAlign`).
    pub(crate) const fn frame_bytes(self, channels: usize) -> usize {
        self.bytes().saturating_mul(channels)
    }
}

/// Facteur d'échelle des entiers 24 bits : `2^23`.
const SCALE_24: i32 = 1 << 23;
/// Facteur d'échelle des entiers 16 bits : `2^15`.
const SCALE_16: i32 = 1 << 15;

impl fmt::Display for SampleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::F32 => "float32",
            Self::Pcm24In32 => "PCM 24 bits dans un conteneur 32",
            Self::Pcm24 => "PCM 24 bits compactés",
            Self::Pcm16 => "PCM 16 bits",
        })
    }
}

/// `f32` → entier signé de `scale = 2^(n−1)` : arrondi au plus proche (demi vers
/// l'extérieur), écrêté à `[−scale, scale − 1]`, `NaN` → 0.
fn to_int(v: f32, scale: i32) -> i32 {
    let scaled = v * scale as f32;
    // `as i32` sature (et rend 0 pour NaN) depuis Rust 1.45 : l'écrêtage explicite
    // qui suit sert à garantir la borne haute `scale − 1`.
    let rounded = if scaled >= 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    } as i32;
    rounded.clamp(-scale, scale - 1)
}

/// Entier signé → `f32` : `v / 2^(n−1)`, toujours dans `[−1, 1]`.
fn to_f32(v: i32, scale: i32) -> f32 {
    v as f32 / scale as f32
}

/// `f32` → PCM 24 bits (valeur dans `[−8 388 608, 8 388 607]`).
pub(crate) fn f32_to_pcm24(v: f32) -> i32 {
    to_int(v, SCALE_24)
}

/// PCM 24 bits → `f32`.
pub(crate) fn pcm24_to_f32(v: i32) -> f32 {
    to_f32(v, SCALE_24)
}

/// `f32` → PCM 16 bits.
pub(crate) fn f32_to_pcm16(v: f32) -> i16 {
    to_int(v, SCALE_16) as i16
}

/// PCM 16 bits → `f32`.
pub(crate) fn pcm16_to_f32(v: i16) -> f32 {
    to_f32(i32::from(v), SCALE_16)
}

/// Écrit les échantillons `f32` de `src` dans `dst`, au format `sample`.
///
/// `dst` doit faire `src.len() × sample.bytes()` octets ; les octets en trop ne
/// sont pas touchés, les échantillons en trop ne sont pas écrits. Sans allocation,
/// appelable depuis le fil du flux.
///
/// # Panics
///
/// Jamais : `chunks_exact_mut` et `zip` bornent tout.
pub(crate) fn write_f32(src: &[f32], sample: SampleType, dst: &mut [u8]) {
    match sample {
        SampleType::F32 => match bytemuck::try_cast_slice_mut::<u8, f32>(dst) {
            Ok(out) => {
                let n = out.len().min(src.len());
                out[..n].copy_from_slice(&src[..n]);
            }
            // Tampon non aligné sur 4 octets : octet par octet.
            Err(_) => {
                for (chunk, &v) in dst.chunks_exact_mut(4).zip(src) {
                    chunk.copy_from_slice(&v.to_le_bytes());
                }
            }
        },
        SampleType::Pcm24In32 => {
            for (chunk, &v) in dst.chunks_exact_mut(4).zip(src) {
                // Aligné à gauche : les 24 bits significatifs occupent les octets de
                // poids fort, l'octet de poids faible est nul.
                let bytes = f32_to_pcm24(v).to_le_bytes();
                chunk.copy_from_slice(&[0, bytes[0], bytes[1], bytes[2]]);
            }
        }
        SampleType::Pcm24 => {
            for (chunk, &v) in dst.chunks_exact_mut(3).zip(src) {
                let bytes = f32_to_pcm24(v).to_le_bytes();
                chunk.copy_from_slice(&bytes[..3]);
            }
        }
        SampleType::Pcm16 => {
            for (chunk, &v) in dst.chunks_exact_mut(2).zip(src) {
                chunk.copy_from_slice(&f32_to_pcm16(v).to_le_bytes());
            }
        }
    }
}

/// Lit les échantillons de `src` (au format `sample`) dans `dst`, en `f32`.
///
/// Symétrique de [`write_f32`] ; les échantillons de `dst` que `src` ne couvre pas
/// sont mis à zéro. Sans allocation.
pub(crate) fn read_f32(src: &[u8], sample: SampleType, dst: &mut [f32]) {
    let converted = src.len() / sample.bytes();
    match sample {
        SampleType::F32 => match bytemuck::try_cast_slice::<u8, f32>(src) {
            Ok(input) => {
                let n = input.len().min(dst.len());
                dst[..n].copy_from_slice(&input[..n]);
            }
            Err(_) => {
                for (out, chunk) in dst.iter_mut().zip(src.chunks_exact(4)) {
                    *out = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
            }
        },
        SampleType::Pcm24In32 => {
            for (out, chunk) in dst.iter_mut().zip(src.chunks_exact(4)) {
                // Décalage arithmétique de 8 : jette l'octet de bourrage et
                // propage le signe des 24 bits significatifs.
                let raw = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) >> 8;
                *out = pcm24_to_f32(raw);
            }
        }
        SampleType::Pcm24 => {
            for (out, chunk) in dst.iter_mut().zip(src.chunks_exact(3)) {
                let raw = i32::from_le_bytes([0, chunk[0], chunk[1], chunk[2]]) >> 8;
                *out = pcm24_to_f32(raw);
            }
        }
        SampleType::Pcm16 => {
            for (out, chunk) in dst.iter_mut().zip(src.chunks_exact(2)) {
                *out = pcm16_to_f32(i16::from_le_bytes([chunk[0], chunk[1]]));
            }
        }
    }
    if converted < dst.len() {
        dst[converted..].fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aller-retour `f32 → format → f32` par les tranches, comme le fil du flux.
    fn round_trip(values: &[f32], sample: SampleType) -> Vec<f32> {
        let mut bytes = vec![0u8; values.len() * sample.bytes()];
        write_f32(values, sample, &mut bytes);
        let mut back = vec![f32::NAN; values.len()];
        read_f32(&bytes, sample, &mut back);
        back
    }

    #[test]
    fn sample_type_geometry() {
        assert_eq!(SampleType::F32.bytes(), 4);
        assert_eq!(SampleType::F32.bits(), 32);
        assert_eq!(SampleType::F32.valid_bits(), 32);
        assert!(!SampleType::F32.is_pcm());
        assert_eq!(SampleType::Pcm24In32.bytes(), 4);
        assert_eq!(SampleType::Pcm24In32.bits(), 32);
        assert_eq!(SampleType::Pcm24In32.valid_bits(), 24);
        assert_eq!(SampleType::Pcm24.bytes(), 3);
        assert_eq!(SampleType::Pcm24.bits(), 24);
        assert_eq!(SampleType::Pcm24.valid_bits(), 24);
        assert_eq!(SampleType::Pcm16.bytes(), 2);
        assert_eq!(SampleType::Pcm16.bits(), 16);
        assert!(SampleType::Pcm16.is_pcm());
        assert_eq!(SampleType::Pcm16.frame_bytes(2), 4);
        assert_eq!(SampleType::Pcm24.frame_bytes(6), 18);
        assert_eq!(
            SampleType::Pcm24In32.to_string(),
            "PCM 24 bits dans un conteneur 32"
        );
        assert_eq!(SCALE_16, 32_768);
        assert_eq!(SCALE_24, 8_388_608);
    }

    #[test]
    fn pcm24_round_trip_is_within_one_lsb() {
        const LSB: f32 = 1.0 / 8_388_608.0;
        let values = [
            0.0, 1e-7, -1e-7, 0.5, -0.5, 0.25, -0.25, 0.999, -0.999, 1.0, -1.0,
        ];
        for &v in &values {
            let back = round_trip(&[v], SampleType::Pcm24)[0];
            assert!(
                (back - v).abs() <= LSB,
                "PCM24 : {v} → {back}, écart {}",
                (back - v).abs()
            );
            let back = round_trip(&[v], SampleType::Pcm24In32)[0];
            assert!(
                (back - v).abs() <= LSB,
                "PCM24-dans-32 : {v} → {back}, écart {}",
                (back - v).abs()
            );
        }
        // Le zéro est exact, le silence reste du silence.
        assert_eq!(round_trip(&[0.0; 4], SampleType::Pcm24), vec![0.0; 4]);
    }

    #[test]
    fn pcm16_round_trip_is_within_one_lsb() {
        const LSB: f32 = 1.0 / 32_768.0;
        for &v in &[0.0f32, 0.5, -0.5, 0.1, -0.1, 1.0, -1.0] {
            let back = round_trip(&[v], SampleType::Pcm16)[0];
            assert!((back - v).abs() <= LSB, "PCM16 : {v} → {back}");
        }
        // −1,0 est représentable exactement (−32 768), +1,0 est écrêté à 32 767.
        assert_eq!(f32_to_pcm16(-1.0), -32_768);
        assert_eq!(f32_to_pcm16(1.0), 32_767);
        assert_eq!(pcm16_to_f32(-32_768), -1.0);
    }

    #[test]
    fn extremes_are_clamped_and_nan_is_silence() {
        assert_eq!(f32_to_pcm24(2.0), 8_388_607);
        assert_eq!(f32_to_pcm24(-2.0), -8_388_608);
        assert_eq!(f32_to_pcm24(f32::INFINITY), 8_388_607);
        assert_eq!(f32_to_pcm24(f32::NEG_INFINITY), -8_388_608);
        assert_eq!(f32_to_pcm24(f32::NAN), 0);
        assert_eq!(f32_to_pcm16(1e9), 32_767);
        assert_eq!(f32_to_pcm16(-1e9), -32_768);
        assert_eq!(f32_to_pcm16(f32::NAN), 0);
        // Le retour ne sort jamais de [−1, 1].
        for raw in [-8_388_608, -1, 0, 1, 8_388_607] {
            let v = pcm24_to_f32(raw);
            assert!((-1.0..=1.0).contains(&v), "{raw} → {v}");
        }
    }

    #[test]
    fn pcm24_in_32_is_left_justified() {
        // +1 LSB de 24 bits : l'octet de poids faible du conteneur reste nul.
        let mut bytes = [0xAAu8; 4];
        write_f32(&[pcm24_to_f32(1)], SampleType::Pcm24In32, &mut bytes);
        assert_eq!(bytes, [0x00, 0x01, 0x00, 0x00]);
        // Valeur négative : complément à deux sur 24 bits, aligné à gauche.
        let mut bytes = [0u8; 4];
        write_f32(&[pcm24_to_f32(-1)], SampleType::Pcm24In32, &mut bytes);
        assert_eq!(bytes, [0x00, 0xFF, 0xFF, 0xFF]);
        // Et compacté : les trois mêmes octets, sans bourrage.
        let mut bytes = [0u8; 3];
        write_f32(&[pcm24_to_f32(-1)], SampleType::Pcm24, &mut bytes);
        assert_eq!(bytes, [0xFF, 0xFF, 0xFF]);
        let mut back = [0.0f32; 1];
        read_f32(&bytes, SampleType::Pcm24, &mut back);
        assert_eq!(back[0], pcm24_to_f32(-1));
    }

    #[test]
    fn f32_passes_through_unchanged() {
        let values = [0.0f32, 0.5, -0.25, 1.0, -1.0, 1e-30];
        assert_eq!(round_trip(&values, SampleType::F32), values.to_vec());
        // Vue non alignée : le repli octet par octet donne le même résultat.
        let mut bytes = vec![0u8; values.len() * 4 + 1];
        write_f32(&values, SampleType::F32, &mut bytes[1..]);
        let mut back = vec![0.0f32; values.len()];
        read_f32(&bytes[1..], SampleType::F32, &mut back);
        assert_eq!(back, values.to_vec());
    }

    #[test]
    fn short_buffers_do_not_panic() {
        // Moins d'octets que d'échantillons : le reste est mis à zéro à la lecture.
        let mut bytes = [0u8; 2];
        write_f32(&[0.5, 0.5, 0.5], SampleType::Pcm16, &mut bytes);
        let mut back = [1.0f32; 3];
        read_f32(&bytes, SampleType::Pcm16, &mut back);
        assert!((back[0] - 0.5).abs() < 1e-4);
        assert_eq!(back[1], 0.0);
        assert_eq!(back[2], 0.0);
        // Plus d'octets que d'échantillons : rien n'est écrit au-delà.
        let mut bytes = [0xFFu8; 8];
        write_f32(&[0.0], SampleType::Pcm16, &mut bytes);
        assert_eq!(bytes, [0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        // Tranches vides.
        write_f32(&[], SampleType::Pcm24, &mut []);
        read_f32(&[], SampleType::Pcm24, &mut []);
    }
}
