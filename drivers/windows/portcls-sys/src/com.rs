//! Liaison des vtables et interfaces générées au modèle objet `conduit-com`
//! (feature `com`, driver-design.md §3).
//!
//! Pour chaque interface PortCls utile au pilote, deux implémentations :
//!
//! - `unsafe impl ComVtable for IXxxVtbl` : la vtable générée est `repr(C)` et commence
//!   par les trois slots de `IUnknown` (`QueryInterface`, `AddRef`, `Release`, vérifié par
//!   `tests/vtables.rs` et `tests/com.rs`) ; `IIDS` liste `IID_IUnknown`, les bases dont
//!   la vtable est un préfixe de celle-ci, puis l'interface elle-même ;
//! - `unsafe impl ComInterface for IXxx` : `struct IXxx { lpVtbl: *mut IXxxVtbl }`.
//!
//! Les slots générés sont des `Option<unsafe extern "C" fn(This, …)>` : sur x64,
//! `extern "C"` est la convention `__stdcall` attendue par PortCls (identique à
//! `extern "system"`), et `Option<fn>` a la disposition d'un pointeur (niche sur nul). Le
//! contrat `ComVtable` exige que les trois premiers slots soient **remplis** (`Some`) sur
//! tout objet vivant : `conduit-com` les lit comme des pointeurs non nuls. C'est le cas
//! des objets PortCls (COM ne connaît pas de slot vide) et des objets construits par
//! `portcls` (thunks génériques).
//!
//! [`guid`] convertit un `GUID` généré (`Data1`, `Data2`, `Data3`, `Data4`) en
//! `conduit_com::Guid` (`data1`…`data4`, même disposition, 16 octets) en contexte `const`.

use conduit_com::{ComInterface, ComVtable, Guid, IID_IUNKNOWN};

use crate::{
    GUID, IAdapterPowerManagement, IAdapterPowerManagementVtbl, IID_IAdapterPowerManagement,
    IID_IMiniport, IID_IMiniportTopology, IID_IMiniportWaveRT, IID_IMiniportWaveRTInputStream,
    IID_IMiniportWaveRTOutputStream, IID_IMiniportWaveRTStream,
    IID_IMiniportWaveRTStreamNotification, IID_IPort, IID_IPortEvents, IID_IPortTopology,
    IID_IPortWaveRT, IID_IPortWaveRTStream, IID_IRegistryKey, IID_IResourceList, IMiniport,
    IMiniportTopology, IMiniportTopologyVtbl, IMiniportVtbl, IMiniportWaveRT,
    IMiniportWaveRTInputStream, IMiniportWaveRTInputStreamVtbl, IMiniportWaveRTOutputStream,
    IMiniportWaveRTOutputStreamVtbl, IMiniportWaveRTStream, IMiniportWaveRTStreamNotification,
    IMiniportWaveRTStreamNotificationVtbl, IMiniportWaveRTStreamVtbl, IMiniportWaveRTVtbl, IPort,
    IPortEvents, IPortEventsVtbl, IPortTopology, IPortTopologyVtbl, IPortVtbl, IPortWaveRT,
    IPortWaveRTStream, IPortWaveRTStreamVtbl, IPortWaveRTVtbl, IRegistryKey, IRegistryKeyVtbl,
    IResourceList, IResourceListVtbl, IUnknown, IUnknownVtbl,
};

/// `GUID` généré → `conduit_com::Guid`, champ par champ (`const`, pour les tables `IIDS`).
///
/// Les deux types ont la même disposition (`repr(C)`, 16 octets, alignement 4) : la
/// conversion ne sert qu'à passer d'un nom de type à l'autre sans transmutation.
pub const fn guid(g: &GUID) -> Guid {
    Guid::new(g.Data1, g.Data2, g.Data3, g.Data4)
}

/// `IID_IUnknown` de `punknown.h`, sous forme `conduit_com::Guid` ; vaut [`IID_IUNKNOWN`]
/// (vérifié par `tests/com.rs`).
pub const IID_IUNKNOWN_PORTCLS: Guid = guid(&crate::IID_IUnknown);

/// Implémente `ComVtable` pour une vtable générée et `ComInterface` pour son interface.
///
/// `[$($base),*]` : IID des interfaces de base (préfixes de vtable), dans l'ordre
/// d'héritage, **sans** `IUnknown` (ajouté en tête) ni l'interface elle-même (ajoutée en
/// queue).
macro_rules! interface_com {
    ($iface:ident / $vtbl:ident : $iid:ident [$($base:ident),*]) => {
        // SAFETY: vtable générée par bindgen en `repr(C)` depuis `portcls.h`, dont les
        // trois premiers champs sont `QueryInterface`, `AddRef`, `Release` (assertions
        // d'offset émises par bindgen, `tests/vtables.rs`, `tests/com.rs`) ; `IIDS`
        // énumère exactement `IUnknown`, les bases dont la vtable est un préfixe de
        // celle-ci, et l'interface (voir la doc du module).
        unsafe impl ComVtable for $vtbl {
            const IIDS: &'static [Guid] = &[IID_IUNKNOWN, $(guid(&$base),)* guid(&$iid)];
        }

        // SAFETY: `struct $iface { lpVtbl: *mut $vtbl }`, `repr(C)`, un seul champ
        // (assertions bindgen : taille 8, `lpVtbl` à l'offset 0) ; tout objet PortCls est
        // utilisable depuis n'importe quel fil à l'IRQL documenté de chaque méthode.
        unsafe impl ComInterface for $iface {
            type Vtbl = $vtbl;
        }
    };
}

// SAFETY: `IUnknownVtbl` généré (`punknown.h`) : `repr(C)`, exactement les trois slots
// `QueryInterface`, `AddRef`, `Release` (24 octets) ; un objet `IUnknown` ne répond qu'à
// `IID_IUnknown`.
unsafe impl ComVtable for IUnknownVtbl {
    const IIDS: &'static [Guid] = &[IID_IUNKNOWN];
}

// SAFETY: `struct IUnknown { lpVtbl: *mut IUnknownVtbl }`, `repr(C)`, un seul champ ; tout
// objet PortCls est utilisable depuis n'importe quel fil.
unsafe impl ComInterface for IUnknown {
    type Vtbl = IUnknownVtbl;
}

interface_com!(IMiniport / IMiniportVtbl : IID_IMiniport []);
interface_com!(IMiniportTopology / IMiniportTopologyVtbl : IID_IMiniportTopology [IID_IMiniport]);
interface_com!(IMiniportWaveRT / IMiniportWaveRTVtbl : IID_IMiniportWaveRT [IID_IMiniport]);
interface_com!(IMiniportWaveRTStream / IMiniportWaveRTStreamVtbl : IID_IMiniportWaveRTStream []);
interface_com!(
    IMiniportWaveRTStreamNotification / IMiniportWaveRTStreamNotificationVtbl :
    IID_IMiniportWaveRTStreamNotification [IID_IMiniportWaveRTStream]
);
// Mode paquets (Windows 10 1507 et suivants, `NTDDI_WINTHRESHOLD`) : deux interfaces
// **indépendantes**, dérivées de `IUnknown` seul et non de `IMiniportWaveRTStream` (leur
// vtable n'en est pas une extension : slot 3 = `GetReadPacket` / `SetWritePacket`). Un
// flux qui les expose est donc un objet à plusieurs vtables (`portcls::packet`).
interface_com!(
    IMiniportWaveRTInputStream / IMiniportWaveRTInputStreamVtbl :
    IID_IMiniportWaveRTInputStream []
);
interface_com!(
    IMiniportWaveRTOutputStream / IMiniportWaveRTOutputStreamVtbl :
    IID_IMiniportWaveRTOutputStream []
);
interface_com!(
    IAdapterPowerManagement / IAdapterPowerManagementVtbl : IID_IAdapterPowerManagement []
);
interface_com!(IPort / IPortVtbl : IID_IPort []);
interface_com!(IPortTopology / IPortTopologyVtbl : IID_IPortTopology [IID_IPort]);
interface_com!(IPortWaveRT / IPortWaveRTVtbl : IID_IPortWaveRT [IID_IPort]);
interface_com!(IPortWaveRTStream / IPortWaveRTStreamVtbl : IID_IPortWaveRTStream []);
// `IPortEvents` dérive directement de `IUnknown` (`DECLARE_INTERFACE_(IPortEvents,IUnknown)`,
// portcls.h) : aucune interface de base intermédiaire, d'où la liste vide. Elle n'est pas
// remise au miniport dans `Init` — elle s'obtient par `QueryInterface` sur le port avec
// `IID_IPortEvents` (voir `portcls::event`).
interface_com!(IPortEvents / IPortEventsVtbl : IID_IPortEvents []);
interface_com!(IResourceList / IResourceListVtbl : IID_IResourceList []);
interface_com!(IRegistryKey / IRegistryKeyVtbl : IID_IRegistryKey []);
