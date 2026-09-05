//! `MiniportWaveRT` vu par un faux PortCls : l'objet construit par `new_wavert_object`
//! est appelé **uniquement à travers sa vtable** (`QueryInterface`, `Init` avec de faux
//! `IResourceList`/`IPortWaveRT`, `GetDescription`, `DataRangeIntersection`, `NewStream`
//! avec un faux `IPortWaveRTStream`, `GetDeviceDescription`, `Release`), et `port_init`
//! est exercé sur un faux `IPort`.

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
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{FAUX_MDL, This, faux_port_stream, query_interface, refcount, release, vtbl_de};
use conduit_com::{
    ComObject, ComRef, Guid, NtStatus, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
    STATUS_SUCCESS, STATUS_UNSUCCESSFUL,
};
use portcls::conduit_com::IID_IUNKNOWN;
use portcls::portcls_sys::com::guid;
use portcls::portcls_sys::{
    _INTERFACE_TYPE, _MEMORY_CACHING_TYPE, BOOLEAN, DEVICE_DESCRIPTION, DEVICE_REGISTRY_PROPERTY,
    IID_IMiniport, IID_IMiniportTopology, IID_IMiniportWaveRT, IID_IMiniportWaveRTStream,
    IID_IMiniportWaveRTStreamNotification, IMiniportWaveRT, IMiniportWaveRTVtbl, IPort, IPortVtbl,
    IPortWaveRT, IPortWaveRTVtbl, IResourceList, IResourceListVtbl, IUnknown, KSDATAFORMAT,
    KSDATARANGE, KSSTATE, NTSTATUS, PCFILTER_DESCRIPTOR, PDEVICE_OBJECT, PIRP, PKSDATAFORMAT, PMDL,
    PMINIPORTWAVERTSTREAM, PPCFILTER_DESCRIPTOR, PPORTWAVERTSTREAM, PRESOURCELIST, PULONG,
    PUNKNOWN, PVOID, ULONG,
};
use portcls::{
    AudioBuffer, MiniportWaveRT, MiniportWaveRTStream, PortWaveRT, PortWaveRTStream, ResourceList,
    STATUS_BUFFER_OVERFLOW, StreamObject, WaveRTVtbl, as_unknown, new_stream_notification_object,
    new_stream_object, new_wavert_object, physical_address, port_init, ref_as_unknown,
    try_new_wavert_object, unknown,
};

const IID_INCONNU: Guid = Guid::from_u128(0xDEADBEEF_0000_4000_8000_000000000003);
const SLOT: usize = 8;

// ---------------------------------------------------------------------------------
// Faux `IResourceList` (vide) et faux `IPortWaveRT` (`GetDeviceProperty` écrit « Faux »).
// ---------------------------------------------------------------------------------

struct FauxRes;

unsafe extern "C" fn faux_number_of_entries(_: This) -> ULONG {
    0
}

static FAUX_RES_VTBL: IResourceListVtbl = IResourceListVtbl {
    QueryInterface: Some(unknown::query_interface::<IResourceListVtbl, FauxRes>),
    AddRef: Some(unknown::add_ref::<IResourceListVtbl, FauxRes>),
    Release: Some(unknown::release::<IResourceListVtbl, FauxRes>),
    NumberOfEntries: Some(faux_number_of_entries),
    NumberOfEntriesOfType: None,
    FindTranslatedEntry: None,
    FindUntranslatedEntry: None,
    AddEntry: None,
    AddEntryFromParent: None,
    TranslatedList: None,
    UntranslatedList: None,
};

struct FauxPort;

const FAUX_PROPRIETE: &[u8] = b"Faux";

unsafe extern "C" fn faux_get_device_property(
    _this: This,
    _property: DEVICE_REGISTRY_PROPERTY::Type,
    len: ULONG,
    buffer: PVOID,
    result: PULONG,
) -> NTSTATUS {
    unsafe { *result = FAUX_PROPRIETE.len() as ULONG };
    if (len as usize) < FAUX_PROPRIETE.len() {
        return portcls::STATUS_BUFFER_TOO_SMALL;
    }
    unsafe {
        ptr::copy_nonoverlapping(FAUX_PROPRIETE.as_ptr(), buffer.cast(), FAUX_PROPRIETE.len())
    };
    STATUS_SUCCESS
}

static FAUX_PORT_VTBL: IPortWaveRTVtbl = IPortWaveRTVtbl {
    QueryInterface: Some(unknown::query_interface::<IPortWaveRTVtbl, FauxPort>),
    AddRef: Some(unknown::add_ref::<IPortWaveRTVtbl, FauxPort>),
    Release: Some(unknown::release::<IPortWaveRTVtbl, FauxPort>),
    Init: None,
    GetDeviceProperty: Some(faux_get_device_property),
    NewRegistryKey: None,
};

// ---------------------------------------------------------------------------------
// Faux `IPort` pour `port_init` : `Init` mémorise ses arguments et rappelle
// `IMiniportWaveRT::Init` du miniport reçu, comme PortCls.
// ---------------------------------------------------------------------------------

struct FauxIPort {
    inits: AtomicU32,
    device_seen: AtomicUsize,
    irp_seen: AtomicUsize,
    adapter_seen: AtomicUsize,
}

unsafe extern "C" fn faux_iport_init(
    this: This,
    device: PDEVICE_OBJECT,
    irp: PIRP,
    miniport: PUNKNOWN,
    adapter: PUNKNOWN,
    resources: PRESOURCELIST,
) -> NTSTATUS {
    let me = unsafe { ComObject::<IPortVtbl, FauxIPort>::inner(this) };
    me.inits.fetch_add(1, Ordering::SeqCst);
    me.device_seen.store(device as usize, Ordering::SeqCst);
    me.irp_seen.store(irp as usize, Ordering::SeqCst);
    me.adapter_seen.store(adapter as usize, Ordering::SeqCst);
    assert!(!miniport.is_null() && !resources.is_null());
    // PortCls appelle `IMiniportWaveRT::Init(adapter, resources, port)` : le port passé
    // est lui-même ; ici on passe le faux `IPortWaveRT` du test faute d'en être un.
    let (status, wavert) = query_interface(miniport.cast(), &guid(&IID_IMiniportWaveRT));
    assert_eq!(status, STATUS_SUCCESS);
    let vt = unsafe { vtbl_de::<IMiniportWaveRTVtbl>(wavert) };
    let port = ComObject::new(&FAUX_PORT_VTBL, FauxPort);
    let status = unsafe { vt.Init.unwrap()(wavert, adapter, resources, port.as_raw().cast()) };
    release(wavert);
    status
}

static FAUX_IPORT_VTBL: IPortVtbl = IPortVtbl {
    QueryInterface: Some(unknown::query_interface::<IPortVtbl, FauxIPort>),
    AddRef: Some(unknown::add_ref::<IPortVtbl, FauxIPort>),
    Release: Some(unknown::release::<IPortVtbl, FauxIPort>),
    Init: Some(faux_iport_init),
    GetDeviceProperty: None,
    NewRegistryKey: None,
};

// ---------------------------------------------------------------------------------
// Le flux que le miniport fabrique.
// ---------------------------------------------------------------------------------

struct Flux {
    port: PortWaveRTStream,
    pin: u32,
    capture: bool,
    dropped: Arc<AtomicUsize>,
}

impl Drop for Flux {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl MiniportWaveRTStream for Flux {
    fn set_state(&self, _: KSSTATE::Type) -> NtStatus {
        STATUS_SUCCESS
    }

    fn position(&self) -> Result<u32, NtStatus> {
        Ok(0)
    }

    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus> {
        let mdl = self
            .port
            .allocate_pages_for_mdl(physical_address(i64::MAX), requested_bytes as usize)?;
        Ok(AudioBuffer {
            mdl,
            actual_bytes: requested_bytes,
            offset_from_first_page: 0,
            cache_type: _MEMORY_CACHING_TYPE::MmCached,
        })
    }

    fn free_audio_buffer(&self, mdl: PMDL, _: u32) {
        unsafe { self.port.free_pages_from_mdl(mdl) }.unwrap();
    }
}

// ---------------------------------------------------------------------------------
// Le miniport sous test.
// ---------------------------------------------------------------------------------

struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

struct Wave {
    inits: AtomicU32,
    /// 0 : pas encore appelé, 1 : `adapter` était `None`, 2 : `Some`.
    adapter_seen: AtomicU8,
    port: Mutex<Option<PortWaveRT>>,
    streams: AtomicU32,
    /// Compte de références du faux `IPortWaveRTStream` observé pendant `new_stream`.
    port_stream_refs_seen: AtomicU32,
    /// Pin refusée par `new_stream` (→ `STATUS_UNSUCCESSFUL`).
    refuse_pin: u32,
    /// Les flux notifient (objet `IMiniportWaveRTStreamNotification`) ou non.
    notifie: bool,
    streams_dropped: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
}

impl Drop for Wave {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// Flux notification : les mêmes méthodes, plus les quatre de notification (vides).
struct FluxNotif(Flux);

impl MiniportWaveRTStream for FluxNotif {
    fn set_state(&self, s: KSSTATE::Type) -> NtStatus {
        self.0.set_state(s)
    }
    fn position(&self) -> Result<u32, NtStatus> {
        self.0.position()
    }
    fn allocate_audio_buffer(&self, n: u32) -> Result<AudioBuffer, NtStatus> {
        self.0.allocate_audio_buffer(n)
    }
    fn free_audio_buffer(&self, mdl: PMDL, n: u32) {
        self.0.free_audio_buffer(mdl, n);
    }
}

impl portcls::MiniportWaveRTStreamNotification for FluxNotif {
    fn allocate_buffer_with_notification(&self, _: u32, n: u32) -> Result<AudioBuffer, NtStatus> {
        self.0.allocate_audio_buffer(n)
    }
    fn free_buffer_with_notification(&self, mdl: PMDL, n: u32) {
        self.0.free_audio_buffer(mdl, n);
    }
    fn register_notification_event(&self, _: portcls::portcls_sys::PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
    fn unregister_notification_event(&self, _: portcls::portcls_sys::PKEVENT) -> NtStatus {
        STATUS_SUCCESS
    }
}

impl MiniportWaveRT for Wave {
    fn init(
        &self,
        adapter: Option<ComRef<IUnknown>>,
        resources: ResourceList,
        port: PortWaveRT,
    ) -> NtStatus {
        self.inits.fetch_add(1, Ordering::SeqCst);
        self.adapter_seen
            .store(if adapter.is_some() { 2 } else { 1 }, Ordering::SeqCst);
        assert_eq!(resources.count(), Ok(0));
        *self.port.lock().unwrap() = Some(port);
        STATUS_SUCCESS
    }

    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        &DESCRIPTION.0
    }

    fn data_range_intersection(
        &self,
        pin_id: u32,
        _: &KSDATARANGE,
        _: &KSDATARANGE,
        _: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        if pin_id == 1 {
            Ok(40)
        } else {
            Err(STATUS_NOT_IMPLEMENTED)
        }
    }

    fn new_stream(
        &self,
        port_stream: PortWaveRTStream,
        pin: u32,
        capture: bool,
        format: &KSDATAFORMAT,
    ) -> Result<StreamObject, NtStatus> {
        assert_eq!(unsafe { format.__bindgen_anon_1.FormatSize }, 64);
        if pin == self.refuse_pin {
            return Err(STATUS_UNSUCCESSFUL);
        }
        self.streams.fetch_add(1, Ordering::SeqCst);
        self.port_stream_refs_seen
            .store(refcount(port_stream.com_ref().as_raw()), Ordering::SeqCst);
        let flux = Flux {
            port: port_stream,
            pin,
            capture,
            dropped: Arc::clone(&self.streams_dropped),
        };
        Ok(if self.notifie {
            new_stream_notification_object(FluxNotif(flux)).into()
        } else {
            StreamObject::from(new_stream_object(flux))
        })
    }
}

fn nouveau(notifie: bool) -> (This, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let streams_dropped = Arc::new(AtomicUsize::new(0));
    let obj = new_wavert_object(Wave {
        inits: AtomicU32::new(0),
        adapter_seen: AtomicU8::new(0),
        port: Mutex::new(None),
        streams: AtomicU32::new(0),
        port_stream_refs_seen: AtomicU32::new(0),
        refuse_pin: 9,
        notifie,
        streams_dropped: Arc::clone(&streams_dropped),
        dropped: Arc::clone(&dropped),
    });
    (obj.into_raw(), dropped, streams_dropped)
}

fn wave(this: This) -> &'static Wave {
    unsafe { ComObject::<IMiniportWaveRTVtbl, Wave>::inner(this) }
}

fn vt(this: This) -> &'static IMiniportWaveRTVtbl {
    unsafe { vtbl_de::<IMiniportWaveRTVtbl>(this) }
}

fn appel_init(this: This, adapter: This, res: This, port: This) -> NTSTATUS {
    unsafe { vt(this).Init.unwrap()(this, adapter.cast(), res.cast(), port.cast()) }
}

fn appel_new_stream(
    this: This,
    out: *mut PMINIPORTWAVERTSTREAM,
    port_stream: PPORTWAVERTSTREAM,
    pin: ULONG,
    capture: BOOLEAN,
    format: PKSDATAFORMAT,
) -> NTSTATUS {
    unsafe { vt(this).NewStream.unwrap()(this, out, port_stream, pin, capture, format) }
}

fn format_64() -> KSDATAFORMAT {
    let mut f = KSDATAFORMAT::default();
    f.__bindgen_anon_1.FormatSize = 64;
    f
}

// ---------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------

#[test]
fn query_interface_iminiport_et_iminiportwavert() {
    let (this, dropped, _) = nouveau(false);

    for iid in [
        IID_IUNKNOWN,
        guid(&IID_IMiniport),
        guid(&IID_IMiniportWaveRT),
    ] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_SUCCESS, "{iid}");
        assert_eq!(out, this, "{iid}");
    }
    assert_eq!(refcount(this), 4);
    for iid in [
        guid(&IID_IMiniportTopology),
        guid(&IID_IMiniportWaveRTStream),
        IID_INCONNU,
    ] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "{iid}");
        assert!(out.is_null());
    }
    release(this);
    release(this);
    release(this);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn init_recoit_le_port_wavert_et_le_conserve() {
    let (this, dropped, _) = nouveau(false);
    let res = ComObject::new(&FAUX_RES_VTBL, FauxRes);
    let port = ComObject::new(&FAUX_PORT_VTBL, FauxPort);

    assert_eq!(
        appel_init(this, ptr::null_mut(), res.as_raw(), port.as_raw()),
        STATUS_SUCCESS
    );
    let w = wave(this);
    assert_eq!(w.inits.load(Ordering::SeqCst), 1);
    assert_eq!(w.adapter_seen.load(Ordering::SeqCst), 1);
    assert_eq!(res.refcount(), 1, "liste lâchée en sortie d'init");
    assert_eq!(port.refcount(), 2, "port conservé");

    // Méthode `IPort` de l'enveloppe `PortWaveRT`.
    let garde = w.port.lock().unwrap();
    let mut tampon = [0u8; 8];
    assert_eq!(
        garde.as_ref().unwrap().get_device_property(
            DEVICE_REGISTRY_PROPERTY::DevicePropertyFriendlyName,
            &mut tampon
        ),
        Ok(4)
    );
    assert_eq!(&tampon[..4], b"Faux");
    drop(garde);

    // Paramètres nuls refusés sans toucher au trait.
    assert_eq!(
        appel_init(this, ptr::null_mut(), ptr::null_mut(), port.as_raw()),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(
        appel_init(this, ptr::null_mut(), res.as_raw(), ptr::null_mut()),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(w.inits.load(Ordering::SeqCst), 1);

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(port.refcount(), 1);
}

#[test]
fn new_stream_rend_un_pointeur_possede_qui_repond_a_iminiportwavertstream() {
    for notifie in [false, true] {
        let (this, _dropped, streams_dropped) = nouveau(notifie);
        let port_stream = faux_port_stream();
        let mut format = format_64();

        let mut out: PMINIPORTWAVERTSTREAM = ptr::NonNull::dangling().as_ptr();
        assert_eq!(
            appel_new_stream(
                this,
                &mut out,
                port_stream.as_raw().cast(),
                1,
                1,
                &mut format
            ),
            STATUS_SUCCESS
        );
        assert!(!out.is_null());
        let w = wave(this);
        assert_eq!(w.streams.load(Ordering::SeqCst), 1);
        assert_eq!(
            w.port_stream_refs_seen.load(Ordering::SeqCst),
            2,
            "pendant new_stream : notre ComPtr + la ComRef du flux"
        );
        assert_eq!(port_stream.refcount(), 2, "le flux garde le port stream");

        // Le pointeur rendu est un objet COM valide : QI vers `IMiniportWaveRTStream`
        // marche, vers la notification seulement si le flux notifie.
        let flux: This = out.cast();
        let (status, p) = query_interface(flux, &guid(&IID_IMiniportWaveRTStream));
        assert_eq!(status, STATUS_SUCCESS);
        assert_eq!(p, flux);
        release(p);
        let (status, _) = query_interface(flux, &guid(&IID_IMiniportWaveRTStreamNotification));
        assert_eq!(
            status,
            if notifie {
                STATUS_SUCCESS
            } else {
                STATUS_INVALID_PARAMETER
            }
        );
        if notifie {
            release(flux);
        }
        assert_eq!(refcount(flux), 1, "PortCls détient l'unique référence");

        // Le flux vit : `AllocateAudioBuffer` passe par le faux port stream.
        let svt = unsafe { vtbl_de::<portcls::portcls_sys::IMiniportWaveRTStreamVtbl>(flux) };
        let mut mdl: PMDL = ptr::null_mut();
        let (mut actual, mut offset, mut cache) = (0u32, 0u32, 0);
        assert_eq!(
            unsafe {
                svt.AllocateAudioBuffer.unwrap()(
                    flux,
                    4096,
                    &mut mdl,
                    &mut actual,
                    &mut offset,
                    &mut cache,
                )
            },
            STATUS_SUCCESS
        );
        assert_eq!(mdl as usize, FAUX_MDL);
        assert_eq!(port_stream.allocs.load(Ordering::SeqCst), 1);
        unsafe { svt.FreeAudioBuffer.unwrap()(flux, mdl, actual) };
        assert_eq!(port_stream.frees.load(Ordering::SeqCst), 1);

        // `Release` détruit le flux une fois, qui rend le port stream.
        assert_eq!(release(flux), 0);
        assert_eq!(streams_dropped.load(Ordering::SeqCst), 1);
        assert_eq!(port_stream.refcount(), 1);

        assert_eq!(release(this), 0);
    }
}

#[test]
fn new_stream_erreurs_laissent_la_sortie_nulle() {
    let (this, _dropped, streams_dropped) = nouveau(false);
    let port_stream = faux_port_stream();
    let mut format = format_64();

    // Pin refusée par le trait : statut transmis, `*Stream` nul, port stream lâché.
    let mut out: PMINIPORTWAVERTSTREAM = ptr::NonNull::dangling().as_ptr();
    assert_eq!(
        appel_new_stream(
            this,
            &mut out,
            port_stream.as_raw().cast(),
            9,
            0,
            &mut format
        ),
        STATUS_UNSUCCESSFUL
    );
    assert!(out.is_null());
    assert_eq!(port_stream.refcount(), 1);
    assert_eq!(wave(this).streams.load(Ordering::SeqCst), 0);

    // Sortie nulle.
    assert_eq!(
        appel_new_stream(
            this,
            ptr::null_mut(),
            port_stream.as_raw().cast(),
            1,
            0,
            &mut format
        ),
        STATUS_INVALID_PARAMETER
    );
    // Port stream nul.
    out = ptr::NonNull::dangling().as_ptr();
    assert_eq!(
        appel_new_stream(this, &mut out, ptr::null_mut(), 1, 0, &mut format),
        STATUS_INVALID_PARAMETER
    );
    assert!(out.is_null());
    // Format nul.
    out = ptr::NonNull::dangling().as_ptr();
    assert_eq!(
        appel_new_stream(
            this,
            &mut out,
            port_stream.as_raw().cast(),
            1,
            0,
            ptr::null_mut()
        ),
        STATUS_INVALID_PARAMETER
    );
    assert!(out.is_null());
    assert_eq!(wave(this).streams.load(Ordering::SeqCst), 0);
    assert_eq!(port_stream.refcount(), 1);
    assert_eq!(streams_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(this), 0);
}

#[test]
fn new_stream_transmet_pin_et_capture() {
    let (this, _dropped, _) = nouveau(false);
    let port_stream = faux_port_stream();
    let mut format = format_64();
    let mut out: PMINIPORTWAVERTSTREAM = ptr::null_mut();
    assert_eq!(
        appel_new_stream(
            this,
            &mut out,
            port_stream.as_raw().cast(),
            3,
            1,
            &mut format
        ),
        STATUS_SUCCESS
    );
    let f = unsafe {
        ComObject::<portcls::portcls_sys::IMiniportWaveRTStreamVtbl, Flux>::inner(out.cast())
    };
    assert_eq!((f.pin, f.capture), (3, true));
    release(out.cast());

    assert_eq!(
        appel_new_stream(
            this,
            &mut out,
            port_stream.as_raw().cast(),
            0,
            0,
            &mut format
        ),
        STATUS_SUCCESS
    );
    let f = unsafe {
        ComObject::<portcls::portcls_sys::IMiniportWaveRTStreamVtbl, Flux>::inner(out.cast())
    };
    assert_eq!((f.pin, f.capture), (0, false));
    release(out.cast());
    assert_eq!(release(this), 0);
}

#[test]
fn get_device_description_par_defaut_comme_sysvad() {
    let (this, _dropped, _) = nouveau(false);
    // `DmaChannel` doit être remis à zéro.
    let mut desc = DEVICE_DESCRIPTION {
        DmaChannel: 77,
        ..DEVICE_DESCRIPTION::default()
    };
    assert_eq!(
        unsafe { vt(this).GetDeviceDescription.unwrap()(this, &mut desc) },
        STATUS_SUCCESS
    );
    assert_eq!(desc.Master, 1);
    assert_eq!(desc.ScatterGather, 1);
    assert_eq!(desc.Dma32BitAddresses, 1);
    assert_eq!(desc.Dma64BitAddresses, 0);
    assert_eq!(desc.InterfaceType, _INTERFACE_TYPE::PCIBus);
    assert_eq!(desc.MaximumLength, 0xFFFF_FFFF);
    assert_eq!(desc.DmaChannel, 0);
    assert_eq!(desc.Version, 0);
    assert_eq!(
        unsafe { vt(this).GetDeviceDescription.unwrap()(this, ptr::null_mut()) },
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(release(this), 0);
}

#[test]
fn get_description_et_data_range_intersection_herites() {
    let (this, _dropped, _) = nouveau(false);
    let mut out: PPCFILTER_DESCRIPTOR = ptr::NonNull::dangling().as_ptr();
    assert_eq!(
        unsafe { vt(this).GetDescription.unwrap()(this, &mut out) },
        STATUS_SUCCESS
    );
    assert!(ptr::eq(out, &DESCRIPTION.0));

    let mut client = KSDATARANGE::default();
    let mut my = KSDATARANGE::default();
    let mut len: ULONG = 0;
    let dri = vt(this).DataRangeIntersection.unwrap();
    assert_eq!(
        unsafe { dri(this, 1, &mut client, &mut my, 0, ptr::null_mut(), &mut len) },
        STATUS_BUFFER_OVERFLOW
    );
    assert_eq!(len, 40);
    assert_eq!(
        unsafe { dri(this, 2, &mut client, &mut my, 0, ptr::null_mut(), &mut len) },
        STATUS_NOT_IMPLEMENTED
    );
    assert_eq!(release(this), 0);
}

#[test]
fn port_init_appelle_iport_init_et_le_miniport_recoit_son_init() {
    let (this, dropped, _) = nouveau(false);
    let miniport = unsafe { conduit_com::ComPtr::<IMiniportWaveRTVtbl, Wave>::from_raw(this) };
    let port = ComObject::new(
        &FAUX_IPORT_VTBL,
        FauxIPort {
            inits: AtomicU32::new(0),
            device_seen: AtomicUsize::new(0),
            irp_seen: AtomicUsize::new(0),
            adapter_seen: AtomicUsize::new(0),
        },
    );
    let res = ComObject::new(&FAUX_RES_VTBL, FauxRes);

    let port_ref: ComRef<IPort> = unsafe { ComRef::from_raw_add_ref(port.as_raw().cast()) };
    let res_ref = ResourceList::from_ref(unsafe {
        ComRef::<IResourceList>::from_raw_add_ref(res.as_raw().cast())
    });
    let mini_unknown = as_unknown(&miniport);
    assert_eq!(miniport.refcount(), 2, "as_unknown prend une référence");
    let port_unknown = ref_as_unknown(&port_ref);
    assert_eq!(port.refcount(), 3);

    let device = 0x1000 as PDEVICE_OBJECT;
    let irp = 0x2000 as PIRP;
    let status = unsafe { port_init(&port_ref, device, irp, &mini_unknown, None, &res_ref) };
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(port.inits.load(Ordering::SeqCst), 1);
    assert_eq!(port.device_seen.load(Ordering::SeqCst), 0x1000);
    assert_eq!(port.irp_seen.load(Ordering::SeqCst), 0x2000);
    assert_eq!(port.adapter_seen.load(Ordering::SeqCst), 0);
    assert_eq!(
        miniport.inits.load(Ordering::SeqCst),
        1,
        "Init du miniport rappelé"
    );
    assert_eq!(miniport.adapter_seen.load(Ordering::SeqCst), 1);

    // Avec un adaptateur (n'importe quel `IUnknown`).
    let status = unsafe {
        port_init(
            &port_ref,
            device,
            irp,
            &mini_unknown,
            Some(&port_unknown),
            &res_ref,
        )
    };
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        port.adapter_seen.load(Ordering::SeqCst),
        port.as_raw() as usize
    );
    assert_eq!(miniport.adapter_seen.load(Ordering::SeqCst), 2);

    drop(mini_unknown);
    drop(port_unknown);
    drop(port_ref);
    drop(res_ref);
    assert_eq!(port.refcount(), 1);
    assert_eq!(res.refcount(), 1);
    assert_eq!(miniport.refcount(), 1);
    drop(miniport);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Ordre des slots : mêmes noms et même ordre que `portcls-sys/tests/vtables.rs`
/// (`ordre_des_slots_iminiportwavert`), sur la vtable remplie par le crate.
#[test]
fn ordre_des_slots_et_vtable_par_type() {
    assert_eq!(offset_of!(IMiniportWaveRT, lpVtbl), 0);
    assert_eq!(offset_of!(IPortWaveRT, lpVtbl), 0);
    let noms = [
        "QueryInterface",
        "AddRef",
        "Release",
        "GetDescription",
        "DataRangeIntersection",
        "Init",
        "NewStream",
        "GetDeviceDescription",
    ];
    let offsets = [
        offset_of!(IMiniportWaveRTVtbl, QueryInterface),
        offset_of!(IMiniportWaveRTVtbl, AddRef),
        offset_of!(IMiniportWaveRTVtbl, Release),
        offset_of!(IMiniportWaveRTVtbl, GetDescription),
        offset_of!(IMiniportWaveRTVtbl, DataRangeIntersection),
        offset_of!(IMiniportWaveRTVtbl, Init),
        offset_of!(IMiniportWaveRTVtbl, NewStream),
        offset_of!(IMiniportWaveRTVtbl, GetDeviceDescription),
    ];
    for (i, (nom, offset)) in noms.iter().zip(offsets).enumerate() {
        assert_eq!(offset, i * SLOT, "slot {i} = {nom}");
    }
    let v = &<Wave as WaveRTVtbl>::VTBL;
    assert!(
        v.QueryInterface.is_some()
            && v.AddRef.is_some()
            && v.Release.is_some()
            && v.GetDescription.is_some()
            && v.DataRangeIntersection.is_some()
            && v.Init.is_some()
            && v.NewStream.is_some()
            && v.GetDeviceDescription.is_some()
    );

    let (this, _d1, _) = nouveau(false);
    let (this2, _d2, _) = nouveau(true);
    let mot1 = unsafe { *this.cast::<*const IMiniportWaveRTVtbl>() };
    let mot2 = unsafe { *this2.cast::<*const IMiniportWaveRTVtbl>() };
    assert!(ptr::eq(mot1, mot2), "même type, même vtable promue");
    release(this);
    release(this2);

    let obj = try_new_wavert_object(Wave {
        inits: AtomicU32::new(0),
        adapter_seen: AtomicU8::new(0),
        port: Mutex::new(None),
        streams: AtomicU32::new(0),
        port_stream_refs_seen: AtomicU32::new(0),
        refuse_pin: 9,
        notifie: false,
        streams_dropped: Arc::new(AtomicUsize::new(0)),
        dropped: Arc::new(AtomicUsize::new(0)),
    })
    .unwrap();
    fn exige<T: Send + Sync>(_: &T) {}
    exige(&obj);
    assert_eq!(obj.refcount(), 1);
}
