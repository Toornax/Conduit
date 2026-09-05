//! `IUnknown` de PortCls (`punknown.h`) : vtable, interface, et le contrat que toute
//! vtable COM du pilote doit respecter ([`ComVtable`]).

use core::mem::size_of;

use crate::comref::ComInterface;
use crate::guid::{Guid, IID_IUNKNOWN};
use crate::status::{NtStatus, RawPtr};

/// Vtable de `IUnknown` : les trois premiers slots de toute interface PortCls.
///
/// Même disposition que `IUnknownVtbl` généré par bindgen dans `portcls-sys` (trois
/// pointeurs de fonction, 24 octets sur x64). Les pointeurs sont non optionnels : un
/// slot vide n'a pas de sens pour `IUnknown`. Convention `extern "system"` (`__stdcall`
/// sur x86, identique à `extern "C"` sur x64).
///
/// - `query_interface(this, iid, out)` : `STATUS_SUCCESS` et `*out = this` avec une
///   référence de plus si l'objet répond à `iid` ; sinon `*out = null` et
///   `STATUS_INVALID_PARAMETER` (`out` nul : `STATUS_INVALID_PARAMETER`).
/// - `add_ref(this)` / `release(this)` : renvoient le nouveau compte de références.
///
/// Les trois sont appelables depuis n'importe quel fil, à `DISPATCH_LEVEL` : elles
/// n'allouent pas et ne verrouillent pas (`release` libère l'objet à la dernière
/// référence, ce qui reste permis à `DISPATCH_LEVEL` pour du pool non paginé).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IUnknownVtbl {
    /// `NTSTATUS QueryInterface(REFIID, PVOID*)`.
    pub query_interface:
        unsafe extern "system" fn(this: RawPtr, iid: *const Guid, out: *mut RawPtr) -> NtStatus,
    /// `ULONG AddRef()`.
    pub add_ref: unsafe extern "system" fn(this: RawPtr) -> u32,
    /// `ULONG Release()`.
    pub release: unsafe extern "system" fn(this: RawPtr) -> u32,
}

/// Interface `IUnknown` telle que PortCls la manipule : un pointeur de vtable.
///
/// Toute interface COM a cette forme (`struct IXxx { lpVtbl: *const IXxxVtbl }`) ; un
/// `*mut IXxx` se lit comme un `*mut IUnknown` puisque la vtable commence par
/// [`IUnknownVtbl`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IUnknown {
    /// Pointeur de vtable (`lpVtbl`), à l'offset 0.
    pub lp_vtbl: *const IUnknownVtbl,
}

/// Contrat des vtables COM du pilote.
///
/// # Safety
///
/// Une implémentation garantit que :
///
/// - `Self` est `#[repr(C)]` et **commence par un [`IUnknownVtbl`]** (en pratique un
///   premier champ de ce type, ou une vtable de base qui elle-même commence ainsi) :
///   c'est ce qui permet à [`unknown_of`] de lire les trois premiers slots et à PortCls
///   d'appeler `QueryInterface`/`AddRef`/`Release` sur n'importe quel objet ;
/// - [`IIDS`](Self::IIDS) liste **tous** les IID auxquels un objet porteur de cette
///   vtable répond : [`IID_IUNKNOWN`], l'interface elle-même et ses bases (par exemple
///   `IID_IUnknown`, `IID_IMiniport`, `IID_IMiniportWaveRT` pour `IMiniportWaveRTVtbl`),
///   et rien d'autre. Un objet ne répond à une base que si sa vtable est un préfixe de
///   celle de l'interface dérivée (`QueryInterface` par préfixe de vtable,
///   driver-design.md §3).
pub unsafe trait ComVtable: 'static + Sized {
    /// IID auxquels l'objet répond, `IID_IUNKNOWN` compris.
    const IIDS: &'static [Guid];
}

/// Vue `IUnknown` d'une vtable : ses trois premiers slots.
///
/// La transmutation de pointeur est justifiée par le contrat de [`ComVtable`] (`V` est
/// `repr(C)` et commence par un `IUnknownVtbl`) ; un `const` la vérifie en taille à la
/// monomorphisation.
pub fn unknown_of<V: ComVtable>(v: &V) -> &IUnknownVtbl {
    const {
        assert!(
            size_of::<V>() >= size_of::<IUnknownVtbl>(),
            "ComVtable : la vtable est plus petite qu'IUnknownVtbl"
        );
    }
    let p: *const V = v;
    // SAFETY: par le contrat `ComVtable`, `V` est `repr(C)` et commence par un
    // `IUnknownVtbl` initialisé ; `p` vient d'une référence valide et vit aussi
    // longtemps qu'elle, la lecture reste dans les bornes de `V` (assertion ci-dessus).
    unsafe { &*p.cast::<IUnknownVtbl>() }
}

// SAFETY: `IUnknownVtbl` est `repr(C)` et « commence » par lui-même ; un objet
// `IUnknown` ne répond qu'à `IID_IUnknown`.
unsafe impl ComVtable for IUnknownVtbl {
    const IIDS: &'static [Guid] = &[IID_IUNKNOWN];
}

// SAFETY: `IUnknown` est `repr(C) { lp_vtbl: *const IUnknownVtbl }` ; tout objet COM
// de PortCls est utilisable depuis n'importe quel fil.
unsafe impl ComInterface for IUnknown {
    type Vtbl = IUnknownVtbl;
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::status::STATUS_SUCCESS;
    use core::mem::offset_of;

    #[test]
    fn disposition_de_la_vtable() {
        let slot = size_of::<usize>();
        assert_eq!(size_of::<IUnknownVtbl>(), 3 * slot);
        assert_eq!(offset_of!(IUnknownVtbl, query_interface), 0);
        assert_eq!(offset_of!(IUnknownVtbl, add_ref), slot);
        assert_eq!(offset_of!(IUnknownVtbl, release), 2 * slot);
        assert_eq!(size_of::<IUnknown>(), slot);
        assert_eq!(offset_of!(IUnknown, lp_vtbl), 0);
    }

    unsafe extern "system" fn qi(_: RawPtr, _: *const Guid, _: *mut RawPtr) -> NtStatus {
        STATUS_SUCCESS
    }
    unsafe extern "system" fn un(_: RawPtr) -> u32 {
        1
    }
    unsafe extern "system" fn deux(_: RawPtr) -> u32 {
        2
    }

    #[test]
    fn unknown_of_lit_les_trois_premiers_slots() {
        let vt = IUnknownVtbl {
            query_interface: qi,
            add_ref: un,
            release: deux,
        };
        let u = unknown_of(&vt);
        // Les adresses de fonctions ne sont pas comparables (dédoublonnage, Miri) :
        // on appelle les slots lus et on vérifie leurs résultats.
        let this = core::ptr::null_mut();
        let mut out = core::ptr::null_mut();
        // SAFETY: ces trois fonctions de test ignorent leurs arguments.
        let status = unsafe { (u.query_interface)(this, core::ptr::null(), &mut out) };
        assert_eq!(status, STATUS_SUCCESS);
        // SAFETY: idem.
        assert_eq!(unsafe { (u.add_ref)(this) }, 1);
        // SAFETY: idem.
        assert_eq!(unsafe { (u.release)(this) }, 2);
        assert_eq!(IUnknownVtbl::IIDS, &[IID_IUNKNOWN]);
    }
}
