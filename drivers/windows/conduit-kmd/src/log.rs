//! Journalisation de débogage du pilote (driver-design.md §7).
//!
//! `kmd_log!` écrit vers le débogueur noyau (`wdk::println!` → `DbgPrint`, visible dans
//! WinDbg ou DebugView « Capture Kernel »), préfixé par `conduit_kmd:`. La macro est
//! **vide en release** : aucune chaîne de format ni appel à `DbgPrint` ne subsiste dans
//! le binaire livré. WPP n'est pas disponible côté Rust ; M1b évaluera `EtwWrite`.
//!
//! IRQL : comme `DbgPrint`, à appeler à IRQL ≤ `DIRQL` ; les chemins à `DISPATCH_LEVEL`
//! (DPC de copie, `GetPosition`) ne journalisent pas.

/// Journalise un message de débogage (syntaxe de `format!`), rien en release.
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! kmd_log {
    ($($arg:tt)*) => {
        ::wdk::println!("conduit_kmd: {}", ::core::format_args!($($arg)*))
    };
}

/// Journalise un message de débogage (syntaxe de `format!`), rien en release.
///
/// Les arguments sont tout de même vérifiés par le compilateur (`format_args!` dans un
/// bloc jamais évalué) pour que les deux profils compilent le même code appelant.
#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! kmd_log {
    ($($arg:tt)*) => {
        if false {
            let _ = ::core::format_args!($($arg)*);
        }
    };
}
