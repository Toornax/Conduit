//! Thunks `IUnknown` génériques au **type exact** des slots générés par bindgen.
//!
//! `conduit_com::ComObject` fournit `query_interface`/`add_ref`/`release` en
//! `unsafe extern "system" fn(RawPtr, *const Guid, *mut RawPtr) -> NtStatus` ; les slots
//! de `portcls-sys` sont des `Option<unsafe extern "C" fn(*mut c_void, *const IID,
//! *mut PVOID) -> NTSTATUS>`. Même ABI (x64), même disposition, mais deux **types** Rust
//! distincts (`"system"` ≠ `"C"` pour le vérificateur de types, `Guid` ≠ `IID`) : une
//! fonction de `conduit-com` ne se place donc pas telle quelle dans une vtable générée.
//! Les trois thunks ci-dessous font l'adaptation (un `cast` de pointeur, inliné), sans
//! transmutation de pointeur de fonction.
//!
//! Ils servent à toutes les vtables du crate ([`power`](crate::power),
//! [`topology`](crate::topology), puis M1a-05) et aux faux objets des tests.

use core::ffi::c_void;

use conduit_com::{ComObject, ComVtable, Guid};
use portcls_sys::{IID, NTSTATUS, PVOID, ULONG};

/// `IUnknown::QueryInterface` d'un `ComObject<V, T>`, au type des slots générés.
///
/// IRQL : `<= DISPATCH_LEVEL`.
///
/// # Safety
///
/// Mêmes préconditions que [`ComObject::query_interface`] : `this` est un
/// `*mut ComObject<V, T>` vivant ; `iid`, s'il n'est pas nul, pointe un `IID` lisible ;
/// `out`, s'il n'est pas nul, pointe un emplacement de pointeur inscriptible.
pub unsafe extern "C" fn query_interface<V: ComVtable, T: Send + Sync>(
    this: *mut c_void,
    iid: *const IID,
    out: *mut PVOID,
) -> NTSTATUS {
    // SAFETY: préconditions transmises telles quelles ; `IID` et `Guid` ont la même
    // disposition (16 octets, `repr(C)`, vérifié par `portcls-sys/tests/com.rs`), le
    // `cast` ne change pas la validité du pointeur.
    unsafe { ComObject::<V, T>::query_interface(this, iid.cast::<Guid>(), out) }
}

/// `IUnknown::AddRef` d'un `ComObject<V, T>`, au type des slots générés.
///
/// IRQL : `<= DISPATCH_LEVEL`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant.
pub unsafe extern "C" fn add_ref<V: ComVtable, T: Send + Sync>(this: *mut c_void) -> ULONG {
    // SAFETY: précondition transmise telle quelle.
    unsafe { ComObject::<V, T>::add_ref(this) }
}

/// `IUnknown::Release` d'un `ComObject<V, T>`, au type des slots générés.
///
/// IRQL : `<= DISPATCH_LEVEL` (si le `Drop` de `T` le supporte).
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant et l'appelant cède la référence qu'il
/// détient : il n'utilise plus `this` après l'appel.
pub unsafe extern "C" fn release<V: ComVtable, T: Send + Sync>(this: *mut c_void) -> ULONG {
    // SAFETY: précondition transmise telle quelle.
    unsafe { ComObject::<V, T>::release(this) }
}
