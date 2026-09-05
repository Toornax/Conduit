//! `conduit-kmd` : pilote noyau Windows des câbles audio virtuels Conduit.
//!
//! Pilote WDM `no_std` construit avec `windows-drivers-rs` ; conception dans
//! [driver-design.md](../../../docs/driver-design.md) (§2.3 pour ce crate), outillage dans
//! [windows-drivers-rs.md](../../../docs/windows-drivers-rs.md), installation et test dans
//! [driver-dev.md](../../../docs/driver-dev.md).
//!
//! État (M1a-07) : pilote **directement bâti sur PortCls**, sans gestion PnP WDM
//! maison : `DriverEntry` délègue à `PcInitializeAdapterDriver` (qui installe les dispatch
//! PnP/Power/SystemControl/Create/Close via `PcDispatchIrp` et son propre `DriverUnload`),
//! `AddDevice` à `PcAddAdapterDevice`, et `StartDevice` à [`adapter::start_device`], qui
//! enregistre les quatre sous-périphériques du câble (driver-design.md §4.1) : ports
//! WaveRT et topologie (`PcNewPort`, `IPort::Init`, `PcRegisterSubdevice`) avec les
//! miniports de [`wave`] et [`topo`], puis les connexions physiques
//! (`PcRegisterPhysicalConnection`). Les tables KS sont les `static` de [`descriptors`],
//! l'état partagé du câble celui de [`cable`]. Les fonctions et types PortCls viennent
//! des bindings générés de `portcls-sys` (M1a-03), les objets COM des traits de
//! `portcls` (M1a-04, M1a-05). Le flux **rendu** est livré ([`stream`], M1a-07) :
//! tampon cyclique par `AllocatePagesForMdl`, position calculée par
//! `KeQueryPerformanceCounter` ([`clock`]), état sous spin lock ([`sync`]),
//! notifications par timer et DPC ([`timer`]). Le flux capture et la boucle locale
//! arrivent en M1a-08 : `WaveCapture::NewStream` répond encore `STATUS_NOT_IMPLEMENTED`.
//!
//! Le gestionnaire de panique est maison (`panic.rs`) : une panique en noyau se traduit
//! par un bug check, jamais par une boucle infinie. La journalisation passe par
//! `kmd_log!` (`log.rs`), vide en release.
//!
//! Ce crate ne se teste pas en mode utilisateur (`wdk-sys` lie les bibliothèques noyau
//! même sous `cargo test`) : l'allocateur et le gestionnaire de panique sont retirés sous
//! `cfg(test)` uniquement pour que `cargo clippy --all-targets` compile la cible de test.

#![no_std]

extern crate alloc;

#[macro_use]
mod log;
#[cfg(not(test))]
mod panic;

mod adapter;
mod cable;
mod clock;
mod descriptors;
mod stream;
mod sync;
mod timer;
mod topo;
mod wave;

use core::cell::UnsafeCell;

use portcls_sys::{PCPFNSTARTDEVICE, PRESOURCELIST, PcAddAdapterDevice, PcInitializeAdapterDriver};
use wdk_sys::{
    DRIVER_OBJECT, DRIVER_UNLOAD, NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, STATUS_SUCCESS, ULONG,
};

// Deux jeux de types NT coexistent : ceux de `wdk-sys` (`DriverEntry`, `DriverUnload`, tout
// ce que le noyau appelle directement) et ceux régénérés par `portcls-sys` depuis les mêmes
// en-têtes (`DRIVER_OBJECT`, `DEVICE_OBJECT`, `IRP`, `UNICODE_STRING`, nommés par les
// prototypes `Pc*` et les rappels PortCls). Même disposition, mêmes en-têtes du WDK
// (`portcls-sys/tests/layout.rs` le vérifie contre `cl.exe`) : le passage de l'un à
// l'autre est un `cast()` de pointeur, sans conversion. Les rappels que PortCls invoque
// (`add_device`, `start_device`) sont déclarés avec les types `portcls_sys`.

/// Allocateur global : pool non paginé, tag `rust` (imposé par `wdk-alloc`).
///
/// Il n'honore pas les alignements supérieurs à celui par défaut : les tampons cycliques
/// audio passeront par des MDL, jamais par `Box` (driver-design.md §2.3).
#[cfg(not(test))]
#[global_allocator]
static GLOBAL_ALLOCATOR: wdk_alloc::WdkAllocator = wdk_alloc::WdkAllocator;

/// Nombre maximal de sous-périphériques par adaptateur, passé à `PcAddAdapterDevice` :
/// quatre par câble (`WaveRender`, `TopoRender`, `WaveCapture`, `TopoCapture`,
/// driver-design.md §4). Un seul câble en M1a ; M1b-02 multipliera par la réserve.
const MAX_MINIPORTS: ULONG = 4;

/// Emplacement du `DriverUnload` installé par PortCls, que le nôtre enchaîne.
///
/// Écrit une seule fois dans `DriverEntry` (contexte exclusif), lu une seule fois dans
/// `DriverUnload` (après la fin de toutes les E/S) : aucun accès concurrent possible,
/// d'où le `Sync` déclaré à la main plutôt qu'un verrou.
struct UnloadSlot(UnsafeCell<DRIVER_UNLOAD>);

// SAFETY: voir la doc de `UnloadSlot` : les deux seuls accès sont sérialisés par le noyau
// (`DriverEntry` précède tout, `DriverUnload` suit tout).
unsafe impl Sync for UnloadSlot {}

static PORTCLS_DRIVER_UNLOAD: UnloadSlot = UnloadSlot(UnsafeCell::new(None));

/// Point d'entrée du pilote, appelé par le gestionnaire d'E/S au chargement.
///
/// Délègue à `PcInitializeAdapterDriver` (dispatch, `AddDevice`), puis remplace le
/// `DriverUnload` de PortCls par [`driver_unload`], qui l'enchaîne (même schéma que
/// SYSVAD : PortCls doit libérer son état par pilote).
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par le noyau, avec un `driver` valide pour toute la durée de vie du
/// pilote et un `registry_path` valide le temps de l'appel.
// SAFETY: « DriverEntry » est le nom de symbole exigé par l'éditeur de liens
// (`/ENTRY:DriverEntry`, émis par `wdk-build`) ; aucune autre fonction de ce crate ne
// l'exporte.
#[unsafe(export_name = "DriverEntry")]
pub unsafe extern "system" fn driver_entry(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    kmd_log!("DriverEntry (WDK {})", portcls_sys::WDK_VERSION);

    // SAFETY: `driver` et `registry_path` sont ceux reçus du noyau, valides le temps de
    // l'appel ; `add_device` est une fonction statique, valide toute la vie du pilote.
    // PortCls ne modifie pas `registry_path` (il en fait une copie) : le retrait du
    // `const` ne sert qu'à satisfaire le prototype `PUNICODE_STRING` de `portcls.h`.
    let status = unsafe {
        PcInitializeAdapterDriver(
            driver.cast(),
            registry_path.cast_mut().cast(),
            Some(add_device),
        )
    };
    if status != STATUS_SUCCESS {
        kmd_log!("PcInitializeAdapterDriver a échoué : {status:#010x}");
        return status;
    }

    // SAFETY: le noyau garantit que `driver` pointe vers un `DRIVER_OBJECT` valide et
    // exclusivement accessible pendant `DriverEntry`. L'objet vit jusqu'au déchargement,
    // donc la fonction de déchargement enregistrée ici reste valide tant qu'elle est
    // appelable. `PORTCLS_DRIVER_UNLOAD` n'est écrit qu'ici (voir `UnloadSlot`).
    unsafe {
        let driver: &mut DRIVER_OBJECT = &mut *driver;
        *PORTCLS_DRIVER_UNLOAD.0.get() = driver.DriverUnload;
        driver.DriverUnload = Some(driver_unload);
    }
    STATUS_SUCCESS
}

/// `AddDevice` : le gestionnaire PnP a énuméré un nœud `Root\ConduitCable` ; PortCls crée
/// l'objet de périphérique fonctionnel de l'adaptateur et l'attache à `pdo`.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par le gestionnaire PnP, avec le `driver` de `DriverEntry` et un
/// `pdo` valide.
// `extern "C"` : type `PDRIVER_ADD_DEVICE` tel que généré par `portcls-sys` (même ABI que
// `system` sur x64) ; types `portcls_sys`, c'est PortCls qui l'appelle.
unsafe extern "C" fn add_device(
    driver: portcls_sys::PDRIVER_OBJECT,
    pdo: portcls_sys::PDEVICE_OBJECT,
) -> NTSTATUS {
    kmd_log!("AddDevice (pdo {pdo:p})");

    const START_DEVICE: PCPFNSTARTDEVICE = Some(start_device);
    // SAFETY: `driver` et `pdo` sont ceux reçus du gestionnaire PnP ; `start_device` est
    // une fonction statique, valide toute la vie du pilote ; 0 demande à PortCls
    // l'extension de périphérique par défaut.
    let status = unsafe { PcAddAdapterDevice(driver, pdo, START_DEVICE, MAX_MINIPORTS, 0) };
    if status != STATUS_SUCCESS {
        kmd_log!("PcAddAdapterDevice a échoué : {status:#010x}");
    }
    status
}

/// `StartDevice` : PortCls a reçu `IRP_MN_START_DEVICE` pour l'adaptateur.
///
/// Délègue à [`adapter::start_device`] : création des ports, enregistrement des
/// sous-périphériques et des connexions physiques du câble (driver-design.md §4.1). Tout
/// échec est renvoyé tel quel : PortCls libère ce qui a été enregistré et l'adaptateur ne
/// démarre pas.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par PortCls, avec un `device` (objet fonctionnel créé par
/// `PcAddAdapterDevice`), un `irp` et une `resource_list` valides le temps de l'appel.
unsafe extern "C" fn start_device(
    device: portcls_sys::PDEVICE_OBJECT,
    irp: portcls_sys::PIRP,
    resource_list: PRESOURCELIST,
) -> NTSTATUS {
    kmd_log!("StartDevice (fdo {device:p})");
    // SAFETY: `device`, `irp` et `resource_list` sont ceux reçus de PortCls (contrat).
    let status = unsafe { adapter::start_device(device, irp, resource_list) };
    if status != STATUS_SUCCESS {
        kmd_log!("StartDevice a échoué : {status:#010x}");
    }
    status
}

/// Déchargement du pilote : rien à libérer côté Conduit ; enchaîne le `DriverUnload` de
/// PortCls (`PcDriverUnload`) mémorisé dans `DriverEntry`.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par le noyau, une seule fois, après que tous les objets de
/// périphérique ont été supprimés et toutes les E/S en cours terminées.
unsafe extern "C" fn driver_unload(driver: PDRIVER_OBJECT) {
    kmd_log!("DriverUnload");

    // SAFETY: lecture unique après la fin de toute activité du pilote (voir `UnloadSlot`) ;
    // la valeur, si présente, est le `DriverUnload` que PortCls avait installé et qui
    // attend le même `driver`.
    unsafe {
        if let Some(portcls_unload) = *PORTCLS_DRIVER_UNLOAD.0.get() {
            portcls_unload(driver);
        }
    }
}
