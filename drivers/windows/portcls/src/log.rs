//! Traces de débogage du crate (`pc_log!`), sur le modèle de `conduit_kmd::kmd_log!`.
//!
//! Le crate `portcls` se teste en **mode utilisateur** et ne peut donc pas dépendre
//! inconditionnellement de `wdk` (qui lie `ntoskrnl`). La macro n'écrit réellement que
//! sous **deux** conditions réunies :
//!
//! - feature `kernel` : le crate est compilé dans le pilote, `wdk::println!` (→
//!   `DbgPrint`) est disponible ;
//! - `debug_assertions` : profil `dev`. En release, aucune chaîne de format ni appel à
//!   `DbgPrint` ne subsiste dans le binaire livré.
//!
//! Dans tous les autres cas (tests en mode utilisateur, release), la macro se réduit à un
//! `if false { … }` : les arguments restent **vérifiés par le compilateur**, si bien que
//! les quatre combinaisons (feature × profil) compilent le même code appelant.
//!
//! Les traces sont préfixées par `conduit_portcls:` pour se distinguer de celles du
//! pilote (`conduit_kmd:`) dans WinDbg ou DebugView.
//!
//! IRQL : `DbgPrint` s'appelle à IRQL ≤ `DIRQL` ; les chemins tracés ici
//! (`QueryInterface`, mode paquets) sont à `<= DISPATCH_LEVEL`.

/// Journalise un message de débogage (syntaxe de `format!`) vers le débogueur noyau.
///
/// Rien n'est émis hors de la combinaison feature `kernel` + `debug_assertions` ; voir la
/// documentation du module.
#[cfg(all(feature = "kernel", debug_assertions))]
#[macro_export]
macro_rules! pc_log {
    ($($arg:tt)*) => {
        ::wdk::println!("conduit_portcls: {}", ::core::format_args!($($arg)*))
    };
}

/// Journalise un message de débogage (syntaxe de `format!`) vers le débogueur noyau.
///
/// Variante inerte (mode utilisateur ou release) : les arguments sont tout de même
/// vérifiés par le compilateur, dans un bloc jamais évalué.
#[cfg(not(all(feature = "kernel", debug_assertions)))]
#[macro_export]
macro_rules! pc_log {
    ($($arg:tt)*) => {
        if false {
            let _ = ::core::format_args!($($arg)*);
        }
    };
}
