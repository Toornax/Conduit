//! `conduit-kmd` : pilote noyau Windows des câbles audio virtuels Conduit.
//!
//! Pilote WDM `no_std` construit avec `windows-drivers-rs` ; conception dans
//! [driver-design.md](../../../docs/driver-design.md) (§2.3 pour ce crate), outillage dans
//! [windows-drivers-rs.md](../../../docs/windows-drivers-rs.md), installation et test dans
//! [driver-dev.md](../../../docs/driver-dev.md).
//!
//! État (M1a-01) : squelette minimal qui se charge et se décharge. `DriverEntry` installe
//! `DriverUnload` et renvoie `STATUS_SUCCESS` ; aucune fonction de dispatch, aucun
//! `AddDevice` (M1a-02), aucun appel à PortCls (M1a-06). Le gestionnaire de panique est
//! maison (`panic.rs`) : une panique en noyau se traduit par un bug check, jamais par une
//! boucle infinie.
//!
//! Ce crate ne se teste pas en mode utilisateur (`wdk-sys` lie les bibliothèques noyau
//! même sous `cargo test`) : l'allocateur et le gestionnaire de panique sont retirés sous
//! `cfg(test)` uniquement pour que `cargo clippy --all-targets` compile la cible de test.

#![no_std]

extern crate alloc;

#[cfg(not(test))]
mod panic;

use wdk_sys::{DRIVER_OBJECT, NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, STATUS_SUCCESS};

/// Allocateur global : pool non paginé, tag `rust` (imposé par `wdk-alloc`).
///
/// Il n'honore pas les alignements supérieurs à celui par défaut : les tampons cycliques
/// audio passeront par des MDL, jamais par `Box` (driver-design.md §2.3).
#[cfg(not(test))]
#[global_allocator]
static GLOBAL_ALLOCATOR: wdk_alloc::WdkAllocator = wdk_alloc::WdkAllocator;

/// Point d'entrée du pilote, appelé par le gestionnaire d'E/S au chargement.
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
    _registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    // SAFETY: le noyau garantit que `driver` pointe vers un `DRIVER_OBJECT` valide et
    // exclusivement accessible pendant `DriverEntry`. L'objet vit jusqu'au déchargement,
    // donc la fonction de déchargement enregistrée ici reste valide tant qu'elle est
    // appelable.
    let driver: &mut DRIVER_OBJECT = unsafe { &mut *driver };
    driver.DriverUnload = Some(driver_unload);
    STATUS_SUCCESS
}

/// Déchargement du pilote. Rien à libérer pour l'instant.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par le noyau, une seule fois, après que toutes les E/S en cours ont
/// été terminées.
unsafe extern "C" fn driver_unload(_driver: PDRIVER_OBJECT) {}
