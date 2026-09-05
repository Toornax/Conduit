//! Lecture du compteur de performance (`KeQueryPerformanceCounter`), seul référentiel
//! de temps du pilote (driver-design.md §5.1) : l'horloge virtuelle de
//! `conduit_kmd_core::position` convertit ses ticks en trames.

use conduit_kmd_core::VirtualClock;
use wdk_sys::LARGE_INTEGER;
use wdk_sys::ntddk::KeQueryPerformanceCounter;

/// Valeur courante du compteur de performance, en ticks.
///
/// IRQL : tout niveau.
pub fn now() -> u64 {
    // SAFETY: `KeQueryPerformanceCounter` accepte un pointeur de fréquence nul et n'a
    // aucune autre précondition.
    let counter = unsafe { KeQueryPerformanceCounter(core::ptr::null_mut()) };
    // SAFETY: le noyau écrit toujours `QuadPart` (les trois membres de l'union
    // occupent les mêmes 8 octets).
    let quad = unsafe { counter.QuadPart };
    u64::try_from(quad).unwrap_or(0)
}

/// Fréquence du compteur de performance, en ticks par seconde (fixe pour la durée de
/// vie du système ; 0 n'arrive pas, mais est renvoyé tel quel plutôt que supposé).
///
/// IRQL : tout niveau.
pub fn frequency() -> u64 {
    let mut freq = LARGE_INTEGER::default();
    // SAFETY: `freq` est une variable locale inscriptible le temps de l'appel.
    unsafe { KeQueryPerformanceCounter(&mut freq) };
    // SAFETY: le noyau a écrit `QuadPart` (voir `now`).
    let quad = unsafe { freq.QuadPart };
    u64::try_from(quad).unwrap_or(0)
}

/// Horloge virtuelle d'un flux à `sample_rate` trames/s, sur la fréquence du compteur
/// de performance lue maintenant. `None` si l'une des deux est nulle.
///
/// IRQL : tout niveau.
pub fn virtual_clock(sample_rate: u32) -> Option<VirtualClock> {
    VirtualClock::new(frequency(), sample_rate)
}
