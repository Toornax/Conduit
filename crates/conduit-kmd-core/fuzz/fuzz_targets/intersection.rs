//! Cible cargo-fuzz : la négociation de format de `DataRangeIntersection` (M1b-21).
//!
//! [`intersect`] est une **frontière de confiance**, et c'est ce qui justifie une cible à
//! elle seule : PortCls l'appelle avec une `KSDATARANGE_AUDIO` que le **client** a écrite —
//! le moteur audio, ou n'importe quelle application qui ouvre un pin en mode exclusif. La
//! moitié des octets vient donc de Windows, et l'autre de nos propres descripteurs. Le
//! résultat, lui, est recopié tel quel dans un `KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]` que le
//! moteur audio relit : un champ dérivé incohérent y devient un endpoint sans format, une
//! panique un `KeBugCheckEx`.
//!
//! # Disposition de l'entrée
//!
//! Deux plages de 21 octets, la plage du client puis la nôtre, et deux octets de queue :
//!
//! | Décalage | Champ |
//! |---|---|
//! | `0` | famille du client : 0 = joker (`KSDATAFORMAT_SUBTYPE_WILDCARD`), 1 = PCM, 2 = flottant, tout le reste = autre sous-format |
//! | `1..5` | `MaximumChannels` du client |
//! | `5..9` | `MinimumBitsPerSample` du client |
//! | `9..13` | `MaximumBitsPerSample` du client |
//! | `13..17` | `MinimumSampleFrequency` du client |
//! | `17..21` | `MaximumSampleFrequency` du client |
//! | `21..42` | les mêmes champs pour **notre** plage |
//! | `42` | index de variante de descripteurs, pour la plage **ponctuelle** du câble |
//! | `43` | rang de la profondeur dans `SAMPLE_DEPTHS` |
//!
//! Un champ absent vaut zéro : une entrée courte reste une entrée valide.
//!
//! # Ce qui est vérifié
//!
//! - **la règle de choix** annoncée : la fréquence et la profondeur retenues sont la
//!   **plus haute valeur de l'intersection**, et le nombre de canaux est exactement le
//!   nôtre ;
//! - **la cohérence interne du descripteur** : `nBlockAlign`, `nAvgBytesPerSec`,
//!   `wFormatTag`, `cbSize`, `FormatSize`, `SampleSize` et le `dwChannelMask` se déduisent
//!   tous des trois nombres retenus, et le masque compte exactement ses canaux ;
//! - **les bornes du câble** : un format rendu tient toujours dans `1..=8` canaux, sur une
//!   profondeur multiple d'octet d'au plus 32 bits, et un flottant y fait 32 bits ;
//! - **la cause nommée est la bonne** : un [`NoMatch`] numérique correspond à un
//!   intervalle réellement vide, et se rend sans paniquer (il part dans le journal
//!   d'événements) ;
//! - **rien n'est proposé hors catalogue** : quand notre plage est une des trois qu'un
//!   câble déclare vraiment, le format rendu est accepté par
//!   [`validate`](conduit_kmd_core::format::validate) contre
//!   [`cable_formats`](conduit_kmd_core::format::cable_formats) — c'est ce qui interdit
//!   d'annoncer un format que `NewStream` refuserait ensuite ;
//! - **un client joker obtient toujours notre plage** : c'est le cas nominal de PortCls, et
//!   un refus y serait un endpoint muet.
//!
//! Toutes les bornes sont lues dans le crate ; la cible ne recopie aucune table.
//!
//! `cargo +nightly fuzz run intersection` depuis `crates/conduit-kmd-core`.
#![no_main]

use conduit_kmd_core::format::{
    cable_formats, validate, variant_channels, variant_rate, SampleKind, SAMPLE_DEPTHS,
};
use conduit_kmd_core::ring::{FrameLayout, SampleFormat};
use conduit_kmd_core::wavefmt::{
    intersect, needs_extensible, speaker_mask, AudioRange, NoMatch, WaveFormat,
    KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES, KSDATAFORMAT_WAVEFORMATEX_BYTES,
    WAVEFORMATEXTENSIBLE_CB_SIZE, WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM,
};
use libfuzzer_sys::fuzz_target;

/// Taille d'une plage dans l'entrée : un octet de famille et cinq `u32`.
const TAILLE_PLAGE: usize = 21;

/// Lit un `u32` petit-boutiste, ou zéro si l'entrée s'arrête avant.
fn mot(data: &[u8], offset: usize) -> u32 {
    match data.get(offset..offset.saturating_add(4)) {
        Some([a, b, c, d]) => u32::from_le_bytes([*a, *b, *c, *d]),
        _ => 0,
    }
}

/// Lit un octet, ou zéro si l'entrée s'arrête avant.
fn octet(data: &[u8], offset: usize) -> u8 {
    data.get(offset).copied().unwrap_or(0)
}

/// Décode la plage qui commence à `debut`.
fn plage(data: &[u8], debut: usize) -> AudioRange {
    AudioRange {
        kind: match octet(data, debut) {
            0 => None,
            1 => Some(SampleKind::Pcm),
            2 => Some(SampleKind::Float),
            _ => Some(SampleKind::Other),
        },
        max_channels: mot(data, debut + 1),
        min_bits: mot(data, debut + 5),
        max_bits: mot(data, debut + 9),
        min_rate: mot(data, debut + 13),
        max_rate: mot(data, debut + 17),
    }
}

/// La famille d'échantillons d'une profondeur, comme `descriptors::subtype_of` du pilote.
fn famille(depth: SampleFormat) -> SampleKind {
    if matches!(depth, SampleFormat::F32) {
        SampleKind::Float
    } else {
        SampleKind::Pcm
    }
}

/// Le contrat de [`intersect`] sur un couple quelconque de plages ; rend le format retenu
/// quand il y en a un, pour que l'appelant y ajoute ce qu'il sait de plus.
fn contrat(client: &AudioRange, notre: &AudioRange) -> Option<WaveFormat> {
    match intersect(client, notre) {
        Ok(format) => {
            let canaux = u32::from(format.channels());
            let bits = u32::from(format.bits_per_sample());

            // La règle de choix annoncée : la plus haute valeur de chaque intersection,
            // et exactement notre compte de canaux.
            assert_eq!(canaux, notre.max_channels, "canaux autres que les nôtres");
            assert!(
                client.max_channels >= notre.max_channels,
                "format rendu alors que le client ne monte pas jusqu'à nos canaux"
            );
            assert_eq!(
                bits,
                client.max_bits.min(notre.max_bits),
                "profondeur autre que la plus haute de l'intersection"
            );
            assert!(bits >= client.min_bits.max(notre.min_bits));
            assert_eq!(
                format.sample_rate(),
                client.max_rate.min(notre.max_rate),
                "fréquence autre que la plus haute de l'intersection"
            );
            assert!(format.sample_rate() >= client.min_rate.max(notre.min_rate));

            // Les bornes du câble : ce que `SampleFormat` et `FrameLayout` savent porter.
            assert!(!matches!(format.kind(), SampleKind::Other));
            assert!(
                canaux >= 1 && canaux <= u32::from(FrameLayout::MAX_CHANNELS),
                "{canaux} canaux hors du domaine d'un câble"
            );
            assert!(bits > 0 && bits <= 32 && bits % 8 == 0, "{bits} bits");
            assert!(
                !matches!(format.kind(), SampleKind::Float) || bits == 32,
                "flottant de {bits} bits"
            );
            assert!(
                FrameLayout::new(canaux as u8, SampleFormat::F32).is_some(),
                "canaux sans disposition de trame"
            );

            // Les champs dérivés se déduisent tous des trois nombres retenus.
            assert_eq!(format.valid_bits(), format.bits_per_sample());
            assert_eq!(u32::from(format.block_align()), bits / 8 * canaux);
            assert_eq!(
                format.avg_bytes_per_sec(),
                format.sample_rate() * u32::from(format.block_align()),
                "débit moyen incohérent"
            );
            assert_eq!(format.sample_size(), u32::from(format.block_align()));
            assert_eq!(format.is_extensible(), needs_extensible(canaux, bits));
            if format.is_extensible() {
                assert_eq!(format.channel_mask(), speaker_mask(canaux as u8).unwrap());
                assert_eq!(
                    format.channel_mask().count_ones(),
                    canaux,
                    "le masque de haut-parleurs ne compte pas ses canaux"
                );
                assert_eq!(format.cb_size(), WAVEFORMATEXTENSIBLE_CB_SIZE);
                assert_eq!(
                    format.format_size(),
                    KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES
                );
                assert_eq!(format.format_tag(), WAVE_FORMAT_EXTENSIBLE);
            } else {
                assert_eq!(format.channel_mask(), 0, "masque sur la forme simple");
                assert_eq!(format.cb_size(), 0);
                assert_eq!(format.format_size(), KSDATAFORMAT_WAVEFORMATEX_BYTES);
                assert!(matches!(
                    format.format_tag(),
                    WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT
                ));
            }

            // Le format retenu se redécrit à l'identique.
            assert_eq!(
                WaveFormat::new(format.kind(), canaux, bits, format.sample_rate()),
                Some(format),
                "format non reconstructible depuis ses propres nombres"
            );
            Some(format)
        }
        Err(cause) => {
            // Cette cause part dans la chaîne d'insertion d'une entrée du journal.
            assert!(!cause.to_string().is_empty());
            assert_eq!(cause.est_numerique(), !matches!(cause, NoMatch::SousType));
            match cause {
                NoMatch::SousType => assert!(
                    notre.kind.is_none()
                        || notre.kind == Some(SampleKind::Other)
                        || client.kind.is_some_and(|k| Some(k) != notre.kind),
                    "sous-types déclarés incompatibles alors qu'ils s'accordent"
                ),
                NoMatch::Canaux { demandes, declares } => {
                    assert_eq!(demandes, client.max_channels);
                    assert_eq!(declares, notre.max_channels);
                    assert!(demandes < declares, "refus de canaux qui se croisent");
                }
                NoMatch::Bits { demandes, declares } => {
                    assert_eq!(demandes, (client.min_bits, client.max_bits));
                    assert_eq!(declares, (notre.min_bits, notre.max_bits));
                    assert!(
                        client.min_bits.max(notre.min_bits) > client.max_bits.min(notre.max_bits),
                        "refus de profondeurs qui se croisent"
                    );
                }
                NoMatch::Frequence { demandes, declares } => {
                    assert_eq!(demandes, (client.min_rate, client.max_rate));
                    assert_eq!(declares, (notre.min_rate, notre.max_rate));
                    assert!(
                        client.min_rate.max(notre.min_rate) > client.max_rate.min(notre.max_rate),
                        "refus de fréquences qui se croisent"
                    );
                }
                // `Indescriptible` et `HorsCatalogue` n'imposent rien de plus ici ; l'énum
                // est `non_exhaustive`, donc ce bras couvre aussi ce que M2 y ajoutera.
                _ => {}
            }
            None
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let client = plage(data, 0);
    let notre = plage(data, TAILLE_PLAGE);

    // 1. Deux plages entièrement libres : le contrat général, et l'absence de panique.
    contrat(&client, &notre);

    // 2. Notre plage telle que le pilote la déclare vraiment : ponctuelle, sur une des
    //    24 variantes et une des trois profondeurs.
    let variante = usize::from(octet(data, TAILLE_PLAGE * 2));
    let (Some(rate), Some(channels)) = (variant_rate(variante), variant_channels(variante)) else {
        return;
    };
    let profondeur =
        SAMPLE_DEPTHS[usize::from(octet(data, TAILLE_PLAGE * 2 + 1)) % SAMPLE_DEPTHS.len()];
    let ponctuelle = AudioRange::punctual(
        famille(profondeur),
        u32::from(channels),
        profondeur.bits_per_sample(),
        rate,
    );
    let catalogue = cable_formats(rate, channels);

    // Le cas nominal de PortCls : un client qui n'impose rien obtient notre plage, et ce
    // qu'il obtient est un format que `NewStream` acceptera.
    let joker = intersect(&AudioRange::WILDCARD, &ponctuelle)
        .expect("un client joker doit toujours obtenir notre plage");
    assert_eq!(joker.sample_rate(), rate);
    assert_eq!(joker.channels(), u16::from(channels));
    assert_eq!(
        u32::from(joker.bits_per_sample()),
        profondeur.bits_per_sample()
    );
    assert!(
        validate(&joker.requested(), &catalogue).is_ok(),
        "format proposé au client joker mais refusé par validate"
    );

    // 3. Le client fuzzé contre cette plage-là : tout ce qui en sort doit rester dans le
    //    catalogue du câble — c'est ce qui rend `NoMatch::HorsCatalogue` inatteignable.
    if let Some(format) = contrat(&client, &ponctuelle) {
        assert!(
            validate(&format.requested(), &catalogue).is_ok(),
            "format négocié hors du catalogue du câble : {format:?}"
        );
    }
});
