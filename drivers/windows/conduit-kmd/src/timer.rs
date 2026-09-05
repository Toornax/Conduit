//! Timer noyau périodique avec DPC ([`PeriodicTimer`] : `KTIMER` + `KDPC`), tel que
//! driver-design.md §5.3 le prévoit pour la boucle locale et §5.3 étape 5 pour les
//! notifications. En M1a-07 chaque flux rendu en possède un (signalement des
//! événements de notification) ; M1a-08 le remontera au câble pour y ajouter la copie
//! rendu → capture.
//!
//! # Cycle de vie
//!
//! 1. [`PeriodicTimer::new`] (`const`, tout à zéro) : la structure peut encore être
//!    déplacée (placée dans l'objet COM du flux, alloué par `Box`).
//! 2. [`PeriodicTimer::init`] (**une fois la structure à son adresse définitive**) :
//!    `KeInitializeTimerEx` + `KeInitializeDpc`. Le noyau garde ensuite des pointeurs
//!    vers `KTIMER` et `KDPC` : la structure ne doit plus bouger tant que le timer peut
//!    être armé.
//! 3. [`PeriodicTimer::start`] / [`PeriodicTimer::stop`] (`KeSetTimerEx`,
//!    `KeCancelTimer`) autant de fois que nécessaire, à IRQL `<= DISPATCH_LEVEL`, y
//!    compris sous un spin lock.
//! 4. [`PeriodicTimer::stop_and_flush`] (`PASSIVE_LEVEL`, **hors** de tout spin lock)
//!    avant de libérer quoi que ce soit que la routine DPC pourrait toucher :
//!    `KeCancelTimer` puis `KeFlushQueuedDpcs`, qui attend la fin des DPC en cours sur
//!    tous les processeurs.
//!
//! La routine DPC est une `unsafe extern "C" fn(PKDPC, PVOID, PVOID, PVOID)` qui reçoit
//! le contexte passé à `init` ; elle s'exécute à `DISPATCH_LEVEL` : ni allocation, ni
//! attente, ni mémoire paginée, et un spin lock pour tout état partagé.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use wdk_sys::ntddk::{
    KeCancelTimer, KeFlushQueuedDpcs, KeInitializeDpc, KeInitializeTimerEx, KeSetTimerEx,
};
use wdk_sys::{_TIMER_TYPE, KDPC, KTIMER, LARGE_INTEGER, PKDPC, PVOID};

/// Signature d'une routine DPC (`KDEFERRED_ROUTINE` sans l'`Option`).
pub type DpcRoutine =
    unsafe extern "C" fn(dpc: PKDPC, context: PVOID, argument1: PVOID, argument2: PVOID);

/// Ticks de 100 ns par milliseconde (unité des échéances de `KeSetTimerEx`).
const HNS_PER_MS: i64 = 10_000;

/// Timer périodique et sa DPC, en mémoire non paginée (celle de la structure qui le
/// contient : objet COM alloué dans le pool non paginé, ou `static`).
pub struct PeriodicTimer {
    timer: UnsafeCell<KTIMER>,
    dpc: UnsafeCell<KDPC>,
    /// Vrai après [`init`](Self::init) : `start`/`stop` sont sans effet avant.
    initialized: AtomicBool,
}

// SAFETY: `timer` et `dpc` ne sont jamais dérérérencés côté Rust ; seul le noyau y
// accède, par les pointeurs remis à `KeInitializeTimerEx`/`KeInitializeDpc`, avec sa
// propre synchronisation (`KeSetTimerEx`/`KeCancelTimer` sont sûrs entre processeurs).
// `initialized` est atomique.
unsafe impl Sync for PeriodicTimer {}
// SAFETY: avant `init`, la structure est une valeur ordinaire déplaçable ; après, elle
// n'est déplacée par personne (contrat de `init`).
unsafe impl Send for PeriodicTimer {}

impl PeriodicTimer {
    /// Timer non initialisé (tout à zéro). À placer à son adresse définitive puis
    /// [`init`](Self::init).
    pub const fn new() -> Self {
        Self {
            // SAFETY: `KTIMER` et `KDPC` ne contiennent que des entiers, des pointeurs
            // et des unions de ceux-ci : le motif tout-à-zéro est une valeur valide, que
            // `KeInitializeTimerEx`/`KeInitializeDpc` écraseront de toute façon.
            timer: UnsafeCell::new(unsafe { core::mem::zeroed() }),
            // SAFETY: idem.
            dpc: UnsafeCell::new(unsafe { core::mem::zeroed() }),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialise le `KTIMER` (type notification) et la `KDPC` (`routine`, `context`).
    /// Sans effet si déjà fait.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    ///
    /// # Safety
    ///
    /// `self` est à son adresse définitive et n'y sera plus déplacé tant que le timer
    /// peut être armé ; `routine` et `context` restent valides jusqu'au retour de
    /// [`stop_and_flush`](Self::stop_and_flush) (ou jusqu'à ce que le timer ne soit
    /// plus jamais armé, avec la garantie qu'aucune DPC n'est en cours).
    pub unsafe fn init(&self, routine: DpcRoutine, context: *mut c_void) {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return;
        }
        // SAFETY: `timer` est en mémoire non paginée à son adresse définitive (contrat)
        // et personne d'autre n'y accède avant l'initialisation.
        unsafe { KeInitializeTimerEx(self.timer.get(), _TIMER_TYPE::NotificationTimer) };
        // SAFETY: idem pour `dpc` ; `routine` et `context` sont valides tant que la DPC
        // peut être appelée (contrat).
        unsafe { KeInitializeDpc(self.dpc.get(), Some(routine), context) };
    }

    /// Arme le timer : première échéance dans `period_ms`, puis toutes les
    /// `period_ms` millisecondes (`period_ms` ≥ 1, sinon 1). Réarmer un timer déjà
    /// armé le réinitialise. Rend faux si le timer n'est pas initialisé.
    ///
    /// IRQL : `<= DISPATCH_LEVEL` (utilisable sous spin lock).
    pub fn start(&self, period_ms: u32) -> bool {
        if !self.initialized.load(Ordering::Acquire) {
            return false;
        }
        let period = i32::try_from(period_ms.max(1)).unwrap_or(i32::MAX);
        // Échéance relative : valeur négative en unités de 100 ns.
        let due = LARGE_INTEGER {
            QuadPart: 0i64.saturating_sub(i64::from(period).saturating_mul(HNS_PER_MS)),
        };
        // SAFETY: `timer` et `dpc` sont initialisés (`initialized`) et à leur adresse
        // définitive (contrat de `init`).
        unsafe { KeSetTimerEx(self.timer.get(), due, period, self.dpc.get()) };
        true
    }

    /// Désarme le timer (`KeCancelTimer`). Une DPC déjà en file peut encore
    /// s'exécuter : voir [`stop_and_flush`](Self::stop_and_flush). Sans effet si non
    /// initialisé.
    ///
    /// IRQL : `<= DISPATCH_LEVEL` (utilisable sous spin lock).
    pub fn stop(&self) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: `timer` est initialisé et à son adresse définitive.
        unsafe { KeCancelTimer(self.timer.get()) };
    }

    /// Désarme le timer puis attend la fin de toute DPC en cours ou en file
    /// (`KeFlushQueuedDpcs`). Après le retour, la routine DPC ne s'exécute plus et
    /// le contexte peut être libéré. Sans effet si non initialisé.
    ///
    /// IRQL : **`PASSIVE_LEVEL`**, hors de tout spin lock.
    pub fn stop_and_flush(&self) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        self.stop();
        // SAFETY: `PASSIVE_LEVEL` (contrat de la méthode) ; aucune autre précondition.
        unsafe { KeFlushQueuedDpcs() };
    }
}

impl Default for PeriodicTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PeriodicTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeriodicTimer")
            .field("timer", &self.timer.get())
            .field("initialized", &self.initialized.load(Ordering::Relaxed))
            .finish()
    }
}
