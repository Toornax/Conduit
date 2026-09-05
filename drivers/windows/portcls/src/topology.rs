//! `IMiniportTopology` : le miniport topologie d'un câble (nœuds volume/mute, jack,
//! bridge vers le miniport WaveRT ; driver-design.md §4), un par sens.
//!
//! Le pilote implémente [`MiniportTopology`] ; [`new_topology_object`] en fait l'objet COM
//! que `PcNewPort(IID_IPortTopology)` puis `IPort::Init` recevront (M1a-06). La vtable est
//! la constante associée [`TopologyVtbl::VTBL`] (même technique que
//! [`crate::power`]) : slots `IUnknown` génériques, puis `GetDescription`,
//! `DataRangeIntersection` (hérités de `IMiniport`) et `Init`, dans l'ordre de
//! `portcls.h`. `QueryInterface` répond à `IID_IUnknown`, `IID_IMiniport` et
//! `IID_IMiniportTopology` (`IIDS` de `portcls_sys::com`).

use core::ffi::c_void;
use core::slice;

use conduit_com::{
    ComObject, ComPtr, ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
    STATUS_SUCCESS,
};
use portcls_sys::{
    IMiniportTopologyVtbl, IUnknown, KSDATARANGE, NTSTATUS, PCFILTER_DESCRIPTOR, PKSDATARANGE,
    PPCFILTER_DESCRIPTOR, PPORTTOPOLOGY, PRESOURCELIST, PULONG, PUNKNOWN, PVOID, ULONG,
};

use crate::received::{PortTopology, ResourceList};
use crate::status::{STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL};
use crate::unknown;

/// Contrat de `IMiniportTopology` (`portcls.h`), vu du pilote.
///
/// `Send + Sync + 'static` : PortCls appelle depuis n'importe quel fil et ne donne jamais
/// qu'un `&self`. L'objet est créé par le pilote dans `StartDevice`, puis PortCls en
/// devient le principal détenteur (le port garde une référence jusqu'à l'arrêt du
/// sous-périphérique).
pub trait MiniportTopology: Send + Sync + 'static {
    /// `Init` : PortCls initialise le miniport après `IPort::Init`. `adapter` est
    /// l'`IUnknown` de l'adaptateur passé à `PcNewPort`/`IPort::Init` (souvent nul),
    /// `resources` la liste de ressources matérielles (vide pour un périphérique racine),
    /// `port` le port topologie qui possède ce miniport ; le miniport peut conserver
    /// ces références (elles lui appartiennent) ou les lâcher en sortie.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn init(
        &self,
        adapter: Option<ComRef<IUnknown>>,
        resources: ResourceList,
        port: PortTopology,
    ) -> NtStatus;

    /// `GetDescription` : le descripteur de filtre KS (pins, nœuds, connexions,
    /// catégories). PortCls **conserve le pointeur** pour toute la vie du filtre, d'où
    /// `'static` : une `static` du pilote, jamais une valeur construite à la volée.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR;

    /// `DataRangeIntersection` : trouve un format commun entre la plage `client` demandée
    /// et la plage `my` du miniport pour la pin `pin_id`, et l'écrit dans `out` (tampon de
    /// PortCls, `None` s'il n'en fournit pas). Renvoie **la taille requise du format**,
    /// écrit ou non : le thunk répond `STATUS_SUCCESS` si `out` a pu le recevoir,
    /// `STATUS_BUFFER_OVERFLOW` si `out` est absent ou vide (interrogation de taille),
    /// `STATUS_BUFFER_TOO_SMALL` s'il est trop petit, toujours avec la taille dans
    /// `ResultantFormatLength`.
    ///
    /// Le défaut, `Err(STATUS_NOT_IMPLEMENTED)`, laisse PortCls faire l'intersection
    /// lui-même : c'est ce que fait un miniport topologie (pas de pin de données).
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
}

/// Vtable `IMiniportTopology` d'un type implémenteur, constante associée (voir
/// [`PowerVtbl`](crate::power::PowerVtbl)).
pub trait TopologyVtbl {
    /// Vtable complète : thunks `IUnknown` génériques puis thunks métier de `Self`.
    const VTBL: IMiniportTopologyVtbl;
}

impl<T: MiniportTopology> TopologyVtbl for T {
    const VTBL: IMiniportTopologyVtbl = IMiniportTopologyVtbl {
        QueryInterface: Some(unknown::query_interface::<IMiniportTopologyVtbl, T>),
        AddRef: Some(unknown::add_ref::<IMiniportTopologyVtbl, T>),
        Release: Some(unknown::release::<IMiniportTopologyVtbl, T>),
        GetDescription: Some(get_description::<T>),
        DataRangeIntersection: Some(data_range_intersection::<T>),
        Init: Some(init::<T>),
    };
}

/// Objet COM `IMiniportTopology` possédé côté Rust.
pub type TopologyObject<T> = ComPtr<IMiniportTopologyVtbl, T>;

/// Alloue l'objet COM `IMiniportTopology` de `inner` (compte de références 1).
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_topology_object`].
pub fn new_topology_object<T: MiniportTopology>(inner: T) -> TopologyObject<T> {
    ComObject::new(&T::VTBL, inner)
}

/// Comme [`new_topology_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_topology_object<T: MiniportTopology>(inner: T) -> Option<TopologyObject<T>> {
    ComObject::try_new(&T::VTBL, inner)
}

/// `IMiniport::GetDescription`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportTopologyVtbl, T>` vivant pendant l'appel ;
/// `description`, s'il n'est pas nul, pointe un emplacement de pointeur inscriptible.
unsafe extern "C" fn get_description<T: MiniportTopology>(
    this: *mut c_void,
    description: *mut PPCFILTER_DESCRIPTOR,
) -> NTSTATUS {
    if description.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<IMiniportTopologyVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IMiniportTopologyVtbl, T>::inner(this) };
    let desc: *const PCFILTER_DESCRIPTOR = me.description();
    // SAFETY: `description` est non nul et inscriptible (contrat). Le prototype C veut un
    // `PPCFILTER_DESCRIPTOR` (non `const`) mais PortCls ne fait que lire le descripteur :
    // le `cast_mut` ne donne lieu à aucune écriture.
    unsafe { *description = desc.cast_mut() };
    STATUS_SUCCESS
}

/// `IMiniport::DataRangeIntersection`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportTopologyVtbl, T>` vivant pendant l'appel ;
/// `data_range` et `matching` pointent des `KSDATARANGE` lisibles ; `out`, s'il n'est
/// pas nul, pointe `out_len` octets inscriptibles ; `result_len` pointe un `ULONG`
/// inscriptible.
unsafe extern "C" fn data_range_intersection<T: MiniportTopology>(
    this: *mut c_void,
    pin_id: ULONG,
    data_range: PKSDATARANGE,
    matching: PKSDATARANGE,
    out_len: ULONG,
    out: PVOID,
    result_len: PULONG,
) -> NTSTATUS {
    if data_range.is_null() || matching.is_null() || result_len.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let Ok(available) = usize::try_from(out_len) else {
        return STATUS_INVALID_PARAMETER;
    };
    // SAFETY: `this` est un `ComObject<IMiniportTopologyVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IMiniportTopologyVtbl, T>::inner(this) };
    // SAFETY: `data_range` est non nul et pointe une `KSDATARANGE` lisible le temps de
    // l'appel (contrat).
    let client = unsafe { &*data_range };
    // SAFETY: idem pour `matching`.
    let my = unsafe { &*matching };
    let buffer: Option<&mut [u8]> = if out.is_null() {
        None
    } else {
        // SAFETY: `out` est non nul et pointe `out_len` octets inscriptibles, exclusifs
        // le temps de l'appel (contrat) ; `u8` n'a pas de contrainte d'alignement.
        Some(unsafe { slice::from_raw_parts_mut(out.cast::<u8>(), available) })
    };
    let has_buffer = buffer.is_some();

    match me.data_range_intersection(pin_id, client, my, buffer) {
        Ok(needed) => {
            // SAFETY: `result_len` est non nul et inscriptible (contrat).
            unsafe { *result_len = needed };
            if has_buffer && needed <= out_len {
                STATUS_SUCCESS
            } else if out_len == 0 {
                STATUS_BUFFER_OVERFLOW
            } else {
                STATUS_BUFFER_TOO_SMALL
            }
        }
        Err(status) => status,
    }
}

/// `IMiniportTopology::Init`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportTopologyVtbl, T>` vivant pendant l'appel ;
/// `adapter`, `resources` et `port`, s'ils ne sont pas nuls, pointent des objets COM
/// vivants sur lesquels PortCls détient une référence pendant l'appel.
unsafe extern "C" fn init<T: MiniportTopology>(
    this: *mut c_void,
    adapter: PUNKNOWN,
    resources: PRESOURCELIST,
    port: PPORTTOPOLOGY,
) -> NTSTATUS {
    // SAFETY: `this` est un `ComObject<IMiniportTopologyVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IMiniportTopologyVtbl, T>::inner(this) };
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
        PortTopology::from_ref(port),
    )
}
