//! Timer noyau périodique **haute résolution** ([`ExTimer`] : `EX_TIMER`), tel que
//! driver-design.md §5.3 le prévoit pour la boucle locale (étapes 1 à 4) et les
//! notifications (étape 5). Un seul par câble ([`crate::cable::Cable`]) depuis M1a-08 ;
//! son rappel fait la copie rendu → capture et signale les événements des deux flux.
//!
//! # Pourquoi `Ex*` et non `KeSetTimerEx`
//!
//! Un `KTIMER` n'a que la résolution de l'horloge système (15,6 ms par défaut) : une
//! période demandée à 1 ms se réveillerait en réalité toutes les 15,6 ms, ce qui
//! condamnerait à la fois les notifications d'un tampon de 10 ms et la boucle locale
//! (l'avance de 2 ms de `conduit_kmd_core::loopback` serait dépassée à chaque tick,
//! donc un débordement à chaque tick). Élever la résolution globale
//! (`ExSetTimerResolution`) pénaliserait tout le système. Les timers `Ex*` créés avec
//! `EX_TIMER_HIGH_RESOLUTION` (Windows 8.1+, ce que fait SYSVAD) donnent la précision
//! demandée sans effet global : c'est le seul mécanisme retenu, **sans repli**
//! `KeSetTimerEx` (voir [`crate::cable::Cable::start`]).
//!
//! # Cycle de vie
//!
//! 1. [`ExTimer::new`] (`const`, pointeur nul) : utilisable dans une `static`.
//! 2. [`ExTimer::create`] au premier `StartDevice` (`PASSIVE_LEVEL`) : `ExAllocateTimer`
//!    alloue l'objet timer dans le pool non paginé du noyau et mémorise le rappel et son
//!    contexte. Sans effet si déjà créé.
//! 3. [`ExTimer::start`] / [`ExTimer::stop`] (`ExSetTimer`, `ExCancelTimer`), à IRQL
//!    `<= DISPATCH_LEVEL`, y compris sous un spin lock.
//! 4. [`ExTimer::delete`] au déchargement du pilote (`PASSIVE_LEVEL`, hors de tout spin
//!    lock) : `ExDeleteTimer(timer, Cancel = TRUE, Wait = TRUE, NULL)` annule le timer et
//!    **attend** la fin du rappel en cours avant de libérer l'objet.
//!
//! Le rappel est une `unsafe extern "C" fn(PEX_TIMER, PVOID)` (`EXT_CALLBACK`) qui reçoit
//! le contexte passé à `create` ; il s'exécute à `DISPATCH_LEVEL` : ni allocation, ni
//! attente, ni mémoire paginée, et un spin lock pour tout état partagé.

use core::ffi::c_void;
use core::fmt;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use wdk_sys::ntddk::{ExAllocateTimer, ExCancelTimer, ExDeleteTimer, ExSetTimer};
use wdk_sys::{_EX_TIMER, BOOLEAN, EX_TIMER_HIGH_RESOLUTION, PEX_TIMER, PVOID};

/// Signature d'un rappel de timer `Ex*` (`EXT_CALLBACK` sans l'`Option`).
pub type TimerCallback = unsafe extern "C" fn(timer: PEX_TIMER, context: PVOID);

/// Ticks de 100 ns par milliseconde : unité de `DueTime` **et** de `Period` pour
/// `ExSetTimer` (contrairement à `KeSetTimerEx`, dont la période est en millisecondes).
const HNS_PER_MS: i64 = 10_000;

/// `TRUE` du WDK au type `BOOLEAN` (`u8`) attendu par `ExDeleteTimer` ; `wdk_sys::TRUE`
/// est un `u32`.
const BOOLEAN_TRUE: BOOLEAN = 1;

/// Timer haute résolution et son rappel : voir la documentation du module.
pub struct ExTimer {
    /// L'objet `EX_TIMER` alloué par `ExAllocateTimer`, nul avant [`Self::create`] et
    /// après [`Self::delete`].
    timer: AtomicPtr<_EX_TIMER>,
}

impl ExTimer {
    /// Timer non créé (pointeur nul). `start`, `stop` et `delete` sont sans effet tant
    /// que [`create`](Self::create) n'a pas réussi.
    pub const fn new() -> Self {
        Self {
            timer: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Alloue le timer haute résolution avec `callback` et `context`. Rend vrai si le
    /// timer existe au retour (créé ici ou déjà créé), faux si `ExAllocateTimer` a
    /// échoué (pool épuisé).
    ///
    /// IRQL : `<= APC_LEVEL` (donc `PASSIVE_LEVEL` en pratique : `StartDevice`).
    ///
    /// # Safety
    ///
    /// `context` reste valide jusqu'au retour de [`delete`](Self::delete) : en pratique
    /// un `&'static Cable`, vivant pour toute la durée de chargement du pilote.
    pub unsafe fn create(&self, callback: TimerCallback, context: *mut c_void) -> bool {
        if !self.timer.load(Ordering::Acquire).is_null() {
            return true;
        }
        // SAFETY: `callback` est une fonction statique du pilote et `context` reste
        // valide tant que le timer existe (contrat) ; `EX_TIMER_HIGH_RESOLUTION` est un
        // attribut valide et l'IRQL est `<= APC_LEVEL` (contrat).
        let timer = unsafe { ExAllocateTimer(Some(callback), context, EX_TIMER_HIGH_RESOLUTION) };
        if timer.is_null() {
            return false;
        }
        // `StartDevice` est sérialisé par le gestionnaire PnP, mais l'échange
        // conditionnel évite d'avoir à le supposer : le perdant supprime son timer.
        if let Err(existing) =
            self.timer
                .compare_exchange(ptr::null_mut(), timer, Ordering::AcqRel, Ordering::Acquire)
        {
            // SAFETY: `timer` vient d'être alloué ici, n'a jamais été armé et n'est
            // publié nulle part ; `Wait = TRUE` est autorisé à `<= APC_LEVEL` (contrat).
            unsafe { ExDeleteTimer(timer, BOOLEAN_TRUE, BOOLEAN_TRUE, ptr::null_mut()) };
            return !existing.is_null();
        }
        true
    }

    /// Arme le timer : première échéance dans `period_ms` millisecondes, puis toutes les
    /// `period_ms` millisecondes (`period_ms` ≥ 1, sinon 1). Réarmer un timer déjà armé
    /// réinitialise son échéance. Rend faux si le timer n'existe pas.
    ///
    /// IRQL : `<= DISPATCH_LEVEL` (utilisable sous spin lock).
    pub fn start(&self, period_ms: u32) -> bool {
        let timer = self.timer.load(Ordering::Acquire);
        if timer.is_null() {
            return false;
        }
        let period = i64::from(period_ms.max(1)).saturating_mul(HNS_PER_MS);
        // Échéance relative : valeur négative, en unités de 100 ns comme la période.
        let due = 0i64.saturating_sub(period);
        // SAFETY: `timer` est un `EX_TIMER` vivant (créé par `create`, supprimé
        // seulement par `delete` au déchargement) ; `Parameters` nul demande les
        // paramètres par défaut ; l'IRQL est `<= DISPATCH_LEVEL` (contrat).
        unsafe { ExSetTimer(timer, due, period, ptr::null_mut()) };
        true
    }

    /// Désarme le timer (`ExCancelTimer`). Un rappel déjà en cours sur un autre
    /// processeur peut encore s'achever : il prendra le spin lock du câble, que
    /// l'appelant détient ou vient de relâcher. Sans effet si le timer n'existe pas.
    ///
    /// IRQL : `<= DISPATCH_LEVEL` (utilisable sous spin lock ; n'attend jamais).
    pub fn stop(&self) {
        let timer = self.timer.load(Ordering::Acquire);
        if timer.is_null() {
            return;
        }
        // SAFETY: `timer` est un `EX_TIMER` vivant (voir `start`) ; `Parameters` nul.
        unsafe { ExCancelTimer(timer, ptr::null_mut()) };
    }

    /// Annule le timer, **attend** la fin du rappel en cours et libère l'objet
    /// (`ExDeleteTimer(…, TRUE, TRUE, NULL)`). Après le retour, le rappel ne s'exécute
    /// plus et son contexte peut être libéré. Sans effet si le timer n'existe pas.
    ///
    /// IRQL : **`PASSIVE_LEVEL`** (`<= APC_LEVEL`), hors de tout spin lock.
    pub fn delete(&self) {
        let timer = self.timer.swap(ptr::null_mut(), Ordering::AcqRel);
        if timer.is_null() {
            return;
        }
        // SAFETY: `timer` a été retiré de `self` par l'échange : personne ne l'armera
        // plus. `Cancel = TRUE` l'annule, `Wait = TRUE` attend le rappel en cours —
        // autorisé à `<= APC_LEVEL` (contrat de la méthode).
        unsafe { ExDeleteTimer(timer, BOOLEAN_TRUE, BOOLEAN_TRUE, ptr::null_mut()) };
    }
}

impl Default for ExTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ExTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExTimer")
            .field("timer", &self.timer.load(Ordering::Relaxed))
            .finish()
    }
}
