//! Cible cargo-fuzz : le parseur de la propriété KS de configuration (M1b-08).
//!
//! `CableState::from_bytes` reçoit, dans le pilote, le tampon `Value` d'un
//! `IOCTL_KS_PROPERTY` — c'est-à-dire des octets qu'un service en espace utilisateur
//! choisit entièrement, et que le gestionnaire de propriété interprète parfois à
//! `DISPATCH_LEVEL`. Une panique y serait un `KeBugCheckEx`.
//!
//! Ce que la cible éprouve, au-delà de l'absence de panique :
//!
//! - **l'aller-retour** : ce qui est accepté se resérialise **à l'octet près** en
//!   l'entrée reçue, et se relit à l'identique. C'est ce qui interdit à `from_bytes`
//!   d'accepter un préfixe valide suivi d'octets en trop, ou de perdre un champ ;
//! - **les domaines annoncés** : un état accepté est dans les bornes que le contrat
//!   promet au pilote, lesquelles sont lues ici dans le crate plutôt que recopiées —
//!   la cible reste donc valide si M1b-05 élargit les canaux ;
//! - **le rendu des refus** : `Display` d'une `ConfigError` part dans le journal du
//!   service, il ne doit pas paniquer non plus.
//!
//! `cargo +nightly fuzz run etat-cable` depuis `crates/conduit-kmd-core`.
#![no_main]

use conduit_kmd_core::config::{CableState, CABLE_MAX, CABLE_STATE_BYTES};
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match CableState::from_bytes(data) {
        Ok(etat) => {
            let octets = etat.to_bytes();
            // La longueur est exigée exacte : un état accepté vient forcément d'une
            // entrée de la bonne taille, et se resérialise en cette entrée-là.
            assert_eq!(data.len(), CABLE_STATE_BYTES);
            assert_eq!(octets.as_slice(), data, "aller-retour infidèle");
            assert_eq!(
                CableState::from_bytes(&octets),
                Ok(etat),
                "relecture infidèle"
            );

            // Les domaines que le gestionnaire de propriété tient pour acquis.
            assert!(etat.cable < CABLE_MAX, "câble hors bornes : {}", etat.cable);
            assert!(
                etat.connected <= 1,
                "connected hors {{0, 1}} : {}",
                etat.connected
            );
            assert!(
                (MIN_CHANNELS..=MAX_CHANNELS).contains(&etat.channels),
                "canaux hors bornes : {}",
                etat.channels
            );
            assert_eq!(etat.reserved, 0, "rembourrage non nul accepté");
            assert_eq!(etat.is_connected(), etat.connected != 0);
        }
        Err(cause) => {
            // Le service journalise cette cause : la rendre ne doit pas paniquer.
            let rendu = cause.to_string();
            assert!(!rendu.is_empty());
        }
    }
});
