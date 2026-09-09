//! Cible cargo-fuzz : le protocole du canal nommé du service d'assistance (M1b-08).
//!
//! # Pourquoi c'est ici que le fuzz a le plus de valeur
//!
//! Les parseurs de `conduit-kmd-core` sont sans allocation, de longueur fixe, et le
//! crate interdit la panique **par construction** (`clippy::panic`, `unwrap_used`,
//! `indexing_slicing`, `arithmetic_side_effects`…) : le fuzz y a valeur de preuve. Ce
//! parseur-ci est l'inverse : il **alloue** (une `String` pour l'ordre `renommer`), il a
//! une **longueur variable** depuis la version 2 du protocole, et il tourne dans un
//! processus **`LocalSystem`** dont le canal est ouvert en lecture/écriture au groupe
//! `INTERACTIVE`. Tout membre de ce groupe choisit donc entièrement les octets que cette
//! cible lui donne.
//!
//! # Les trois surfaces éprouvées
//!
//! 1. `decouper` — le seul endroit où quatre octets venus du canal deviennent une taille
//!    de tampon. La borne (`MAX_TRAME_OCTETS`) est vérifiée **avant** toute allocation :
//!    la cible confirme qu'aucune charge découpée ne la dépasse et que le découpage
//!    progresse toujours ;
//! 2. `Requete::from_bytes` — sur chaque charge découpée **et** sur l'entrée brute, pour
//!    que le fuzzer n'ait pas à deviner un en-tête de trame valide avant d'atteindre le
//!    parseur ;
//! 3. `Reponse::from_bytes` — le parseur du client, qui n'a pas plus de raison de faire
//!    confiance au canal (un canal nommé se squatte).
//!
//! # Les invariants
//!
//! - **aller-retour** : une requête acceptée se resérialise à l'octet près en l'entrée
//!   reçue. C'est ce qui interdit d'accepter un préfixe valide suivi d'octets en trop ;
//! - **domaines** : le câble est dans `1..=CABLE_MAX`, les canaux dans les bornes de
//!   `conduit_kmd_core::params`, le nom est non vide, tient dans `MAX_NOM_OCTETS` et
//!   passe `validate_cable_name` — le juge du dépôt, pas une règle recopiée ici. Aucune
//!   borne n'est écrite en dur : la cible survit à un déplacement de domaine ;
//! - **cadrage** : ce que `encadrer` produit, `decouper` le rend intact.
//!
//! `cargo +nightly fuzz run protocole` depuis `crates/conduit-helper`.
#![no_main]

use conduit_backend::cable::validate_cable_name;
use conduit_helper::protocole::{
    decouper, longueur_annoncee, Reponse, Requete, EN_TETE_OCTETS, MAX_NOM_OCTETS,
    MAX_TRAME_OCTETS, PROTOCOLE_VERSION, TAILLE_REPONSE, TAILLE_REQUETE, TAILLE_REQUETE_MAX,
};
use conduit_kmd_core::config::CABLE_MAX;
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
use libfuzzer_sys::fuzz_target;

/// Nombre de trames découpées au-delà duquel on arrête : le fuzzer n'apprend plus rien
/// d'une entrée qui répète la même trame, et une exécution bornée reste rapide.
const MAX_TRAMES: usize = 64;

/// Éprouve le parseur des requêtes sur `octets`, quelle qu'en soit la provenance.
fn verifier_requete(octets: &[u8]) {
    let requete = match Requete::from_bytes(octets) {
        Ok(requete) => requete,
        Err(cause) => {
            // Le service journalise cette cause avec l'identité de l'appelant, et la
            // traduit en statut de trame : ni l'un ni l'autre ne doit paniquer.
            assert!(!cause.to_string().is_empty());
            let _ = cause.statut();
            return;
        }
    };

    // Aller-retour : rien n'a été perdu, et rien n'a été toléré en trop.
    let refait = requete.to_bytes();
    assert_eq!(refait.as_slice(), octets, "aller-retour infidèle");
    assert_eq!(
        Requete::from_bytes(&refait),
        Ok(requete.clone()),
        "relecture infidèle"
    );
    assert!((TAILLE_REQUETE..=TAILLE_REQUETE_MAX).contains(&refait.len()));

    // L'en-tête reconstruit dit la même chose que la requête.
    let tete = requete.en_tete();
    assert_eq!(tete[0], PROTOCOLE_VERSION);
    assert_eq!(tete[1], requete.code());
    assert_eq!(&tete[..], &refait[..TAILLE_REQUETE]);
    assert!(!requete.label().is_empty());
    // Un ordre qui touche le pilote modifie forcément quelque chose.
    assert!(!requete.touche_le_pilote() || requete.modifie());

    // Les domaines que le service tient pour acquis avant d'agir.
    if let Some(cable) = requete.cable() {
        assert!(
            cable.0 >= 1 && cable.0 <= CABLE_MAX,
            "câble hors bornes : {cable}"
        );
    }
    if let Requete::Canaux { canaux, .. } = &requete {
        assert!(
            (MIN_CHANNELS..=MAX_CHANNELS).contains(canaux),
            "canaux hors bornes : {canaux}"
        );
    }
    if let Some(nom) = requete.nom() {
        assert!(!nom.is_empty(), "nom vide accepté");
        assert!(
            nom.len() <= MAX_NOM_OCTETS,
            "nom de {} octets accepté",
            nom.len()
        );
        assert!(
            validate_cable_name(nom).is_ok(),
            "nom accepté par le protocole et refusé par la règle du dépôt : {nom:?}"
        );
    }

    // Le cadrage est réversible : ce qu'`encadrer` produit se redécoupe à l'identique.
    let trame = requete.encadrer();
    match decouper(&trame) {
        Ok(Some((charge, consommes))) => {
            assert_eq!(charge, refait.as_slice(), "cadrage infidèle");
            assert_eq!(consommes, trame.len());
        }
        autre => panic!("une trame produite par `encadrer` ne se redécoupe pas : {autre:?}"),
    }
}

fuzz_target!(|data: &[u8]| {
    // 1. Le découpage, trame après trame.
    let mut reste = data;
    for _ in 0..MAX_TRAMES {
        match decouper(reste) {
            Ok(Some((charge, consommes))) => {
                assert!(consommes > EN_TETE_OCTETS, "trame vide consommée");
                assert!(consommes <= reste.len(), "découpage au-delà du tampon");
                assert_eq!(charge.len(), consommes - EN_TETE_OCTETS);
                assert!(
                    charge.len() <= MAX_TRAME_OCTETS,
                    "charge de {} octets au-dessus de la borne {MAX_TRAME_OCTETS}",
                    charge.len()
                );
                verifier_requete(charge);
                let _ = Reponse::from_bytes(charge);
                reste = &reste[consommes..];
            }
            Ok(None) => break,
            Err(cause) => {
                assert!(!cause.to_string().is_empty());
                break;
            }
        }
    }

    // 2. Le parseur des requêtes sur l'entrée brute.
    verifier_requete(data);

    // 3. Le parseur des réponses, côté client.
    match Reponse::from_bytes(data) {
        Ok(reponse) => {
            assert_eq!(data.len(), TAILLE_REPONSE);
            let refait = reponse.to_bytes();
            assert_eq!(refait.as_slice(), data, "aller-retour infidèle");
            assert_eq!(Reponse::from_bytes(&refait), Ok(reponse));
            assert!(!reponse.statut.to_string().is_empty());
        }
        Err(cause) => assert!(!cause.to_string().is_empty()),
    }

    // 4. La borne d'allocation, prise isolément.
    if let Some(tete) = data.get(..EN_TETE_OCTETS) {
        let tete: [u8; EN_TETE_OCTETS] = tete.try_into().unwrap_or([0; EN_TETE_OCTETS]);
        match longueur_annoncee(tete) {
            Ok(annoncee) => assert!((1..=MAX_TRAME_OCTETS).contains(&annoncee)),
            Err(cause) => assert!(!cause.to_string().is_empty()),
        }
    }
});
