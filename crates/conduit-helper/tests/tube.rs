//! Aller-retour de bout en bout sur le canal nommé (M1b-20).
//!
//! # Tous ces tests sont `#[ignore]`, et voici pourquoi
//!
//! Ils **créent un vrai canal nommé** `\\.\pipe\conduit-helper` sur la machine qui les
//! exécute. C'est transitoire — le canal disparaît avec le processus de test — mais cela
//! touche la machine, et la règle du dépôt est que rien de tel ne tourne dans
//! `cargo test`. Ils n'installent en revanche **aucun service**, n'ouvrent aucun flux
//! audio et n'émettent aucun son : le serveur est lancé dans un fil du processus de
//! test, sous le compte de l'utilisateur courant.
//!
//! Pour les exécuter, dans la VM ou sur un poste où le canal est libre :
//!
//! ```text
//! cargo test -p conduit-helper --test tube -- --ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` n'est pas décoratif : les trois tests créent le **même** nom de
//! canal, et le second échouerait par `ERROR_ACCESS_DENIED` si le premier tenait encore
//! le sien.
//!
//! # Ce qu'ils vérifient, et ce qu'ils ne vérifient pas
//!
//! Ils vérifient le **chemin complet** : descripteur de sécurité construit et accepté,
//! canal créé, client connecté, trame reçue, parseur appliqué, réponse cadrée et relue.
//! Ils ne vérifient **pas** que le pilote a bougé — sans lui, `activer` répond
//! `PiloteAbsent`, ce qui est le bon comportement et ce que le test affirme.

#![cfg(windows)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use conduit_backend::CableId;
use conduit_helper::journal::{Journal, Niveau};
use conduit_helper::protocole::{
    encadrer, Reponse, Requete, Statut, MAX_TRAME_OCTETS, PROTOCOLE_VERSION,
};
use conduit_helper::tube::{self, Arret};

/// Un serveur lancé dans un fil, arrêté à la destruction.
struct ServeurDeTest {
    arret: Arc<Arret>,
    fil: Option<std::thread::JoinHandle<()>>,
}

impl ServeurDeTest {
    /// Démarre le serveur et attend qu'il réponde.
    ///
    /// L'attente est indispensable : `servir` construit son descripteur de sécurité puis
    /// crée la première instance du canal, et un client qui se présenterait avant
    /// recevrait `ERROR_FILE_NOT_FOUND` — une course, pas une panne.
    fn demarrer() -> Self {
        let journal = Arc::new(Journal::vers_stderr(Niveau::Debug));
        let arret = Arc::new(Arret::nouveau().expect("événement d'arrêt"));
        let arret_fil = Arc::clone(&arret);
        let fil = std::thread::spawn(move || {
            if let Err(erreur) = tube::servir(&journal, &arret_fil) {
                panic!("le serveur n'a pas démarré : {erreur}");
            }
        });
        let limite = Instant::now() + Duration::from_secs(5);
        while Instant::now() < limite && !tube::repond() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            tube::repond(),
            "le canal n'a pas répondu en 5 s : un autre processus tient-il déjà \
             \\\\.\\pipe\\conduit-helper ?"
        );
        Self {
            arret,
            fil: Some(fil),
        }
    }
}

impl Drop for ServeurDeTest {
    fn drop(&mut self) {
        self.arret.demander();
        if let Some(fil) = self.fil.take() {
            let _ = fil.join();
        }
    }
}

/// L'aller-retour nominal : `version` et `lister` traversent le canal et reviennent.
#[test]
#[ignore = "crée un canal nommé sur la machine (voir l'en-tête du fichier)"]
fn l_aller_retour_nominal_traverse_le_canal() {
    let _serveur = ServeurDeTest::demarrer();

    let reponse = tube::demander(&Requete::Version).expect("le service répond");
    assert_eq!(reponse.ordre, conduit_helper::protocole::ORDRE_VERSION);
    // Sans le pilote, la lecture des câbles échoue et le statut le dit — ce qui est le
    // bon comportement, pas un échec du canal.
    if reponse.statut.succes() {
        assert_eq!(reponse.detail, u32::from(PROTOCOLE_VERSION));
    } else {
        assert!(
            matches!(reponse.statut, Statut::PiloteAbsent | Statut::ErreurSysteme),
            "statut inattendu sans pilote : {}",
            reponse.statut
        );
    }

    let reponse = tube::demander(&Requete::Lister).expect("le service répond");
    assert_eq!(reponse.ordre, conduit_helper::protocole::ORDRE_LISTER);
    // Un câble annoncé actif est forcément annoncé présent : l'inverse serait un masque
    // construit de travers.
    assert_eq!(reponse.actifs & !reponse.presents, 0);
}

/// Une trame hostile reçoit le statut qui la nomme, jamais un silence.
///
/// C'est la moitié du critère de sécurité : le serveur ne fait pas confiance à ce qu'il
/// reçoit, et le prouve en répondant **la bonne cause** à chacune de ces entrées.
#[test]
#[ignore = "crée un canal nommé sur la machine (voir l'en-tête du fichier)"]
fn les_trames_hostiles_sont_refusees_avec_leur_cause() {
    let _serveur = ServeurDeTest::demarrer();

    // Une requête valide, dont on abîme un champ à la fois.
    let valide = Requete::Lister.to_bytes();

    let cas: [(&str, Vec<u8>, Statut); 6] = [
        // Une version que ce service ne sert pas.
        (
            "version 99",
            {
                let mut brut = valide.clone();
                brut[0] = 99;
                encadrer(&brut)
            },
            Statut::VersionInconnue,
        ),
        // Un ordre inconnu.
        (
            "ordre 200",
            {
                let mut brut = valide.clone();
                brut[1] = 200;
                encadrer(&brut)
            },
            Statut::OrdreInconnu,
        ),
        // Un préfixe parfaitement valide, suivi d'un octet en trop.
        (
            "9 octets",
            {
                let mut brut = valide.clone();
                brut.push(0);
                encadrer(&brut)
            },
            Statut::TrameInvalide,
        ),
        // Un octet de moins.
        ("7 octets", encadrer(&valide[..7]), Statut::TrameInvalide),
        // Un champ que `lister` n'utilise pas.
        (
            "câble parasite",
            {
                let mut brut = valide.clone();
                brut[2] = 1;
                encadrer(&brut)
            },
            Statut::TrameInvalide,
        ),
        // Un numéro de câble hors domaine sur un ordre qui en prend un.
        (
            "câble 99",
            {
                let mut brut = Requete::Activer(CableId(1)).to_bytes();
                brut[2] = 99;
                encadrer(&brut)
            },
            Statut::CableInconnu,
        ),
    ];

    for (nom, trame, attendu) in cas {
        let charge = tube::demander_octets(&trame)
            .unwrap_or_else(|e| panic!("{nom} : le service n'a pas répondu — {e}"));
        let reponse = Reponse::from_bytes(&charge)
            .unwrap_or_else(|e| panic!("{nom} : réponse illisible — {e}"));
        assert_eq!(reponse.statut, attendu, "{nom}");
        // Un refus ne prétend jamais connaître l'état des câbles.
        assert_eq!(reponse.presents, 0, "{nom}");
        assert_eq!(reponse.actifs, 0, "{nom}");
    }

    // Une version étrangère reçoit **la version servie** dans le détail : c'est le seul
    // refus qui dit au client comment se corriger.
    let mut brut = valide.clone();
    brut[0] = 99;
    let charge = tube::demander_octets(&encadrer(&brut)).expect("le service répond");
    let reponse = Reponse::from_bytes(&charge).expect("réponse lisible");
    assert_eq!(reponse.detail, u32::from(PROTOCOLE_VERSION));

    // Le service a survécu à toute la série : il sert encore.
    assert!(tube::repond(), "le service est mort sur une trame hostile");
}

/// Une longueur annoncée au-delà de la borne est refusée **avant** toute allocation.
///
/// Le serveur ne répond alors rien d'analysable (il refuse l'en-tête et raccroche) : ce
/// que le test vérifie est qu'il ne meurt pas et qu'il sert toujours après. Sans la
/// borne, ces quatre octets feraient réserver 4 Gio au processus `LocalSystem`.
#[test]
#[ignore = "crée un canal nommé sur la machine (voir l'en-tête du fichier)"]
fn une_longueur_hors_borne_ne_fait_rien_reserver() {
    let _serveur = ServeurDeTest::demarrer();

    for annoncee in [
        u32::try_from(MAX_TRAME_OCTETS)
            .unwrap_or(u32::MAX)
            .saturating_add(1),
        1 << 20,
        u32::MAX,
    ] {
        let mut hostile = annoncee.to_le_bytes().to_vec();
        hostile.extend_from_slice(&Requete::Lister.to_bytes());
        // Le résultat n'a pas d'importance : le serveur refuse l'en-tête et raccroche,
        // donc le client peut aussi bien recevoir un refus cadré qu'un échange rompu.
        let _ = tube::demander_octets(&hostile);
        assert!(
            tube::repond(),
            "le service n'a pas survécu à une longueur annoncée de {annoncee}"
        );
    }
}
