//! `conduit-helper` — le service d'assistance Windows (M1b-20).
//!
//! # Pourquoi ce service existe
//!
//! Le pilote `conduit-kmd` expose son jeu de propriétés KS privé `KSPROPSETID_Conduit`
//! (`docs/driver-design.md` §6), et **toute écriture exige
//! `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`**. Mesuré en machine virtuelle le
//! 2026-09-08 :
//!
//! - une écriture depuis une session **non élevée** est refusée par
//!   `ERROR_PRIVILEGE_NOT_HELD` (1314) ;
//! - le privilège est **présent mais désactivé** dans tout jeton neuf — y compris celui
//!   de `LocalSystem` — et doit être armé par `AdjustTokenPrivileges` ;
//! - une fois armé, l'écriture passe et l'endpoint apparaît en **77 ms**.
//!
//! Le démon `conduitd` tourne dans la session de l'utilisateur, **sans privilèges** : il
//! ne peut donc pas écrire lui-même. Ce service tourne en `LocalSystem` et fait
//! l'écriture pour lui, sur ordre reçu par un canal nommé.
//!
//! # Les huit modules, et ce que chacun garantit
//!
//! | Module | Rôle | Testé sans Windows ? |
//! |---|---|---|
//! | [`protocole`] | le format des trames, le parseur, les domaines | **oui**, entièrement |
//! | [`controle`] | le `CableControl` du démon (M1b-34) | oui, sauf l'aller-retour |
//! | [`rapport`] | la mise en forme des réponses à l'écran | **oui**, entièrement |
//! | [`cli`] | les sous-commandes du binaire | **oui**, entièrement |
//! | [`journal`] | l'horodatage, les niveaux, l'identité de l'appelant | oui, sauf l'heure du système |
//! | [`securite`] | le SDDL du canal et sa vérification | le SDDL oui, sa conversion non |
//! | `tube` | le serveur et le client du canal nommé | non (Windows) |
//! | `cables` | l'exécution d'un ordre par le transport KS | partiellement (traduction des refus) |
//! | `scm` | l'installation et le dispatcher de service | oui pour les parties pures |
//!
//! Le partage est délibéré : tout ce qui **décide** est pur et vérifiable sans machine
//! virtuelle ; tout ce qui **appelle** le système est mince et se lit d'un coup.
//!
//! # Ce que ce crate ne réécrit pas
//!
//! - le **contrat** du jeu de propriétés (GUID, identifiants, disposition de
//!   `CableState`, parseur, bornes) vient de `conduit-kmd-core`, partagé avec le
//!   pilote ;
//! - le **transport** (énumération `KSCATEGORY_TOPOLOGY`, appariement exact de la chaîne
//!   de référence, `IOCTL_KS_PROPERTY`, armement du privilège et garde de restauration)
//!   vient de `conduit_backend_wasapi::cable`, écrit et vérifié en machine en M1b-04.
//!
//! Ce n'est pas non plus le protocole de `conduit-protocol`, qui est le protocole
//! **utilisateur** entre le démon et `conduitctl` : voir l'en-tête de [`protocole`].
//!
//! # La sécurité du canal, en une phrase
//!
//! Un service `LocalSystem` qui accepte des ordres par un canal nommé est une élévation
//! de privilège s'il est mal fermé. Le descripteur de sécurité est donc **construit
//! explicitement** (`SYSTEM` et `Administrateurs` en contrôle total, `INTERACTIVE` en
//! lecture/écriture de données seulement), **jamais nul**, et vérifié après construction
//! — le service refuse de démarrer s'il ne peut pas le prouver. Voir [`securite`].
//!
//! # Aucun test n'installe quoi que ce soit
//!
//! Contrainte absolue du dépôt. Les tests que `cargo test` exécute ne créent aucun
//! service, n'ouvrent aucun canal, n'ouvrent aucun flux audio et n'émettent aucun son.
//! Ceux qui touchent au système se limitent à convertir une chaîne SDDL et à lire l'heure
//! locale.
//!
//! `tests/tube.rs` fait l'aller-retour complet sur un vrai canal nommé et est donc
//! **`#[ignore]`** dans son ensemble ; il n'installe pas de service pour autant, et se
//! lance à la main :
//!
//! ```text
//! cargo test -p conduit-helper --test tube -- --ignored --test-threads=1
//! ```

// `unsafe` confiné aux appels Win32, chaque bloc justifié par un `SAFETY:`.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

// Portables : leurs tests tournent sur Linux et macOS, ce qui est tout l'intérêt de les
// avoir écrits purs.
pub mod cli;
pub mod controle;
pub mod journal;
pub mod protocole;
pub mod rapport;
pub mod securite;

#[cfg(windows)]
pub mod cables;
#[cfg(windows)]
pub mod scm;
#[cfg(windows)]
pub mod tube;
