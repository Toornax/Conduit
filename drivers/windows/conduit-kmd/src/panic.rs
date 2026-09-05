//! Gestionnaire de panique du pilote.
//!
//! Le crate `wdk-panic` 0.4.1 publié se contente d'un `loop {}` qui gèle la machine
//! ([windows-drivers-rs.md](../../../docs/windows-drivers-rs.md), « pièges connus »). Une
//! panique en noyau est un bogue du pilote : on préfère un bug check explicite, analysable
//! dans WinDbg (`!analyze -v`) et dans le vidage mémoire, à un gel silencieux
//! (driver-design.md §2.3).
//!
//! Les lints `clippy::panic`, `unwrap_used`, `expect_used`, `indexing_slicing` et
//! `arithmetic_side_effects` en `deny` visent à ce que ce gestionnaire ne soit jamais
//! atteint par du code de Conduit ; il reste nécessaire au compilateur (`no_std`) et
//! couvre les paniques des dépendances.

use core::panic::PanicInfo;

use wdk_sys::ntddk::KeBugCheckEx;

/// Code de bug check privé de Conduit (plage `0xE000_0000`–`0xEFFF_FFFF` réservée aux
/// pilotes tiers). Affiché sur l'écran bleu et par `!analyze -v`.
const CONDUIT_BUGCHECK_PANIC: u32 = 0xE000_0001;

/// Traduit toute panique Rust en bug check `CONDUIT_BUGCHECK_PANIC`.
///
/// Les quatre paramètres du bug check sont laissés à zéro en M1a-01 ; les prochaines
/// tâches y placeront un pointeur vers le message et l'emplacement source.
#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    // SAFETY: `KeBugCheckEx` s'appelle à tout IRQL, sans précondition sur ses paramètres
    // (ils ne sont qu'affichés et journalisés), et ne revient jamais.
    unsafe { KeBugCheckEx(CONDUIT_BUGCHECK_PANIC, 0, 0, 0, 0) }
}
