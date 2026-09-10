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
//!   passe `validate_cable_name` — le juge du dépôt, pas une règle recopiée ici. Le format
//!   d'un `Requete::Format` se réencode à l'identique et sa fréquence est l'une des trois
//!   que le contrat sert. Aucune borne n'est écrite en dur : la cible survit à un
//!   déplacement de domaine ;
//! - **longueur par ordre** (version 3) : une réponse acceptée fait exactement ce que
//!   `longueur_reponse` exige pour son **octet d'ordre** — 92 pour un `lister`, 28 sinon —
//!   et ses dix-sept mots de format sont chacun 0 ou décodables ;
//! - **cadrage** : ce que `encadrer` produit, `decouper` le rend intact.
//!
//! `cargo +nightly fuzz run protocole` depuis `crates/conduit-helper`.
#![no_main]

use conduit_backend::cable::validate_cable_name;
use conduit_helper::protocole::{
    decouper, longueur_annoncee, longueur_reponse, Reponse, Requete, EN_TETE_OCTETS,
    MAX_NOM_OCTETS, MAX_TRAME_OCTETS, PROTOCOLE_VERSION, TAILLE_REPONSE_MAX, TAILLE_REQUETE,
    TAILLE_REQUETE_FORMAT, TAILLE_REQUETE_MAX,
};
use conduit_kmd_core::config::{CableFormat, CABLE_MAX};
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
use libfuzzer_sys::fuzz_target;

/// Les fréquences que le contrat sert, **déduites** de son propre codec.
///
/// Rien n'est recopié : chaque valeur est celle que `CableFormat::decode` rend pour le
/// code correspondant. Une quatrième fréquence ajoutée au contrat entre dans cette liste
/// sans qu'on rouvre la cible, et la propriété continue de dire quelque chose.
fn frequences_servies() -> [u32; 3] {
    let mut vues = [0u32; 3];
    for (rang, code) in [1u32, 2, 3].into_iter().enumerate() {
        // Un encodage complet dont seule la fréquence est relue.
        let brut = code | (3 << 8) | (2 << 16);
        vues[rang] = CableFormat::decode(brut).map_or(0, |f| f.sample_rate);
    }
    vues
}

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
    // Le domaine du format, et surtout son **aller-retour exact** : sans le refus de
    // l'octet de poids fort, deux encodages donneraient le même format et la
    // resérialisation ne rendrait plus les octets reçus.
    if let Requete::Format { format, .. } = &requete {
        assert_eq!(refait.len(), TAILLE_REQUETE_FORMAT, "charge de format tordue");
        let brut = requete.charge_format().expect("un format porte sa charge");
        assert_eq!(format.encode(), brut, "l'encodage n'est pas celui du format");
        assert_eq!(
            CableFormat::decode(brut),
            Ok(*format),
            "encodage accepté qui ne se relit pas à l'identique : {brut:#010x}"
        );
        assert!(
            frequences_servies().contains(&format.sample_rate),
            "fréquence {} hors des trois que le contrat sert",
            format.sample_rate
        );
        assert!(
            (MIN_CHANNELS..=MAX_CHANNELS).contains(&u32::from(format.channels)),
            "format à {} canaux accepté",
            format.channels
        );
    } else {
        assert_eq!(requete.charge_format(), None, "charge de format parasite");
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

/// Éprouve le parseur des réponses sur `octets`, quelle qu'en soit la provenance.
///
/// Deux propriétés que la version 3 a rendues non triviales :
///
/// - **la longueur acceptée est celle que l'octet d'ordre exige**, et elle seule. C'est ce
///   qui remplace le « toujours 28 octets » de la version 2, et le fuzzer est le mieux
///   placé pour trouver l'entrée où le parseur lirait une table qu'il n'a pas reçue ;
/// - **les dix-sept mots de format sont dans le domaine** : chacun est 0 ou décodable.
///   Sans quoi un encodage aberrant traverserait le client jusqu'à l'affichage.
fn verifier_reponse(octets: &[u8]) {
    // La longueur exigée se déduit du code d'ordre, jamais d'un nombre porté par la trame.
    let attendue = longueur_reponse(octets.get(1).copied().unwrap_or(u8::MAX));
    assert!(attendue <= TAILLE_REPONSE_MAX);
    match Reponse::from_bytes(octets) {
        Ok(reponse) => {
            assert_eq!(
                octets.len(),
                attendue,
                "réponse acceptée d'une longueur que son ordre n'exige pas"
            );
            let refait = reponse.to_bytes();
            assert_eq!(refait.as_slice(), octets, "aller-retour infidèle");
            assert_eq!(Reponse::from_bytes(&refait), Ok(reponse));
            assert!(!reponse.statut.to_string().is_empty());

            // Le format du câble visé, puis les seize de la table : dix-sept mots, tous
            // dans le domaine.
            let mut mots = 0usize;
            for mot in core::iter::once(reponse.format).chain(reponse.formats) {
                assert!(
                    mot == 0 || CableFormat::decode(mot).is_ok(),
                    "mot de format hors domaine accepté : {mot:#010x}"
                );
                mots = mots.saturating_add(1);
            }
            assert_eq!(mots, CABLE_MAX as usize + 1);

            // La table ne voyage que pour `lister` : ailleurs elle est nulle, et l'ordre
            // d'un `Reponse` relu le dit.
            if reponse.ordre != octets[1] {
                panic!("l'ordre relu n'est pas celui de la trame");
            }
        }
        Err(cause) => assert!(!cause.to_string().is_empty()),
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
                verifier_reponse(charge);
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
    verifier_reponse(data);

    // 4. La borne d'allocation, prise isolément.
    if let Some(tete) = data.get(..EN_TETE_OCTETS) {
        let tete: [u8; EN_TETE_OCTETS] = tete.try_into().unwrap_or([0; EN_TETE_OCTETS]);
        match longueur_annoncee(tete) {
            Ok(annoncee) => assert!((1..=MAX_TRAME_OCTETS).contains(&annoncee)),
            Err(cause) => assert!(!cause.to_string().is_empty()),
        }
    }
});
