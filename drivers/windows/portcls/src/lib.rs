//! `portcls` — enveloppes sûres entre les traits Rust du pilote et les vtables PortCls.
//!
//! Crate du workspace noyau (ADR-012, driver-design.md §3), `#![no_std]` + `alloc`, sans
//! dépendance à `wdk-sys` : il se construit sur [`conduit_com`] (modèle objet générique :
//! `ComObject`, `ComPtr`, `ComRef`) et sur les vtables générées de [`portcls_sys`]
//! (feature `com`, qui les déclare `ComVtable`/`ComInterface`). Il se teste en **mode
//! utilisateur** (`cargo test -p portcls`) avec un faux PortCls qui n'appelle les objets
//! qu'à travers leurs vtables, comme le noyau.
//!
//! # Répartition
//!
//! | Module | Rôle |
//! |---|---|
//! | [`unknown`] | thunks `QueryInterface`/`AddRef`/`Release` génériques, au type exact des slots générés |
//! | [`miniport`] | thunks `GetDescription`/`DataRangeIntersection` hérités de `IMiniport`, partagés |
//! | [`power`] | trait [`AdapterPowerManagement`] ↔ `IAdapterPowerManagementVtbl` |
//! | [`topology`] | trait [`MiniportTopology`] ↔ `IMiniportTopologyVtbl` |
//! | [`wavert`] | trait [`MiniportWaveRT`] ↔ `IMiniportWaveRTVtbl`, [`StreamObject`] (flux à type effacé rendu à `NewStream`) |
//! | [`stream`] | traits [`MiniportWaveRTStream`] / [`MiniportWaveRTStreamNotification`] ↔ leurs vtables, [`AudioBuffer`] |
//! | [`received`] | enveloppes des interfaces reçues : [`ResourceList`], [`PortTopology`], [`PortWaveRT`], [`PortWaveRTStream`], [`RegistryKey`] |
//! | [`property`] | trait [`PropertyHandler`] ↔ `PCPROPERTY_ITEM`/`PCPROPERTY_REQUEST` : les propriétés KS d'un miniport |
//! | [`audio`] | sémantique des nœuds volume/sourdine au-dessus de [`property`] : trait [`AudioNodes`], gestionnaires [`Volume`] et [`Mute`], échelle en 1/65536 dB |
//! | [`jack`] | `KSPROPERTY_JACK_DESCRIPTION` au-dessus de [`property`] : trait [`JackInfo`], gestionnaire [`JackDescription`] — une propriété du **filtre** qui décrit une **broche** |
//! | [`config`] | jeu de propriétés **privé** `KSPROPSETID_Conduit` au-dessus de [`property`] : trait [`CableConfig`], gestionnaires [`ConduitCableState`], [`ConduitVersion`] et [`ConduitCounters`] — la surface de contrôle du démon (M1b-04) et le relevé de la boucle locale sans débogueur (M1b-21) ; toute la validation vit dans `conduit_kmd_core::config`, pour être fuzzable |
//! | [`event`] | événements KS : trait [`EventHandler`] ↔ `PCEVENT_ITEM`/`PCEVENT_REQUEST`, enveloppe [`PortEvents`] (`IPortEvents`) et `KSEVENT_PINCAPS_JACKINFOCHANGE`, la notification sans laquelle Windows ne relit jamais le jack ; [`JackTargets`], les deux filtres de topologie d'un câble et le signalement des deux |
//! | [`adapter`] | côté adaptateur : `PcNewPort`, `IPort::Init`, `PcRegisterSubdevice`, `PcRegisterPhysicalConnection` (feature `kernel`), noms des sous-périphériques et GUID de nom de broche partagés avec l'INF |
//! | [`status`] | les codes `NTSTATUS` du contrat PortCls absents de `conduit-com` |
//!
//! # Feature `kernel`
//!
//! Les enveloppes des fonctions `Pc*` ([`adapter::new_port`],
//! [`adapter::register_subdevice`], [`adapter::register_physical_connection`],
//! [`adapter::register_adapter_power_management`]) appellent des symboles de
//! `portcls.sys` que seul `conduit-kmd` lie : elles n'existent que sous la feature
//! `kernel`, que `conduit-kmd` active. Tout le reste (thunks, enveloppes reçues,
//! `port_init`, [`power::is_powered`]) est indépendant de la feature et testé en mode
//! utilisateur.
//!
//! # Vtable par type : constante associée
//!
//! Chaque trait métier a un trait compagnon (`PowerVtbl`, `TopologyVtbl`, `WaveRTVtbl`,
//! `StreamVtbl`, `StreamNotificationVtbl`) implémenté pour tout type implémenteur, dont
//! l'unique membre est `const VTBL: IXxxVtbl` : les slots sont les thunks génériques
//! instanciés pour `T`. `ComObject::new(&T::VTBL, …)` obtient le `&'static` requis par
//! **promotion de constante** (la vtable est une valeur `const` sans mutabilité
//! intérieure ni destructeur) ; ni `static` par type, ni macro.
//!
//! Pour les deux vtables de miniport, `&T::VTBL` n'est **pas** écrit à l'endroit de
//! l'appel mais dans une fonction dédiée — [`topology::vtbl_of`], [`wavert::vtbl_of`] —
//! marquée `#[inline(never)]`. La garde de vtable de [`property::handler`] compare des
//! **adresses** de vtable, et une constante promue est `unnamed_addr` : sans occurrence
//! unique et non inlinée, l'allocation se duplique d'une unité de génération de code à
//! l'autre et la garde refuse des objets légitimes dès qu'on optimise. Voir la
//! documentation de [`topology::vtbl_of`].
//!
//! # Contrat `'static` et durée de vie
//!
//! Les types implémenteurs sont `Send + Sync + 'static` : PortCls appelle depuis
//! n'importe quel fil, ne remet jamais qu'un `&self`, et peut garder l'objet aussi
//! longtemps qu'il le souhaite (jusqu'au `Release` final, qui déclenche le `Drop`). Les
//! données que PortCls conserve par pointeur (`PCFILTER_DESCRIPTOR` de `GetDescription`)
//! sont exigées `&'static`.
//!
//! # IRQL
//!
//! `Init`, `GetDescription`, `DataRangeIntersection`, `NewStream`, `GetDeviceDescription`
//! et les méthodes de flux hors `GetPosition` sont appelées à `PASSIVE_LEVEL` (contexte
//! des IRP PnP et des propriétés KS traitées par PortCls) ; `GetPosition` et
//! `QueryInterface`/`AddRef`/`Release` à `<= DISPATCH_LEVEL`. Les méthodes des enveloppes
//! reçues sont documentées une à une.
//!
//! Les **trois méthodes d'alimentation** font exception : leur IRQL n'est pas documenté
//! par PortCls — seul « must reside in paged memory » l'est. Voir l'en-tête de [`power`],
//! qui dit ce qu'on en sait et ce qu'on n'en sait pas.

#![no_std]
#![warn(missing_docs)]
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]

extern crate alloc;

#[cfg(test)]
extern crate std;

pub mod adapter;
pub mod audio;
pub mod config;
pub mod event;
pub mod jack;
pub mod miniport;
pub mod power;
pub mod property;
pub mod received;
pub mod status;
pub mod stream;
pub mod topology;
pub mod unknown;
pub mod wavert;

pub use conduit_com;
pub use portcls_sys;

pub use adapter::{
    CABLE_COUNT, CAPTURE_NAME_LEN, PIN_NAME_GUIDS, RENDER_NAME_LEN, TOPO_CAPTURE_NAMES,
    TOPO_RENDER_NAMES, WAVE_CAPTURE_NAMES, WAVE_RENDER_NAMES, as_unknown, pin_name_guid, port_init,
    ref_as_unknown, subdevice_names, utf16z, utf16z_numbered,
};
#[cfg(feature = "kernel")]
pub use adapter::{
    new_port, register_adapter_power_management, register_physical_connection, register_subdevice,
};
pub use audio::{
    ACCESS_FLAGS, AudioNodes, Channel, Mute, Trace, VOLUME_DELTA, VOLUME_MAX, VOLUME_MIN, Volume,
    mute_item, volume_item,
};
pub use conduit_kmd_core;
pub use config::{
    CABLE_STATE_ACCESS_FLAGS, COUNTERS_ACCESS_FLAGS, CableConfig, ConduitCableState,
    ConduitCounters, ConduitVersion, ConfigTrace, VERSION_ACCESS_FLAGS, cable_state_item,
    counters_item, version_item,
};
pub use event::{
    EventEntry, EventHandler, EventRequest, EventSource, EventTrace, JACK_EVENT_FLAGS,
    JACK_INFO_CHANGE_ID, JackInfoChange, JackTarget, JackTargets, PortEvents,
    jack_info_change_item, with_events,
};
pub use jack::{
    JACK_ACCESS_FLAGS, JACK_COLOR, JACK_CONNECTION_TYPE, JACK_GEN_LOCATION, JACK_GEO_LOCATION,
    JACK_PORT_CONNECTION, JackDescription, JackInfo, JackTrace, Pin, jack_description_item,
};
pub use power::{
    AdapterPowerManagement, PowerObject, PowerVtbl, is_powered, new_power_object,
    try_new_power_object,
};
pub use property::{PropertyHandler, Request, TargetVtbl};
pub use received::{
    PortTopology, PortWaveRT, PortWaveRTStream, RegistryKey, ResourceList, physical_address,
};
pub use status::{
    STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NO_MATCH, STATUS_NOT_FOUND, STATUS_NOT_SUPPORTED, STATUS_PRIVILEGE_NOT_HELD,
};
pub use stream::{
    AudioBuffer, MiniportWaveRTStream, MiniportWaveRTStreamNotification, StreamNotificationPtr,
    StreamNotificationVtbl, StreamPtr, StreamVtbl, new_stream_notification_object,
    new_stream_object, try_new_stream_notification_object, try_new_stream_object,
};
pub use topology::{
    MiniportTopology, TopologyObject, TopologyVtbl, new_topology_object, try_new_topology_object,
};
pub use wavert::{
    MiniportWaveRT, StreamObject, WaveRTObject, WaveRTVtbl, default_device_description,
    new_wavert_object, try_new_wavert_object,
};
