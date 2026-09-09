//! `IMiniportTopology` : le miniport topologie d'un câble (nœuds volume/mute, jack,
//! bridge vers le miniport WaveRT ; driver-design.md §4), un par sens.
//!
//! Le pilote implémente [`MiniportTopology`] ; [`new_topology_object`] en fait l'objet COM
//! que `PcNewPort(IID_IPortTopology)` puis `IPort::Init` recevront (M1a-06). La vtable est
//! la constante associée [`TopologyVtbl::VTBL`] (même technique que
//! [`crate::power`]) : slots `IUnknown` génériques, puis `GetDescription`,
//! `DataRangeIntersection` (hérités de `IMiniport`, thunks de [`crate::miniport`]) et
//! `Init`, dans l'ordre de `portcls.h`. `QueryInterface` répond à `IID_IUnknown`,
//! `IID_IMiniport` et `IID_IMiniportTopology` (`IIDS` de `portcls_sys::com`).

use core::ffi::c_void;

use conduit_com::{
    ComObject, ComPtr, ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
};
use portcls_sys::{
    IMiniportTopologyVtbl, IUnknown, KSDATARANGE, NTSTATUS, PCFILTER_DESCRIPTOR, PPORTTOPOLOGY,
    PRESOURCELIST, PUNKNOWN,
};

use crate::miniport::{self, MiniportSlots};
use crate::property::TargetVtbl;
use crate::received::{PortTopology, ResourceList};
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
    /// # Ce que les deux références couvrent
    ///
    /// `client` et `my` sont des `KSDATARANGE` **suivies de leur extension** :
    /// `FormatSize` octets lisibles à partir de leur adresse, comme la `KSDATAFORMAT` de
    /// `NewStream`. Le type de la référence n'en décrit que les 64 premiers ; un
    /// gestionnaire qui veut les champs d'une `KSDATARANGE_AUDIO` doit donc d'abord
    /// vérifier que `FormatSize` les couvre, puis élargir le pointeur — une plage de 64
    /// octets avec des jokers est parfaitement licite côté client.
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

impl<T: MiniportTopology> MiniportSlots<T> for IMiniportTopologyVtbl {
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
        GetDescription: Some(miniport::get_description::<IMiniportTopologyVtbl, T>),
        DataRangeIntersection: Some(miniport::data_range_intersection::<IMiniportTopologyVtbl, T>),
        Init: Some(init::<T>),
    };
}

/// Objet COM `IMiniportTopology` possédé côté Rust.
pub type TopologyObject<T> = ComPtr<IMiniportTopologyVtbl, T>;

/// **Unique** occurrence de `&T::VTBL` pour `IMiniportTopology` : l'adresse que porte tout
/// `ComObject<IMiniportTopologyVtbl, T>` du crate.
///
/// C'est le point d'appui de la garde de vtable de [`crate::property::handler`], et il
/// demande deux précautions, l'une et l'autre nécessaires :
///
/// 1. **Une seule occurrence de `&T::VTBL` dans tout le crate**, ici. `T::VTBL` est une
///    constante associée ; `&T::VTBL` est donc une **constante promue**, et deux
///    occurrences dans deux fonctions monomorphisées différentes ne sont pas garanties de
///    partager une allocation. Les deux constructeurs ci-dessous et
///    [`TargetVtbl::vtbl`](crate::property::TargetVtbl::vtbl) passent tous par ici.
/// 2. **`#[inline(never)]`**, et ce n'est pas une optimisation : une constante promue est
///    émise en `unnamed_addr`, c'est-à-dire que le compilateur a le droit d'en dupliquer
///    l'allocation. Si cette fonction est inlinée, chaque unité de génération de code peut
///    recevoir sa propre copie de la vtable, et la comparaison d'adresses de la garde
///    échoue alors sur des objets pourtant légitimes — pilote muet, aucune propriété KS ne
///    répond. Le cas est réel : sans cet attribut, `cargo test -p portcls --release`
///    (opt-level 3 + LTO, le profil du pilote) fait tomber douze des tests de
///    `tests/property.rs` avec `STATUS_INVALID_DEVICE_REQUEST`, alors qu'en `dev`, faute
///    d'inlining, tout passe. `#[inline(never)]` garde une définition unique de la
///    fonction, donc une seule allocation promue, et tous les appelants reçoivent la même
///    adresse.
#[inline(never)]
pub fn vtbl_of<T: MiniportTopology>() -> &'static IMiniportTopologyVtbl {
    &T::VTBL
}

// SAFETY: `vtbl_of::<T>()` est l'unique occurrence de `&T::VTBL` du crate pour cette
// vtable, et `#[inline(never)]` lui garantit une allocation unique ; c'est elle que les
// deux constructeurs ci-dessous passent à `ComObject`. L'adresse rendue est donc
// exactement celle que porte tout `ComObject<IMiniportTopologyVtbl, T>` vivant, et aucun
// objet d'un autre type ne la porte.
unsafe impl<T: MiniportTopology> TargetVtbl<T> for IMiniportTopologyVtbl {
    fn vtbl() -> &'static Self {
        vtbl_of::<T>()
    }
}

/// Alloue l'objet COM `IMiniportTopology` de `inner` (compte de références 1).
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_topology_object`].
pub fn new_topology_object<T: MiniportTopology>(inner: T) -> TopologyObject<T> {
    ComObject::new(vtbl_of::<T>(), inner)
}

/// Comme [`new_topology_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_topology_object<T: MiniportTopology>(inner: T) -> Option<TopologyObject<T>> {
    ComObject::try_new(vtbl_of::<T>(), inner)
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
