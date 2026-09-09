//! `IAdapterPowerManagement` : l'objet d'alimentation de l'adaptateur, enregistré auprès
//! de PortCls par [`crate::adapter::register_adapter_power_management`] (M1a-06 pour
//! l'enveloppe, M1b-06 pour l'enregistrement et son premier implémenteur).
//!
//! Le pilote implémente [`AdapterPowerManagement`] ; [`new_power_object`] en fait un objet
//! COM dont la vtable est [`PowerVtbl::VTBL`], une **constante associée** engendrée pour
//! chaque type implémenteur (`impl<T: AdapterPowerManagement> PowerVtbl for T`). Ses
//! slots `IUnknown` sont les thunks génériques de [`crate::unknown`], ses slots
//! métier des thunks `extern "C"` qui retrouvent `&T` par `ComObject::inner` puis
//! appellent le trait.
//!
//! # L'IRQL des trois méthodes n'est **pas** documenté
//!
//! Les pages des trois rappels — comme celle d'`IPowerNotify::PowerChangeNotify` — n'ont
//! **aucune ligne IRQL**. La seule contrainte écrite est « The code for this method must
//! reside in paged memory », qui exclut `DISPATCH_LEVEL` sans l'énoncer. En amont, l'IRQL
//! d'un `IRP_MN_SET_POWER` est conditionnel : `PASSIVE_LEVEL` si la pile porte
//! `DO_POWER_PAGABLE`, `DISPATCH_LEVEL` si elle porte `DO_POWER_INRUSH` — et la
//! documentation ne dit pas lequel PortCls pose sur le FDO qu'il crée dans
//! `PcAddAdapterDevice`.
//!
//! `PASSIVE_LEVEL` est donc la lecture de bon sens, pas une garantie. Chaque méthode le
//! répète à sa façon ; un implémenteur qui ferait quelque chose d'interdit à
//! `DISPATCH_LEVEL` — attendre, toucher de la mémoire paginée, allouer en pool paginé —
//! s'appuierait sur une supposition, et devrait l'écrire. `conduit_kmd::power` a fait le
//! choix inverse : tout ce qu'il fait reste valide jusqu'à `DISPATCH_LEVEL`.

use core::ffi::c_void;

use conduit_com::{ComObject, ComPtr, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls_sys::{
    _DEVICE_POWER_STATE, DEVICE_CAPABILITIES, DEVICE_POWER_STATE, IAdapterPowerManagementVtbl,
    NTSTATUS, PDEVICE_CAPABILITIES, POWER_STATE,
};

use crate::unknown;

/// Vrai si `state` est l'état **allumé** `PowerDeviceD0`, faux pour tout le reste.
///
/// C'est la seule décision que le pilote tire d'un `PowerChangeState`, et elle est ici
/// plutôt que dans `conduit-kmd` pour une raison unique : `conduit-kmd` ne se teste pas
/// en mode utilisateur (`wdk-sys` lie les bibliothèques noyau même sous `cargo test`).
/// Le *branchement* — désarmer, réarmer — reste dans le pilote et n'est vérifiable qu'en
/// machine ; la *règle*, elle, est éprouvée par `portcls/tests/power.rs`.
///
/// # Pourquoi « tout le reste » et pas « `D3` »
///
/// `_DEVICE_POWER_STATE` compte six valeurs (`PowerDeviceUnspecified` 0, `D0` 1, `D1` 2,
/// `D2` 3, `D3` 4, `PowerDeviceMaximum` 5) et le champ reçu est un `c_int` : rien
/// n'interdit à un état inconnu d'arriver, et une énumération C ne se referme pas. Tester
/// l'égalité à `D0` plutôt que l'appartenance à `{D1, D2, D3}` fait que le cas par défaut
/// est le **prudent** — on n'entretient pas une activité périodique dans un état dont on
/// ne sait rien —, jamais une panique ni une transition ignorée. Un périphérique audio
/// PortCls ne voit en pratique que `D0` et `D3` : `D1` et `D2` ne sont pas dans les
/// capacités d'un périphérique racine sans matériel.
#[must_use]
pub const fn is_powered(state: DEVICE_POWER_STATE) -> bool {
    state == _DEVICE_POWER_STATE::PowerDeviceD0
}

/// Contrat de `IAdapterPowerManagement` (`portcls.h`), vu du pilote.
///
/// `Send + Sync + 'static` : PortCls appelle depuis n'importe quel fil et ne donne jamais
/// qu'un `&self` (état mutable par atomiques ou verrou tournant). Les trois méthodes sont
/// appelées dans le contexte des IRP `IRP_MJ_POWER` et `IRP_MN_QUERY_CAPABILITIES` que
/// PortCls traite pour l'adaptateur ; leur IRQL n'est pas documenté (voir l'en-tête de
/// module).
pub trait AdapterPowerManagement: Send + Sync + 'static {
    /// `PowerChangeState` : l'adaptateur passe dans l'état `new_state` (`PowerDeviceD0`
    /// à l'allumage, `PowerDeviceD3` à l'extinction). Pas de valeur de retour : le
    /// changement est acquis, et la documentation est catégorique — « **This call must
    /// not fail** ».
    ///
    /// Deux points du contrat que l'implémenteur doit connaître, tous deux documentés :
    ///
    /// - le changement doit être **acquis au retour** (« The miniport driver must perform
    ///   the requested change to the device's power state before it returns ») ;
    /// - **PortCls met les flux actifs en pause avant** cet appel à la descente, et ne les
    ///   relance qu'**après** à la montée. L'implémenteur n'a donc pas à arrêter les flux
    ///   lui-même : c'est déjà fait quand il est appelé.
    ///
    /// C'est `new_state.DeviceState` que le thunk extrait — le seul champ de l'union que
    /// la documentation déclare valide pour cette interface ; les états système
    /// n'arrivent qu'à `IAdapterPowerManagement2::PowerChangeState2`, que ce module
    /// n'enveloppe pas.
    ///
    /// IRQL : non documenté (voir l'en-tête de module).
    fn power_change_state(&self, new_state: DEVICE_POWER_STATE);

    /// `QueryPowerChangeState` : PortCls demande si l'adaptateur accepte de passer en
    /// `new_state`, sur réception d'un `IRP_MN_QUERY_POWER`. `STATUS_SUCCESS` (défaut)
    /// accepte ; « The driver can deny the power state change by returning a value other
    /// than STATUS_SUCCESS ».
    ///
    /// # Ce qu'il faut refuser : rien que la documentation nomme
    ///
    /// Aucune page ne donne de critère de refus, et deux avertissements documentés
    /// invitent à ne pas s'y fier : « A call to QueryPowerStateChange is **not guaranteed
    /// to occur prior to all PowerChangeState calls** », et, côté gestionnaire
    /// d'alimentation, « Although a driver might fail a system query-power IRP, the power
    /// manager **might still change the system power state to a sleep state** ». Un veto
    /// n'est donc ni obligatoire ni sûr : la logique doit de toute façon supporter la
    /// transition, puisque `power_change_state` n'a pas le droit d'échouer. D'où le défaut
    /// qui accepte tout — c'est le comportement correct pour un pilote qui n'a pas
    /// d'opération matérielle irréversible en cours.
    ///
    /// IRQL : non documenté (voir l'en-tête de module).
    fn query_power_change_state(&self, new_state: DEVICE_POWER_STATE) -> NtStatus {
        let _ = new_state;
        STATUS_SUCCESS
    }

    /// `QueryDeviceCapabilities` : l'adaptateur peut amender la correspondance entre
    /// états système et états de périphérique que PortCls remonte au bus, sur réception
    /// d'un `IRP_MN_QUERY_CAPABILITIES`. PortCls a déjà écrit les valeurs par défaut dans
    /// la structure ; le défaut du trait les laisse telles quelles.
    ///
    /// # Deux raisons de ne pas surcharger
    ///
    /// La documentation le déconseille d'emblée (« **Typically, the adapter driver should
    /// not change these settings** ») et borne ce qui est permis : une correspondance ne
    /// peut être déplacée que vers un état **plus** éteint, jamais vers un état plus
    /// allumé.
    ///
    /// Surtout, la méthode n'est appelée **que si l'objet a été enregistré avant le
    /// démarrage** : « The operating system queries devices **before** calling the adapter
    /// driver's device-startup routine », d'où la note du WDK — enregistrer « in or before
    /// your AddDevice() function » si l'on veut remplir la structure. Un pilote qui
    /// s'enregistre depuis `StartDevice` — le cas de Conduit, et celui que la page
    /// « Implementing IAdapterPowerManagement » décrit — ne verra jamais cet appel.
    ///
    /// IRQL : non documenté (voir l'en-tête de module).
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
