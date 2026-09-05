//! Corrections manuelles des bindings là où le mode C des en-têtes du WDK 26100 est
//! incomplet (driver-design.md §2.2).
//!
//! Toutes les interfaces PortCls utilisées par Conduit (`IMiniportWaveRT`,
//! `IMiniportTopology`, `IPortWaveRT`, `IAdapterPowerManagement`…) recopient les slots
//! hérités (`DEFINE_ABSTRACT_UNKNOWN()`, `DEFINE_ABSTRACT_MINIPORT()`…) et sortent donc
//! de bindgen avec la disposition COM exacte. Quatre interfaces de `portcls.h` ne le
//! font pas :
//!
//! - `IPortClsVersion` : `GetVersion(THIS)` seul, sans les trois slots de `IUnknown`.
//!   La vtable générée (1 slot) est bloquée dans `build.rs` et redéfinie ici avec le
//!   préfixe `IUnknown` ; `tests/layout.golden` (`sizeof(IUnknownVtbl) +
//!   sizeof(IPortClsVersionVtbl)` côté C) et `tests/vtables.rs` la vérifient.
//! - `IPortClsPower`, `IPortClsRuntimePower`, `IPortClsEtwHelper` : méthodes sans
//!   `THIS_` ni `IUnknown` (déclarations C++ seulement) : slots et signatures faux en C,
//!   exclues des bindings. À écrire à la main, avec `This` en premier paramètre et le
//!   préfixe `IUnknown`, si une tâche ultérieure en a besoin.

use core::{ffi::c_void, mem::size_of};

use crate::bindings::{DWORD, IID, IUnknown, NTSTATUS, PVOID, ULONG, ULONG_PTR};

/// `PORT_CLASS_DEVICE_EXTENSION_SIZE` (`portcls.h`) : taille de l'extension de
/// périphérique réservée par PortCls, `64 * sizeof(ULONG_PTR)`.
///
/// Macro à `sizeof`, que bindgen n'évalue pas ; recopiée ici (512 octets sur x64).
pub const PORT_CLASS_DEVICE_EXTENSION_SIZE: usize = 64 * size_of::<ULONG_PTR>();

/// Vtable de `IPortClsVersion` (`portcls.h`) : les trois slots de `IUnknown` puis
/// `GetVersion`, qui renvoie une valeur de `EPcVersion` (`kVersionWinXP`…).
///
/// Même forme que les vtables générées par bindgen : pointeurs de fonction
/// `extern "C"` optionnels, dans l'ordre du header C++.
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
#[allow(non_snake_case)]
pub struct IPortClsVersionVtbl {
    pub QueryInterface: Option<
        unsafe extern "C" fn(This: *mut IUnknown, arg1: *const IID, arg2: *mut PVOID) -> NTSTATUS,
    >,
    pub AddRef: Option<unsafe extern "C" fn(This: *mut IUnknown) -> ULONG>,
    pub Release: Option<unsafe extern "C" fn(This: *mut IUnknown) -> ULONG>,
    pub GetVersion: Option<unsafe extern "C" fn(This: *mut c_void) -> DWORD>,
}
