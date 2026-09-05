//! `MiniportWaveRTStream` et `MiniportWaveRTStreamNotification` vus par un faux
//! PortCls : les objets construits par `new_stream_object` /
//! `new_stream_notification_object` sont appelés **uniquement à travers leur vtable**
//! (`QueryInterface`, `SetFormat`, `SetState`, `GetPosition`, `AllocateAudioBuffer`,
//! `FreeAudioBuffer`, `GetHWLatency`, `GetPositionRegister`, `GetClockRegister`, puis
//! `AllocateBufferWithNotification`, `RegisterNotificationEvent`…, `Release`), et le
//! flux appelle lui-même un faux `IPortWaveRTStream` par l'enveloppe `PortWaveRTStream`.

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
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use common::{
    FAUX_MDL, FauxPortStream, This, faux_port_stream, query_interface, refcount, release, vtbl_de,
};
use conduit_com::{
    ComPtr, Guid, NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER,
    STATUS_NOT_IMPLEMENTED, STATUS_SUCCESS, STATUS_UNSUCCESSFUL,
};
use portcls::conduit_com::IID_IUNKNOWN;
use portcls::portcls_sys::com::guid;
use portcls::portcls_sys::{
    _KEVENT, _MEMORY_CACHING_TYPE, IID_IMiniportWaveRT, IID_IMiniportWaveRTStream,
    IID_IMiniportWaveRTStreamNotification, IMiniportWaveRTStream,
    IMiniportWaveRTStreamNotification, IMiniportWaveRTStreamNotificationVtbl,
    IMiniportWaveRTStreamVtbl, IPortWaveRTStreamVtbl, KSAUDIO_POSITION, KSDATAFORMAT,
    KSRTAUDIO_HWLATENCY, KSRTAUDIO_HWREGISTER, KSSTATE, MEMORY_CACHING_TYPE, PKEVENT, PMDL, ULONG,
};
use portcls::{
    AudioBuffer, MiniportWaveRTStream, MiniportWaveRTStreamNotification, PortWaveRTStream,
    STATUS_NOT_SUPPORTED, StreamNotificationVtbl, StreamVtbl, new_stream_notification_object,
    new_stream_object, physical_address, try_new_stream_object,
};

const SLOT: usize = 8;

// ---------------------------------------------------------------------------------
// Le flux sous test : position fixe, tampon alloué par le faux port, compteurs.
// ---------------------------------------------------------------------------------

struct Flux {
    port: PortWaveRTStream,
    position: AtomicU32,
    state: AtomicU32,
    formats: AtomicU32,
    frees: AtomicU32,
    notif_count: AtomicU32,
    events: AtomicU32,
    dropped: Arc<AtomicUsize>,
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
        Ok(self.position.load(Ordering::SeqCst))
    }

    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus> {
        // Arrondi à la trame (8 octets : 2 canaux float32), jamais moins que demandé.
        let actual = requested_bytes.div_ceil(8) * 8;
        let mdl = self
            .port
            .allocate_pages_for_mdl(physical_address(u32::MAX as i64), actual as usize)?;
        Ok(AudioBuffer {
            mdl,
            actual_bytes: actual,
            offset_from_first_page: 0,
            cache_type: _MEMORY_CACHING_TYPE::MmCached,
        })
    }

    fn free_audio_buffer(&self, mdl: PMDL, size: u32) {
        assert_eq!(size % 8, 0);
        unsafe { self.port.free_pages_from_mdl(mdl) }.unwrap();
        self.frees.fetch_add(1, Ordering::SeqCst);
    }
}

impl MiniportWaveRTStreamNotification for Flux {
    fn allocate_buffer_with_notification(
        &self,
        notification_count: u32,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        self.notif_count.store(notification_count, Ordering::SeqCst);
        self.allocate_audio_buffer(requested_bytes)
    }

    fn free_buffer_with_notification(&self, mdl: PMDL, size: u32) {
        self.free_audio_buffer(mdl, size);
    }

    fn register_notification_event(&self, event: PKEVENT) -> NtStatus {
        assert!(!event.is_null());
        self.events.fetch_add(1, Ordering::SeqCst);
        STATUS_SUCCESS
    }

    fn unregister_notification_event(&self, event: PKEVENT) -> NtStatus {
        assert!(!event.is_null());
        self.events.fetch_sub(1, Ordering::SeqCst);
        STATUS_SUCCESS
    }
}

fn flux(port: &ComPtr<IPortWaveRTStreamVtbl, FauxPortStream>) -> (Flux, Arc<AtomicUsize>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    // Le flux prend sa référence sur le port, comme le thunk `NewStream` le ferait.
    let port_ref = unsafe { conduit_com::ComRef::from_raw_add_ref(port.as_raw().cast()) };
    (
        Flux {
            port: PortWaveRTStream::from_ref(port_ref),
            position: AtomicU32::new(1234),
            state: AtomicU32::new(u32::MAX),
            formats: AtomicU32::new(0),
            frees: AtomicU32::new(0),
            notif_count: AtomicU32::new(0),
            events: AtomicU32::new(0),
            dropped: Arc::clone(&dropped),
        },
        dropped,
    )
}

/// Flux qui accepte `SetFormat` et refuse `AllocateAudioBuffer`.
struct FluxRefus {
    formats: AtomicU32,
}

impl MiniportWaveRTStream for FluxRefus {
    fn set_format(&self, format: &KSDATAFORMAT) -> NtStatus {
        assert_eq!(unsafe { format.__bindgen_anon_1.FormatSize }, 64);
        self.formats.fetch_add(1, Ordering::SeqCst);
        STATUS_SUCCESS
    }

    fn set_state(&self, _: KSSTATE::Type) -> NtStatus {
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Err(STATUS_UNSUCCESSFUL)
    }

    fn allocate_audio_buffer(&self, _: u32) -> Result<AudioBuffer, NtStatus> {
        Err(STATUS_INSUFFICIENT_RESOURCES)
    }

    fn free_audio_buffer(&self, _: PMDL, _: u32) {
        panic!("jamais appelé");
    }

    fn hw_latency(&self, out: &mut KSRTAUDIO_HWLATENCY) {
        out.FifoSize = 16;
    }

    fn position_register(&self) -> Result<KSRTAUDIO_HWREGISTER, NtStatus> {
        Ok(KSRTAUDIO_HWREGISTER {
            Register: ptr::null_mut(),
            Width: 32,
            Numerator: 1,
            Denominator: 48_000,
            Accuracy: 0,
        })
    }
}

// ---------------------------------------------------------------------------------
// Appels de slots, à la manière de PortCls. Les deux vtables partagent leurs onze
// premiers slots : on lit toujours `IMiniportWaveRTStreamVtbl` pour ceux-là.
// ---------------------------------------------------------------------------------

fn vt(this: This) -> &'static IMiniportWaveRTStreamVtbl {
    unsafe { vtbl_de::<IMiniportWaveRTStreamVtbl>(this) }
}

fn vt_notif(this: This) -> &'static IMiniportWaveRTStreamNotificationVtbl {
    unsafe { vtbl_de::<IMiniportWaveRTStreamNotificationVtbl>(this) }
}

fn appel_set_state(this: This, state: KSSTATE::Type) -> i32 {
    unsafe { vt(this).SetState.unwrap()(this, state) }
}

fn appel_get_position(this: This, out: *mut KSAUDIO_POSITION) -> i32 {
    unsafe { vt(this).GetPosition.unwrap()(this, out) }
}

struct Sorties {
    mdl: PMDL,
    actual: ULONG,
    offset: ULONG,
    cache: MEMORY_CACHING_TYPE,
}

fn sorties_empoisonnees() -> Sorties {
    Sorties {
        mdl: ptr::NonNull::dangling().as_ptr(),
        actual: 0xDEAD,
        offset: 0xDEAD,
        cache: -7,
    }
}

fn appel_allocate(this: This, requested: ULONG, s: &mut Sorties) -> i32 {
    unsafe {
        vt(this).AllocateAudioBuffer.unwrap()(
            this,
            requested,
            &mut s.mdl,
            &mut s.actual,
            &mut s.offset,
            &mut s.cache,
        )
    }
}

fn appel_allocate_notif(this: This, count: ULONG, requested: ULONG, s: &mut Sorties) -> i32 {
    unsafe {
        vt_notif(this).AllocateBufferWithNotification.unwrap()(
            this,
            count,
            requested,
            &mut s.mdl,
            &mut s.actual,
            &mut s.offset,
            &mut s.cache,
        )
    }
}

// ---------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------

#[test]
fn query_interface_flux_simple_et_notification() {
    let port = faux_port_stream();
    let (f, dropped) = flux(&port);
    let this = new_stream_object(f).into_raw();
    assert_eq!(port.refcount(), 2, "le flux tient le port");

    for iid in [IID_IUNKNOWN, guid(&IID_IMiniportWaveRTStream)] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_SUCCESS, "{iid}");
        assert_eq!(out, this);
    }
    for iid in [
        guid(&IID_IMiniportWaveRTStreamNotification),
        guid(&IID_IMiniportWaveRT),
    ] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "{iid}");
        assert!(out.is_null());
    }
    assert_eq!(refcount(this), 3);
    release(this);
    release(this);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(port.refcount(), 1, "le Drop du flux rend le port");

    // Objet notification : répond en plus à `IID_IMiniportWaveRTStreamNotification`.
    let (f, dropped) = flux(&port);
    let this = new_stream_notification_object(f).into_raw();
    for iid in [
        IID_IUNKNOWN,
        guid(&IID_IMiniportWaveRTStream),
        guid(&IID_IMiniportWaveRTStreamNotification),
    ] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_SUCCESS, "{iid}");
        assert_eq!(out, this);
        release(out);
    }
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn set_state_run_transmis_et_set_format_refuse_par_defaut() {
    let port = faux_port_stream();
    let (f, _dropped) = flux(&port);
    let this = new_stream_object(f).into_raw();
    let me = unsafe { conduit_com::ComObject::<IMiniportWaveRTStreamVtbl, Flux>::inner(this) };

    assert_eq!(appel_set_state(this, KSSTATE::KSSTATE_RUN), STATUS_SUCCESS);
    assert_eq!(
        me.state.load(Ordering::SeqCst),
        3,
        "KSSTATE_RUN == 3 transmis"
    );
    assert_eq!(appel_set_state(this, KSSTATE::KSSTATE_STOP), STATUS_SUCCESS);
    assert_eq!(me.state.load(Ordering::SeqCst), 0);

    let mut format = KSDATAFORMAT::default();
    format.__bindgen_anon_1.FormatSize = 64;
    assert_eq!(
        unsafe { vt(this).SetFormat.unwrap()(this, &mut format) },
        STATUS_NOT_SUPPORTED,
        "défaut : format figé par NewStream"
    );
    assert_eq!(
        unsafe { vt(this).SetFormat.unwrap()(this, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(me.formats.load(Ordering::SeqCst), 0);
    assert_eq!(release(this), 0);

    // Un flux qui accepte le format le voit.
    let this = new_stream_object(FluxRefus {
        formats: AtomicU32::new(0),
    })
    .into_raw();
    assert_eq!(
        unsafe { vt(this).SetFormat.unwrap()(this, &mut format) },
        STATUS_SUCCESS
    );
    let me = unsafe { conduit_com::ComObject::<IMiniportWaveRTStreamVtbl, FluxRefus>::inner(this) };
    assert_eq!(me.formats.load(Ordering::SeqCst), 1);
    assert_eq!(release(this), 0);
}

#[test]
fn get_position_ecrit_play_et_write_offset() {
    let port = faux_port_stream();
    let (f, _dropped) = flux(&port);
    let this = new_stream_object(f).into_raw();

    let mut pos = KSAUDIO_POSITION {
        PlayOffset: u64::MAX,
        WriteOffset: u64::MAX,
    };
    assert_eq!(appel_get_position(this, &mut pos), STATUS_SUCCESS);
    assert_eq!(pos.PlayOffset, 1234);
    assert_eq!(pos.WriteOffset, 1234);
    assert_eq!(
        appel_get_position(this, ptr::null_mut()),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(release(this), 0);

    // Erreur du trait : statut transmis, structure non touchée.
    let this = new_stream_object(FluxRefus {
        formats: AtomicU32::new(0),
    })
    .into_raw();
    pos.PlayOffset = 7;
    assert_eq!(appel_get_position(this, &mut pos), STATUS_UNSUCCESSFUL);
    assert_eq!(pos.PlayOffset, 7);
    assert_eq!(release(this), 0);
}

#[test]
fn allocate_audio_buffer_ecrit_les_quatre_sorties_depuis_audio_buffer() {
    let port = faux_port_stream();
    let (f, _dropped) = flux(&port);
    let this = new_stream_object(f).into_raw();
    let me = unsafe { conduit_com::ComObject::<IMiniportWaveRTStreamVtbl, Flux>::inner(this) };

    let mut s = sorties_empoisonnees();
    assert_eq!(appel_allocate(this, 4801, &mut s), STATUS_SUCCESS);
    assert_eq!(s.mdl as usize, FAUX_MDL, "MDL du faux port");
    assert_eq!(
        s.actual, 4808,
        "arrondi à la trame, jamais moins que demandé"
    );
    assert_eq!(s.offset, 0);
    assert_eq!(s.cache, _MEMORY_CACHING_TYPE::MmCached);
    // Le faux port a été appelé par `PortWaveRTStream::allocate_pages_for_mdl`, avec la
    // borne haute et la taille demandées.
    assert_eq!(port.allocs.load(Ordering::SeqCst), 1);
    assert_eq!(port.last_total.load(Ordering::SeqCst), 4808);
    assert_eq!(port.last_high.load(Ordering::SeqCst), u64::from(u32::MAX));
    assert_eq!(
        unsafe { me.port.physical_pages_count(s.mdl) },
        Ok(2),
        "GetPhysicalPagesCount par l'enveloppe"
    );

    // `FreeAudioBuffer` rend la MDL et la taille : le flux libère par le faux port.
    unsafe { vt(this).FreeAudioBuffer.unwrap()(this, s.mdl, s.actual) };
    assert_eq!(me.frees.load(Ordering::SeqCst), 1);
    assert_eq!(port.frees.load(Ordering::SeqCst), 1);

    // Sortie nulle : refus sans appeler le trait ni le port.
    let mut s = sorties_empoisonnees();
    let status = unsafe {
        vt(this).AllocateAudioBuffer.unwrap()(
            this,
            64,
            ptr::null_mut(),
            &mut s.actual,
            &mut s.offset,
            &mut s.cache,
        )
    };
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert_eq!(port.allocs.load(Ordering::SeqCst), 1);

    // Échec du port (MDL nulle) : `STATUS_INSUFFICIENT_RESOURCES`, `*AudioBufferMdl` nul.
    port.fail.store(1, Ordering::SeqCst);
    let mut s = sorties_empoisonnees();
    assert_eq!(
        appel_allocate(this, 64, &mut s),
        STATUS_INSUFFICIENT_RESOURCES
    );
    assert!(s.mdl.is_null(), "MDL nulle en cas d'erreur");
    assert_eq!(s.actual, 0xDEAD, "les autres sorties ne sont pas touchées");
    assert_eq!(port.allocs.load(Ordering::SeqCst), 2);

    assert_eq!(release(this), 0);
    assert_eq!(port.refcount(), 1);
}

#[test]
fn slots_vides_du_port_donnent_status_not_implemented() {
    let port = faux_port_stream();
    let port_ref: PortWaveRTStream =
        unsafe { conduit_com::ComRef::from_raw_add_ref(port.as_raw().cast()) }.into();
    assert_eq!(
        port_ref.allocate_contiguous_pages_for_mdl(
            physical_address(0),
            physical_address(i64::MAX),
            4096
        ),
        Err(STATUS_NOT_IMPLEMENTED)
    );
    let mdl = FAUX_MDL as PMDL;
    assert_eq!(
        unsafe { port_ref.map_allocated_pages(mdl, _MEMORY_CACHING_TYPE::MmCached) },
        Err(STATUS_NOT_IMPLEMENTED)
    );
    assert_eq!(
        unsafe { port_ref.unmap_allocated_pages(ptr::null_mut(), mdl) },
        Err(STATUS_NOT_IMPLEMENTED)
    );
    assert_eq!(
        unsafe { port_ref.physical_page_address(mdl, 0) }.map(|a| unsafe { a.QuadPart }),
        Err(STATUS_NOT_IMPLEMENTED)
    );
    assert_eq!(port.refcount(), 2);
    drop(port_ref);
    assert_eq!(port.refcount(), 1);
}

#[test]
fn hw_latency_et_registres_par_defaut() {
    let port = faux_port_stream();
    let (f, _dropped) = flux(&port);
    let this = new_stream_object(f).into_raw();

    let mut latence = KSRTAUDIO_HWLATENCY {
        FifoSize: 9,
        ChipsetDelay: 9,
        CodecDelay: 9,
    };
    unsafe { vt(this).GetHWLatency.unwrap()(this, &mut latence) };
    assert_eq!(
        (latence.FifoSize, latence.ChipsetDelay, latence.CodecDelay),
        (0, 0, 0),
        "défaut : zéros"
    );
    // Pointeur nul toléré (méthode sans retour).
    unsafe { vt(this).GetHWLatency.unwrap()(this, ptr::null_mut()) };

    let mut reg = KSRTAUDIO_HWREGISTER {
        Width: 99,
        ..KSRTAUDIO_HWREGISTER::default()
    };
    assert_eq!(
        unsafe { vt(this).GetPositionRegister.unwrap()(this, &mut reg) },
        STATUS_NOT_IMPLEMENTED
    );
    assert_eq!(
        unsafe { vt(this).GetClockRegister.unwrap()(this, &mut reg) },
        STATUS_NOT_IMPLEMENTED
    );
    assert_eq!(reg.Width, 99, "non touché");
    assert_eq!(
        unsafe { vt(this).GetPositionRegister.unwrap()(this, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(release(this), 0);

    // Surcharges.
    let this = new_stream_object(FluxRefus {
        formats: AtomicU32::new(0),
    })
    .into_raw();
    unsafe { vt(this).GetHWLatency.unwrap()(this, &mut latence) };
    assert_eq!(latence.FifoSize, 16);
    assert_eq!(latence.ChipsetDelay, 0, "remis à zéro avant l'appel");
    assert_eq!(
        unsafe { vt(this).GetPositionRegister.unwrap()(this, &mut reg) },
        STATUS_SUCCESS
    );
    assert_eq!((reg.Width, reg.Denominator), (32, 48_000));
    assert_eq!(release(this), 0);
}

#[test]
fn notification_allocation_et_evenements() {
    let port = faux_port_stream();
    let (f, dropped) = flux(&port);
    let this = new_stream_notification_object(f).into_raw();
    let me = unsafe {
        conduit_com::ComObject::<IMiniportWaveRTStreamNotificationVtbl, Flux>::inner(this)
    };

    // Les onze premiers slots servent aussi sur l'objet notification.
    assert_eq!(
        appel_set_state(this, KSSTATE::KSSTATE_PAUSE),
        STATUS_SUCCESS
    );
    assert_eq!(me.state.load(Ordering::SeqCst), 2);

    let mut s = sorties_empoisonnees();
    assert_eq!(appel_allocate_notif(this, 2, 960, &mut s), STATUS_SUCCESS);
    assert_eq!(me.notif_count.load(Ordering::SeqCst), 2);
    assert_eq!(s.mdl as usize, FAUX_MDL);
    assert_eq!(s.actual, 960);
    assert_eq!(port.allocs.load(Ordering::SeqCst), 1);
    unsafe { vt_notif(this).FreeBufferWithNotification.unwrap()(this, s.mdl, s.actual) };
    assert_eq!(port.frees.load(Ordering::SeqCst), 1);

    let mut event = _KEVENT::default();
    let register = vt_notif(this).RegisterNotificationEvent.unwrap();
    let unregister = vt_notif(this).UnregisterNotificationEvent.unwrap();
    assert_eq!(unsafe { register(this, &mut event) }, STATUS_SUCCESS);
    assert_eq!(me.events.load(Ordering::SeqCst), 1);
    assert_eq!(
        unsafe { register(this, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(unsafe { unregister(this, &mut event) }, STATUS_SUCCESS);
    assert_eq!(me.events.load(Ordering::SeqCst), 0);

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(port.refcount(), 1);
}

/// Ordre des slots : mêmes noms et même ordre que `portcls-sys/tests/vtables.rs`
/// (`ordre_des_slots_iminiportwavertstream`), vérifiés ici sur les vtables **remplies**
/// par le crate : chaque slot est `Some` et à l'offset attendu.
#[test]
fn ordre_des_slots_et_vtables_par_type() {
    assert_eq!(offset_of!(IMiniportWaveRTStream, lpVtbl), 0);
    assert_eq!(offset_of!(IMiniportWaveRTStreamNotification, lpVtbl), 0);

    let noms = [
        "QueryInterface",
        "AddRef",
        "Release",
        "SetFormat",
        "SetState",
        "GetPosition",
        "AllocateAudioBuffer",
        "FreeAudioBuffer",
        "GetHWLatency",
        "GetPositionRegister",
        "GetClockRegister",
    ];
    let offsets = [
        offset_of!(IMiniportWaveRTStreamVtbl, QueryInterface),
        offset_of!(IMiniportWaveRTStreamVtbl, AddRef),
        offset_of!(IMiniportWaveRTStreamVtbl, Release),
        offset_of!(IMiniportWaveRTStreamVtbl, SetFormat),
        offset_of!(IMiniportWaveRTStreamVtbl, SetState),
        offset_of!(IMiniportWaveRTStreamVtbl, GetPosition),
        offset_of!(IMiniportWaveRTStreamVtbl, AllocateAudioBuffer),
        offset_of!(IMiniportWaveRTStreamVtbl, FreeAudioBuffer),
        offset_of!(IMiniportWaveRTStreamVtbl, GetHWLatency),
        offset_of!(IMiniportWaveRTStreamVtbl, GetPositionRegister),
        offset_of!(IMiniportWaveRTStreamVtbl, GetClockRegister),
    ];
    for (i, (nom, offset)) in noms.iter().zip(offsets).enumerate() {
        assert_eq!(offset, i * SLOT, "slot {i} = {nom}");
    }

    let v = &<Flux as StreamVtbl>::VTBL;
    let remplis = [
        v.QueryInterface.is_some(),
        v.AddRef.is_some(),
        v.Release.is_some(),
        v.SetFormat.is_some(),
        v.SetState.is_some(),
        v.GetPosition.is_some(),
        v.AllocateAudioBuffer.is_some(),
        v.FreeAudioBuffer.is_some(),
        v.GetHWLatency.is_some(),
        v.GetPositionRegister.is_some(),
        v.GetClockRegister.is_some(),
    ];
    assert!(remplis.iter().all(|r| *r), "tous les slots sont remplis");

    let n = &<Flux as StreamNotificationVtbl>::VTBL;
    let noms_notif = [
        "AllocateBufferWithNotification",
        "FreeBufferWithNotification",
        "RegisterNotificationEvent",
        "UnregisterNotificationEvent",
    ];
    let offsets_notif = [
        offset_of!(
            IMiniportWaveRTStreamNotificationVtbl,
            AllocateBufferWithNotification
        ),
        offset_of!(
            IMiniportWaveRTStreamNotificationVtbl,
            FreeBufferWithNotification
        ),
        offset_of!(
            IMiniportWaveRTStreamNotificationVtbl,
            RegisterNotificationEvent
        ),
        offset_of!(
            IMiniportWaveRTStreamNotificationVtbl,
            UnregisterNotificationEvent
        ),
    ];
    for (i, (nom, offset)) in noms_notif.iter().zip(offsets_notif).enumerate() {
        assert_eq!(offset, (11 + i) * SLOT, "slot {} = {nom}", 11 + i);
    }
    assert!(
        n.AllocateBufferWithNotification.is_some()
            && n.FreeBufferWithNotification.is_some()
            && n.RegisterNotificationEvent.is_some()
            && n.UnregisterNotificationEvent.is_some()
    );
    // Deux objets du même type partagent la même vtable.
    let port = faux_port_stream();
    let (f1, _) = flux(&port);
    let (f2, _) = flux(&port);
    let o1 = new_stream_object(f1);
    let o2 = try_new_stream_object(f2).unwrap();
    assert!(ptr::eq(o1.object().vtbl(), o2.object().vtbl()));
}

/// Un IID inconnu de tous ne passe nulle part.
#[test]
fn iid_inconnu() {
    const IID_INCONNU: Guid = Guid::from_u128(0xDEADBEEF_0000_4000_8000_000000000002);
    let port = faux_port_stream();
    let (f, _dropped) = flux(&port);
    let this = new_stream_notification_object(f).into_raw();
    let (status, out) = query_interface(this, &IID_INCONNU);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(out.is_null());
    assert_eq!(release(this), 0);
}
