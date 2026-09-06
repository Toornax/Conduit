//! `StartDevice` de l'adaptateur (driver-design.md §4.1) : pour chaque câble, création
//! des quatre ports PortCls, initialisation avec leur miniport, enregistrement des
//! sous-périphériques puis des connexions physiques entre broches bridge.
//!
//! Séquence par sous-périphérique (SYSVAD `InstallSubdevice`, structure) :
//! `PcNewPort(classe)` → objet miniport (`new_wavert_object` / `new_topology_object`)
//! → `IPort::Init(device, irp, miniport, None, resources)` → `PcRegisterSubdevice(device,
//! nom, port)`. Les quatre faits, `PcRegisterPhysicalConnection` relie
//! `WaveRender<n>` broche bridge → `TopoRender<n>` broche bridge et `TopoCapture<n>`
//! broche bridge → `WaveCapture<n>` broche bridge ; sans ces connexions, Windows ne
//! construit pas d'endpoint.
//!
//! # Erreurs et propriété
//!
//! Chaque étape qui échoue journalise son `NTSTATUS` et **le renvoie** : PortCls, qui
//! reçoit l'échec de `StartDevice`, détruit ce qui a été enregistré. Les ports et
//! miniports ne sont pas conservés par l'adaptateur : PortCls en prend ses propres
//! références (`IPort::Init`, `PcRegisterSubdevice`) et les références locales
//! (`ComRef`, `ComPtr`) sont relâchées à la sortie de chaque fonction (`Drop`). Seul
//! l'état partagé des câbles ([`crate::cable`]) survit, et il est `static`.
//!
//! `PcRegisterAdapterPowerManagement` n'est pas appelé avant M1b-06.
//!
//! IRQL : `PASSIVE_LEVEL` partout (contexte de `IRP_MN_START_DEVICE`).

use portcls::conduit_com::{
    ComRef, NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
    nt_success,
};
use portcls::{
    ResourceList, TOPO_CAPTURE_0, TOPO_RENDER_0, WAVE_CAPTURE_0, WAVE_RENDER_0, as_unknown,
    new_port, port_init, ref_as_unknown, register_physical_connection, register_subdevice,
    try_new_topology_object, try_new_wavert_object,
};
use portcls_sys::{
    CLSID_PortTopology, CLSID_PortWaveRT, GUID, IUnknown, NTSTATUS, PDEVICE_OBJECT, PIRP,
    PRESOURCELIST,
};

use crate::cable;
use crate::descriptors::{
    TOPO_CAPTURE_PIN_BRIDGE, TOPO_RENDER_PIN_BRIDGE, WAVE_CAPTURE_PIN_BRIDGE,
    WAVE_RENDER_PIN_BRIDGE,
};
use crate::topo::{TopoCapture, TopoRender};
use crate::wave::{WaveCapture, WaveRender};

/// Un sous-périphérique enregistré : l'`IUnknown` de son port, tel que
/// `PcRegisterPhysicalConnection` l'attend.
type Subdevice = ComRef<IUnknown>;

/// Journalise `what` et l'échec `status`, puis le rend en `Err`.
fn fail<T>(what: &str, status: NtStatus) -> Result<T, NtStatus> {
    kmd_log!("StartDevice : {what} a échoué : {status:#010x}");
    Err(status)
}

/// Crée un port de classe `class`, l'initialise avec `miniport` et l'enregistre sous
/// `name` (UTF-16 terminé par NUL). Rend l'`IUnknown` du port enregistré.
///
/// # Safety
///
/// `device` et `irp` sont ceux remis à `StartDevice`, valides le temps de l'appel.
unsafe fn install_subdevice(
    device: PDEVICE_OBJECT,
    irp: PIRP,
    resources: &ResourceList,
    class: &GUID,
    name: &[u16],
    miniport: &ComRef<IUnknown>,
) -> Result<Subdevice, NtStatus> {
    let port = match new_port(class) {
        Ok(port) => port,
        Err(status) => return fail("PcNewPort", status),
    };
    // SAFETY: `device` et `irp` sont ceux de `StartDevice` (contrat) ; `port`,
    // `miniport` et `resources` sont vivants le temps de l'appel.
    let status = unsafe { port_init(&port, device, irp, miniport, None, resources) };
    if !nt_success(status) {
        return fail("IPort::Init", status);
    }
    let port = ref_as_unknown(&port);
    // SAFETY: `device` est celui de `StartDevice` (contrat) ; `name` est un nom
    // UTF-16 terminé par NUL de `portcls::adapter` ; `port` est vivant.
    let status = unsafe { register_subdevice(device, name, &port) };
    if !nt_success(status) {
        return fail("PcRegisterSubdevice", status);
    }
    Ok(port)
}

/// Enregistre les quatre sous-périphériques du câble `n` et leurs deux connexions
/// physiques.
///
/// # Safety
///
/// `device` et `irp` sont ceux remis à `StartDevice`, valides le temps de l'appel.
unsafe fn install_cable(
    device: PDEVICE_OBJECT,
    irp: PIRP,
    resources: &ResourceList,
    n: u32,
) -> Result<(), NtStatus> {
    let Some(cable) = cable::cable(n) else {
        return fail("câble inconnu", STATUS_INVALID_PARAMETER);
    };
    // Oublie les flux d'un éventuel cycle précédent et crée le timer haute résolution de
    // la boucle locale (§5.3) : sans lui, le câble ne transporterait rien.
    if let Err(status) = cable.start() {
        return fail("démarrage du câble (timer haute résolution)", status);
    }
    // M1a : seuls les noms du câble 0 existent (`portcls::adapter`) ; M1b-02 les
    // générera par numéro.
    if n != 0 {
        return fail("nom de sous-périphérique", STATUS_INVALID_PARAMETER);
    }

    // 1. WaveRender<n>.
    let mini = try_new_wavert_object(WaveRender { n, cable })
        .ok_or(STATUS_INSUFFICIENT_RESOURCES)
        .or_else(|status| fail("allocation de WaveRender", status))?;
    // SAFETY: contrat de la fonction relayé.
    let wave_render = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortWaveRT,
            &WAVE_RENDER_0,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    // 2. TopoRender<n>.
    let mini = try_new_topology_object(TopoRender { n })
        .ok_or(STATUS_INSUFFICIENT_RESOURCES)
        .or_else(|status| fail("allocation de TopoRender", status))?;
    // SAFETY: idem.
    let topo_render = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortTopology,
            &TOPO_RENDER_0,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    // 3. WaveCapture<n> et TopoCapture<n>.
    let mini = try_new_wavert_object(WaveCapture { n, cable })
        .ok_or(STATUS_INSUFFICIENT_RESOURCES)
        .or_else(|status| fail("allocation de WaveCapture", status))?;
    // SAFETY: idem.
    let wave_capture = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortWaveRT,
            &WAVE_CAPTURE_0,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    let mini = try_new_topology_object(TopoCapture { n })
        .ok_or(STATUS_INSUFFICIENT_RESOURCES)
        .or_else(|status| fail("allocation de TopoCapture", status))?;
    // SAFETY: idem.
    let topo_capture = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortTopology,
            &TOPO_CAPTURE_0,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    // 4. Connexions physiques entre broches bridge (§4.1).
    // SAFETY: `device` est celui de `StartDevice` (contrat) ; les deux ports sont
    // enregistrés et vivants.
    let status = unsafe {
        register_physical_connection(
            device,
            &wave_render,
            WAVE_RENDER_PIN_BRIDGE,
            &topo_render,
            TOPO_RENDER_PIN_BRIDGE,
        )
    };
    if !nt_success(status) {
        return fail("PcRegisterPhysicalConnection (rendu)", status);
    }
    // SAFETY: idem.
    let status = unsafe {
        register_physical_connection(
            device,
            &topo_capture,
            TOPO_CAPTURE_PIN_BRIDGE,
            &wave_capture,
            WAVE_CAPTURE_PIN_BRIDGE,
        )
    };
    if !nt_success(status) {
        return fail("PcRegisterPhysicalConnection (capture)", status);
    }

    kmd_log!("StartDevice : câble {n} enregistré (4 sous-périphériques, 2 connexions)");
    Ok(())
}

/// `StartDevice` : enregistre les sous-périphériques de tous les câbles
/// (`cable::CABLE_COUNT`) auprès de `device`.
///
/// `resources` nul → `STATUS_INVALID_PARAMETER` (PortCls fournit toujours une liste,
/// vide pour un périphérique racine).
///
/// # Safety
///
/// `device`, `irp` et `resources` sont ceux que PortCls a remis au rappel `StartDevice`,
/// valides le temps de l'appel.
pub unsafe fn start_device(
    device: PDEVICE_OBJECT,
    irp: PIRP,
    resources: PRESOURCELIST,
) -> NTSTATUS {
    // SAFETY: `resources` est nul ou pointe un `IResourceList` vivant sur lequel PortCls
    // détient une référence pendant l'appel (contrat) ; on prend la nôtre.
    let Some(resources) = (unsafe { ComRef::try_from_raw_add_ref(resources) }) else {
        kmd_log!("StartDevice : liste de ressources absente");
        return STATUS_INVALID_PARAMETER;
    };
    let resources = ResourceList::from_ref(resources);
    for n in 0..cable::CABLE_COUNT {
        // SAFETY: contrat de la fonction relayé.
        if let Err(status) = unsafe { install_cable(device, irp, &resources, n) } {
            return status;
        }
    }
    STATUS_SUCCESS
}
