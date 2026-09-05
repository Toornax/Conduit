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
//!
//! S'y ajoutent quelques macros que bindgen n'émet pas (transtypage, `sizeof`) ou qui
//! viennent d'un en-tête hors de la liste d'autorisation (`mmreg.h`), recopiées ici avec
//! leur valeur du WDK 26100 : [`PCFILTER_NODE`], [`WAVE_FORMAT_PCM`],
//! [`WAVE_FORMAT_IEEE_FLOAT`], [`WAVE_FORMAT_EXTENSIBLE`].

use core::{ffi::c_void, mem::size_of};

use crate::bindings::{DWORD, IID, IUnknown, NTSTATUS, PVOID, ULONG, ULONG_PTR};

/// `PORT_CLASS_DEVICE_EXTENSION_SIZE` (`portcls.h`) : taille de l'extension de
/// périphérique réservée par PortCls, `64 * sizeof(ULONG_PTR)`.
///
/// Macro à `sizeof`, que bindgen n'évalue pas ; recopiée ici (512 octets sur x64).
pub const PORT_CLASS_DEVICE_EXTENSION_SIZE: usize = 64 * size_of::<ULONG_PTR>();

/// `PCFILTER_NODE` (`portcls.h`, alias de `KSFILTER_NODE` dans `ks.h`) : `((ULONG)-1)`,
/// le « nœud » qui désigne le filtre lui-même dans une `PCCONNECTION_DESCRIPTOR`
/// (connexion directe d'une broche du filtre à une autre).
///
/// Macro à transtypage, que bindgen n'émet pas ; recopiée ici.
pub const PCFILTER_NODE: ULONG = ULONG::MAX;

/// `WAVE_FORMAT_PCM` (`mmreg.h`, en-tête hors de la liste d'autorisation des bindings) :
/// `wFormatTag` d'un `WAVEFORMATEX` PCM entier.
pub const WAVE_FORMAT_PCM: u16 = 0x0001;

/// `WAVE_FORMAT_IEEE_FLOAT` (`mmreg.h`) : `wFormatTag` d'un `WAVEFORMATEX` flottant.
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;

/// `WAVE_FORMAT_EXTENSIBLE` (`mmreg.h`) : `wFormatTag` annonçant un
/// `WAVEFORMATEXTENSIBLE` (sous-format dans `SubFormat`, bits valides dans `Samples`).
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

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
