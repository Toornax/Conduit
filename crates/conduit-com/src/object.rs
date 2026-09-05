//! Objets COM **implémentés** par le pilote : [`ComObject`] (disposition mémoire et
//! `IUnknown` génériques) et [`ComPtr`] (possession Rust d'un tel objet).

use alloc::boxed::Box;
use core::alloc::Layout;
use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;
use core::ptr::{self, NonNull};
use core::sync::atomic::{fence, AtomicU32, Ordering};

use crate::guid::Guid;
use crate::status::{NtStatus, RawPtr, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use crate::unknown::ComVtable;

/// Objet COM : pointeur de vtable, compte de références, état Rust.
///
/// `repr(C)` garantit `vtbl` à l'offset 0 : PortCls lit `*(void**)this` pour trouver
/// la vtable, et `this` est exactement un `*mut ComObject<V, T>`. Un objet Rust par
/// interface, pas d'héritage multiple (driver-design.md §3).
///
/// Les trois méthodes `IUnknown` génériques ([`query_interface`](Self::query_interface),
/// [`add_ref`](Self::add_ref), [`release`](Self::release)) sont des fonctions
/// `extern "system"` à placer telles quelles dans la vtable concrète `V` ; les méthodes
/// métier récupèrent l'état par [`inner`](Self::inner).
///
/// Bornes : `V: ComVtable` (contrat de vtable), `T: Send + Sync` parce que PortCls
/// appelle depuis n'importe quel fil et que `inner` ne donne jamais qu'un `&T` : tout
/// état mutable de `T` passe par des atomiques ou de la mutabilité intérieure sûre à
/// `DISPATCH_LEVEL` (spin lock).
#[repr(C)]
pub struct ComObject<V: ComVtable, T: Send + Sync> {
    vtbl: &'static V,
    refcount: AtomicU32,
    inner: T,
}

// Vérifié à la compilation, indépendamment des tests : l'offset 0 est le contrat ABI.
const _: () = assert!(core::mem::offset_of!(ComObject<crate::IUnknownVtbl, ()>, vtbl) == 0);

impl<V: ComVtable, T: Send + Sync> ComObject<V, T> {
    /// Alloue l'objet (`Box`, allocateur global) avec un compte de références de 1 et
    /// en rend la possession sous forme de [`ComPtr`].
    ///
    /// En cas d'échec d'allocation, l'allocateur global décide (`handle_alloc_error`,
    /// panique en `no_std`) : dans le pilote, préférer [`try_new`](Self::try_new).
    // Un `ComObject` ne se manipule jamais par valeur : `new` rend la poignée comptée,
    // comme `Arc::new`.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(vtbl: &'static V, inner: T) -> ComPtr<V, T> {
        let boxed = Box::new(Self {
            vtbl,
            refcount: AtomicU32::new(1),
            inner,
        });
        // SAFETY: `Box::into_raw` ne renvoie jamais nul.
        let ptr = unsafe { NonNull::new_unchecked(Box::into_raw(boxed)) };
        ComPtr {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Comme [`new`](Self::new), mais renvoie `None` (et abandonne `inner`) si
    /// l'allocateur global échoue, pour répondre `STATUS_INSUFFICIENT_RESOURCES` plutôt
    /// que de paniquer. La mémoire vient du même allocateur et de la même disposition
    /// que `Box`, donc [`release`](Self::release) la libère avec `Box::from_raw`.
    pub fn try_new(vtbl: &'static V, inner: T) -> Option<ComPtr<V, T>> {
        // `Self` contient un pointeur : jamais de taille nulle, `alloc` est permis.
        let layout = Layout::new::<Self>();
        // SAFETY: `layout` a une taille non nulle.
        let raw = unsafe { alloc::alloc::alloc(layout) };
        let ptr = NonNull::new(raw.cast::<Self>())?;
        // SAFETY: `ptr` vient d'être alloué pour `Layout::new::<Self>()`, aligné, non
        // partagé, et n'est lu par personne avant cette écriture.
        unsafe {
            ptr.as_ptr().write(Self {
                vtbl,
                refcount: AtomicU32::new(1),
                inner,
            });
        }
        Some(ComPtr {
            ptr,
            _marker: PhantomData,
        })
    }

    /// `IUnknown::QueryInterface` générique.
    ///
    /// `out` nul → `STATUS_INVALID_PARAMETER`. `iid` dans [`V::IIDS`](ComVtable::IIDS)
    /// → une référence de plus, `*out = this`, `STATUS_SUCCESS`. Sinon `*out = null`,
    /// `STATUS_INVALID_PARAMETER` (comportement SYSVAD). Ni allocation ni verrou.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    ///
    /// # Safety
    ///
    /// `this` est un `*mut ComObject<V, T>` vivant (une référence au moins est
    /// détenue par l'appelant) ; `iid`, s'il n'est pas nul, pointe un `Guid` lisible ;
    /// `out`, s'il n'est pas nul, pointe un emplacement de pointeur inscriptible.
    pub unsafe extern "system" fn query_interface(
        this: RawPtr,
        iid: *const Guid,
        out: *mut RawPtr,
    ) -> NtStatus {
        if out.is_null() {
            return STATUS_INVALID_PARAMETER;
        }
        if iid.is_null() {
            // SAFETY: `out` est non nul et inscriptible (contrat de la fonction).
            unsafe { *out = ptr::null_mut() };
            return STATUS_INVALID_PARAMETER;
        }
        // SAFETY: `iid` est non nul et pointe un `Guid` lisible (contrat).
        let iid = unsafe { *iid };
        if V::IIDS.contains(&iid) {
            // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
            unsafe { Self::add_ref(this) };
            // SAFETY: `out` est non nul et inscriptible (contrat).
            unsafe { *out = this };
            STATUS_SUCCESS
        } else {
            // SAFETY: `out` est non nul et inscriptible (contrat).
            unsafe { *out = ptr::null_mut() };
            STATUS_INVALID_PARAMETER
        }
    }

    /// `IUnknown::AddRef` générique : incrémente et renvoie le nouveau compte.
    ///
    /// `Relaxed` suffit (schéma `Arc`) : l'appelant détient déjà une référence, donc
    /// aucune libération ne peut être concurrente de cet incrément. Le compteur est un
    /// `u32` qui *boucle* à 2³² ; l'atteindre demanderait quatre milliards de références
    /// simultanées (32 Gio de pointeurs), impossible en pratique ; la valeur renvoyée est
    /// saturée pour n'avoir aucun débordement arithmétique dans le code Rust.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    ///
    /// # Safety
    ///
    /// `this` est un `*mut ComObject<V, T>` vivant.
    pub unsafe extern "system" fn add_ref(this: RawPtr) -> u32 {
        // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat) ; référence partagée
        // seulement, le compteur est atomique.
        let obj = unsafe { &*this.cast::<Self>() };
        obj.refcount
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    /// `IUnknown::Release` générique : décrémente, libère l'objet si le compte tombe à
    /// zéro, renvoie le nouveau compte.
    ///
    /// Ordres mémoire (schéma `Arc`) : la décrémentation est `Release` pour que tous
    /// les accès à l'objet faits par ce fil précèdent la libération ; si l'ancien compte
    /// valait 1, une barrière `Acquire` rend visibles à ce fil les accès des autres
    /// détenteurs avant que `drop` ne s'exécute. C'est la seule opération de ce module
    /// qui libère de la mémoire (`Box::from_raw`) ; le `Drop` de `T` ne doit pas appeler
    /// PortCls.
    ///
    /// Un `release` de trop (compte déjà à zéro) est un usage après libération : rien
    /// n'est fait pour le détecter, la valeur renvoyée est saturée à 0.
    ///
    /// IRQL : `<= DISPATCH_LEVEL` (à condition que le `Drop` de `T` le supporte).
    ///
    /// # Safety
    ///
    /// `this` est un `*mut ComObject<V, T>` vivant, alloué par [`new`](Self::new) ou
    /// [`try_new`](Self::try_new), et l'appelant cède la référence qu'il détient : il
    /// n'utilise plus `this` après l'appel.
    pub unsafe extern "system" fn release(this: RawPtr) -> u32 {
        let obj_ptr = this.cast::<Self>();
        // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat) ; la référence
        // partagée n'est plus utilisée après `fetch_sub`.
        let old = unsafe { &*obj_ptr }
            .refcount
            .fetch_sub(1, Ordering::Release);
        if old == 1 {
            fence(Ordering::Acquire);
            // SAFETY: le compte vient de tomber à zéro : plus aucune référence n'existe,
            // et `obj_ptr` vient de `Box::into_raw` (ou d'`alloc` à la même
            // disposition) : reconstruire le `Box` et le libérer est l'unique
            // libération de cet objet.
            drop(unsafe { Box::from_raw(obj_ptr) });
            0
        } else {
            old.saturating_sub(1)
        }
    }

    /// État Rust de l'objet, pour les rappels des vtables concrètes.
    ///
    /// Ne renvoie jamais `&mut T` : PortCls peut appeler plusieurs méthodes en parallèle
    /// sur le même objet. La durée de vie `'a` est laissée à l'appelant : elle ne doit
    /// pas dépasser l'appel COM en cours (PortCls détient une référence pendant l'appel).
    ///
    /// # Safety
    ///
    /// `this` est un `*mut ComObject<V, T>` vivant pendant toute la durée `'a`.
    pub unsafe fn inner<'a>(this: RawPtr) -> &'a T {
        // SAFETY: `this` est un `ComObject<V, T>` vivant pendant `'a` (contrat).
        unsafe { &(*this.cast::<Self>()).inner }
    }

    /// Compte de références courant (instantané, `Relaxed`) : tests et diagnostics.
    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Relaxed)
    }

    /// Vtable de l'objet.
    pub fn vtbl(&self) -> &'static V {
        self.vtbl
    }

    /// État Rust de l'objet.
    pub fn get(&self) -> &T {
        &self.inner
    }
}

impl<V: ComVtable, T: Send + Sync> fmt::Debug for ComObject<V, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vtbl: *const V = self.vtbl;
        f.debug_struct("ComObject")
            .field("vtbl", &vtbl)
            .field("refcount", &self.refcount())
            .finish_non_exhaustive()
    }
}

/// Possession Rust d'une référence sur un [`ComObject`] : `Clone` fait `AddRef`,
/// `Drop` fait `Release`, `Deref` donne l'état `T`.
///
/// C'est le côté Rust du comptage de références ; [`into_raw`](Self::into_raw) et
/// [`from_raw`](Self::from_raw) font passer une référence vers ou depuis PortCls (par
/// exemple le `*out` de `NewStream`, ou `PcNewPort`). `Send + Sync` comme `Arc<T>`,
/// dès que `T` l'est.
pub struct ComPtr<V: ComVtable, T: Send + Sync> {
    ptr: NonNull<ComObject<V, T>>,
    /// Possession logique d'un `ComObject<V, T>` (variance, auto-traits, drop check).
    _marker: PhantomData<ComObject<V, T>>,
}

// SAFETY: un `ComPtr` est une référence comptée sur un objet dont l'état `T` est
// `Send + Sync` (borne du type) et dont le compteur est atomique : même contrat
// qu'`Arc<T>`.
unsafe impl<V: ComVtable, T: Send + Sync> Send for ComPtr<V, T> {}
// SAFETY: voir `Send` ; `&ComPtr` ne donne que `&T`.
unsafe impl<V: ComVtable, T: Send + Sync> Sync for ComPtr<V, T> {}

impl<V: ComVtable, T: Send + Sync> ComPtr<V, T> {
    /// L'objet complet (vtable, compte, état).
    pub fn object(&self) -> &ComObject<V, T> {
        // SAFETY: `ptr` est vivant tant que ce `ComPtr` détient sa référence.
        unsafe { self.ptr.as_ref() }
    }

    /// Pointeur brut (`this`) **sans** transfert de référence : à passer à PortCls pour
    /// la durée d'un appel seulement.
    pub fn as_raw(&self) -> RawPtr {
        self.ptr.as_ptr().cast()
    }

    /// Transfère la référence détenue à l'appelant (PortCls) : pas de `Release` au
    /// passage, et le pointeur renvoyé devra recevoir un `Release` ou revenir par
    /// [`from_raw`](Self::from_raw).
    pub fn into_raw(self) -> RawPtr {
        let raw = self.as_raw();
        core::mem::forget(self);
        raw
    }

    /// Reprend la possession d'une référence transférée par [`into_raw`](Self::into_raw)
    /// (ou d'un `AddRef` fait par PortCls sur l'objet).
    ///
    /// # Safety
    ///
    /// `raw` est non nul, pointe un `ComObject<V, T>` vivant créé par ce module, et
    /// l'appelant possède une référence sur lui qu'il cède ici.
    pub unsafe fn from_raw(raw: RawPtr) -> Self {
        // SAFETY: `raw` est non nul (contrat).
        let ptr = unsafe { NonNull::new_unchecked(raw.cast::<ComObject<V, T>>()) };
        Self {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Compte de références courant (tests et diagnostics).
    pub fn refcount(&self) -> u32 {
        self.object().refcount()
    }
}

impl<V: ComVtable, T: Send + Sync> Deref for ComPtr<V, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.object().inner
    }
}

impl<V: ComVtable, T: Send + Sync> Clone for ComPtr<V, T> {
    fn clone(&self) -> Self {
        // SAFETY: `self` détient une référence, l'objet est vivant.
        unsafe { ComObject::<V, T>::add_ref(self.as_raw()) };
        Self {
            ptr: self.ptr,
            _marker: PhantomData,
        }
    }
}

impl<V: ComVtable, T: Send + Sync> Drop for ComPtr<V, T> {
    fn drop(&mut self) {
        // SAFETY: `self` détient une référence et la cède ; `ptr` n'est plus utilisé.
        unsafe { ComObject::<V, T>::release(self.as_raw()) };
    }
}

impl<V: ComVtable, T: Send + Sync> fmt::Debug for ComPtr<V, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComPtr")
            .field("ptr", &self.ptr)
            .field("refcount", &self.refcount())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::IUnknownVtbl;
    use core::mem::{offset_of, size_of};

    #[test]
    fn vtbl_a_l_offset_zero() {
        assert_eq!(offset_of!(ComObject<IUnknownVtbl, u64>, vtbl), 0);
        assert_eq!(offset_of!(ComObject<IUnknownVtbl, ()>, vtbl), 0);
        assert_eq!(
            offset_of!(ComObject<IUnknownVtbl, u64>, refcount),
            size_of::<usize>()
        );
        assert_eq!(size_of::<ComPtr<IUnknownVtbl, u64>>(), size_of::<usize>());
        assert_eq!(
            size_of::<Option<ComPtr<IUnknownVtbl, u64>>>(),
            size_of::<usize>()
        );
    }
}
