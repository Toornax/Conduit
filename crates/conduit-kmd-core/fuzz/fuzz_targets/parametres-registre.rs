//! Cible cargo-fuzz : le chemin de lecture des paramètres de registre (M1b-08).
//!
//! Le pilote lit quatre valeurs sous `Device Parameters` au `StartDevice`, les décode
//! (`decode_dword`) puis les écrête (`sanitize`). Le critère de M1b-01 est que **rien**
//! de ce que l'administrateur peut écrire dans `regedit` ne fasse échouer le chargement :
//! `sanitize` est donc une fonction **totale**, et cette cible le vérifie sur des types
//! `REG_*`, des longueurs et des valeurs arbitraires.
//!
//! # Disposition de l'entrée
//!
//! Quatre enregistrements de 13 octets, un par paramètre, dans l'ordre de `Param::ALL`
//! (`ReserveSize`, `Channels`, `BufferMs`, `PacketMode`) :
//!
//! | Décalage | Champ |
//! |---|---|
//! | `0..4` | le type de la valeur (`REG_DWORD` vaut 4), petit-boutiste |
//! | `4` | la longueur annoncée des données, ramenée dans `0..=8` |
//! | `5..13` | les données, dont seuls les `longueur` premiers octets sont décodés |
//!
//! Un enregistrement absent (entrée trop courte) vaut « valeur absente du registre »,
//! ce qui est le cas d'une clé `Parameters` vide. Les octets au-delà du 52ᵉ sont ignorés.
//!
//! # Ce qui est vérifié
//!
//! - `sanitize` rend toujours des valeurs **dans les bornes** de `Param::bounds`, lues
//!   dans le crate et non recopiées : la cible survit à un déplacement de borne ;
//! - chaque `Correction` rapportée est **cohérente** — la valeur trouvée est bien hors
//!   bornes, la valeur retenue est bien la borne franchie — et se rend sans paniquer ;
//! - une valeur **absente** prend sa valeur par défaut, **sans** correction à
//!   journaliser ;
//! - `sanitize` est **idempotente** : réinjecter ce qu'elle a produit ne corrige rien.
//!
//! `cargo +nightly fuzz run parametres-registre` depuis `crates/conduit-kmd-core`.
#![no_main]
// `..RawParams::MISSING` est redondant tant que les quatre champs sont énumérés ; il est
// là exprès, pour que la cible continue de compiler si M1b-05 ajoute un paramètre.
#![allow(clippy::needless_update)]

use conduit_kmd_core::params::{decode_dword, sanitize, Fix, Param, RawParams};
use libfuzzer_sys::fuzz_target;

/// Taille d'un enregistrement : 4 octets de type, 1 de longueur, 8 de données.
const TAILLE_ENREGISTREMENT: usize = 13;
/// Longueur maximale des données d'un enregistrement.
const MAX_DONNEES: usize = 8;

/// Décode le `index`-ième enregistrement, ou `None` si l'entrée s'arrête avant.
fn lire(data: &[u8], index: usize) -> Option<u32> {
    let debut = index.checked_mul(TAILLE_ENREGISTREMENT)?;
    let fin = debut.checked_add(TAILLE_ENREGISTREMENT)?;
    let enr = data.get(debut..fin)?;
    let kind = u32::from_le_bytes([enr[0], enr[1], enr[2], enr[3]]);
    let longueur = usize::from(enr[4]) % (MAX_DONNEES + 1);
    decode_dword(kind, enr.get(5..5 + longueur)?)
}

fuzz_target!(|data: &[u8]| {
    let brut = RawParams {
        reserve: lire(data, 0),
        channels: lire(data, 1),
        buffer_ms: lire(data, 2),
        packet_mode: lire(data, 3),
        ..RawParams::MISSING
    };

    let (params, rapport) = sanitize(brut);

    // Ce que chaque paramètre a reçu du registre, et ce que `sanitize` en a fait.
    let table = [
        (Param::Reserve, brut.reserve, u32::from(params.reserve)),
        (Param::Channels, brut.channels, u32::from(params.channels)),
        (Param::BufferMs, brut.buffer_ms, params.buffer_ms),
        (
            Param::PacketMode,
            brut.packet_mode,
            u32::from(params.packet_mode),
        ),
    ];

    for (param, trouve, retenu) in table {
        let (min, max) = param.bounds();
        assert!(
            (min..=max).contains(&retenu),
            "{param} hors bornes après écrêtage : {retenu}"
        );
        match rapport.get(param) {
            None => {
                // Rien à journaliser : soit la valeur était déjà bonne, soit elle était
                // absente et le défaut s'applique en silence.
                let attendu = trouve.unwrap_or_else(|| param.default_value());
                assert_eq!(retenu, attendu, "{param} modifié sans correction rapportée");
            }
            Some(correction) => {
                assert_eq!(correction.param, param);
                assert_eq!(correction.applied, retenu);
                assert_eq!(
                    Some(correction.found),
                    trouve,
                    "{param} corrigé sur une valeur que le registre n'a pas donnée"
                );
                let borne = match correction.fix {
                    Fix::TooLow => {
                        assert!(correction.found < min, "{param} corrigé à tort vers le bas");
                        min
                    }
                    Fix::TooHigh => {
                        assert!(
                            correction.found > max,
                            "{param} corrigé à tort vers le haut"
                        );
                        max
                    }
                };
                assert_eq!(correction.applied, borne);
                // Cette ligne part dans le journal du pilote.
                assert!(!correction.to_string().is_empty());
            }
        }
    }

    // Le rapport se compte de la même façon par ses deux accesseurs.
    assert_eq!(rapport.len(), rapport.corrections().count());
    assert_eq!(rapport.is_empty(), rapport.len() == 0);
    assert!(rapport.len() <= Param::ALL.len());

    // Idempotence : ce que `sanitize` a retenu est un point fixe.
    let (encore, rien) = sanitize(RawParams {
        reserve: Some(u32::from(params.reserve)),
        channels: Some(u32::from(params.channels)),
        buffer_ms: Some(params.buffer_ms),
        packet_mode: Some(u32::from(params.packet_mode)),
        ..RawParams::MISSING
    });
    assert_eq!(encore, params, "sanitize n'est pas idempotente");
    assert!(
        rien.is_empty(),
        "sanitize corrige ce qu'elle vient de produire"
    );
});
