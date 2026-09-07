//! Forme des objets COM générés en mode C : `lpVtbl` à l'offset 0 de chaque interface,
//! vtables plates commençant par les trois slots de `IUnknown`, ordre des slots de
//! `IMiniportWaveRT` tel que dans `portcls.h` (WDK 26100), constantes clés.
//!
//! Les tailles de vtable contre `cl.exe` sont dans `layout.rs` ; les assertions de
//! disposition de bindgen (taille, alignement, offset de chaque champ) sont vérifiées à
//! la compilation du crate.

// Tests en mode utilisateur : une panique est un échec de test, pas un écran bleu.
#![allow(clippy::panic, clippy::arithmetic_side_effects)]

use std::mem::{offset_of, size_of};

use portcls_sys::*;

/// Taille d'un slot : pointeur de fonction x64.
const SLOT: usize = 8;

/// Pour chaque interface : `lpVtbl` à l'offset 0 (et seul champ), vtable commençant par
/// `QueryInterface`, `AddRef`, `Release` aux slots 0, 1, 2.
macro_rules! interfaces_com {
    ($($iface:ident / $vtbl:ident),* $(,)?) => {
        #[test]
        fn lpvtbl_a_l_offset_zero() {
            $(
                assert_eq!(offset_of!($iface, lpVtbl), 0, "{}::lpVtbl", stringify!($iface));
                assert_eq!(size_of::<$iface>(), SLOT, "{} : un seul pointeur", stringify!($iface));
            )*
        }

        #[test]
        fn vtables_commencent_par_iunknown() {
            $(
                assert_eq!(offset_of!($vtbl, QueryInterface), 0, "{}::QueryInterface", stringify!($vtbl));
                assert_eq!(offset_of!($vtbl, AddRef), SLOT, "{}::AddRef", stringify!($vtbl));
                assert_eq!(offset_of!($vtbl, Release), 2 * SLOT, "{}::Release", stringify!($vtbl));
                assert!(size_of::<$vtbl>() >= 3 * SLOT, "{} : au moins IUnknown", stringify!($vtbl));
            )*
        }

        /// Nombre de slots de chaque vtable (taille / 8), pour le rapport et pour figer la
        /// version des interfaces (WDK 26100).
        #[test]
        fn nombre_de_slots() {
            let attendus: &[(&str, usize)] = &[
                ("IUnknownVtbl", 3),
                ("IMiniportVtbl", 5),
                ("IMiniportWaveRTVtbl", 8),
                ("IMiniportWaveRTStreamVtbl", 11),
                ("IMiniportWaveRTStreamNotificationVtbl", 15),
                ("IMiniportTopologyVtbl", 6),
                ("IAdapterPowerManagementVtbl", 6),
                ("IPortVtbl", 6),
                ("IPortWaveRTVtbl", 6),
                ("IPortTopologyVtbl", 6),
                ("IPortWaveRTStreamVtbl", 10),
                ("IResourceListVtbl", 11),
                ("IRegistryKeyVtbl", 11),
                // `IPortEvents` : IUnknown, puis AddEventToEventList et GenerateEventList.
                ("IPortEventsVtbl", 5),
                ("IPortClsVersionVtbl", 4),
            ];
            let obtenus: &[(&str, usize)] = &[
                $((stringify!($vtbl), size_of::<$vtbl>() / SLOT),)*
            ];
            assert_eq!(obtenus, attendus);
        }
    };
}

interfaces_com! {
    IUnknown / IUnknownVtbl,
    IMiniport / IMiniportVtbl,
    IMiniportWaveRT / IMiniportWaveRTVtbl,
    IMiniportWaveRTStream / IMiniportWaveRTStreamVtbl,
    IMiniportWaveRTStreamNotification / IMiniportWaveRTStreamNotificationVtbl,
    IMiniportTopology / IMiniportTopologyVtbl,
    IAdapterPowerManagement / IAdapterPowerManagementVtbl,
    IPort / IPortVtbl,
    IPortWaveRT / IPortWaveRTVtbl,
    IPortTopology / IPortTopologyVtbl,
    IPortWaveRTStream / IPortWaveRTStreamVtbl,
    IResourceList / IResourceListVtbl,
    IRegistryKey / IRegistryKeyVtbl,
    IPortEvents / IPortEventsVtbl,
    IPortClsVersion / IPortClsVersionVtbl,
}

/// Ordre des slots de `IPortEvents` (`portcls.h` 26100) : `IUnknown`, puis
/// `AddEventToEventList`, puis `GenerateEventList`.
///
/// Les deux méthodes rendent `void` : **aucun moyen de savoir si le port a fait quelque
/// chose**. C'est ce qui rend l'événement de changement de prise indétectable en test
/// unitaire (voir `portcls::event`), et c'est pourquoi l'ordre des slots est figé ici :
/// une inversion enverrait l'entrée d'événement dans `GenerateEventList` sans le moindre
/// diagnostic.
#[test]
fn ordre_des_slots_iportevents() {
    let offsets = [
        (
            "QueryInterface",
            offset_of!(IPortEventsVtbl, QueryInterface),
        ),
        ("AddRef", offset_of!(IPortEventsVtbl, AddRef)),
        ("Release", offset_of!(IPortEventsVtbl, Release)),
        (
            "AddEventToEventList",
            offset_of!(IPortEventsVtbl, AddEventToEventList),
        ),
        (
            "GenerateEventList",
            offset_of!(IPortEventsVtbl, GenerateEventList),
        ),
    ];
    for (i, (nom, offset)) in offsets.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }
    assert_eq!(size_of::<IPortEventsVtbl>(), offsets.len() * SLOT);
}

/// Les verbes d'événement de PortCls sont des **bits**, pas un énuméré dense :
/// `SUPPORT` vaut 4, pas 3. Un `match` exhaustif sur 0..=3 les manquerait.
#[test]
fn verbes_d_evenement() {
    assert_eq!(PCEVENT_VERB_NONE, 0);
    assert_eq!(PCEVENT_VERB_ADD, 1);
    assert_eq!(PCEVENT_VERB_REMOVE, 2);
    assert_eq!(PCEVENT_VERB_SUPPORT, 4);
    // Les flags de `PCEVENT_ITEM` sont les `KSEVENT_TYPE_*` homonymes.
    assert_eq!(PCEVENT_ITEM_FLAG_ENABLE, KSEVENT_TYPE_ENABLE);
    assert_eq!(PCEVENT_ITEM_FLAG_ONESHOT, KSEVENT_TYPE_ONESHOT);
    assert_eq!(PCEVENT_ITEM_FLAG_BASICSUPPORT, KSEVENT_TYPE_BASICSUPPORT);
    // `KSEVENT_PINCAPS_JACKINFOCHANGE` est la **deuxième** valeur de l'énumération
    // `KSEVENT_PINCAPS_CHANGENOTIFICATIONS` : 1, pas 0 (0 est FORMATCHANGE).
    assert_eq!(
        KSEVENT_PINCAPS_CHANGENOTIFICATIONS::KSEVENT_PINCAPS_FORMATCHANGE,
        0
    );
    assert_eq!(
        KSEVENT_PINCAPS_CHANGENOTIFICATIONS::KSEVENT_PINCAPS_JACKINFOCHANGE,
        1
    );
}

/// Ordre des slots de `IMiniportWaveRT` dans `portcls.h` 26100 : `IUnknown`, puis
/// `IMiniport` (`GetDescription`, `DataRangeIntersection`), puis `Init`, `NewStream`,
/// `GetDeviceDescription`.
#[test]
fn ordre_des_slots_iminiportwavert() {
    let offsets = [
        (
            "QueryInterface",
            offset_of!(IMiniportWaveRTVtbl, QueryInterface),
        ),
        ("AddRef", offset_of!(IMiniportWaveRTVtbl, AddRef)),
        ("Release", offset_of!(IMiniportWaveRTVtbl, Release)),
        (
            "GetDescription",
            offset_of!(IMiniportWaveRTVtbl, GetDescription),
        ),
        (
            "DataRangeIntersection",
            offset_of!(IMiniportWaveRTVtbl, DataRangeIntersection),
        ),
        ("Init", offset_of!(IMiniportWaveRTVtbl, Init)),
        ("NewStream", offset_of!(IMiniportWaveRTVtbl, NewStream)),
        (
            "GetDeviceDescription",
            offset_of!(IMiniportWaveRTVtbl, GetDeviceDescription),
        ),
    ];
    for (i, (nom, offset)) in offsets.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }
    assert_eq!(size_of::<IMiniportWaveRTVtbl>(), offsets.len() * SLOT);
}

/// Ordre des slots de `IMiniportWaveRTStream` (`DEFINE_ABSTRACT_MINIPORTWAVERTSTREAM()`,
/// `portcls.h` 26100) : `IUnknown`, `SetFormat`, `SetState`, `GetPosition`,
/// `AllocateAudioBuffer`, `FreeAudioBuffer`, `GetHWLatency`, `GetPositionRegister`,
/// `GetClockRegister`. `IMiniportWaveRTStreamNotification` reprend ces onze slots puis
/// ajoute `AllocateBufferWithNotification`, `FreeBufferWithNotification`,
/// `RegisterNotificationEvent`, `UnregisterNotificationEvent`.
#[test]
fn ordre_des_slots_iminiportwavertstream() {
    let offsets = [
        (
            "QueryInterface",
            offset_of!(IMiniportWaveRTStreamVtbl, QueryInterface),
        ),
        ("AddRef", offset_of!(IMiniportWaveRTStreamVtbl, AddRef)),
        ("Release", offset_of!(IMiniportWaveRTStreamVtbl, Release)),
        (
            "SetFormat",
            offset_of!(IMiniportWaveRTStreamVtbl, SetFormat),
        ),
        ("SetState", offset_of!(IMiniportWaveRTStreamVtbl, SetState)),
        (
            "GetPosition",
            offset_of!(IMiniportWaveRTStreamVtbl, GetPosition),
        ),
        (
            "AllocateAudioBuffer",
            offset_of!(IMiniportWaveRTStreamVtbl, AllocateAudioBuffer),
        ),
        (
            "FreeAudioBuffer",
            offset_of!(IMiniportWaveRTStreamVtbl, FreeAudioBuffer),
        ),
        (
            "GetHWLatency",
            offset_of!(IMiniportWaveRTStreamVtbl, GetHWLatency),
        ),
        (
            "GetPositionRegister",
            offset_of!(IMiniportWaveRTStreamVtbl, GetPositionRegister),
        ),
        (
            "GetClockRegister",
            offset_of!(IMiniportWaveRTStreamVtbl, GetClockRegister),
        ),
    ];
    for (i, (nom, offset)) in offsets.iter().enumerate() {
        assert_eq!(*offset, i * SLOT, "slot {i} = {nom}");
    }
    assert_eq!(size_of::<IMiniportWaveRTStreamVtbl>(), offsets.len() * SLOT);

    let notification = [
        (
            "GetClockRegister",
            offset_of!(IMiniportWaveRTStreamNotificationVtbl, GetClockRegister),
        ),
        (
            "AllocateBufferWithNotification",
            offset_of!(
                IMiniportWaveRTStreamNotificationVtbl,
                AllocateBufferWithNotification
            ),
        ),
        (
            "FreeBufferWithNotification",
            offset_of!(
                IMiniportWaveRTStreamNotificationVtbl,
                FreeBufferWithNotification
            ),
        ),
        (
            "RegisterNotificationEvent",
            offset_of!(
                IMiniportWaveRTStreamNotificationVtbl,
                RegisterNotificationEvent
            ),
        ),
        (
            "UnregisterNotificationEvent",
            offset_of!(
                IMiniportWaveRTStreamNotificationVtbl,
                UnregisterNotificationEvent
            ),
        ),
    ];
    for (i, (nom, offset)) in notification.iter().enumerate() {
        assert_eq!(*offset, (10 + i) * SLOT, "slot {} = {nom}", 10 + i);
    }
    assert_eq!(
        size_of::<IMiniportWaveRTStreamNotificationVtbl>(),
        15 * SLOT
    );
}

/// `IPortClsVersion` corrigé à la main (`fixups`) : `GetVersion` au slot 3.
#[test]
fn iportclsversion_corrigee() {
    assert_eq!(offset_of!(IPortClsVersionVtbl, GetVersion), 3 * SLOT);
    assert_eq!(size_of::<IPortClsVersionVtbl>(), 4 * SLOT);
}

/// Constantes clés : `KSSTATE` en module de constantes, `KSSTATE_RUN == 3`.
#[test]
fn constantes_ks() {
    assert_eq!(KSSTATE::KSSTATE_STOP, 0);
    assert_eq!(KSSTATE::KSSTATE_ACQUIRE, 1);
    assert_eq!(KSSTATE::KSSTATE_PAUSE, 2);
    assert_eq!(KSSTATE::KSSTATE_RUN, 3);
    assert_eq!(PORT_CLASS_DEVICE_EXTENSION_SIZE, 64 * 8);
}

/// Les types que `conduit-kmd` passe à PortCls existent avec la signature attendue
/// (vérification à la compilation : `extern "C"` = `__stdcall` sur x64).
#[test]
fn signatures_des_rappels() {
    unsafe extern "C" fn start(_: PDEVICE_OBJECT, _: PIRP, _: PRESOURCELIST) -> NTSTATUS {
        0
    }
    unsafe extern "C" fn add(_: PDRIVER_OBJECT, _: PDEVICE_OBJECT) -> NTSTATUS {
        0
    }
    let start_device: PCPFNSTARTDEVICE = Some(start);
    let add_device: PDRIVER_ADD_DEVICE = Some(add);
    assert!(start_device.is_some() && add_device.is_some());
    assert_eq!(size_of::<PRESOURCELIST>(), SLOT);
}
