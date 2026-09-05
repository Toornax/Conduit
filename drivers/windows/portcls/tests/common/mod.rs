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

// ---------------------------------------------------------------------------------
// Faux `IPortWaveRTStream` : `AllocatePagesForMdl` rend un pointeur factice et compte
// les appels, `FreePagesFromMdl` compte, `GetPhysicalPagesCount` rend le nombre de pages
// de la dernière allocation ; les autres slots sont vides (→ `STATUS_NOT_IMPLEMENTED`
// par l'enveloppe). Partagé par `wavert.rs` et `stream.rs`.
// ---------------------------------------------------------------------------------

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use conduit_com::{ComObject, ComPtr};
use portcls::unknown;
use portcls_sys::{IPortWaveRTStreamVtbl, PHYSICAL_ADDRESS, PMDL, SIZE_T};

/// Valeur du pointeur factice de MDL rendu par le faux port (jamais déréférencé).
pub const FAUX_MDL: usize = 0x4D44_4C00;

pub struct FauxPortStream {
    pub allocs: AtomicU32,
    pub frees: AtomicU32,
    /// `TotalBytes` de la dernière allocation.
    pub last_total: AtomicU64,
    /// `HighAddress.QuadPart` de la dernière allocation.
    pub last_high: AtomicU64,
    /// Si vrai, `AllocatePagesForMdl` échoue (rend nul).
    pub fail: AtomicU32,
}

unsafe extern "C" fn faux_allocate_pages(
    this: This,
    high: PHYSICAL_ADDRESS,
    total: SIZE_T,
) -> PMDL {
    let me = unsafe { ComObject::<IPortWaveRTStreamVtbl, FauxPortStream>::inner(this) };
    me.allocs.fetch_add(1, Ordering::SeqCst);
    me.last_total.store(total, Ordering::SeqCst);
    me.last_high
        .store(unsafe { high.QuadPart } as u64, Ordering::SeqCst);
    if me.fail.load(Ordering::SeqCst) != 0 {
        return ptr::null_mut();
    }
    FAUX_MDL as PMDL
}

unsafe extern "C" fn faux_free_pages(this: This, mdl: PMDL) {
    let me = unsafe { ComObject::<IPortWaveRTStreamVtbl, FauxPortStream>::inner(this) };
    assert_eq!(mdl as usize, FAUX_MDL, "la MDL rendue est celle allouée");
    me.frees.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn faux_pages_count(this: This, mdl: PMDL) -> ULONG {
    let me = unsafe { ComObject::<IPortWaveRTStreamVtbl, FauxPortStream>::inner(this) };
    assert_eq!(mdl as usize, FAUX_MDL);
    (me.last_total.load(Ordering::SeqCst).div_ceil(4096)) as ULONG
}

pub static FAUX_PORT_STREAM_VTBL: IPortWaveRTStreamVtbl = IPortWaveRTStreamVtbl {
    QueryInterface: Some(unknown::query_interface::<IPortWaveRTStreamVtbl, FauxPortStream>),
    AddRef: Some(unknown::add_ref::<IPortWaveRTStreamVtbl, FauxPortStream>),
    Release: Some(unknown::release::<IPortWaveRTStreamVtbl, FauxPortStream>),
    AllocatePagesForMdl: Some(faux_allocate_pages),
    AllocateContiguousPagesForMdl: None,
    MapAllocatedPages: None,
    UnmapAllocatedPages: None,
    FreePagesFromMdl: Some(faux_free_pages),
    GetPhysicalPagesCount: Some(faux_pages_count),
    GetPhysicalPageAddress: None,
};

pub fn faux_port_stream() -> ComPtr<IPortWaveRTStreamVtbl, FauxPortStream> {
    ComObject::new(
        &FAUX_PORT_STREAM_VTBL,
        FauxPortStream {
            allocs: AtomicU32::new(0),
            frees: AtomicU32::new(0),
            last_total: AtomicU64::new(0),
            last_high: AtomicU64::new(0),
            fail: AtomicU32::new(0),
        },
    )
}
