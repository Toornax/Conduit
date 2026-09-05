//! Le faux PortCls des tests : ne connaît que `this`, lit le pointeur de vtable à
//! l'offset 0 et appelle les slots, comme le noyau. Rien ici ne passe par les
//! `ComPtr`/`ComRef` de l'objet testé.

#![allow(dead_code)]

use std::ptr;

use conduit_com::{Guid, NtStatus, RawPtr};
use portcls_sys::{IID, IUnknownVtbl, NTSTATUS, PVOID, ULONG};

/// Pointeur `this` opaque, tel que PortCls le manipule.
pub type This = *mut core::ffi::c_void;

/// `(*(IXxxVtbl**)this)` : la vtable de n'importe quel objet COM.
///
/// # Safety
///
/// `this` est un objet COM vivant dont la vtable a la forme `V`.
pub unsafe fn vtbl_de<V>(this: This) -> &'static V {
    assert!(!this.is_null());
    // SAFETY: contrat de la fonction ; les vtables sont des constantes promues ou des
    // `static`, donc `'static`.
    unsafe { &*(*this.cast::<*const V>()) }
}

/// Les trois premiers slots, lus au format généré (`Option<extern "C" fn>`).
fn unknown_de(this: This) -> &'static IUnknownVtbl {
    // SAFETY: toute vtable COM commence par `IUnknownVtbl` (24 octets).
    unsafe { vtbl_de::<IUnknownVtbl>(this) }
}

/// `QueryInterface` par la vtable : `(status, *out)`. `out` est empoisonné avant l'appel
/// pour vérifier que l'objet l'écrit toujours.
pub fn query_interface(this: This, iid: &Guid) -> (NtStatus, RawPtr) {
    let slot = unknown_de(this)
        .QueryInterface
        .expect("slot QueryInterface");
    let mut out: PVOID = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    let iid: *const Guid = iid;
    // SAFETY: `this` est vivant ; `iid` pointe un `Guid` (même disposition que `IID`) ;
    // `out` est une variable locale. `IUnknownVtbl` généré nomme `This: *mut IUnknown`,
    // d'où le `cast`.
    let status: NTSTATUS = unsafe { slot(this.cast(), iid.cast::<IID>(), &mut out) };
    (status, out)
}

/// `AddRef` par la vtable : nouveau compte.
pub fn add_ref(this: This) -> ULONG {
    let slot = unknown_de(this).AddRef.expect("slot AddRef");
    // SAFETY: `this` est vivant.
    unsafe { slot(this.cast()) }
}

/// `Release` par la vtable : nouveau compte (0 = objet libéré, `this` mort).
pub fn release(this: This) -> ULONG {
    let slot = unknown_de(this).Release.expect("slot Release");
    // SAFETY: `this` est vivant et l'appelant cède sa référence.
    unsafe { slot(this.cast()) }
}

/// Compte de références courant d'un objet vivant, observé par `AddRef` puis `Release`.
pub fn refcount(this: This) -> ULONG {
    add_ref(this);
    release(this)
}
