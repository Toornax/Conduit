//! `IAdapterPowerManagement` : l'objet d'alimentation de l'adaptateur, enregistré auprès
//! de PortCls par `PcRegisterAdapterPowerManagement` (M1a-06).
//!
//! Le pilote implémente [`AdapterPowerManagement`] ; [`new_power_object`] en fait un objet
//! COM dont la vtable est [`PowerVtbl::VTBL`], une **constante associée** engendrée pour
//! chaque type implémenteur (`impl<T: AdapterPowerManagement> PowerVtbl for T`). Ses
//! slots `IUnknown` sont les thunks génériques de [`crate::unknown`], ses slots
//! métier des thunks `extern "C"` qui retrouvent `&T` par `ComObject::inner` puis
//! appellent le trait.

use core::ffi::c_void;

use conduit_com::{ComObject, ComPtr, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls_sys::{
    DEVICE_CAPABILITIES, DEVICE_POWER_STATE, IAdapterPowerManagementVtbl, NTSTATUS,
    PDEVICE_CAPABILITIES, POWER_STATE,
};

use crate::unknown;

/// Contrat de `IAdapterPowerManagement` (`portcls.h`), vu du pilote.
///
/// `Send + Sync + 'static` : PortCls appelle depuis n'importe quel fil et ne donne jamais
/// qu'un `&self` (état mutable par atomiques ou verrou tournant). Les trois méthodes sont
/// appelées à `PASSIVE_LEVEL`, dans le contexte des IRP `IRP_MJ_POWER` que PortCls
/// traite pour l'adaptateur.
pub trait AdapterPowerManagement: Send + Sync + 'static {
    /// `PowerChangeState` : l'adaptateur passe dans l'état `new_state` (`PowerDeviceD0`
    /// à l'allumage, `PowerDeviceD3` à l'extinction). Pas de valeur de retour : le
    /// changement est acquis.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn power_change_state(&self, new_state: DEVICE_POWER_STATE);

    /// `QueryPowerChangeState` : PortCls demande si l'adaptateur accepte de passer en
    /// `new_state`. `STATUS_SUCCESS` (défaut) accepte ; un échec fait refuser la
    /// transition par le gestionnaire d'alimentation.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn query_power_change_state(&self, new_state: DEVICE_POWER_STATE) -> NtStatus {
        let _ = new_state;
        STATUS_SUCCESS
    }

    /// `QueryDeviceCapabilities` : l'adaptateur peut amender les capacités
    /// d'alimentation que PortCls remonte au bus (`DeviceState`, latences…). Le défaut
    /// les laisse telles quelles.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn query_device_capabilities(&self, caps: &mut DEVICE_CAPABILITIES) -> NtStatus {
        let _ = caps;
        STATUS_SUCCESS
    }
}

/// Vtable `IAdapterPowerManagement` d'un type implémenteur, constante associée.
///
/// Implémenté pour tout `T: AdapterPowerManagement` : `T::VTBL` est une valeur `const`
/// sans mutabilité intérieure, donc `&T::VTBL` est promu en `&'static` par le compilateur
/// (promotion des constantes), ce que [`ComObject::new`] exige.
pub trait PowerVtbl {
    /// Vtable complète : thunks `IUnknown` génériques puis thunks métier de `Self`.
    const VTBL: IAdapterPowerManagementVtbl;
}

impl<T: AdapterPowerManagement> PowerVtbl for T {
    const VTBL: IAdapterPowerManagementVtbl = IAdapterPowerManagementVtbl {
        QueryInterface: Some(unknown::query_interface::<IAdapterPowerManagementVtbl, T>),
        AddRef: Some(unknown::add_ref::<IAdapterPowerManagementVtbl, T>),
        Release: Some(unknown::release::<IAdapterPowerManagementVtbl, T>),
        PowerChangeState: Some(power_change_state::<T>),
        QueryPowerChangeState: Some(query_power_change_state::<T>),
        QueryDeviceCapabilities: Some(query_device_capabilities::<T>),
    };
}

/// Objet COM `IAdapterPowerManagement` possédé côté Rust.
pub type PowerObject<T> = ComPtr<IAdapterPowerManagementVtbl, T>;

/// Alloue l'objet COM `IAdapterPowerManagement` de `inner` (compte de références 1).
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_power_object`].
pub fn new_power_object<T: AdapterPowerManagement>(inner: T) -> PowerObject<T> {
    ComObject::new(&T::VTBL, inner)
}

/// Comme [`new_power_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_power_object<T: AdapterPowerManagement>(inner: T) -> Option<PowerObject<T>> {
    ComObject::try_new(&T::VTBL, inner)
}

/// `IAdapterPowerManagement::PowerChangeState`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IAdapterPowerManagementVtbl, T>` vivant pendant l'appel
/// (PortCls détient une référence) ; `new_state` porte un état de périphérique.
unsafe extern "C" fn power_change_state<T: AdapterPowerManagement>(
    this: *mut c_void,
    new_state: POWER_STATE,
) {
    // SAFETY: `this` est un `ComObject<IAdapterPowerManagementVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IAdapterPowerManagementVtbl, T>::inner(this) };
    // SAFETY: `POWER_STATE` est une union `{ SystemState, DeviceState }` de deux entiers
    // C de même taille ; PortCls y passe l'état de périphérique cible (contrat de
    // `IAdapterPowerManagement::PowerChangeState`), toute valeur est un entier lisible.
    let state = unsafe { new_state.DeviceState };
    me.power_change_state(state);
}

/// `IAdapterPowerManagement::QueryPowerChangeState`.
///
/// # Safety
///
/// Mêmes préconditions que [`power_change_state`].
unsafe extern "C" fn query_power_change_state<T: AdapterPowerManagement>(
    this: *mut c_void,
    new_state: POWER_STATE,
) -> NTSTATUS {
    // SAFETY: `this` est un `ComObject<IAdapterPowerManagementVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IAdapterPowerManagementVtbl, T>::inner(this) };
    // SAFETY: voir `power_change_state`.
    let state = unsafe { new_state.DeviceState };
    me.query_power_change_state(state)
}

/// `IAdapterPowerManagement::QueryDeviceCapabilities`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IAdapterPowerManagementVtbl, T>` vivant pendant l'appel ;
/// `caps`, s'il n'est pas nul, pointe une `DEVICE_CAPABILITIES` inscriptible que personne
/// d'autre ne lit ni n'écrit pendant l'appel.
unsafe extern "C" fn query_device_capabilities<T: AdapterPowerManagement>(
    this: *mut c_void,
    caps: PDEVICE_CAPABILITIES,
) -> NTSTATUS {
    // SAFETY: `this` est un `ComObject<IAdapterPowerManagementVtbl, T>` vivant (contrat).
    let me = unsafe { ComObject::<IAdapterPowerManagementVtbl, T>::inner(this) };
    if caps.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `caps` est non nul et pointe une structure inscriptible et exclusive le
    // temps de l'appel (contrat).
    let caps = unsafe { &mut *caps };
    me.query_device_capabilities(caps)
}
