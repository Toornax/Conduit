//! Cible cargo-fuzz : la validation des formats et le dimensionnement du tampon
//! cyclique (M1b-08).
//!
//! `validate` reçoit un `WAVEFORMATEXTENSIBLE` recopié depuis une requête KS, et
//! `buffer_bytes` une taille demandée par `AllocateBufferWithNotification` : deux
//! nombres que le moteur audio choisit, sur un chemin où une panique bloque le
//! démarrage du flux — ou pire, le système.
//!
//! # Disposition de l'entrée
//!
//! | Décalage | Champ |
//! |---|---|
//! | `0..4` | `nSamplesPerSec` demandé (u32 petit-boutiste) |
//! | `4..6` | `nChannels` |
//! | `6..8` | `wBitsPerSample` |
//! | `8..10` | `wValidBitsPerSample` |
//! | `10` | famille : 0 = PCM, 1 = flottant, tout le reste = autre sous-format |
//! | `11..15` | taille de tampon demandée, en octets |
//! | `15..19` | taille d'une trame, en octets |
//! | `19..23` | fréquence servant au calcul du tampon |
//! | `23..27` | nombre de notifications par tour |
//! | `27..` | jusqu'à 8 formats supportés, 6 octets chacun : fréquence (u32), canaux (u8), 0 = F32 sinon I16 |
//!
//! La liste de formats supportés est **dérivée de `M1A_FORMATS`** par recopie puis
//! réécriture de ses champs publics : la cible reste valide si M1b-05 ajoute une entrée
//! à cette table ou un champ à `SupportedFormat`.
//!
//! # Ce qui est vérifié
//!
//! - `validate` ne rend que des formats **présents dans la liste** et qui **acceptent**
//!   la demande ; un refus signifie qu'aucune entrée ne l'accepte ;
//! - le contrat de `buffer_bytes` : la taille rendue est un multiple entier de trame,
//!   **jamais inférieure à la demande** (écrêter serait pire qu'un refus), et sous le
//!   plafond de `MAX_BUFFER_MS` ;
//! - celui de `buffer_bytes_for_notifications` : au moins autant que `buffer_bytes`, et
//!   un nombre de trames divisible par le nombre de notifications.
//!
//! `cargo +nightly fuzz run formats` depuis `crates/conduit-kmd-core`.
#![no_main]

use conduit_kmd_core::format::{
    buffer_bytes, buffer_bytes_for_notifications, validate, RequestedFormat, SampleKind,
    SupportedFormat, M1A_FORMATS, MAX_BUFFER_MS,
};
use conduit_kmd_core::ring::SampleFormat;
use libfuzzer_sys::fuzz_target;

/// Décalage où commence la liste des formats supportés.
const DEBUT_LISTE: usize = 27;
/// Taille d'une entrée de cette liste.
const TAILLE_ENTREE: usize = 6;
/// Nombre maximal d'entrées lues : au-delà, le fuzzer ne trouverait plus rien de neuf.
const MAX_ENTREES: usize = 8;

/// Lit un `u32` petit-boutiste, ou zéro si l'entrée s'arrête avant.
fn mot(data: &[u8], offset: usize) -> u32 {
    match data.get(offset..offset.saturating_add(4)) {
        Some([a, b, c, d]) => u32::from_le_bytes([*a, *b, *c, *d]),
        _ => 0,
    }
}

/// Lit un `u16` petit-boutiste, ou zéro si l'entrée s'arrête avant.
fn demi_mot(data: &[u8], offset: usize) -> u16 {
    match data.get(offset..offset.saturating_add(2)) {
        Some([a, b]) => u16::from_le_bytes([*a, *b]),
        _ => 0,
    }
}

/// Construit la liste des formats supportés en partant des entrées de `M1A_FORMATS`,
/// dont seuls les champs publics sont réécrits.
fn liste_supportee(data: &[u8]) -> Vec<SupportedFormat> {
    assert!(!M1A_FORMATS.is_empty(), "table des formats vide");
    let mut liste = Vec::new();
    for index in 0..MAX_ENTREES {
        let debut = DEBUT_LISTE + index * TAILLE_ENTREE;
        let Some(entree) = data.get(debut..debut + TAILLE_ENTREE) else {
            break;
        };
        let mut format = M1A_FORMATS[index % M1A_FORMATS.len()];
        format.sample_rate = u32::from_le_bytes([entree[0], entree[1], entree[2], entree[3]]);
        format.channels = entree[4];
        format.format = if entree[5] == 0 {
            SampleFormat::F32
        } else {
            SampleFormat::I16
        };
        liste.push(format);
    }
    liste
}

fuzz_target!(|data: &[u8]| {
    let demande = RequestedFormat {
        sample_rate: mot(data, 0),
        channels: demi_mot(data, 4),
        bits_per_sample: demi_mot(data, 6),
        valid_bits: demi_mot(data, 8),
        kind: match data.get(10) {
            Some(0) => SampleKind::Pcm,
            Some(1) => SampleKind::Float,
            _ => SampleKind::Other,
        },
    };

    let fuzzee = liste_supportee(data);
    for (du_crate, supportes) in [(true, M1A_FORMATS.as_slice()), (false, fuzzee.as_slice())] {
        match validate(&demande, supportes) {
            Ok(retenu) => {
                assert!(
                    supportes.contains(&retenu),
                    "format retenu absent de la liste"
                );
                assert!(retenu.accepts(&demande), "format retenu qui n'accepte pas");
                // `validate` **fait confiance à sa liste** : `accepts` compare le nombre
                // de canaux demandé à celui de l'entrée, sans vérifier que celui-ci soit
                // représentable. Une liste portant `channels = 0` rend donc un format
                // dont `layout()` est `None` — impossible avec la table du crate, d'où
                // l'assertion réservée à celle-ci (voir le rapport de M1b-08).
                if du_crate {
                    assert!(
                        retenu.layout().is_some(),
                        "format retenu sans disposition de trame"
                    );
                }
            }
            Err(cause) => {
                assert!(
                    !supportes.iter().any(|s| s.accepts(&demande)),
                    "refus alors qu'une entrée accepte la demande"
                );
                assert!(!cause.to_string().is_empty());
            }
        }
    }

    let demandes_octets = mot(data, 11);
    let octets_par_trame = mot(data, 15);
    let frequence = mot(data, 19);
    let notifications = mot(data, 23);

    let taille = buffer_bytes(demandes_octets, octets_par_trame, frequence);
    if let Some(octets) = taille {
        // `buffer_bytes` ne rend `Some` que si la trame est non nulle.
        assert_ne!(octets_par_trame, 0);
        assert_eq!(
            octets % octets_par_trame,
            0,
            "taille non alignée sur la trame"
        );
        assert!(
            octets >= demandes_octets,
            "tampon plus petit que demandé : {octets} < {demandes_octets}"
        );
        let trames = u64::from(octets / octets_par_trame);
        assert!(trames >= 1, "tampon vide");
        // Le plafond du contrat : floor(fréquence × MAX_BUFFER_MS / 1000) trames.
        let plafond = u64::from(frequence) * u64::from(MAX_BUFFER_MS) / 1_000;
        assert!(
            trames <= plafond,
            "tampon de {trames} trames au-dessus du plafond {plafond}"
        );
    }

    if let Some(alignee) =
        buffer_bytes_for_notifications(demandes_octets, octets_par_trame, frequence, notifications)
    {
        let base = taille.expect("l'alignement a réussi là où le dimensionnement a échoué");
        assert_ne!(notifications, 0);
        assert!(alignee >= base, "l'alignement a rétréci le tampon");
        assert_eq!(
            alignee % octets_par_trame,
            0,
            "taille non alignée sur la trame"
        );
        assert_eq!(
            (alignee / octets_par_trame) % notifications,
            0,
            "période de notification non entière en trames"
        );
    }
});
