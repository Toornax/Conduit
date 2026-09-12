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
//! # Trois parseurs, la même entrée
//!
//! Depuis le lot 2 du mode paquets, les mêmes octets partent aussi dans
//! `CablePackets::from_bytes` (`KSPROPERTY_CONDUIT_PACKETS`, 248 octets), et depuis la
//! déclaration des contraintes de taille de paquet dans `CableTransport::from_bytes`
//! (`KSPROPERTY_CONDUIT_TRANSPORT`, 80 octets). Les trois longueurs exigées étant
//! différentes, une entrée donnée n'en intéresse qu'un seul à la fois — c'est voulu : le
//! fuzzer explore les trois domaines sans qu'on ait à deviner lequel il vise, et un préfixe
//! valide de l'un ne doit être accepté par aucun des trois.
//!
//! `cargo +nightly fuzz run etat-cable` depuis `crates/conduit-kmd-core`.
#![no_main]

use conduit_kmd_core::config::{
    CablePackets, CableState, CableTransport, StreamSide, ALLOCATION_MODE_MAX, CABLE_MAX,
    CABLE_PACKETS_BYTES, CABLE_STATE_BYTES, CABLE_TRANSPORT_BYTES, KS_RUN_STATE_MAX,
    PACKET_EXPOSURE_MAX, PACKET_IRQL_MAX,
};
use conduit_kmd_core::params::{MAX_CHANNELS, MAX_PACKET_MODE, MIN_CHANNELS};
use libfuzzer_sys::fuzz_target;

/// L'état du transport : mêmes exigences que l'état, sur ses propres domaines.
///
/// Les deux `NTSTATUS` de pose des contraintes de taille de paquet n'ont **aucun** domaine
/// — il n'existe pas de liste fermée de `NTSTATUS` —, et c'est justement ce que
/// l'aller-retour éprouve : ils doivent traverser le parseur à l'octet près, quelle que
/// soit leur valeur, sans jamais faire refuser une valeur par ailleurs correcte.
fn eprouver_transport(data: &[u8]) {
    match CableTransport::from_bytes(data) {
        Ok(transport) => {
            let octets = transport.to_bytes();
            assert_eq!(data.len(), CABLE_TRANSPORT_BYTES);
            assert_eq!(octets.as_slice(), data, "aller-retour infidèle");
            assert_eq!(
                CableTransport::from_bytes(&octets),
                Ok(transport),
                "relecture infidèle"
            );

            // Les domaines fermés que le relevé tient pour acquis.
            assert!(transport.cable < CABLE_MAX, "câble hors bornes");
            assert_eq!(transport.reserved, 0, "rembourrage non nul accepté");
            for sens in StreamSide::ALL {
                let bloc = transport.side(sens);
                assert!(bloc.mode <= ALLOCATION_MODE_MAX, "mode hors bornes");
                assert!(bloc.mode().is_some(), "mode sans nom");
                assert!(bloc.ks_state <= KS_RUN_STATE_MAX, "état KS hors bornes");
                assert!(bloc.ks_state().is_some(), "état KS sans nom");
                // Le libellé de la pose part dans le relevé : le rendre ne doit rien casser.
                assert!(!transport.constraints_label(sens).is_empty());
            }
            assert_eq!(
                transport.contraintes_declarees(),
                transport.constraints_render == 0 && transport.constraints_capture == 0
            );
        }
        Err(cause) => {
            // Cette cause part dans le relevé : la rendre ne doit pas paniquer.
            assert!(!cause.to_string().is_empty());
        }
    }
}

/// Le relevé de paquets : mêmes exigences que l'état, sur ses propres domaines.
fn eprouver_paquets(data: &[u8]) {
    match CablePackets::from_bytes(data) {
        Ok(paquets) => {
            let octets = paquets.to_bytes();
            assert_eq!(data.len(), CABLE_PACKETS_BYTES);
            assert_eq!(octets.as_slice(), data, "aller-retour infidèle");
            assert_eq!(
                CablePackets::from_bytes(&octets),
                Ok(paquets),
                "relecture infidèle"
            );

            // Les domaines fermés que le relevé tient pour acquis.
            assert!(paquets.cable < CABLE_MAX, "câble hors bornes");
            assert!(
                paquets.packet_mode <= MAX_PACKET_MODE,
                "mode paquets hors de {{0, 1}} : {}",
                paquets.packet_mode
            );
            assert_eq!(paquets.mode_actif(), paquets.packet_mode != 0);
            for sens in StreamSide::ALL {
                let bloc = paquets.side(sens);
                assert!(bloc.exposure <= PACKET_EXPOSURE_MAX, "exposition hors bornes");
                assert!(bloc.exposure().is_some(), "exposition sans nom");
                assert!(bloc.irql_last <= PACKET_IRQL_MAX, "IRQL trop large");
                assert!(bloc.irql_max <= PACKET_IRQL_MAX, "IRQL max trop large");
                assert_eq!(bloc.reserved, 0, "rembourrage non nul accepté");
                // Le total ne recule pas, quels que soient les quatre compteurs.
                assert!(bloc.appels() >= bloc.set_write_packet);
                assert_eq!(bloc.emprunte(), bloc.appels() > 0);
            }
            assert_eq!(paquets.emprunte(), paquets.appels_total() > 0);
        }
        Err(cause) => {
            // Cette cause part dans le relevé : la rendre ne doit pas paniquer.
            assert!(!cause.to_string().is_empty());
        }
    }
}

fuzz_target!(|data: &[u8]| {
    eprouver_paquets(data);
    eprouver_transport(data);

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
