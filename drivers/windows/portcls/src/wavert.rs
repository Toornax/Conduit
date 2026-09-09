//! `IMiniportWaveRT` : le miniport WaveRT d'un câble (filtre rendu ou capture ;
//! driver-design.md §4), un par sens, qui fabrique les flux ([`crate::stream`]).
//!
//! Le pilote implémente [`MiniportWaveRT`] ; [`new_wavert_object`] en fait l'objet COM
//! que `PcNewPort(CLSID_PortWaveRT)` puis `IPort::Init` recevront (M1a-06,
//! [`crate::adapter`]). La vtable est la constante associée [`WaveRTVtbl::VTBL`] : slots
//! `IUnknown` génériques, `GetDescription` et `DataRangeIntersection` hérités de
//! `IMiniport` (thunks de [`crate::miniport`]), puis `Init`, `NewStream`,
//! `GetDeviceDescription`, dans l'ordre de `portcls.h` 26100
//! (`portcls-sys/tests/vtables.rs`). `QueryInterface` répond à `IID_IUnknown`,
//! `IID_IMiniport` et `IID_IMiniportWaveRT`.
//!
//! # Flux rendus à PortCls
//!
//! `NewStream` rend **un pointeur `IMiniportWaveRTStream` possédé** par PortCls : le trait
//! renvoie un [`StreamObject`], pointeur COM à type effacé construit depuis l'objet typé
//! ([`StreamPtr<T>`](crate::stream::StreamPtr) ou
//! [`StreamNotificationPtr<T>`](crate::stream::StreamNotificationPtr)) ; le thunk le
//! transfère tel quel (`into_raw`) dans `*Stream`. PortCls interroge ensuite l'objet
//! (`QueryInterface(IID_IMiniportWaveRTStreamNotification)`) pour savoir s'il notifie.

use core::ffi::c_void;

use conduit_com::{
    ComObject, ComPtr, ComRef, ComVtable, NtStatus, STATUS_INVALID_PARAMETER,
    STATUS_NOT_IMPLEMENTED, STATUS_SUCCESS,
};
use portcls_sys::{
    _INTERFACE_TYPE, BOOLEAN, DEVICE_DESCRIPTION, IMiniportWaveRTStream,
    IMiniportWaveRTStreamNotificationVtbl, IMiniportWaveRTStreamVtbl, IMiniportWaveRTVtbl,
    IUnknown, KSDATAFORMAT, KSDATARANGE, NTSTATUS, PCFILTER_DESCRIPTOR, PDEVICE_DESCRIPTION,
    PKSDATAFORMAT, PMINIPORTWAVERTSTREAM, PPORTWAVERT, PPORTWAVERTSTREAM, PRESOURCELIST, PUNKNOWN,
    ULONG,
};

use crate::miniport::{self, MiniportSlots};
use crate::property::TargetVtbl;
use crate::received::{PortWaveRT, PortWaveRTStream, ResourceList};
use crate::unknown;

/// Pointeur COM **possédé**, à type effacé, sur un flux créé par le pilote : ce que
/// [`MiniportWaveRT::new_stream`] rend et que le thunk `NewStream` transfère à PortCls.
///
/// Se construit depuis un objet typé par [`From`] (`StreamObject::from(ptr)` ou
/// `ptr.into()`), pour `StreamPtr<T>` comme pour `StreamNotificationPtr<T>` : la vtable
/// `IMiniportWaveRTStreamNotificationVtbl` a celle de `IMiniportWaveRTStream` pour
/// préfixe, l'objet est donc un `IMiniportWaveRTStream` valide. Lâché sans être
/// transféré, il fait `Release` (l'objet est détruit si PortCls n'en a pas pris).
#[derive(Debug)]
pub struct StreamObject(ComRef<IMiniportWaveRTStream>);

/// Vtables acceptées par [`StreamObject`] (scellé : les deux vtables de flux).
///
/// # Safety
///
/// La vtable a `IMiniportWaveRTStreamVtbl` pour préfixe et son `QueryInterface` répond à
/// `IID_IMiniportWaveRTStream`.
pub unsafe trait StreamVtable: ComVtable {}

// SAFETY: c'est la vtable `IMiniportWaveRTStream` elle-même ; `IIDS` la liste.
unsafe impl StreamVtable for IMiniportWaveRTStreamVtbl {}
// SAFETY: les onze premiers slots sont ceux de `IMiniportWaveRTStreamVtbl`
// (`portcls-sys/tests/vtables.rs`) et `IIDS` liste `IID_IMiniportWaveRTStream`.
unsafe impl StreamVtable for IMiniportWaveRTStreamNotificationVtbl {}

impl<V: StreamVtable, T: Send + Sync> From<ComPtr<V, T>> for StreamObject {
    fn from(ptr: ComPtr<V, T>) -> Self {
        let raw: *mut IMiniportWaveRTStream = ptr.into_raw().cast();
        // SAFETY: `raw` vient d'un `ComPtr` vivant qui cède sa référence ; l'objet commence
        // par un pointeur de vtable dont la forme est `IMiniportWaveRTStreamVtbl` ou une
        // extension (contrat `StreamVtable`), c'est donc un `IMiniportWaveRTStream`.
        Self(unsafe { ComRef::from_raw_owned(raw) })
    }
}

impl StreamObject {
    /// Adopte une référence déjà comptée sur un flux (objet rendu par un autre chemin).
    pub fn from_ref(r: ComRef<IMiniportWaveRTStream>) -> Self {
        Self(r)
    }

    /// La référence sous-jacente.
    pub fn com_ref(&self) -> &ComRef<IMiniportWaveRTStream> {
        &self.0
    }

    /// Cède la référence à l'appelant (PortCls, par `*Stream`) : pas de `Release`.
    pub fn into_raw(self) -> PMINIPORTWAVERTSTREAM {
        self.0.into_raw()
    }
}

/// Contrat de `IMiniportWaveRT` (`portcls.h`), vu du pilote.
///
/// `Send + Sync + 'static` : PortCls appelle depuis n'importe quel fil et ne donne jamais
/// qu'un `&self`. L'objet est créé par le pilote dans `StartDevice`, puis PortCls en
/// devient le principal détenteur (le port garde une référence jusqu'à l'arrêt du
/// sous-périphérique). Toutes les méthodes sont à `PASSIVE_LEVEL`.
pub trait MiniportWaveRT: Send + Sync + 'static {
    /// `Init` : PortCls initialise le miniport après `IPort::Init`. `adapter` est
    /// l'`IUnknown` de l'adaptateur passé à `IPort::Init` (souvent nul), `resources` la
    /// liste de ressources matérielles (vide pour un périphérique racine), `port` le port
    /// WaveRT qui possède ce miniport ; le miniport peut conserver ces références ou les
    /// lâcher en sortie.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn init(
        &self,
        adapter: Option<ComRef<IUnknown>>,
        resources: ResourceList,
        port: PortWaveRT,
    ) -> NtStatus;

    /// `GetDescription` : le descripteur de filtre KS (pins rendu/capture et bridge,
    /// nœuds, connexions, catégories). PortCls **conserve le pointeur** pour toute la vie
    /// du filtre, d'où `'static` : une `static` du pilote.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR;

    /// `DataRangeIntersection` : même contrat que
    /// [`MiniportTopology::data_range_intersection`](crate::topology::MiniportTopology::data_range_intersection),
    /// **extension des plages comprise**.
    ///
    /// Le défaut, `Err(STATUS_NOT_IMPLEMENTED)`, laisse PortCls intersecter lui-même les
    /// `KSDATARANGE_AUDIO` du descripteur. Son gestionnaire par défaut est limité — « *only
    /// PCM data formats* », « *only mono and stereo audio streams* », aucun format contenant
    /// un `WAVEFORMATEXTENSIBLE` : un miniport qui déclare du flottant ou plus de deux
    /// canaux doit écrire le sien.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn data_range_intersection(
        &self,
        pin_id: u32,
        client: &KSDATARANGE,
        my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        let _ = (pin_id, client, my, out);
        Err(STATUS_NOT_IMPLEMENTED)
    }

    /// `NewStream` : crée le flux de la pin `pin` (`capture` : `TRUE` pour une pin de
    /// capture) au format `format` (`KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]`, à lire par
    /// `FormatSize`), avec `port_stream`, l'objet d'aide qui allouera son tampon
    /// cyclique (à conserver dans le flux). Le flux naît à `KSSTATE_STOP`, position 0.
    ///
    /// Renvoie l'objet COM du flux, dont PortCls prend la référence ; en cas d'erreur, le
    /// thunk laisse `*Stream` nul et renvoie le statut (`STATUS_INVALID_PARAMETER` pour
    /// un format refusé, `STATUS_INSUFFICIENT_RESOURCES`…).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn new_stream(
        &self,
        port_stream: PortWaveRTStream,
        pin: u32,
        capture: bool,
        format: &KSDATAFORMAT,
    ) -> Result<StreamObject, NtStatus>;

    /// `GetDeviceDescription` : la `DEVICE_DESCRIPTION` DMA que PortCls attend même d'un
    /// périphérique sans DMA. Le défaut, [`default_device_description`], reprend les
    /// champs usuels de SYSVAD (`Master`, `ScatterGather`, `Dma32BitAddresses`,
    /// `InterfaceType = PCIBus`, `MaximumLength = 0xFFFFFFFF`) sur une structure mise à
    /// zéro.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn device_description(&self, out: &mut DEVICE_DESCRIPTION) -> NtStatus {
        default_device_description(out);
        STATUS_SUCCESS
    }
}

/// Remplit `out` comme SYSVAD (`CMiniportWaveRT::GetDeviceDescription`) : tout à zéro
/// puis `Master = TRUE`, `ScatterGather = TRUE`, `Dma32BitAddresses = TRUE`,
/// `InterfaceType = PCIBus`, `MaximumLength = 0xFFFFFFFF`.
pub fn default_device_description(out: &mut DEVICE_DESCRIPTION) {
    *out = DEVICE_DESCRIPTION::default();
    out.Master = 1;
    out.ScatterGather = 1;
    out.Dma32BitAddresses = 1;
    out.InterfaceType = _INTERFACE_TYPE::PCIBus;
    out.MaximumLength = ULONG::MAX;
}

impl<T: MiniportWaveRT> MiniportSlots<T> for IMiniportWaveRTVtbl {
    fn description(me: &T) -> &'static PCFILTER_DESCRIPTOR {
        me.description()
    }

    fn data_range_intersection(
        me: &T,
        pin_id: u32,
        client: &KSDATARANGE,
        my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        me.data_range_intersection(pin_id, client, my, out)
    }
}

/// Vtable `IMiniportWaveRT` d'un type implémenteur, constante associée (voir
/// [`PowerVtbl`](crate::power::PowerVtbl)).
pub trait WaveRTVtbl {
    /// Vtable complète : thunks `IUnknown` génériques puis thunks métier de `Self`.
    const VTBL: IMiniportWaveRTVtbl;
}

impl<T: MiniportWaveRT> WaveRTVtbl for T {
    const VTBL: IMiniportWaveRTVtbl = IMiniportWaveRTVtbl {
        QueryInterface: Some(unknown::query_interface::<IMiniportWaveRTVtbl, T>),
        AddRef: Some(unknown::add_ref::<IMiniportWaveRTVtbl, T>),
        Release: Some(unknown::release::<IMiniportWaveRTVtbl, T>),
        GetDescription: Some(miniport::get_description::<IMiniportWaveRTVtbl, T>),
        DataRangeIntersection: Some(miniport::data_range_intersection::<IMiniportWaveRTVtbl, T>),
        Init: Some(init::<T>),
        NewStream: Some(new_stream::<T>),
        GetDeviceDescription: Some(get_device_description::<T>),
    };
}

/// Objet COM `IMiniportWaveRT` possédé côté Rust.
pub type WaveRTObject<T> = ComPtr<IMiniportWaveRTVtbl, T>;

/// **Unique** occurrence de `&T::VTBL` pour `IMiniportWaveRT` : même rôle, mêmes raisons
/// et même `#[inline(never)]` obligatoire que
/// [`topology::vtbl_of`](crate::topology::vtbl_of), dont la documentation détaille le
/// piège de la constante promue.
#[inline(never)]
pub fn vtbl_of<T: MiniportWaveRT>() -> &'static IMiniportWaveRTVtbl {
    &T::VTBL
}

// SAFETY: `vtbl_of::<T>()` est l'unique occurrence de `&T::VTBL` du crate pour cette
// vtable, et `#[inline(never)]` lui garantit une allocation unique ; c'est elle que les
// deux constructeurs ci-dessous passent à `ComObject`. L'adresse rendue est donc
// exactement celle que porte tout `ComObject<IMiniportWaveRTVtbl, T>` vivant, et aucun
// objet d'un autre type ne la porte.
unsafe impl<T: MiniportWaveRT> TargetVtbl<T> for IMiniportWaveRTVtbl {
    fn vtbl() -> &'static Self {
        vtbl_of::<T>()
    }
}

/// Alloue l'objet COM `IMiniportWaveRT` de `inner` (compte de références 1).
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_wavert_object`].
pub fn new_wavert_object<T: MiniportWaveRT>(inner: T) -> WaveRTObject<T> {
    ComObject::new(vtbl_of::<T>(), inner)
}

/// Comme [`new_wavert_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_wavert_object<T: MiniportWaveRT>(inner: T) -> Option<WaveRTObject<T>> {
    ComObject::try_new(vtbl_of::<T>(), inner)
}

/// `IMiniportWaveRT::Init`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportWaveRTVtbl, T>` vivant pendant l'appel ;
/// `adapter`, `resources` et `port`, s'ils ne sont pas nuls, pointent des objets COM
/// vivants sur lesquels PortCls détient une référence pendant l'appel.
unsafe extern "C" fn init<T: MiniportWaveRT>(
    this: *mut c_void,
    adapter: PUNKNOWN,
    resources: PRESOURCELIST,
    port: PPORTWAVERT,
) -> NTSTATUS {
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTVtbl, T>::inner(this) };
    // PortCls garde ses propres références : on prend les nôtres (`AddRef`), que les
    // enveloppes rendront au `Drop`.
    // SAFETY: `adapter` est nul ou pointe un objet vivant (contrat).
    let adapter = unsafe { ComRef::<IUnknown>::try_from_raw_add_ref(adapter) };
    // SAFETY: `resources` est nul ou pointe un objet vivant (contrat).
    let Some(resources) = (unsafe { ComRef::try_from_raw_add_ref(resources) }) else {
        return STATUS_INVALID_PARAMETER;
    };
    // SAFETY: `port` est nul ou pointe un objet vivant (contrat).
    let Some(port) = (unsafe { ComRef::try_from_raw_add_ref(port) }) else {
        return STATUS_INVALID_PARAMETER;
    };
    me.init(
        adapter,
        ResourceList::from_ref(resources),
        PortWaveRT::from_ref(port),
    )
}

/// `IMiniportWaveRT::NewStream`.
///
/// Contrat de sortie : `*stream` reçoit le pointeur possédé du flux en cas de succès,
/// nul sinon ; `stream`, `port_stream` et `format` nuls → `STATUS_INVALID_PARAMETER`
/// sans appeler le trait.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportWaveRTVtbl, T>` vivant pendant l'appel ;
/// `stream`, s'il n'est pas nul, pointe un emplacement de pointeur inscriptible ;
/// `port_stream`, s'il n'est pas nul, pointe un objet COM vivant sur lequel PortCls
/// détient une référence pendant l'appel ; `format`, s'il n'est pas nul, pointe une
/// `KSDATAFORMAT` lisible (suivie de son extension `FormatSize`).
unsafe extern "C" fn new_stream<T: MiniportWaveRT>(
    this: *mut c_void,
    stream: *mut PMINIPORTWAVERTSTREAM,
    port_stream: PPORTWAVERTSTREAM,
    pin: ULONG,
    capture: BOOLEAN,
    format: PKSDATAFORMAT,
) -> NTSTATUS {
    if stream.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `stream` est non nul et inscriptible (contrat) : nul tant que rien n'est
    // rendu.
    unsafe { *stream = core::ptr::null_mut() };
    if format.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTVtbl, T>::inner(this) };
    // SAFETY: `port_stream` est nul ou pointe un objet vivant (contrat).
    let Some(port_stream) = (unsafe { ComRef::try_from_raw_add_ref(port_stream) }) else {
        return STATUS_INVALID_PARAMETER;
    };
    // SAFETY: `format` est non nul et lisible le temps de l'appel (contrat).
    let format = unsafe { &*format };
    match me.new_stream(
        PortWaveRTStream::from_ref(port_stream),
        pin,
        capture != 0,
        format,
    ) {
        Ok(object) => {
            // SAFETY: `stream` est non nul et inscriptible (contrat) ; la référence du
            // `StreamObject` est transférée à PortCls.
            unsafe { *stream = object.into_raw() };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// `IMiniportWaveRT::GetDeviceDescription`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportWaveRTVtbl, T>` vivant pendant l'appel ;
/// `out`, s'il n'est pas nul, pointe une `DEVICE_DESCRIPTION` inscriptible et exclusive
/// le temps de l'appel.
unsafe extern "C" fn get_device_description<T: MiniportWaveRT>(
    this: *mut c_void,
    out: PDEVICE_DESCRIPTION,
) -> NTSTATUS {
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTVtbl, T>::inner(this) };
    // SAFETY: `out` est non nul, inscriptible et exclusif (contrat).
    let out = unsafe { &mut *out };
    me.device_description(out)
}
