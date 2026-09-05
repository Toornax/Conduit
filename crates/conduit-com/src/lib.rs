//! `conduit-com` — modèle objet COM générique du pilote noyau Windows.
//!
//! PortCls dialogue avec un miniport par des interfaces COM (vtable C++ pure,
//! `IUnknown` en tête, convention `__stdcall`). Ce crate en fournit la partie
//! **indépendante de PortCls**, portable et testée sans WDK
//! ([`docs/driver-design.md`](https://github.com/ToorNax/conduit/blob/main/docs/driver-design.md)
//! §3, ADR-012) :
//!
//! - [`Guid`] et [`IID_IUNKNOWN`] (disposition `GUID` du WDK), [`NtStatus`] et les
//!   quelques `STATUS_*` du contrat ;
//! - [`IUnknownVtbl`], [`IUnknown`] et le contrat [`ComVtable`] (« ma vtable commence
//!   par `IUnknown` et voici les IID auxquels je réponds ») ;
//! - [`ComObject`] : objets **implémentés** par le pilote (`vtbl` à l'offset 0,
//!   compteur atomique, état `T`), avec `QueryInterface`/`AddRef`/`Release` génériques
//!   à placer dans les vtables concrètes, et [`ComPtr`] pour les posséder côté Rust ;
//! - [`ComRef`] et [`ComInterface`] : références comptées sur les interfaces
//!   **reçues** de PortCls.
//!
//! Les vtables PortCls elles-mêmes (`IMiniportTopology`, `IAdapterPowerManagement`,
//! `IMiniportWaveRT`…), les traits Rust sûrs qui les implémentent et les enveloppes
//! des interfaces reçues (`IPortWaveRT`, `IResourceList`…) vivent dans le workspace
//! noyau : `drivers/windows/portcls`, écrit sur ce crate et testé en mode utilisateur
//! avec de faux ports. `conduit-kmd` n'implémente que les traits.
//!
//! # Contrat COM (punknown.h)
//!
//! - `NTSTATUS QueryInterface(REFIID, PVOID*)` : IID connu → `AddRef` puis `*out = this`,
//!   `STATUS_SUCCESS` ; IID inconnu → `*out = null`, `STATUS_INVALID_PARAMETER` ; `out`
//!   nul → `STATUS_INVALID_PARAMETER`.
//! - `ULONG AddRef()`, `ULONG Release()` : renvoient le nouveau compte ; `Release`
//!   libère l'objet à zéro.
//! - Tout est appelable depuis n'importe quel fil, à `DISPATCH_LEVEL` : ni allocation
//!   (hors la libération finale), ni verrou, ni panique. D'où les lints de ce crate
//!   (`clippy::panic`, `unwrap_used`, `expect_used`, `indexing_slicing`,
//!   `arithmetic_side_effects` en `deny`), les bornes `T: Send + Sync` de [`ComObject`],
//!   et le fait qu'[`ComObject::inner`] ne rende jamais qu'un `&T`.
//!
//! # Allocation
//!
//! `#![no_std]` + `alloc` : [`ComObject::new`] et [`ComObject::try_new`] passent par
//! l'allocateur global, que le binaire final fournit. Dans le pilote c'est `wdk-alloc`
//! (pool non paginé), déclaré par `conduit-kmd` ; en mode utilisateur (tests, Miri,
//! `drivers/windows/portcls`), celui de `std`. Aucune dépendance : le crate ne connaît
//! ni `wdk-sys` ni `portcls-sys`, dont les `GUID`/`IUnknownVtbl` ont la même
//! disposition que les siens (vérifié par `portcls-sys/tests/layout.golden`).
//!
//! # `unsafe`
//!
//! Contrairement à `conduit-kmd-core`, ce crate contient de l'`unsafe` : c'est sa
//! raison d'être (transmutation `this` ↔ `ComObject`, lecture de vtables étrangères,
//! libération). Chaque bloc porte un `// SAFETY:` (lint en `deny`), chaque fonction
//! `unsafe` documente ses préconditions, et l'ensemble passe Miri
//! (`cargo +nightly miri test -p conduit-com --all-features`).

#![no_std]
#![warn(missing_docs)]
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

extern crate alloc;

#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod comref;
pub mod guid;
pub mod object;
pub mod status;
pub mod unknown;

pub use comref::{ComInterface, ComRef};
pub use guid::{Guid, IID_IUNKNOWN};
pub use object::{ComObject, ComPtr};
pub use status::{
    nt_success, NtStatus, RawPtr, ULong, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER,
    STATUS_NOT_IMPLEMENTED, STATUS_SUCCESS, STATUS_UNSUCCESSFUL,
};
pub use unknown::{unknown_of, ComVtable, IUnknown, IUnknownVtbl};
