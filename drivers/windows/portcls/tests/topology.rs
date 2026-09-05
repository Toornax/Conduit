//! `MiniportTopology` vu par un faux PortCls : l'objet construit par
//! `new_topology_object` est appelé **uniquement à travers sa vtable** (`QueryInterface`,
//! `Init` avec de faux `IResourceList`/`IPortTopology` eux-mêmes bâtis sur `ComObject`,
//! `GetDescription`, `DataRangeIntersection`, `Release`), comme le ferait le noyau.

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

use common::{This, add_ref, query_interface, refcount, release, vtbl_de};
use conduit_com::{
    ComObject, ComRef, Guid, NtStatus, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
    STATUS_SUCCESS,
};
use portcls::conduit_com::IID_IUNKNOWN;
use portcls::portcls_sys::com::guid;
use portcls::portcls_sys::{
    DEVICE_REGISTRY_PROPERTY, IID_IAdapterPowerManagement, IID_IMiniport, IID_IMiniportTopology,
    IMiniportTopology, IMiniportTopologyVtbl, IPortTopology, IPortTopologyVtbl, IResourceList,
    IResourceListVtbl, IUnknown, KSDATARANGE, NTSTATUS, PCFILTER_DESCRIPTOR, PPCFILTER_DESCRIPTOR,
    PULONG, PVOID, ULONG,
};
use portcls::{
    MiniportTopology, PortTopology, ResourceList, STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL,
    TopologyVtbl, new_topology_object, try_new_topology_object, unknown,
};

const IID_INCONNU: Guid = Guid::from_u128(0xDEADBEEF_0000_4000_8000_000000000001);

// ---------------------------------------------------------------------------------
// Faux `IResourceList` : `NumberOfEntries` renvoie une valeur fixe, le reste est vide.
// ---------------------------------------------------------------------------------

struct FauxRes {
    entries: u32,
}

unsafe extern "C" fn faux_number_of_entries(this: This) -> ULONG {
    let me = unsafe { ComObject::<IResourceListVtbl, FauxRes>::inner(this) };
    me.entries
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

fn faux_res(entries: u32) -> conduit_com::ComPtr<IResourceListVtbl, FauxRes> {
    ComObject::new(&FAUX_RES_VTBL, FauxRes { entries })
}

// ---------------------------------------------------------------------------------
// Faux `IPortTopology` : `GetDeviceProperty` écrit « Faux » ; `Init`, `NewRegistryKey`
// vides.
// ---------------------------------------------------------------------------------

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
        return STATUS_BUFFER_TOO_SMALL;
    }
    unsafe {
        ptr::copy_nonoverlapping(FAUX_PROPRIETE.as_ptr(), buffer.cast(), FAUX_PROPRIETE.len())
    };
    STATUS_SUCCESS
}

static FAUX_PORT_VTBL: IPortTopologyVtbl = IPortTopologyVtbl {
    QueryInterface: Some(unknown::query_interface::<IPortTopologyVtbl, FauxPort>),
    AddRef: Some(unknown::add_ref::<IPortTopologyVtbl, FauxPort>),
    Release: Some(unknown::release::<IPortTopologyVtbl, FauxPort>),
    Init: None,
    GetDeviceProperty: Some(faux_get_device_property),
    NewRegistryKey: None,
};

fn faux_port() -> conduit_com::ComPtr<IPortTopologyVtbl, FauxPort> {
    ComObject::new(&FAUX_PORT_VTBL, FauxPort)
}

// ---------------------------------------------------------------------------------
// Le miniport sous test.
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour
/// la `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}

/// Descripteur zéro-initialisé (entiers à 0, pointeurs nuls : valeur valide).
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

struct Topo {
    inits: AtomicU32,
    /// `resources.count()` vu pendant `init`.
    res_count_seen: AtomicU32,
    /// Compte de références du faux `IResourceList` observé pendant `init`.
    res_refs_during_init: AtomicU32,
    /// 0 : pas encore appelé, 1 : `adapter` était `None`, 2 : `Some`.
    adapter_seen: AtomicU8,
    /// Le port est conservé : sa référence vit aussi longtemps que le miniport.
    port: Mutex<Option<PortTopology>>,
    dropped: Arc<AtomicUsize>,
}

impl Drop for Topo {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl MiniportTopology for Topo {
    fn init(
        &self,
        adapter: Option<ComRef<IUnknown>>,
        resources: ResourceList,
        port: PortTopology,
    ) -> NtStatus {
        self.inits.fetch_add(1, Ordering::SeqCst);
        self.adapter_seen
            .store(if adapter.is_some() { 2 } else { 1 }, Ordering::SeqCst);
        // Les références reçues sont valides : appel de slot, et sonde du compte.
        self.res_count_seen
            .store(resources.count().unwrap(), Ordering::SeqCst);
        self.res_refs_during_init
            .store(refcount(resources.com_ref().as_raw()), Ordering::SeqCst);
        assert_eq!(
            resources.count_of_type(1),
            Err(STATUS_NOT_IMPLEMENTED),
            "slot vide → STATUS_NOT_IMPLEMENTED"
        );
        *self.port.lock().unwrap() = Some(port);
        STATUS_SUCCESS
    }

    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        &DESCRIPTION.0
    }
}

fn nouveau() -> (This, Arc<AtomicUsize>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let obj = new_topology_object(Topo {
        inits: AtomicU32::new(0),
        res_count_seen: AtomicU32::new(u32::MAX),
        res_refs_during_init: AtomicU32::new(0),
        adapter_seen: AtomicU8::new(0),
        port: Mutex::new(None),
        dropped: Arc::clone(&dropped),
    });
    (obj.into_raw(), dropped)
}

/// `&Topo` derrière un `this` vivant (pour lire les compteurs depuis le test).
fn topo(this: This) -> &'static Topo {
    unsafe { ComObject::<IMiniportTopologyVtbl, Topo>::inner(this) }
}

fn appel_init(this: This, adapter: This, res: This, port: This) -> NTSTATUS {
    let slot = unsafe { vtbl_de::<IMiniportTopologyVtbl>(this) }
        .Init
        .expect("slot Init");
    unsafe { slot(this, adapter.cast(), res.cast(), port.cast()) }
}

fn appel_get_description(this: This, out: *mut PPCFILTER_DESCRIPTOR) -> NTSTATUS {
    let slot = unsafe { vtbl_de::<IMiniportTopologyVtbl>(this) }
        .GetDescription
        .expect("slot GetDescription");
    unsafe { slot(this, out) }
}

#[allow(clippy::too_many_arguments)]
fn appel_intersection(
    this: This,
    client: *mut KSDATARANGE,
    my: *mut KSDATARANGE,
    out: Option<&mut [u8]>,
    result_len: PULONG,
) -> NTSTATUS {
    let slot = unsafe { vtbl_de::<IMiniportTopologyVtbl>(this) }
        .DataRangeIntersection
        .expect("slot DataRangeIntersection");
    let (ptr, len) = match out {
        Some(b) => (b.as_mut_ptr().cast::<core::ffi::c_void>(), b.len() as ULONG),
        None => (ptr::null_mut(), 0),
    };
    unsafe { slot(this, 7, client, my, len, ptr, result_len) }
}

// ---------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------

#[test]
fn query_interface_iminiport_et_iminiporttopology_renvoient_this() {
    let (this, dropped) = nouveau();

    for iid in [
        IID_IUNKNOWN,
        guid(&IID_IMiniport),
        guid(&IID_IMiniportTopology),
    ] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_SUCCESS, "{iid}");
        assert_eq!(out, this, "{iid}");
    }
    // 1 (création) + 3.
    assert_eq!(refcount(this), 4);

    for iid in [guid(&IID_IAdapterPowerManagement), IID_INCONNU] {
        let (status, out) = query_interface(this, &iid);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "{iid}");
        assert!(out.is_null(), "{iid}");
    }
    assert_eq!(refcount(this), 4);

    assert_eq!(release(this), 3);
    assert_eq!(release(this), 2);
    assert_eq!(release(this), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(release(this), 0);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "Drop exactement une fois"
    );
}

#[test]
fn init_recoit_des_comref_valides_et_les_comptes_suivent() {
    let (this, dropped) = nouveau();
    let res = faux_res(3);
    let port = faux_port();
    assert_eq!(res.refcount(), 1);
    assert_eq!(port.refcount(), 1);

    let status = appel_init(this, ptr::null_mut(), res.as_raw(), port.as_raw());
    assert_eq!(status, STATUS_SUCCESS);

    let t = topo(this);
    assert_eq!(t.inits.load(Ordering::SeqCst), 1);
    assert_eq!(
        t.adapter_seen.load(Ordering::SeqCst),
        1,
        "adapter nul → None"
    );
    assert_eq!(
        t.res_count_seen.load(Ordering::SeqCst),
        3,
        "NumberOfEntries par la vtable"
    );
    assert_eq!(
        t.res_refs_during_init.load(Ordering::SeqCst),
        2,
        "pendant init : notre ComPtr + la ComRef du miniport"
    );
    assert_eq!(res.refcount(), 1, "la liste est lâchée en sortie d'init");
    assert_eq!(port.refcount(), 2, "le port est conservé par le miniport");

    // Le port conservé sert encore : méthode sûre d'une enveloppe reçue.
    let garde = t.port.lock().unwrap();
    let port_recu = garde.as_ref().unwrap();
    let mut tampon = [0u8; 8];
    assert_eq!(
        port_recu.get_device_property(
            DEVICE_REGISTRY_PROPERTY::DevicePropertyFriendlyName,
            &mut tampon
        ),
        Ok(4)
    );
    assert_eq!(&tampon[..4], b"Faux");
    let mut petit = [0u8; 2];
    assert_eq!(
        port_recu.get_device_property(
            DEVICE_REGISTRY_PROPERTY::DevicePropertyFriendlyName,
            &mut petit
        ),
        Err(STATUS_BUFFER_TOO_SMALL)
    );
    drop(garde);

    // Release final : Drop du miniport, qui rend sa référence sur le port.
    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(port.refcount(), 1);
    assert_eq!(res.refcount(), 1);
}

#[test]
fn init_avec_adaptateur_et_parametres_nuls() {
    let (this, dropped) = nouveau();
    let res = faux_res(0);
    let port = faux_port();
    // N'importe quel objet COM sert d'`IUnknown` adaptateur : le faux port.
    let adaptateur = faux_port();

    assert_eq!(
        appel_init(this, adaptateur.as_raw(), res.as_raw(), port.as_raw()),
        STATUS_SUCCESS
    );
    let t = topo(this);
    assert_eq!(
        t.adapter_seen.load(Ordering::SeqCst),
        2,
        "adapter non nul → Some"
    );
    assert_eq!(
        adaptateur.refcount(),
        1,
        "la ComRef adaptateur est lâchée en sortie d'init"
    );
    assert_eq!(t.res_count_seen.load(Ordering::SeqCst), 0);

    // Liste ou port nul : refus sans toucher au trait, et sans fuite de référence.
    assert_eq!(
        appel_init(this, ptr::null_mut(), ptr::null_mut(), port.as_raw()),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(
        appel_init(this, ptr::null_mut(), res.as_raw(), ptr::null_mut()),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(t.inits.load(Ordering::SeqCst), 1);
    assert_eq!(res.refcount(), 1);
    assert_eq!(port.refcount(), 2);

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(port.refcount(), 1);
}

#[test]
fn get_description_renvoie_l_adresse_de_la_static() {
    let (this, dropped) = nouveau();

    let mut out: PPCFILTER_DESCRIPTOR = ptr::NonNull::dangling().as_ptr();
    assert_eq!(appel_get_description(this, &mut out), STATUS_SUCCESS);
    assert!(
        ptr::eq(out, &DESCRIPTION.0),
        "le pointeur est celui de la static"
    );
    assert_eq!(unsafe { (*out).PinCount }, 0);

    assert_eq!(
        appel_get_description(this, ptr::null_mut()),
        STATUS_INVALID_PARAMETER
    );

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn data_range_intersection_par_defaut_non_implementee() {
    let (this, dropped) = nouveau();
    let mut client = KSDATARANGE::default();
    let mut my = KSDATARANGE::default();
    let mut len: ULONG = 99;
    let mut tampon = [0u8; 16];

    assert_eq!(
        appel_intersection(this, &mut client, &mut my, Some(&mut tampon), &mut len),
        STATUS_NOT_IMPLEMENTED
    );
    assert_eq!(len, 99, "longueur non touchée en cas d'erreur");

    // Pointeurs obligatoires nuls : refus avant d'appeler le trait.
    assert_eq!(
        appel_intersection(this, ptr::null_mut(), &mut my, None, &mut len),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(
        appel_intersection(this, &mut client, ptr::null_mut(), None, &mut len),
        STATUS_INVALID_PARAMETER
    );
    assert_eq!(
        appel_intersection(this, &mut client, &mut my, None, ptr::null_mut()),
        STATUS_INVALID_PARAMETER
    );

    assert_eq!(release(this), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

/// Miniport qui implémente l'intersection : « format » de 4 octets `0xAB`.
struct TopoFormat;

impl MiniportTopology for TopoFormat {
    fn init(&self, _: Option<ComRef<IUnknown>>, _: ResourceList, _: PortTopology) -> NtStatus {
        STATUS_SUCCESS
    }

    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        &DESCRIPTION.0
    }

    fn data_range_intersection(
        &self,
        pin_id: u32,
        _client: &KSDATARANGE,
        _my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        assert_eq!(pin_id, 7);
        if let Some(out) = out {
            if out.len() >= 4 {
                out[..4].fill(0xAB);
            }
        }
        Ok(4)
    }
}

#[test]
fn data_range_intersection_traduit_result_en_status_et_longueur() {
    let this = new_topology_object(TopoFormat).into_raw();
    let mut client = KSDATARANGE::default();
    let mut my = KSDATARANGE::default();
    let mut len: ULONG = 0;

    // Interrogation de taille : pas de tampon.
    assert_eq!(
        appel_intersection(this, &mut client, &mut my, None, &mut len),
        STATUS_BUFFER_OVERFLOW
    );
    assert_eq!(len, 4);

    // Tampon présent mais vide : idem.
    let mut vide = [0u8; 0];
    len = 0;
    assert_eq!(
        appel_intersection(this, &mut client, &mut my, Some(&mut vide), &mut len),
        STATUS_BUFFER_OVERFLOW
    );
    assert_eq!(len, 4);

    // Trop petit.
    let mut petit = [0u8; 2];
    len = 0;
    assert_eq!(
        appel_intersection(this, &mut client, &mut my, Some(&mut petit), &mut len),
        STATUS_BUFFER_TOO_SMALL
    );
    assert_eq!(len, 4);
    assert_eq!(petit, [0, 0], "rien d'écrit");

    // Assez grand : écrit, succès, longueur = 4.
    let mut grand = [0u8; 8];
    len = 0;
    assert_eq!(
        appel_intersection(this, &mut client, &mut my, Some(&mut grand), &mut len),
        STATUS_SUCCESS
    );
    assert_eq!(len, 4);
    assert_eq!(grand, [0xAB, 0xAB, 0xAB, 0xAB, 0, 0, 0, 0]);

    assert_eq!(release(this), 0);
}

#[test]
fn dispositions_et_vtable_par_type() {
    assert_eq!(offset_of!(IMiniportTopology, lpVtbl), 0);
    assert_eq!(offset_of!(IResourceList, lpVtbl), 0);
    assert_eq!(offset_of!(IPortTopology, lpVtbl), 0);

    let (this, _dropped) = nouveau();
    // Le premier mot de l'objet est le pointeur de vtable, et c'est bien `Topo::VTBL`.
    let premier_mot = unsafe { *this.cast::<*const IMiniportTopologyVtbl>() };
    let obj = unsafe { conduit_com::ComPtr::<IMiniportTopologyVtbl, Topo>::from_raw(this) };
    assert!(ptr::eq(premier_mot, obj.object().vtbl()));
    // Tous les slots sont remplis (COM ne connaît pas de slot vide).
    let vt: &IMiniportTopologyVtbl = obj.object().vtbl();
    assert!(vt.QueryInterface.is_some() && vt.AddRef.is_some() && vt.Release.is_some());
    assert!(vt.GetDescription.is_some() && vt.DataRangeIntersection.is_some() && vt.Init.is_some());
    assert!(<Topo as TopologyVtbl>::VTBL.Init.is_some());
    // Un second objet du même type partage la même vtable (constante promue).
    let (this2, _dropped2) = nouveau();
    let mot2 = unsafe { *this2.cast::<*const IMiniportTopologyVtbl>() };
    assert!(ptr::eq(premier_mot, mot2));
    assert_eq!(release(this2), 0);
    // `TopoFormat` a la sienne.
    let autre = new_topology_object(TopoFormat);
    assert!(!ptr::eq(premier_mot, autre.object().vtbl()));
}

#[test]
fn try_new_et_comptr_cote_rust() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let obj = try_new_topology_object(Topo {
        inits: AtomicU32::new(0),
        res_count_seen: AtomicU32::new(0),
        res_refs_during_init: AtomicU32::new(0),
        adapter_seen: AtomicU8::new(0),
        port: Mutex::new(None),
        dropped: Arc::clone(&dropped),
    })
    .expect("allocation en mode utilisateur");
    fn exige<T: Send + Sync>(_: &T) {}
    exige(&obj);
    assert_eq!(obj.refcount(), 1);
    assert_eq!(add_ref(obj.as_raw()), 2);
    assert_eq!(release(obj.as_raw()), 1);
    drop(obj);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
