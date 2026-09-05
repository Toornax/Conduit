//! Validation des formats (`docs/driver-design.md` §5.2, §5.4).
//!
//! Le moteur audio de Windows propose un `KSDATAFORMAT_WAVEFORMATEXTENSIBLE` à
//! `NewStream` / `AllocateAudioBuffer`. Le pilote en extrait les champs utiles dans
//! un [`RequestedFormat`] (le décodage de `wFormatTag`/`SubFormat` en
//! [`SampleKind`] se fait dans `conduit-kmd`, ce crate ne connaît pas les GUID) et
//! le confronte ici à la liste des formats supportés par le câble
//! ([`validate`]). La taille de tampon demandée par le moteur est arrondie à la
//! trame et bornée ([`buffer_bytes`]).
//!
//! Ce crate ne dépend pas de `conduit-core` : fréquences et comptes de canaux sont
//! des entiers simples.

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
    /// nombre de canaux, et F32 ⇔ `Float` 32/32 bits, I16 ⇔ `Pcm` 16/16 bits.
    pub fn accepts(self, requested: &RequestedFormat) -> bool {
        if requested.sample_rate != self.sample_rate
            || requested.channels != u16::from(self.channels)
        {
            return false;
        }
        let (kind, bits) = match self.format {
            SampleFormat::F32 => (SampleKind::Float, 32),
            SampleFormat::I16 => (SampleKind::Pcm, 16),
        };
        requested.kind == kind && requested.bits_per_sample == bits && requested.valid_bits == bits
    }
}

/// Formats du spike M1a : 48 kHz stéréo, F32 et I16 (§5.4).
pub const M1A_FORMATS: [SupportedFormat; 2] = [
    SupportedFormat {
        sample_rate: 48_000,
        channels: 2,
        format: SampleFormat::F32,
    },
    SupportedFormat {
        sample_rate: 48_000,
        channels: 2,
        format: SampleFormat::I16,
    },
];

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
/// Correspondance : F32 ⇔ [`SampleKind::Float`] 32 bits sur 32 ; I16 ⇔
/// [`SampleKind::Pcm`] 16 bits sur 16 ; fréquence et canaux égaux. Tout le reste
/// (24 bits, `Other`, bits valides partiels) est [`FormatError::Unsupported`] : le
/// pilote répond `STATUS_INVALID_PARAMETER` et le moteur choisit un autre format.
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
pub const MAX_BUFFER_MS: u32 = 100;

/// Taille du tampon cyclique à allouer pour `AllocateAudioBuffer`.
///
/// `requested_bytes` (la taille demandée par le moteur audio) est arrondie **vers
/// le haut** au multiple de `frame_bytes` — le moteur reçoit au moins ce qu'il a
/// demandé — puis bornée entre [`MIN_BUFFER_MS`] et [`MAX_BUFFER_MS`] de son à
/// `sample_rate` : en dessous d'une milliseconde le DPC (période 1 ms) ne peut pas
/// suivre ; au-delà de 100 ms la latence n'a plus de sens pour un câble virtuel et
/// la mémoire non paginée est gaspillée. La borne basse est arrondie vers le haut
/// (≥ 1 ms), la borne haute vers le bas (≤ 100 ms).
///
/// Renvoie `None` si `frame_bytes == 0`, si `sample_rate == 0`, si les bornes sont
/// incohérentes (fréquence < 10 Hz) ou si la taille retenue ne tient pas dans un
/// `u32`.
pub fn buffer_bytes(requested_bytes: u32, frame_bytes: u32, sample_rate: u32) -> Option<u32> {
    if frame_bytes == 0 || sample_rate == 0 {
        return None;
    }
    let frame_bytes = u64::from(frame_bytes);
    let rate = u64::from(sample_rate);
    // ceil(rate × MIN_MS / 1000) et floor(rate × MAX_MS / 1000), en u64 : pas de
    // débordement (u32 × 100 < 2^39).
    let min_frames = rate
        .checked_mul(u64::from(MIN_BUFFER_MS))?
        .checked_add(999)?
        .checked_div(1_000)?;
    let max_frames = rate
        .checked_mul(u64::from(MAX_BUFFER_MS))?
        .checked_div(1_000)?;
    if min_frames > max_frames {
        return None;
    }
    let requested_frames = u64::from(requested_bytes)
        .checked_add(frame_bytes.checked_sub(1)?)?
        .checked_div(frame_bytes)?;
    let frames = requested_frames.clamp(min_frames, max_frames);
    u32::try_from(frames.checked_mul(frame_bytes)?).ok()
}

/// Taille du tampon cyclique pour `AllocateBufferWithNotification` : comme
/// [`buffer_bytes`], puis arrondie **vers le haut** à un multiple de
/// `notification_count` trames, pour que chaque période de notification
/// (`bytes / notification_count`) soit un nombre entier de trames et que la fin du
/// tampon soit une frontière de période ([`crate::notify`]). L'arrondi peut dépasser
/// [`MAX_BUFFER_MS`] d'au plus `notification_count − 1` trames.
///
/// `None` dans les cas de [`buffer_bytes`], si `notification_count == 0`, ou si le
/// résultat ne tient pas dans un `u32`.
pub fn buffer_bytes_for_notifications(
    requested_bytes: u32,
    frame_bytes: u32,
    sample_rate: u32,
    notification_count: u32,
) -> Option<u32> {
    let bytes = buffer_bytes(requested_bytes, frame_bytes, sample_rate)?;
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

    #[test]
    fn validate_table() {
        let cases: [(RequestedFormat, Result<SupportedFormat, FormatError>); 12] = [
            (
                req(48_000, 2, 32, 32, SampleKind::Float),
                Ok(M1A_FORMATS[0]),
            ),
            (req(48_000, 2, 16, 16, SampleKind::Pcm), Ok(M1A_FORMATS[1])),
            // Mauvaise fréquence.
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
                req(48_000, 2, 24, 24, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            // Bits valides partiels (PCM 24 dans 32, float « 24 bits »).
            (
                req(48_000, 2, 32, 24, SampleKind::Pcm),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 2, 32, 24, SampleKind::Float),
                Err(FormatError::Unsupported),
            ),
            (
                req(48_000, 2, 16, 16, SampleKind::Other),
                Err(FormatError::Unsupported),
            ),
        ];
        for (requested, expected) in cases {
            assert_eq!(
                validate(&requested, &M1A_FORMATS),
                expected,
                "{requested:?}"
            );
        }
        assert_eq!(
            validate(&req(48_000, 2, 32, 32, SampleKind::Float), &[]),
            Err(FormatError::Unsupported)
        );
    }

    #[test]
    fn supported_layout() {
        assert_eq!(M1A_FORMATS[0].layout().unwrap().frame_bytes(), 8);
        assert_eq!(M1A_FORMATS[1].layout().unwrap().frame_bytes(), 4);
        let bad = SupportedFormat {
            sample_rate: 48_000,
            channels: 0,
            format: SampleFormat::F32,
        };
        assert!(bad.layout().is_none());
    }

    #[test]
    fn error_display_is_french() {
        assert!(FormatError::Unsupported.to_string().contains("supportés"));
    }

    #[test]
    fn buffer_bytes_table() {
        // 48 kHz stéréo F32 : 8 octets/trame, bornes 48 trames (384 o) et 4 800 (38 400 o).
        assert_eq!(buffer_bytes(0, 8, 48_000), Some(384));
        assert_eq!(buffer_bytes(1, 8, 48_000), Some(384));
        assert_eq!(buffer_bytes(384, 8, 48_000), Some(384));
        // 10 ms demandés par le moteur en mode partagé.
        assert_eq!(buffer_bytes(3_840, 8, 48_000), Some(3_840));
        // Arrondi vers le haut à la trame.
        assert_eq!(buffer_bytes(3_841, 8, 48_000), Some(3_848));
        assert_eq!(buffer_bytes(3_847, 8, 48_000), Some(3_848));
        // Plafond 100 ms.
        assert_eq!(buffer_bytes(38_400, 8, 48_000), Some(38_400));
        assert_eq!(buffer_bytes(38_401, 8, 48_000), Some(38_400));
        assert_eq!(buffer_bytes(u32::MAX, 8, 48_000), Some(38_400));
        // 44,1 kHz I16 stéréo : 1 ms = 44,1 trames → 45 ; 100 ms = 4 410 trames.
        assert_eq!(buffer_bytes(0, 4, 44_100), Some(180));
        assert_eq!(buffer_bytes(u32::MAX, 4, 44_100), Some(17_640));
        // Cas invalides.
        assert_eq!(buffer_bytes(1_000, 0, 48_000), None);
        assert_eq!(buffer_bytes(1_000, 8, 0), None);
        // 5 Hz : 1 ms = 1 trame (ceil) > 100 ms = 0 trame (floor).
        assert_eq!(buffer_bytes(1_000, 8, 5), None);
        // u32::MAX Hz × 32 octets × 100 ms ne tient pas dans un u32 ; la borne basse
        // (1 ms = 4 294 968 trames) tient encore.
        assert_eq!(buffer_bytes(u32::MAX, 32, u32::MAX), None);
        assert_eq!(buffer_bytes(1_000, 32, u32::MAX), Some(4_294_968 * 32));
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
        // Plafond 100 ms = 4 800 trames, pair : inchangé.
        assert_eq!(
            buffer_bytes_for_notifications(u32::MAX, 8, 48_000, 2),
            Some(38_400)
        );
        // 44,1 kHz : 4 410 trames, pair.
        assert_eq!(
            buffer_bytes_for_notifications(u32::MAX, 4, 44_100, 2),
            Some(17_640)
        );
        assert_eq!(buffer_bytes_for_notifications(3_840, 8, 48_000, 0), None);
        assert_eq!(buffer_bytes_for_notifications(3_840, 0, 48_000, 1), None);
    }

    proptest! {
        /// Multiple de `frame_bytes × count`, au moins `buffer_bytes`, moins de `count`
        /// trames au-dessus.
        #[test]
        fn buffer_bytes_for_notifications_aligned(
            requested in any::<u32>(),
            frame_bytes in 1u32..=32,
            sample_rate in 8_000u32..=384_000,
            count in 1u32..=2,
        ) {
            let base = buffer_bytes(requested, frame_bytes, sample_rate).unwrap();
            let bytes = buffer_bytes_for_notifications(requested, frame_bytes, sample_rate, count).unwrap();
            prop_assert_eq!(bytes % (frame_bytes * count), 0);
            prop_assert!(bytes >= base);
            prop_assert!(bytes - base < frame_bytes * count);
        }

        /// Résultat multiple de `frame_bytes`, dans [1 ms ; 100 ms], et au moins la
        /// demande quand celle-ci est dans les bornes.
        #[test]
        fn buffer_bytes_in_bounds(
            requested in any::<u32>(),
            frame_bytes in 1u32..=32,
            sample_rate in 8_000u32..=384_000,
        ) {
            let bytes = buffer_bytes(requested, frame_bytes, sample_rate).unwrap();
            prop_assert_eq!(bytes % frame_bytes, 0);
            let frames = u64::from(bytes / frame_bytes);
            let rate = u64::from(sample_rate);
            prop_assert!(frames * 1_000 >= rate, "moins de 1 ms : {frames} trames");
            prop_assert!(frames * 10 <= rate, "plus de 100 ms : {frames} trames");
            let requested_frames = u64::from(requested).div_ceil(u64::from(frame_bytes));
            let min = rate.div_ceil(1_000);
            let max = rate / 10;
            prop_assert_eq!(frames, requested_frames.clamp(min, max));
        }

        /// Un format supporté s'accepte lui-même ; changer un champ le refuse.
        #[test]
        fn supported_accepts_itself(
            sample_rate in 1u32..=384_000,
            channels in 1u8..=8,
            is_float in any::<bool>(),
        ) {
            let format = if is_float { SampleFormat::F32 } else { SampleFormat::I16 };
            let s = SupportedFormat { sample_rate, channels, format };
            let bits = format.bytes_per_sample() as u16 * 8;
            let kind = if is_float { SampleKind::Float } else { SampleKind::Pcm };
            let r = req(sample_rate, u16::from(channels), bits, bits, kind);
            prop_assert_eq!(validate(&r, &[s]), Ok(s));
            let other_kind = if is_float { SampleKind::Pcm } else { SampleKind::Float };
            let variants = [
                RequestedFormat { kind: other_kind, ..r },
                RequestedFormat { valid_bits: bits - 1, ..r },
                RequestedFormat { sample_rate: sample_rate + 1, ..r },
                RequestedFormat { channels: u16::from(channels) + 1, ..r },
            ];
            for variant in variants {
                let refused = validate(&variant, &[s]).is_err();
                prop_assert!(refused, "{:?} accepté par {:?}", variant, s);
            }
        }
    }
}
