//! Interfaces COM **reçues** de PortCls (`IPortWaveRT`, `IPortWaveRTStream`,
//! `IResourceList`, `IRegistryKey`…) : [`ComRef`] tient une référence comptée sur un
//! objet étranger, comme `_com_ptr_t` ou `windows::core::IUnknown`.

use core::fmt;
use core::ptr::NonNull;

use crate::status::RawPtr;
use crate::unknown::{unknown_of, ComVtable, IUnknownVtbl};

/// Contrat d'un type « interface COM » : la forme sous laquelle PortCls passe un objet.
///
/// # Safety
///
/// Une implémentation garantit que :
///
/// - `Self` est `#[repr(C)] { lp_vtbl: *const Self::Vtbl }` : un seul champ, le
///   pointeur de vtable, à l'offset 0 (forme des `IXxx` générés par bindgen dans
///   `portcls-sys` et de [`IUnknown`](crate::IUnknown)) ;
/// - un `*mut Self` non nul reçu de PortCls pointe un objet vivant dont la vtable a bien
///   la forme `Self::Vtbl`, et cet objet est utilisable depuis n'importe quel fil (tous
///   les objets PortCls le sont, à l'IRQL documenté de chaque méthode) : c'est ce qui
///   rend `ComRef<Self>` `Send + Sync`.
pub unsafe trait ComInterface: 'static {
    /// Vtable de l'interface (commence par [`IUnknownVtbl`]).
    type Vtbl: ComVtable;
}

/// Référence comptée sur une interface reçue : `AddRef` à la copie, `Release` à la
/// destruction. `Option<ComRef<I>>` a la taille d'un pointeur (`NonNull`).
pub struct ComRef<I: ComInterface> {
    ptr: NonNull<I>,
}

// SAFETY: contrat de `ComInterface` : l'objet pointé est utilisable depuis n'importe
// quel fil et son comptage de références est atomique (COM).
unsafe impl<I: ComInterface> Send for ComRef<I> {}
// SAFETY: voir `Send` ; `&ComRef` ne donne que des pointeurs et la vtable, et les
// méthodes de l'objet acceptent les appels concurrents.
unsafe impl<I: ComInterface> Sync for ComRef<I> {}

impl<I: ComInterface> ComRef<I> {
    /// Adopte une référence **déjà comptée** (paramètre de sortie de `PcNewPort`,
    /// `NewStream` d'un port, résultat d'un `QueryInterface`…).
    ///
    /// # Safety
    ///
    /// `raw` est non nul, pointe un objet vivant de type `I`, et l'appelant possède
    /// sur lui une référence qu'il cède ici.
    pub unsafe fn from_raw_owned(raw: *mut I) -> Self {
        // SAFETY: `raw` est non nul (contrat).
        let ptr = unsafe { NonNull::new_unchecked(raw) };
        Self { ptr }
    }

    /// Comme [`from_raw_owned`](Self::from_raw_owned), mais accepte un pointeur nul
    /// (renvoie `None`) : pour les sorties que PortCls laisse nulles en cas d'échec.
    ///
    /// # Safety
    ///
    /// Si `raw` n'est pas nul : mêmes conditions que [`from_raw_owned`](Self::from_raw_owned).
    pub unsafe fn try_from_raw_owned(raw: *mut I) -> Option<Self> {
        NonNull::new(raw).map(|ptr| Self { ptr })
    }

    /// Emprunte une référence détenue par l'appelant : fait `AddRef` (paramètres
    /// d'entrée de `Init`, comme `IResourceList` ou le port).
    ///
    /// # Safety
    ///
    /// `raw` est non nul et pointe un objet vivant de type `I` (l'appelant en détient
    /// une référence pendant l'appel).
    pub unsafe fn from_raw_add_ref(raw: *mut I) -> Self {
        // SAFETY: `raw` est non nul et vivant (contrat).
        let this = unsafe { Self::from_raw_owned(raw) };
        // SAFETY: l'objet est vivant (contrat) ; on prend la référence que `this`
        // représentera désormais.
        unsafe { (this.unknown().add_ref)(this.as_raw()) };
        this
    }

    /// Comme [`from_raw_add_ref`](Self::from_raw_add_ref), mais accepte un pointeur nul
    /// (renvoie `None`).
    ///
    /// # Safety
    ///
    /// Si `raw` n'est pas nul : mêmes conditions que [`from_raw_add_ref`](Self::from_raw_add_ref).
    pub unsafe fn try_from_raw_add_ref(raw: *mut I) -> Option<Self> {
        if raw.is_null() {
            return None;
        }
        // SAFETY: `raw` est non nul et vivant (contrat).
        Some(unsafe { Self::from_raw_add_ref(raw) })
    }

    /// Pointeur d'interface, **sans** transfert de référence.
    pub fn as_ptr(&self) -> *mut I {
        self.ptr.as_ptr()
    }

    /// Pointeur `this` opaque, pour appeler un slot de la vtable.
    pub fn as_raw(&self) -> RawPtr {
        self.ptr.as_ptr().cast()
    }

    /// Vtable de l'objet, lue par le pointeur `lp_vtbl`.
    pub fn vtbl(&self) -> &I::Vtbl {
        // SAFETY: par le contrat `ComInterface`, `I` commence par un `*const I::Vtbl`
        // et l'objet est vivant tant que `self` détient sa référence.
        let vtbl = unsafe { *self.ptr.as_ptr().cast::<*const I::Vtbl>() };
        // SAFETY: la vtable d'un objet vivant est valide et n'est jamais modifiée
        // (contrat `ComInterface`) ; la référence rendue est liée à `&self`.
        unsafe { &*vtbl }
    }

    /// Les trois slots `IUnknown` de la vtable.
    pub fn unknown(&self) -> &IUnknownVtbl {
        unknown_of(self.vtbl())
    }

    /// Cède la référence détenue à l'appelant : pas de `Release`, le pointeur devra en
    /// recevoir un (ou revenir par [`from_raw_owned`](Self::from_raw_owned)).
    pub fn into_raw(self) -> *mut I {
        let raw = self.as_ptr();
        core::mem::forget(self);
        raw
    }
}

impl<I: ComInterface> Clone for ComRef<I> {
    fn clone(&self) -> Self {
        // SAFETY: `self` détient une référence, l'objet est vivant.
        unsafe { (self.unknown().add_ref)(self.as_raw()) };
        Self { ptr: self.ptr }
    }
}

impl<I: ComInterface> Drop for ComRef<I> {
    fn drop(&mut self) {
        // SAFETY: `self` détient une référence et la cède ; `ptr` n'est plus utilisé.
        unsafe { (self.unknown().release)(self.as_raw()) };
    }
}

impl<I: ComInterface> fmt::Debug for ComRef<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComRef").field("ptr", &self.ptr).finish()
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::IUnknown;
    use core::mem::size_of;

    #[test]
    fn option_comref_a_la_taille_d_un_pointeur() {
        assert_eq!(size_of::<ComRef<IUnknown>>(), size_of::<usize>());
        assert_eq!(size_of::<Option<ComRef<IUnknown>>>(), size_of::<usize>());
    }

    #[test]
    fn try_from_nul_donne_none() {
        // SAFETY: pointeur nul, aucun objet n'est touché.
        let owned = unsafe { ComRef::<IUnknown>::try_from_raw_owned(core::ptr::null_mut()) };
        assert!(owned.is_none());
        // SAFETY: idem.
        let borrowed = unsafe { ComRef::<IUnknown>::try_from_raw_add_ref(core::ptr::null_mut()) };
        assert!(borrowed.is_none());
    }
}
