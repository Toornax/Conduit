//! Déclarations **écrites à la main et provisoires** des fonctions PortCls utilisées par
//! le pilote minimal (M1a-02).
//!
//! Elles reproduisent les prototypes de `portcls.h` (WDK [`WDK_VERSION`]) et seront
//! **remplacées par les bindings générés** par `bindgen` en M1a-03 (driver-design.md §2.2) ;
//! rien ici n'a vocation à survivre au-delà. Le module n'existe qu'avec la feature
//! `kernel`, seule à tirer `wdk-sys` pour les types NT (`PDRIVER_OBJECT`, `PDEVICE_OBJECT`,
//! `PIRP`, `PUNICODE_STRING`, `NTSTATUS`, `DRIVER_ADD_DEVICE`).
//!
//! Aucun `#[link]` : l'édition de liens avec `portcls.lib` est décidée par
//! `conduit-kmd/build.rs` (`cargo::rustc-link-lib=portcls`), ce qui laisse ce crate
//! testable en mode utilisateur sans la feature.
//!
//! Convention d'appel : `NTAPI` = `__stdcall`, soit `extern "system"` en Rust (identique à
//! `extern "C"` sur x64, seule architecture visée ; `wdk-sys` génère ses pointeurs de
//! rappel en `extern "C"`, d'où le type `DRIVER_ADD_DEVICE` repris tel quel).
//!
//! [`WDK_VERSION`]: crate::WDK_VERSION

use core::ffi::c_void;

use wdk_sys::{
    DRIVER_ADD_DEVICE, NTSTATUS, PDEVICE_OBJECT, PDRIVER_OBJECT, PIRP, PUNICODE_STRING, ULONG,
};

/// `PRESOURCELIST` : pointeur vers l'interface COM `IResourceList` de PortCls.
///
/// Opaque tant que la vtable n'est pas générée (M1a-03) : le pilote minimal ne fait que
/// le recevoir dans `StartDevice` sans jamais le déréférencer.
pub type PRESOURCELIST = *mut c_void;

/// `PCPFNSTARTDEVICE` : rappel `StartDevice` passé à [`PcAddAdapterDevice`], appelé par
/// PortCls sur `IRP_MN_START_DEVICE` (IRQL `PASSIVE_LEVEL`) avec l'objet de périphérique
/// fonctionnel, l'IRP et la liste de ressources traduite.
pub type PCPFNSTARTDEVICE =
    Option<unsafe extern "system" fn(PDEVICE_OBJECT, PIRP, PRESOURCELIST) -> NTSTATUS>;

// SAFETY: prototypes recopiés de `portcls.h` (WDK 10.0.26100.0, lignes
// `PcInitializeAdapterDriver` et `PcAddAdapterDevice`) ; les symboles sont fournis par
// `portcls.lib`, liée par `conduit-kmd/build.rs`. Aucune de ces fonctions n'est
// appelée sans cette liaison : le module n'est compilé qu'avec la feature `kernel`,
// activée uniquement par `conduit-kmd`.
unsafe extern "system" {
    /// Initialise un pilote d'adaptateur PortCls : installe dans `driver` les fonctions de
    /// dispatch PnP/Power/SystemControl/Create/Close (`PcDispatchIrp`), `DriverUnload`
    /// (`PcDriverUnload`) et l'`AddDevice` fourni.
    ///
    /// IRQL : `PASSIVE_LEVEL` ; à appeler depuis `DriverEntry`.
    ///
    /// # Safety
    ///
    /// `driver` et `registry_path` doivent être ceux reçus par `DriverEntry` ; `add_device`
    /// doit rester valide toute la vie du pilote.
    pub fn PcInitializeAdapterDriver(
        driver: PDRIVER_OBJECT,
        registry_path: PUNICODE_STRING,
        add_device: DRIVER_ADD_DEVICE,
    ) -> NTSTATUS;

    /// Crée et attache l'objet de périphérique fonctionnel d'un adaptateur au-dessus de
    /// `physical_device_object`, et mémorise `start_device` pour `IRP_MN_START_DEVICE`.
    /// `max_objects` borne le nombre de sous-périphériques que `PcRegisterSubdevice`
    /// acceptera ; `device_extension_size` vaut 0 pour la taille par défaut.
    ///
    /// IRQL : `PASSIVE_LEVEL` ; à appeler depuis `AddDevice`.
    ///
    /// # Safety
    ///
    /// `driver` et `physical_device_object` doivent être ceux reçus par `AddDevice` ;
    /// `start_device` doit rester valide toute la vie du pilote.
    pub fn PcAddAdapterDevice(
        driver: PDRIVER_OBJECT,
        physical_device_object: PDEVICE_OBJECT,
        start_device: PCPFNSTARTDEVICE,
        max_objects: ULONG,
        device_extension_size: ULONG,
    ) -> NTSTATUS;
}
