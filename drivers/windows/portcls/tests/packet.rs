//! Mode paquets vu par un faux PortCls : l'objet composite de `portcls::packet` est
//! appelé **uniquement à travers ses vtables** (celle du principal et les deux têtes
//! satellites), comme le noyau le ferait.
//!
//! Points vérifiés : les quatre interfaces répondent sur un même objet, avec un compteur
//! de références unique et juste après chaque interrogation ; les têtes satellites ont
//! une adresse stable, distincte du principal ; `IID_IUnknown` rend toujours le
//! principal, d'où qu'on l'interroge ; les slots métier atteignent bien le flux du
//! pilote avec leurs paramètres ; les onze slots de `IMiniportWaveRTStream` continuent de
//! fonctionner à travers la délégation de `PacketStream`.

#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use std::mem::offset_of;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use common::{FAUX_MDL, This, add_ref, query_interface, refcount, release, vtbl_de};
use conduit_com::{Guid, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls::conduit_com::IID_IUNKNOWN;
use portcls::portcls_sys::com::guid;
use portcls::portcls_sys::{
    _MEMORY_CACHING_TYPE, BOOL, DWORD, IID_IMiniportWaveRT, IID_IMiniportWaveRTInputStream,
    IID_IMiniportWaveRTOutputStream, IID_IMiniportWaveRTStream,
    IID_IMiniportWaveRTStreamNotification, IMiniportWaveRTInputStreamVtbl,
    IMiniportWaveRTOutputStreamVtbl, IMiniportWaveRTStreamNotificationVtbl,
    IMiniportWaveRTStreamVtbl, KSAUDIO_POSITION, KSAUDIO_PRESENTATION_POSITION, KSSTATE, PKEVENT,
    PMDL, ULONG, ULONG64,
};
use portcls::{
    AudioBuffer, MiniportWaveRTInputStream, MiniportWaveRTOutputStream, MiniportWaveRTStream,
    MiniportWaveRTStreamNotification, PacketInterfaces, PacketStreamVtbl, ReadPacket,
    STATUS_NOT_SUPPORTED, new_packet_stream_object, try_new_packet_stream_object,
};

const SLOT: usize = 8;

// ---------------------------------------------------------------------------------
// Le flux sous test : sert le mode paquets et compte ses appels.
// ---------------------------------------------------------------------------------

struct Flux {
    state: AtomicU32,
    /// Dernier `(packet_number, flags, eos_packet_length)` de `SetWritePacket`.
    dernier_write: AtomicU64,
    lectures: AtomicU32,
    dropped: Arc<AtomicUsize>,
}

impl Flux {
    fn nouveau() -> (Self, Arc<AtomicUsize>) {
        let dropped = Arc::new(AtomicUsize::new(0));
        (
            Self {
                state: AtomicU32::new(u32::MAX),
                dernier_write: AtomicU64::new(0),
                lectures: AtomicU32::new(0),
                dropped: Arc::clone(&dropped),
            },
            dropped,
        )
    }
}

impl Drop for Flux {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl MiniportWaveRTStream for Flux {
    fn set_state(&self, state: KSSTATE::Type) -> NtStatus {
        self.state.store(state as u32, Ordering::SeqCst);
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(4321)
    }

    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus> {
        Ok(AudioBuffer {
            mdl: FAUX_MDL as PMDL,
            actual_bytes: requested_bytes,
            offset_from_first_page: 0,
            cache_type: _MEMORY_CACHING_TYPE::MmCached,
        })
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {}
}

impl MiniportWaveRTStreamNotification for Flux {
    fn allocate_buffer_with_notification(
        &self,
        _: u32,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        self.allocate_audio_buffer(requested_bytes)
    }

    fn free_buffer_with_notification(&self, _: PMDL, _: u32) {}

    fn register_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRTInputStream for Flux {
    fn read_packet(&self) -> Result<ReadPacket, NtStatus> {
        let n = self.lectures.fetch_add(1, Ordering::SeqCst);
        Ok(ReadPacket {
            packet_number: n,
            flags: 0x8000,
            performance_counter: 0x0123_4567_89AB_CDEF,
            more_data: n % 2 == 0,
        })
    }
}

impl MiniportWaveRTOutputStream for Flux {
    fn set_write_packet(&self, packet_number: u32, flags: u32, eos_packet_length: u32) -> NtStatus {
        self.dernier_write.store(
            (u64::from(packet_number) << 40)
                | (u64::from(flags) << 20)
                | u64::from(eos_packet_length),
            Ordering::SeqCst,
        );
        STATUS_SUCCESS
    }

    fn presentation_position(&self) -> Result<KSAUDIO_PRESENTATION_POSITION, NtStatus> {
        Ok(KSAUDIO_PRESENTATION_POSITION {
            u64PositionInBlocks: 77,
            u64QPCPosition: 88,
        })
    }

    fn packet_count(&self) -> Result<u32, NtStatus> {
        Ok(9)
    }
}

/// Flux qui laisse tous les défauts du mode paquets (refus).
struct FluxMuet;

impl MiniportWaveRTStream for FluxMuet {
    fn set_state(&self, _: KSSTATE::Type) -> NtStatus {
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(0)
    }

    fn allocate_audio_buffer(&self, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {}
}

impl MiniportWaveRTStreamNotification for FluxMuet {
    fn allocate_buffer_with_notification(&self, _: u32, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INVALID_PARAMETER)
    }

    fn free_buffer_with_notification(&self, _: PMDL, _: u32) {}

    fn register_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, _: PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRTInputStream for FluxMuet {}
impl MiniportWaveRTOutputStream for FluxMuet {}

// ---------------------------------------------------------------------------------
// Appels de slots, à la manière de PortCls.
// ---------------------------------------------------------------------------------

fn iid_entree() -> Guid {
    guid(&IID_IMiniportWaveRTInputStream)
}

fn iid_sortie() -> Guid {
    guid(&IID_IMiniportWaveRTOutputStream)
}

/// `QueryInterface` qui doit réussir : rend le pointeur et laisse la référence prise.
fn qi_ok(this: This, iid: &Guid) -> This {
    let (status, out) = query_interface(this, iid);
    assert_eq!(status, STATUS_SUCCESS, "{iid}");
    assert!(!out.is_null(), "{iid}");
    out
}

fn appel_get_read_packet(this: This) -> (i32, u32, u32, u64, i32) {
    let vt = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(this) };
    let mut numero: ULONG = 0xDEAD;
    let mut flags: DWORD = 0xDEAD;
    let mut qpc: ULONG64 = 0xDEAD;
    let mut suite: BOOL = 0xDEAD;
    let status =
        unsafe { vt.GetReadPacket.unwrap()(this, &mut numero, &mut flags, &mut qpc, &mut suite) };
    (status, numero, flags, qpc, suite)
}

fn vt_sortie(this: This) -> &'static IMiniportWaveRTOutputStreamVtbl {
    unsafe { vtbl_de::<IMiniportWaveRTOutputStreamVtbl>(this) }
}

// ---------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------

/// Les quatre interfaces sur un seul objet, avec un compteur de références unique : chaque
/// interrogation réussie ajoute exactement une référence, chaque `Release` en rend une, et
/// le dernier détruit le flux.
#[test]
fn quatre_interfaces_sur_un_meme_objet() {
    let (flux, dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
    assert_eq!(refcount(this), 1);

    // Les trois interfaces « classiques » rendent le principal lui-même.
    let mut prises = Vec::new();
    for (i, iid) in [
        IID_IUNKNOWN,
        guid(&IID_IMiniportWaveRTStream),
        guid(&IID_IMiniportWaveRTStreamNotification),
    ]
    .into_iter()
    .enumerate()
    {
        let out = qi_ok(this, &iid);
        assert_eq!(out, this, "{iid} : le principal répond pour lui-même");
        assert_eq!(refcount(this), 2 + i as u32, "une référence de plus");
        prises.push(out);
    }

    // Les deux interfaces de paquets rendent des têtes satellites, distinctes entre elles
    // et du principal, mais comptées sur le même objet.
    let entree = qi_ok(this, &iid_entree());
    assert_eq!(refcount(this), 5);
    let sortie = qi_ok(this, &iid_sortie());
    assert_eq!(refcount(this), 6);
    assert_ne!(entree, this);
    assert_ne!(sortie, this);
    assert_ne!(entree, sortie);
    // Les têtes sont dans l'allocation du principal, dans l'ordre `input`, `output`.
    assert!(entree > this && sortie > entree);
    // Adresses stables : deux interrogations rendent la même tête.
    let entree2 = qi_ok(this, &iid_entree());
    assert_eq!(entree2, entree);
    assert_eq!(refcount(this), 7);
    release(entree2);

    // Un IID inconnu ne passe pas et ne compte rien.
    let (status, out) = query_interface(this, &guid(&IID_IMiniportWaveRT));
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());
    assert_eq!(refcount(this), 6);

    // Rendu des six références : rien n'est détruit avant la dernière.
    for prise in prises {
        release(prise);
    }
    assert_eq!(release(entree), 2, "le satellite décompte sur le principal");
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(sortie), 1);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Le sens du flux filtre les interfaces exposées : un flux de rendu ne répond qu'à
/// `IMiniportWaveRTOutputStream`, un flux de capture qu'à `IMiniportWaveRTInputStream`.
#[test]
fn le_sens_du_flux_filtre_les_interfaces() {
    for (interfaces, attendue, refusee) in [
        (PacketInterfaces::Output, iid_sortie(), iid_entree()),
        (PacketInterfaces::Input, iid_entree(), iid_sortie()),
    ] {
        let (flux, dropped) = Flux::nouveau();
        let this = new_packet_stream_object(flux, interfaces).into_raw();

        let out = qi_ok(this, &attendue);
        assert_ne!(out, this);
        assert_eq!(refcount(this), 2);
        release(out);

        let (status, out) = query_interface(this, &refusee);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "{interfaces:?}");
        assert!(out.is_null(), "{interfaces:?}");
        assert_eq!(refcount(this), 1, "un refus ne compte pas de référence");

        assert_eq!(release(this), 0);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
    assert!(PacketInterfaces::Both.has_input() && PacketInterfaces::Both.has_output());
    assert!(!PacketInterfaces::Input.has_output());
    assert!(!PacketInterfaces::Output.has_input());
}

/// `IID_IUnknown` interrogé depuis une tête satellite rend **le principal** (identité COM),
/// et l'on passe d'une tête à l'autre.
#[test]
fn identite_et_transitivite_depuis_un_satellite() {
    let (flux, dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
    let entree = qi_ok(this, &iid_entree());

    let unknown = qi_ok(entree, &IID_IUNKNOWN);
    assert_eq!(unknown, this, "IID_IUnknown est toujours le principal");
    let notification = qi_ok(entree, &guid(&IID_IMiniportWaveRTStreamNotification));
    assert_eq!(notification, this);
    let sortie = qi_ok(entree, &iid_sortie());
    assert_ne!(sortie, entree);
    // 1 (initiale) + entrée + unknown + notification + sortie.
    assert_eq!(refcount(this), 5);

    // `AddRef` sur un satellite compte sur le principal.
    assert_eq!(add_ref(sortie), 6);
    assert_eq!(refcount(this), 6);
    release(sortie);

    let (status, out) = query_interface(entree, &guid(&IID_IMiniportWaveRT));
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());

    release(unknown);
    release(notification);
    release(sortie);
    release(entree);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Le dernier `Release` peut venir d'une tête satellite : il détruit l'objet entier.
#[test]
fn le_dernier_release_peut_venir_d_un_satellite() {
    let (flux, dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Input).into_raw();
    let entree = qi_ok(this, &iid_entree());
    assert_eq!(release(this), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(entree), 0, "le satellite libère l'objet");
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Slots métier de `IMiniportWaveRTInputStream`, appelés par la vtable de la tête.
#[test]
fn get_read_packet_par_la_vtable() {
    let (flux, _dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Input).into_raw();
    let entree = qi_ok(this, &iid_entree());

    let (status, numero, flags, qpc, suite) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        (numero, flags, qpc, suite),
        (0, 0x8000, 0x0123_4567_89AB_CDEF, 1)
    );
    let (status, numero, _, _, suite) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        (numero, suite),
        (1, 0),
        "MoreData = FALSE au deuxième appel"
    );

    // Sortie nulle : refus sans appeler le trait.
    let vt = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(entree) };
    let mut flags: DWORD = 0;
    let mut qpc: ULONG64 = 0;
    let mut plus: BOOL = 0;
    let status = unsafe {
        vt.GetReadPacket.unwrap()(entree, ptr::null_mut(), &mut flags, &mut qpc, &mut plus)
    };
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    let (status, ..) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_SUCCESS);

    release(entree);
    assert_eq!(release(this), 0);
}

/// Slots métier de `IMiniportWaveRTOutputStream`, appelés par la vtable de la tête.
#[test]
fn slots_de_sortie_par_la_vtable() {
    let (flux, _dropped) = Flux::nouveau();
    let objet = new_packet_stream_object(flux, PacketInterfaces::Output);
    let this = objet.into_raw();
    let sortie = qi_ok(this, &iid_sortie());
    let vt = vt_sortie(sortie);

    assert_eq!(
        unsafe { vt.SetWritePacket.unwrap()(sortie, 12, 0x2, 480) },
        STATUS_SUCCESS
    );

    let mut position = KSAUDIO_PRESENTATION_POSITION {
        u64PositionInBlocks: 0,
        u64QPCPosition: 0,
    };
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, &mut position) },
        STATUS_SUCCESS
    );
    assert_eq!(
        (position.u64PositionInBlocks, position.u64QPCPosition),
        (77, 88)
    );
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );

    let mut compte: ULONG = 0xDEAD;
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, &mut compte) },
        STATUS_SUCCESS
    );
    assert_eq!(compte, 9);
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );

    release(sortie);
    assert_eq!(release(this), 0);
}

/// Les défauts des deux traits refusent (`STATUS_NOT_SUPPORTED`) sans rien écrire.
#[test]
fn defauts_du_mode_paquets_refusent() {
    let this = try_new_packet_stream_object(FluxMuet, PacketInterfaces::Both)
        .unwrap()
        .into_raw();
    let entree = qi_ok(this, &iid_entree());
    let sortie = qi_ok(this, &iid_sortie());

    let (status, numero, ..) = appel_get_read_packet(entree);
    assert_eq!(status, STATUS_NOT_SUPPORTED);
    assert_eq!(numero, 0xDEAD, "sortie non touchée");

    let vt = vt_sortie(sortie);
    assert_eq!(
        unsafe { vt.SetWritePacket.unwrap()(sortie, 1, 0, 0) },
        STATUS_NOT_SUPPORTED
    );
    let mut compte: ULONG = 0xDEAD;
    assert_eq!(
        unsafe { vt.GetPacketCount.unwrap()(sortie, &mut compte) },
        STATUS_NOT_SUPPORTED
    );
    assert_eq!(compte, 0xDEAD);
    let mut position = KSAUDIO_PRESENTATION_POSITION {
        u64PositionInBlocks: 5,
        u64QPCPosition: 5,
    };
    assert_eq!(
        unsafe { vt.GetOutputStreamPresentationPosition.unwrap()(sortie, &mut position) },
        STATUS_NOT_SUPPORTED
    );
    assert_eq!(position.u64PositionInBlocks, 5);

    release(entree);
    release(sortie);
    assert_eq!(release(this), 0);
}

/// La délégation de `PacketStream` laisse intacts les quinze slots de flux du principal :
/// ils atteignent le flux du pilote comme sur un objet non composite.
#[test]
fn les_slots_de_flux_traversent_le_composite() {
    let (flux, _dropped) = Flux::nouveau();
    let objet = new_packet_stream_object(flux, PacketInterfaces::Output);
    // `Deref` : `ComPtr` → `PacketStream<Flux>` → `Flux`.
    assert_eq!(objet.interfaces(), PacketInterfaces::Output);
    assert_eq!(objet.get().lectures.load(Ordering::SeqCst), 0);
    let this = objet.into_raw();

    let vt = unsafe { vtbl_de::<IMiniportWaveRTStreamVtbl>(this) };
    assert_eq!(
        unsafe { vt.SetState.unwrap()(this, KSSTATE::KSSTATE_RUN) },
        STATUS_SUCCESS
    );
    let mut position = KSAUDIO_POSITION {
        PlayOffset: 0,
        WriteOffset: 0,
    };
    assert_eq!(
        unsafe { vt.GetPosition.unwrap()(this, &mut position) },
        STATUS_SUCCESS
    );
    assert_eq!((position.PlayOffset, position.WriteOffset), (4321, 4321));

    let mut mdl: PMDL = ptr::null_mut();
    let mut actual: ULONG = 0;
    let mut offset: ULONG = 0xDEAD;
    let mut cache = -7;
    let vt_notif = unsafe { vtbl_de::<IMiniportWaveRTStreamNotificationVtbl>(this) };
    let status = unsafe {
        vt_notif.AllocateBufferWithNotification.unwrap()(
            this,
            2,
            960,
            &mut mdl,
            &mut actual,
            &mut offset,
            &mut cache,
        )
    };
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!((mdl as usize, actual, offset), (FAUX_MDL, 960, 0));
    unsafe { vt_notif.FreeBufferWithNotification.unwrap()(this, mdl, actual) };

    assert_eq!(release(this), 0);
}

/// Ordre et remplissage des slots des deux vtables satellites, sur les vtables
/// **construites par le crate** : mêmes offsets que `portcls-sys/tests/vtables.rs`.
#[test]
fn ordre_des_slots_des_vtables_satellites() {
    let entree = [
        (
            "QueryInterface",
            offset_of!(IMiniportWaveRTInputStreamVtbl, QueryInterface),
        ),
        ("AddRef", offset_of!(IMiniportWaveRTInputStreamVtbl, AddRef)),
        (
            "Release",
            offset_of!(IMiniportWaveRTInputStreamVtbl, Release),
        ),
        (
            "GetReadPacket",
            offset_of!(IMiniportWaveRTInputStreamVtbl, GetReadPacket),
        ),
    ];
    for (i, (nom, offset)) in entree.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }
    let sortie = [
        (
            "QueryInterface",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, QueryInterface),
        ),
        (
            "AddRef",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, AddRef),
        ),
        (
            "Release",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, Release),
        ),
        (
            "SetWritePacket",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, SetWritePacket),
        ),
        (
            "GetOutputStreamPresentationPosition",
            offset_of!(
                IMiniportWaveRTOutputStreamVtbl,
                GetOutputStreamPresentationPosition
            ),
        ),
        (
            "GetPacketCount",
            offset_of!(IMiniportWaveRTOutputStreamVtbl, GetPacketCount),
        ),
    ];
    for (i, (nom, offset)) in sortie.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }

    let e = &<Flux as PacketStreamVtbl>::INPUT_VTBL;
    assert!(
        e.QueryInterface.is_some()
            && e.AddRef.is_some()
            && e.Release.is_some()
            && e.GetReadPacket.is_some()
    );
    let s = &<Flux as PacketStreamVtbl>::OUTPUT_VTBL;
    assert!(
        s.QueryInterface.is_some()
            && s.AddRef.is_some()
            && s.Release.is_some()
            && s.SetWritePacket.is_some()
            && s.GetOutputStreamPresentationPosition.is_some()
            && s.GetPacketCount.is_some()
    );
    let p = &<Flux as PacketStreamVtbl>::VTBL;
    assert!(p.QueryInterface.is_some() && p.UnregisterNotificationEvent.is_some());

    // Deux objets du même type partagent leurs trois vtables (constantes promues).
    let (f1, _) = Flux::nouveau();
    let (f2, _) = Flux::nouveau();
    let o1 = new_packet_stream_object(f1, PacketInterfaces::Both);
    let o2 = try_new_packet_stream_object(f2, PacketInterfaces::Both).unwrap();
    assert!(ptr::eq(o1.object().vtbl(), o2.object().vtbl()));
    let t1 = qi_ok(o1.as_raw(), &iid_entree());
    let t2 = qi_ok(o2.as_raw(), &iid_entree());
    let v1 = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(t1) };
    let v2 = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(t2) };
    assert!(ptr::eq(v1, v2));
    release(t1);
    release(t2);
}

/// `QueryInterface` avec des arguments dégénérés, sur le principal comme sur un satellite.
#[test]
fn query_interface_arguments_degeneres() {
    let (flux, _dropped) = Flux::nouveau();
    let this = new_packet_stream_object(flux, PacketInterfaces::Both).into_raw();
    let entree = qi_ok(this, &iid_entree());

    let vt = unsafe { vtbl_de::<IMiniportWaveRTStreamNotificationVtbl>(this) };
    let iid = iid_entree();
    // `out` nul : refus, rien n'est écrit ni compté.
    assert_eq!(
        unsafe {
            vt.QueryInterface.unwrap()(
                this,
                ptr::from_ref(&iid).cast(),
                ptr::null_mut::<*mut core::ffi::c_void>().cast(),
            )
        },
        STATUS_INVALID_PARAMETER
    );
    // `iid` nul : refus, `*out` mis à nul.
    let mut out: *mut core::ffi::c_void = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    assert_eq!(
        unsafe { vt.QueryInterface.unwrap()(this, ptr::null(), (&raw mut out).cast()) },
        STATUS_INVALID_PARAMETER
    );
    assert!(out.is_null());
    assert_eq!(refcount(this), 2);

    // Mêmes réponses depuis un satellite.
    let vt = unsafe { vtbl_de::<IMiniportWaveRTInputStreamVtbl>(entree) };
    let mut out: *mut core::ffi::c_void = ptr::NonNull::<u8>::dangling().as_ptr().cast();
    assert_eq!(
        unsafe { vt.QueryInterface.unwrap()(entree, ptr::null(), (&raw mut out).cast()) },
        STATUS_INVALID_PARAMETER
    );
    assert!(out.is_null());
    assert_eq!(refcount(this), 2);

    release(entree);
    assert_eq!(release(this), 0);
}
