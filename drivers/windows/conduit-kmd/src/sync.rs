//! Spin lock noyau (`KSPIN_LOCK`) avec garde RAII : [`SpinLock`] protège l'état d'un
//! flux ou d'un câble entre les méthodes PortCls (`PASSIVE_LEVEL`), `GetPosition`
//! (`<= DISPATCH_LEVEL`) et les DPC (`DISPATCH_LEVEL`).
//!
//! # IRQL
//!
//! [`SpinLock::lock`] appelle `KeAcquireSpinLockRaiseToDpc` (élève à `DISPATCH_LEVEL`,
//! ou reste à `DISPATCH_LEVEL` depuis une DPC) et la garde restaure l'IRQL d'origine par
//! `KeReleaseSpinLock` à sa destruction. Appelable à IRQL `<= DISPATCH_LEVEL`
//! seulement ; **pendant la garde, l'IRQL est `DISPATCH_LEVEL`** : ni allocation
//! paginée, ni attente, ni appel à une fonction `PASSIVE_LEVEL` (`KeFlushQueuedDpcs`,
//! `IPortWaveRTStream::*`, `DbgPrint` à éviter). Les sections critiques doivent rester
//! courtes : calcul de position, copie de quelques champs.
//!
//! # Initialisation
//!
//! `KeInitializeSpinLock` se réduit sur x64 à `*SpinLock = 0` (`KzInitializeSpinLock`,
//! `wdm.h`) ; [`SpinLock::new`] est donc `const` et écrit 0, ce qui permet un
//! `SpinLock` dans une `static` (le câble de [`crate::cable`]) sans appel au noyau.
//! Un `KSPIN_LOCK` n'a pas d'adresse figée : la structure peut être déplacée tant
//! qu'elle n'est pas verrouillée.

use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

use wdk_sys::ntddk::{KeAcquireSpinLockRaiseToDpc, KeReleaseSpinLock};
use wdk_sys::{KIRQL, KSPIN_LOCK};

/// Donnée `T` protégée par un `KSPIN_LOCK`.
pub struct SpinLock<T> {
    lock: UnsafeCell<KSPIN_LOCK>,
    data: UnsafeCell<T>,
}

// SAFETY: l'accès à `data` est exclusif sous le spin lock (`lock` est le seul chemin et
// rend une garde unique), donc partager `&SpinLock<T>` entre fils revient à envoyer `T`
// d'un fil à l'autre : `T: Send` suffit, comme pour `std::sync::Mutex`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
// SAFETY: un `SpinLock<T>` possède son `T` ; le déplacer déplace `T`.
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Verrou libre (`KSPIN_LOCK = 0`, ce que fait `KeInitializeSpinLock`) autour de
    /// `data`.
    pub const fn new(data: T) -> Self {
        Self {
            lock: UnsafeCell::new(0),
            data: UnsafeCell::new(data),
        }
    }

    /// Prend le verrou (élève l'IRQL à `DISPATCH_LEVEL`) et rend la garde qui donne
    /// accès à `T` et libère le verrou à sa destruction.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`. Ne jamais reprendre un verrou déjà détenu par le fil
    /// courant (interblocage), ni détenir deux verrous dans un ordre variable
    /// (ordre du pilote : câble puis flux, [`crate::cable`]).
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        // SAFETY: `self.lock` est un `KSPIN_LOCK` valide (initialisé à 0, jamais déplacé
        // pendant qu'il est détenu puisque `&self` le maintient en place) ; l'IRQL
        // courant est `<= DISPATCH_LEVEL` (contrat de la méthode).
        let irql = unsafe { KeAcquireSpinLockRaiseToDpc(self.lock.get()) };
        SpinLockGuard {
            lock: self,
            irql,
            _not_send: PhantomData,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for SpinLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Pas de prise de verrou pour un affichage de débogage : seule l'adresse.
        f.debug_struct("SpinLock")
            .field("lock", &self.lock.get())
            .finish_non_exhaustive()
    }
}

/// Garde d'un [`SpinLock`] : accès exclusif à `T`, IRQL `DISPATCH_LEVEL`, libération
/// et restauration de l'IRQL au `Drop`.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    irql: KIRQL,
    /// La garde est liée au fil qui a élevé l'IRQL : ni `Send` ni `Sync`.
    _not_send: PhantomData<*mut ()>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: le verrou est détenu par cette garde : accès exclusif à `data`.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: le verrou est détenu par cette garde (`&mut self`) : accès exclusif.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: le verrou a été pris par `lock` avec `irql` comme IRQL d'origine, et
        // il n'est libéré qu'ici, une fois.
        unsafe { KeReleaseSpinLock(self.lock.lock.get(), self.irql) };
    }
}

impl<T: fmt::Debug> fmt::Debug for SpinLockGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpinLockGuard")
            .field("irql", &self.irql)
            .field("data", &**self)
            .finish()
    }
}
