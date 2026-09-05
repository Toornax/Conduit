//! Le modèle objet vu par un faux PortCls : une interface `IPing` implémentée avec
//! `ComObject`, appelée **uniquement à travers les pointeurs de vtable**, comme le
//! ferait le noyau. Ce fichier n'utilise que l'API publique du crate : c'est aussi la
//! preuve que `drivers/windows/portcls` peut se construire dessus.
//!
//! Compatible Miri : aucune référence `&mut` sur l'état partagé (atomiques
//! seulement), aucune conversion entier → pointeur, aucune lecture hors bornes.

#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects
)]

use std::mem::{offset_of, size_of};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use conduit_com::{
    nt_success, ComInterface, ComObject, ComPtr, ComRef, ComVtable, Guid, IUnknownVtbl, NtStatus,
    RawPtr, IID_IUNKNOWN, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
};

// ---------------------------------------------------------------------------------
// Fausse interface `IPing` : IUnknown + `ULONG Ping(ULONG)`.
// ---------------------------------------------------------------------------------

/// IID de `IPing` (valeur arbitraire).
const IID_IPING: Guid = Guid::from_u128(0x7A1E5F10_2C4B_4D8E_9F3A_1B2C3D4E5F60);
/// IID que personne n'implémente.
const IID_INCONNU: Guid = Guid::from_u128(0xDEADBEEF_0000_4000_8000_000000000001);

#[repr(C)]
struct IPingVtbl {
    unknown: IUnknownVtbl,
    ping: unsafe extern "system" fn(this: RawPtr, n: u32) -> u32,
}

// SAFETY: `repr(C)`, premier champ `IUnknownVtbl` ; un objet `IPing` répond à
// `IID_IUnknown` et `IID_IPing`.
unsafe impl ComVtable for IPingVtbl {
    const IIDS: &'static [Guid] = &[IID_IUNKNOWN, IID_IPING];
}

/// Forme « interface reçue » de `IPing`, comme un `IXxx` de bindgen.
#[repr(C)]
struct IPing {
    lp_vtbl: *const IPingVtbl,
}

// SAFETY: `repr(C) { lp_vtbl }` ; les objets `Ping` sont `Send + Sync`.
unsafe impl ComInterface for IPing {
    type Vtbl = IPingVtbl;
}

/// État Rust de l'objet : compteur d'appels, et compteur de destructions partagé
/// avec le test.
struct Ping {
    hits: AtomicU32,
    dropped: Arc<AtomicUsize>,
}

impl Drop for Ping {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe extern "system" fn ping(this: RawPtr, n: u32) -> u32 {
    // SAFETY: `this` est un `ComObject<IPingVtbl, Ping>` vivant (PortCls détient une
    // référence pendant l'appel).
    let me: &Ping = unsafe { ComObject::<IPingVtbl, Ping>::inner(this) };
    me.hits.fetch_add(1, Ordering::Relaxed);
    n.wrapping_add(1)
}

static PING_VTBL: IPingVtbl = IPingVtbl {
    unknown: IUnknownVtbl {
        query_interface: ComObject::<IPingVtbl, Ping>::query_interface,
        add_ref: ComObject::<IPingVtbl, Ping>::add_ref,
        release: ComObject::<IPingVtbl, Ping>::release,
    },
    ping,
};

fn nouveau() -> (ComPtr<IPingVtbl, Ping>, Arc<AtomicUsize>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let obj = ComObject::new(
        &PING_VTBL,
        Ping {
            hits: AtomicU32::new(0),
            dropped: Arc::clone(&dropped),
        },
    );
    (obj, dropped)
}

// ---------------------------------------------------------------------------------
// Le faux PortCls : ne connaît que `this` et lit la vtable à l'offset 0.
// ---------------------------------------------------------------------------------

/// Ce que fait PortCls : `(*(IPingVtbl**)this)`.
fn vtbl_de(this: RawPtr) -> &'static IPingVtbl {
    assert!(!this.is_null());
    // SAFETY: `this` est un objet COM vivant dont le premier mot est le pointeur de
    // vtable ; la vtable est une `static`.
    unsafe { &*(*this.cast::<*const IPingVtbl>()) }
}

fn qi(this: RawPtr, iid: &Guid) -> (NtStatus, RawPtr) {
    let mut out: RawPtr = ptr::NonNull::<u8>::dangling().as_ptr().cast(); // poison
    let status = unsafe { (vtbl_de(this).unknown.query_interface)(this, iid, &mut out) };
    (status, out)
}

fn add_ref(this: RawPtr) -> u32 {
    unsafe { (vtbl_de(this).unknown.add_ref)(this) }
}

fn release(this: RawPtr) -> u32 {
    unsafe { (vtbl_de(this).unknown.release)(this) }
}

fn appel_ping(this: RawPtr, n: u32) -> u32 {
    unsafe { (vtbl_de(this).ping)(this, n) }
}

// ---------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------

#[test]
fn query_interface_iunknown_et_iping_renvoient_this_et_incrementent() {
    let (obj, dropped) = nouveau();
    let this = obj.into_raw();

    let (status, out) = qi(this, &IID_IUNKNOWN);
    assert_eq!(status, STATUS_SUCCESS);
    assert!(nt_success(status));
    assert_eq!(out, this);

    let (status, out) = qi(this, &IID_IPING);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(out, this);

    // 1 (création) + 2 (QueryInterface) = 3 ; release renvoie le nouveau compte.
    assert_eq!(release(this), 2);
    assert_eq!(release(this), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn query_interface_iid_inconnu() {
    let (obj, dropped) = nouveau();
    let this = obj.into_raw();

    let (status, out) = qi(this, &IID_INCONNU);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(!nt_success(status));
    assert!(out.is_null());

    // Compte inchangé : un seul release libère.
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn query_interface_out_nul() {
    let (obj, dropped) = nouveau();
    let this = obj.into_raw();

    let status =
        unsafe { (vtbl_de(this).unknown.query_interface)(this, &IID_IPING, ptr::null_mut()) };
    assert_eq!(status, STATUS_INVALID_PARAMETER);

    // `iid` nul : invalide aussi, `*out` mis à nul.
    let mut out: RawPtr = this;
    let status = unsafe { (vtbl_de(this).unknown.query_interface)(this, ptr::null(), &mut out) };
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn add_ref_release_renvoient_le_nouveau_compte() {
    let (obj, dropped) = nouveau();
    let this = obj.into_raw();

    assert_eq!(add_ref(this), 2);
    assert_eq!(add_ref(this), 3);
    assert_eq!(add_ref(this), 4);
    assert_eq!(release(this), 3);
    assert_eq!(release(this), 2);
    assert_eq!(add_ref(this), 3);
    assert_eq!(release(this), 2);
    assert_eq!(release(this), 1);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        0,
        "l'objet vit tant qu'une référence reste"
    );
    assert_eq!(release(this), 0);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "Drop exactement une fois"
    );
}

#[test]
fn methode_metier_par_la_vtable() {
    let (obj, dropped) = nouveau();
    assert_eq!(obj.hits.load(Ordering::Relaxed), 0);
    let this = obj.into_raw();

    assert_eq!(appel_ping(this, 41), 42);
    assert_eq!(appel_ping(this, u32::MAX), 0);

    // Reprise côté Rust : l'état a bien été touché par les appels de vtable.
    let obj = unsafe { ComPtr::<IPingVtbl, Ping>::from_raw(this) };
    assert_eq!(obj.hits.load(Ordering::Relaxed), 2);
    assert_eq!(obj.refcount(), 1);
    drop(obj);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn comptr_clone_et_drop() {
    let (obj, dropped) = nouveau();
    assert_eq!(obj.refcount(), 1);

    let b = obj.clone();
    assert_eq!(obj.refcount(), 2);
    assert_eq!(b.as_raw(), obj.as_raw());
    let c = b.clone();
    assert_eq!(c.refcount(), 3);

    drop(b);
    assert_eq!(obj.refcount(), 2);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(c);
    assert_eq!(obj.refcount(), 1);
    drop(obj);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn comptr_into_raw_puis_from_raw() {
    let (obj, dropped) = nouveau();
    let clone = obj.clone();
    let this = obj.into_raw(); // la référence part vers « PortCls »
    assert_eq!(clone.refcount(), 2, "into_raw ne fait pas de Release");

    // PortCls prend une référence de plus puis rend la sienne.
    assert_eq!(add_ref(this), 3);
    assert_eq!(release(this), 2);

    // Le pilote reprend la référence transférée.
    let revenu = unsafe { ComPtr::<IPingVtbl, Ping>::from_raw(this) };
    assert_eq!(revenu.refcount(), 2);
    assert_eq!(revenu.as_raw(), clone.as_raw());
    drop(clone);
    assert_eq!(revenu.refcount(), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(revenu);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn try_new_alloue_et_libere_comme_new() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let obj = ComObject::try_new(
        &PING_VTBL,
        Ping {
            hits: AtomicU32::new(0),
            dropped: Arc::clone(&dropped),
        },
    )
    .expect("allocation en mode utilisateur");
    let this = obj.into_raw();
    assert_eq!(appel_ping(this, 1), 2);
    assert_eq!(add_ref(this), 2);
    assert_eq!(release(this), 1);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn comref_sur_une_interface_recue() {
    let (obj, dropped) = nouveau();
    // Vu de l'autre côté : un `IPing*` que « PortCls » nous aurait donné, déjà compté.
    let recu: *mut IPing = obj.into_raw().cast();

    let r = unsafe { ComRef::<IPing>::from_raw_owned(recu) };
    assert_eq!(r.as_ptr(), recu);
    assert_eq!(r.as_raw(), recu.cast::<core::ffi::c_void>());
    // La vtable lue par `ComRef` est bien la nôtre.
    assert!(ptr::eq(r.vtbl(), &PING_VTBL));
    assert!(ptr::eq(r.unknown(), &PING_VTBL.unknown));

    // Appel d'une méthode par la vtable de la référence.
    assert_eq!(unsafe { (r.vtbl().ping)(r.as_raw(), 9) }, 10);

    // Clone → +1, drop → −1 (observés par le compteur de l'objet).
    let compte = |p: RawPtr| {
        add_ref(p);
        release(p)
    };
    assert_eq!(compte(r.as_raw()), 1);
    let r2 = r.clone();
    assert_eq!(compte(r.as_raw()), 2);
    drop(r2);
    assert_eq!(compte(r.as_raw()), 1);

    // Emprunt d'une référence détenue par ailleurs : AddRef immédiat.
    let r3 = unsafe { ComRef::<IPing>::from_raw_add_ref(recu) };
    assert_eq!(compte(r.as_raw()), 2);
    let r4 = unsafe { ComRef::<IPing>::try_from_raw_add_ref(recu) }.unwrap();
    assert_eq!(compte(r.as_raw()), 3);
    drop(r4);
    drop(r3);
    assert_eq!(compte(r.as_raw()), 1);

    // into_raw cède la référence ; on la rend à la main, l'objet meurt une fois.
    let rendu = r.into_raw();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(rendu.cast()), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn comref_try_from_raw_owned_adopte() {
    let (obj, dropped) = nouveau();
    let recu: *mut IPing = obj.into_raw().cast();
    let r = unsafe { ComRef::<IPing>::try_from_raw_owned(recu) }.unwrap();
    assert!(unsafe { ComRef::<IPing>::try_from_raw_owned(ptr::null_mut()) }.is_none());
    drop(r);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn dispositions() {
    // `vtbl` à l'offset 0 est vérifié dans le crate (`offset_of!` sur champ privé) ;
    // ici, la vue « PortCls » : le premier mot de l'objet est le pointeur de vtable.
    let (obj, _dropped) = nouveau();
    let this = obj.as_raw();
    let premier_mot = unsafe { *this.cast::<*const IPingVtbl>() };
    assert!(ptr::eq(premier_mot, &PING_VTBL));

    assert_eq!(offset_of!(IPingVtbl, unknown), 0);
    assert_eq!(offset_of!(IPingVtbl, ping), size_of::<IUnknownVtbl>());
    assert_eq!(size_of::<IPingVtbl>(), 4 * size_of::<usize>());
    assert_eq!(offset_of!(IPing, lp_vtbl), 0);

    assert_eq!(size_of::<ComRef<IPing>>(), size_of::<usize>());
    assert_eq!(size_of::<Option<ComRef<IPing>>>(), size_of::<usize>());
    assert_eq!(size_of::<ComPtr<IPingVtbl, Ping>>(), size_of::<usize>());
    assert_eq!(
        size_of::<Option<ComPtr<IPingVtbl, Ping>>>(),
        size_of::<usize>()
    );
}

/// Pointeur `this` partageable entre fils (pas de conversion entier ↔ pointeur, pour
/// garder la provenance sous Miri).
#[derive(Clone, Copy)]
struct This(RawPtr);
// SAFETY: l'objet pointé est `Send + Sync` et son compteur atomique ; les fils ne
// font qu'AddRef/Release/Ping à travers la vtable.
unsafe impl Send for This {}
unsafe impl Sync for This {}
impl This {
    /// Accès par méthode : en édition 2021 une fermeture capturerait sinon le champ
    /// `*mut c_void` seul, qui n'est pas `Send`.
    fn get(self) -> RawPtr {
        self.0
    }
}

#[test]
fn concurrence_add_ref_release() {
    const FILS: usize = 8;
    #[cfg(not(miri))]
    const ITERATIONS: u32 = 10_000;
    #[cfg(miri)]
    const ITERATIONS: u32 = 200;

    let (obj, dropped) = nouveau();
    let this = This(obj.into_raw());

    thread::scope(|s| {
        for _ in 0..FILS {
            s.spawn(move || {
                let p = this.get();
                for i in 0..ITERATIONS {
                    let apres = add_ref(p);
                    assert!(apres >= 2, "au moins la référence initiale et la nôtre");
                    if i % 8 == 0 {
                        appel_ping(p, i);
                    }
                    release(p);
                }
            });
        }
    });

    assert_eq!(dropped.load(Ordering::SeqCst), 0, "l'objet survit aux fils");
    let obj = unsafe { ComPtr::<IPingVtbl, Ping>::from_raw(this.0) };
    assert_eq!(obj.refcount(), 1);
    assert_eq!(
        obj.hits.load(Ordering::Relaxed),
        FILS as u32 * ITERATIONS.div_ceil(8)
    );
    drop(obj);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "libéré exactement une fois"
    );
}

#[test]
fn comptr_est_send_et_sync() {
    fn exige<T: Send + Sync>(_: &T) {}
    let (obj, _dropped) = nouveau();
    exige(&obj);
    let r = unsafe { ComRef::<IPing>::from_raw_owned(obj.clone().into_raw().cast()) };
    exige(&r);
    // Un clone traverse un fil et revient.
    let clone = obj.clone();
    let hits = thread::spawn(move || {
        appel_ping(clone.as_raw(), 0);
        clone.hits.load(Ordering::Relaxed)
    })
    .join()
    .unwrap();
    assert_eq!(hits, 1);
    assert_eq!(obj.refcount(), 2);
}
