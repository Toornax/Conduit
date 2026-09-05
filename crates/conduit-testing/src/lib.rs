//! Utilitaires de test partagés par les crates Conduit.
//!
//! # Allocateur de garde
//!
//! [`GuardAllocator`] est un allocateur global qui délègue à l'allocateur système
//! mais surveille, **par fil**, un mode de garde : en mode « interdit », toute
//! allocation ou libération fait paniquer ; en mode « comptage », elle est comptée.
//! Il sert à prouver qu'un cycle de traitement audio n'alloue pas.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: conduit_testing::GuardAllocator = conduit_testing::GuardAllocator;
//!
//! conduit_testing::assert_no_alloc(|| executor.run(256));
//! ```

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use std::alloc::System;

/// Mode de garde du fil courant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Aucune surveillance.
    Off,
    /// Compte les allocations et libérations sans paniquer.
    Count,
    /// Panique à la première allocation ou libération.
    Forbid,
}

thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Off) };
    static VIOLATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Allocateur global de test : voir la documentation du crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct GuardAllocator;

#[inline]
fn check(what: &'static str) {
    // `try_with` : pendant la destruction du fil, la TLS peut être inaccessible ;
    // dans ce cas on laisse passer.
    let mode = MODE.try_with(Cell::get).unwrap_or(Mode::Off);
    match mode {
        Mode::Off => {}
        Mode::Count => {
            let _ = VIOLATIONS.try_with(|v| v.set(v.get() + 1));
        }
        Mode::Forbid => {
            // Désarme avant de paniquer : la panique elle-même peut allouer.
            let _ = MODE.try_with(|m| m.set(Mode::Off));
            let _ = VIOLATIONS.try_with(|v| v.set(v.get() + 1));
            panic!("{what} interdite dans une section temps réel");
        }
    }
}

// SAFETY: chaque méthode délègue à `System` avec les mêmes arguments ; la garde
// n'altère ni la disposition ni les pointeurs. `check` n'alloue pas (accès TLS à
// des `Cell` initialisées en `const`), sauf en cas de panique, où le mode a été
// désarmé juste avant.
unsafe impl GlobalAlloc for GuardAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        check("allocation");
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        check("libération");
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check("allocation");
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        check("réallocation");
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Exécute `f` en interdisant toute allocation ou libération sur le fil courant.
/// Panique à la première violation. Le mode précédent est restauré ensuite.
pub fn assert_no_alloc<R>(f: impl FnOnce() -> R) -> R {
    let prev = MODE.with(|m| m.replace(Mode::Forbid));
    let r = f();
    MODE.with(|m| m.set(prev));
    r
}

/// Exécute `f` en comptant les allocations et libérations sur le fil courant.
/// Retourne le résultat et le nombre de violations.
pub fn count_allocs<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let prev = MODE.with(|m| m.replace(Mode::Count));
    let before = VIOLATIONS.with(Cell::get);
    let r = f();
    let after = VIOLATIONS.with(Cell::get);
    MODE.with(|m| m.set(prev));
    (r, after - before)
}

/// Nombre total de violations enregistrées sur le fil courant (tous modes).
pub fn violations() -> usize {
    VIOLATIONS.with(Cell::get)
}
