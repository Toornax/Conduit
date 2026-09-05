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
//! | [`power`] | trait [`AdapterPowerManagement`] ↔ `IAdapterPowerManagementVtbl` |
//! | [`topology`] | trait [`MiniportTopology`] ↔ `IMiniportTopologyVtbl` |
//! | [`received`] | enveloppes des interfaces reçues : [`ResourceList`], [`PortTopology`], [`RegistryKey`] |
//! | [`status`] | `STATUS_BUFFER_OVERFLOW`, `STATUS_BUFFER_TOO_SMALL` |
//!
//! M1a-05 ajoutera sur le même schéma `MiniportWaveRT`, `MiniportWaveRTStream` et les
//! enveloppes `PortWaveRT`/`PortWaveRTStream`.
//!
//! # Vtable par type : constante associée
//!
//! Chaque trait métier a un trait compagnon (`PowerVtbl`, `TopologyVtbl`) implémenté
//! pour tout type implémenteur, dont l'unique membre est `const VTBL: IXxxVtbl` : les
//! slots sont les thunks génériques instanciés pour `T`. `ComObject::new(&T::VTBL, …)`
//! obtient le `&'static` requis par **promotion de constante** (la vtable est une valeur
//! `const` sans mutabilité intérieure ni destructeur) ; ni `static` par type, ni macro.
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
//! `Init`, `GetDescription`, `DataRangeIntersection` et les trois méthodes
//! d'alimentation sont appelées à `PASSIVE_LEVEL` (contexte des IRP PnP/Power traités par
//! PortCls) ; `QueryInterface`/`AddRef`/`Release` à `<= DISPATCH_LEVEL`. Les méthodes des
//! enveloppes reçues sont documentées une à une.

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

pub mod power;
pub mod received;
pub mod status;
pub mod topology;
pub mod unknown;

pub use conduit_com;
pub use portcls_sys;

pub use power::{
    AdapterPowerManagement, PowerObject, PowerVtbl, new_power_object, try_new_power_object,
};
pub use received::{PortTopology, RegistryKey, ResourceList};
pub use status::{STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL};
pub use topology::{
    MiniportTopology, TopologyObject, TopologyVtbl, new_topology_object, try_new_topology_object,
};
