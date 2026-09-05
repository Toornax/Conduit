//! Côté **adaptateur** : ce que `StartDevice` appelle pour bâtir un câble
//! (driver-design.md §4) — création des ports (`PcNewPort`), initialisation
//! (`IPort::Init` avec le miniport), enregistrement des sous-périphériques
//! (`PcRegisterSubdevice`) et des connexions physiques
//! (`PcRegisterPhysicalConnection`) — et les conversions qui vont avec.
//!
//! Les fonctions `Pc*` sont des symboles de `portcls.sys` que seul le pilote lie
//! (`conduit-kmd/build.rs`) : leurs enveloppes ([`new_port`], [`register_subdevice`],
//! [`register_physical_connection`]) n'existent que sous la feature **`kernel`** du
//! crate, activée par `conduit-kmd`. Le reste du module ([`port_init`], [`as_unknown`],
//! les noms) est un appel de vtable ou une conversion, disponible et testé en mode
//! utilisateur.
//!
//! Ordre d'appel par sous-périphérique (SYSVAD `InstallSubdevice`, structure) :
//! `new_port(&CLSID_PortWaveRT)` → `new_wavert_object(miniport)` → `port_init(…)` →
//! `register_subdevice(device, WAVE_RENDER_0, &as_unknown(&port))`, puis, les quatre
//! sous-périphériques créés, `register_physical_connection` entre les pins bridge.
//! Toutes ces fonctions sont à `PASSIVE_LEVEL` (contexte de `IRP_MN_START_DEVICE`).
//! Celles qui reçoivent le `PDEVICE_OBJECT` (et l'`IRP`) de `StartDevice` sont `unsafe` :
//! PortCls les déréférence, et seul l'appelant sait qu'ils sont ceux du rappel en cours.

use conduit_com::{ComPtr, ComRef, ComVtable, NtStatus, STATUS_NOT_IMPLEMENTED};
use portcls_sys::{IPort, IUnknown, PDEVICE_OBJECT, PIRP};

use crate::received::ResourceList;

#[cfg(feature = "kernel")]
use conduit_com::{STATUS_INVALID_PARAMETER, nt_success};
#[cfg(feature = "kernel")]
use portcls_sys::{GUID, PPORT, PcNewPort, PcRegisterPhysicalConnection, PcRegisterSubdevice};

/// Référence `IUnknown` sur un objet COM du pilote (`AddRef`) : la forme sous laquelle
/// `IPort::Init` reçoit le miniport et `PcRegisterSubdevice` le port.
///
/// Sûr : tout `ComObject` commence par une vtable `ComVtable` (donc `IUnknown` en tête)
/// et `ptr` le garde vivant le temps de prendre la référence.
pub fn as_unknown<V: ComVtable, T: Send + Sync>(ptr: &ComPtr<V, T>) -> ComRef<IUnknown> {
    // SAFETY: `ptr.as_raw()` est non nul et pointe un objet vivant dont la vtable commence
    // par `IUnknown` (contrat `ComVtable`) : c'est un `IUnknown` valide, sur lequel
    // `from_raw_add_ref` prend sa propre référence.
    unsafe { ComRef::from_raw_add_ref(ptr.as_raw().cast::<IUnknown>()) }
}

/// Référence `IUnknown` sur une interface reçue (`AddRef`) : pour passer un port
/// (`ComRef<IPort>`) à `PcRegisterSubdevice` ou `PcRegisterPhysicalConnection`.
pub fn ref_as_unknown<I: conduit_com::ComInterface>(r: &ComRef<I>) -> ComRef<IUnknown> {
    // SAFETY: `r` détient une référence sur un objet vivant dont la vtable commence par
    // `IUnknown` (contrat `ComInterface` + `ComVtable`).
    unsafe { ComRef::from_raw_add_ref(r.as_raw().cast::<IUnknown>()) }
}

/// `IPort::Init` : lie `port` (créé par [`new_port`]) au `miniport` (objet
/// `IMiniportWaveRT` ou `IMiniportTopology`, par [`as_unknown`]), à l'objet de
/// périphérique fonctionnel `device`, à l'IRP de démarrage `irp` et à `resources` ; le
/// port appelle en retour `IMiniportXxx::Init` du miniport avec `adapter` (l'`IUnknown`
/// de l'adaptateur, `None` en M1a) et `resources`. Le port prend ses propres références.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` et `irp` sont ceux que PortCls a remis à `StartDevice`, valides le temps de
/// l'appel (le port les déréférence).
pub unsafe fn port_init(
    port: &ComRef<IPort>,
    device: PDEVICE_OBJECT,
    irp: PIRP,
    miniport: &ComRef<IUnknown>,
    adapter: Option<&ComRef<IUnknown>>,
    resources: &ResourceList,
) -> NtStatus {
    let Some(slot) = port.vtbl().Init else {
        return STATUS_NOT_IMPLEMENTED;
    };
    let adapter = adapter.map_or(core::ptr::null_mut(), ComRef::as_ptr);
    // SAFETY: `port` détient une référence sur un objet vivant dont la vtable est un
    // `IPortVtbl` (contrat `ComInterface`) ; le slot est non nul ; `miniport`,
    // `resources` et `adapter` (s'il est fourni) sont vivants le temps de l'appel ;
    // `device` et `irp` sont ceux que PortCls a remis à `StartDevice`.
    unsafe {
        slot(
            port.as_raw(),
            device,
            irp,
            miniport.as_ptr(),
            adapter,
            resources.com_ref().as_ptr(),
        )
    }
}

/// `PcNewPort` : crée un port PortCls de classe `class` (`CLSID_PortWaveRT`,
/// `CLSID_PortTopology`), référence possédée. À initialiser par [`port_init`].
///
/// IRQL : `PASSIVE_LEVEL`.
#[cfg(feature = "kernel")]
pub fn new_port(class: &GUID) -> Result<ComRef<IPort>, NtStatus> {
    let mut out: PPORT = core::ptr::null_mut();
    // SAFETY: `out` est une variable locale ; `class` est un GUID lisible. Fonction
    // exportée par `portcls.sys`, liée par `conduit-kmd/build.rs`.
    let status = unsafe { PcNewPort(&mut out, class) };
    if !nt_success(status) {
        return Err(status);
    }
    // SAFETY: `PcNewPort` a réussi : `out` est non nul et porte la référence qu'il nous
    // cède.
    unsafe { ComRef::try_from_raw_owned(out) }.ok_or(STATUS_INVALID_PARAMETER)
}

/// `PcRegisterSubdevice` : enregistre `unknown` (le port initialisé, par
/// [`ref_as_unknown`]) sous le nom `name` (UTF-16 **terminé par NUL**, voir
/// [`WAVE_RENDER_0`] et ses frères) auprès de l'objet de périphérique `device` ; c'est ce
/// nom qui apparaît dans l'interface KS du filtre. PortCls prend sa référence.
///
/// `name` sans NUL final → `STATUS_INVALID_PARAMETER`, sans appel.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique fonctionnel remis à `StartDevice` (PortCls le
/// déréférence).
#[cfg(feature = "kernel")]
pub unsafe fn register_subdevice(
    device: PDEVICE_OBJECT,
    name: &[u16],
    unknown: &ComRef<IUnknown>,
) -> NtStatus {
    if name.last() != Some(&0) {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `name` est une chaîne UTF-16 terminée par NUL (vérifié), que PortCls ne
    // modifie pas (il en fait une copie : le `cast_mut` satisfait le prototype `PWSTR`) ;
    // `unknown` est vivant le temps de l'appel ; `device` est celui de `StartDevice`.
    unsafe { PcRegisterSubdevice(device, name.as_ptr().cast_mut(), unknown.as_ptr()) }
}

/// `PcRegisterPhysicalConnection` : connecte la pin `from_pin` du sous-périphérique
/// `from` à la pin `to_pin` de `to` (les deux : ports enregistrés, par
/// [`ref_as_unknown`]). Sans ces connexions, Windows ne construit pas d'endpoint
/// (driver-design.md §4).
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique fonctionnel remis à `StartDevice`.
#[cfg(feature = "kernel")]
pub unsafe fn register_physical_connection(
    device: PDEVICE_OBJECT,
    from: &ComRef<IUnknown>,
    from_pin: u32,
    to: &ComRef<IUnknown>,
    to_pin: u32,
) -> NtStatus {
    // SAFETY: `from` et `to` sont vivants le temps de l'appel ; `device` est celui de
    // `StartDevice`.
    unsafe { PcRegisterPhysicalConnection(device, from.as_ptr(), from_pin, to.as_ptr(), to_pin) }
}

/// Chaîne ASCII → UTF-16 terminée par NUL, en contexte `const` (noms de
/// sous-périphérique). `N` = longueur + 1 ; une longueur fausse ou un octet non ASCII
/// fait échouer la compilation.
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // évalué à la compilation
pub const fn utf16z<const N: usize>(ascii: &str) -> [u16; N] {
    let bytes = ascii.as_bytes();
    assert!(bytes.len() + 1 == N, "N doit valoir longueur + 1");
    let mut out = [0u16; N];
    let mut i = 0;
    while i < bytes.len() {
        assert!(bytes[i].is_ascii(), "nom de sous-périphérique non ASCII");
        out[i] = bytes[i] as u16;
        i += 1;
    }
    out
}

/// Nom du sous-périphérique WaveRT rendu du câble 0 (`WaveRender0`).
pub const WAVE_RENDER_0: [u16; 12] = utf16z("WaveRender0");
/// Nom du sous-périphérique topologie rendu du câble 0 (`TopoRender0`).
pub const TOPO_RENDER_0: [u16; 12] = utf16z("TopoRender0");
/// Nom du sous-périphérique WaveRT capture du câble 0 (`WaveCapture0`).
pub const WAVE_CAPTURE_0: [u16; 13] = utf16z("WaveCapture0");
/// Nom du sous-périphérique topologie capture du câble 0 (`TopoCapture0`).
pub const TOPO_CAPTURE_0: [u16; 13] = utf16z("TopoCapture0");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms_utf16_termines_par_nul() {
        for (nom, attendu) in [
            (&WAVE_RENDER_0[..], "WaveRender0"),
            (&TOPO_RENDER_0[..], "TopoRender0"),
            (&WAVE_CAPTURE_0[..], "WaveCapture0"),
            (&TOPO_CAPTURE_0[..], "TopoCapture0"),
        ] {
            let (nul, texte) = nom.split_last().unwrap_or((&1, &[]));
            assert_eq!(*nul, 0);
            assert_eq!(
                std::string::String::from_utf16(texte).ok().as_deref(),
                Some(attendu)
            );
        }
    }
}
