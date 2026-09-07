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
//! `register_subdevice(device, WAVE_RENDER_NAMES[n], &as_unknown(&port))`, puis, les quatre
//! sous-périphériques créés, `register_physical_connection` entre les pins bridge.
//! Toutes ces fonctions sont à `PASSIVE_LEVEL` (contexte de `IRP_MN_START_DEVICE`).
//! Celles qui reçoivent le `PDEVICE_OBJECT` (et l'`IRP`) de `StartDevice` sont `unsafe` :
//! PortCls les déréférence, et seul l'appelant sait qu'ils sont ceux du rappel en cours.

use conduit_com::{ComPtr, ComRef, ComVtable, NtStatus, STATUS_NOT_IMPLEMENTED};
use portcls_sys::{GUID, IPort, IUnknown, PDEVICE_OBJECT, PIRP};

use crate::received::ResourceList;

#[cfg(feature = "kernel")]
use conduit_com::{STATUS_INVALID_PARAMETER, nt_success};
#[cfg(feature = "kernel")]
use portcls_sys::{PPORT, PcNewPort, PcRegisterPhysicalConnection, PcRegisterSubdevice};

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
/// [`WAVE_RENDER_NAMES`] et ses frères) auprès de l'objet de périphérique `device` ;
/// c'est ce nom qui apparaît dans l'interface KS du filtre. PortCls prend sa référence.
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

/// Nombre de câbles servis par le pilote (M1b-02, driver-design.md §2.1).
///
/// Tout ce qui est *par câble* s'y dimensionne : noms de sous-périphériques et GUID de
/// noms de broche ci-dessous, tableau `conduit_kmd::cable::CABLES`, descripteurs de
/// topologie, et les blocs engendrés de `conduit_kmd.inx`.
///
/// **Constante pour l'instant** : M1b-01 la rendra configurable (nombre de câbles lu
/// dans le registre au démarrage, `conduit-kmd-core::params`) ; le branchement est une
/// tâche à part.
pub const CABLE_COUNT: usize = 16;

const _: () = {
    assert!(CABLE_COUNT >= 1, "au moins un câble");
    assert!(
        CABLE_COUNT <= 100,
        "le gabarit des noms ne réserve que deux chiffres"
    );
};

/// Chaîne ASCII → UTF-16 terminée par NUL, en contexte `const` (noms de
/// sous-périphérique). `N` = longueur + 1 ; une longueur fausse ou un octet non ASCII
/// fait échouer la compilation.
///
/// La longueur est **dans le type** : c'est une garantie qu'on garde pour les chaînes
/// fixes. Les noms numérotés, eux, ne peuvent pas s'y plier (« WaveRender0 » fait 12
/// unités et « WaveRender15 » 13, et un `[[u16; N]; CABLE_COUNT]` n'admet qu'un seul
/// `N`) : ils passent par [`utf16z_numbered`].
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

/// Nom de sous-périphérique **numéroté**, en contexte `const` : `prefix`, le numéro en
/// décimal, puis des NUL jusqu'à `N`.
///
/// Gabarit **unique** et remplissage à zéro, là où [`utf16z`] impose `N` = longueur + 1 :
/// les seize noms d'une même famille doivent partager un type pour tenir dans un tableau.
/// Le remplissage est sans danger — PortCls reçoit un `PWSTR` et s'arrête au premier NUL,
/// et le garde-fou de [`register_subdevice`] ne vérifie que le NUL **final**, que le
/// remplissage fournit toujours.
///
/// `N` trop court, numéro à trois chiffres ou préfixe non ASCII : échec de compilation.
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // évalué à la compilation
pub const fn utf16z_numbered<const N: usize>(prefix: &str, number: usize) -> [u16; N] {
    let bytes = prefix.as_bytes();
    assert!(
        bytes.len() + 3 <= N,
        "N doit tenir le préfixe, deux chiffres et le NUL"
    );
    assert!(number < 100, "le gabarit ne réserve que deux chiffres");
    let mut out = [0u16; N];
    let mut i = 0;
    while i < bytes.len() {
        assert!(bytes[i].is_ascii(), "nom de sous-périphérique non ASCII");
        out[i] = bytes[i] as u16;
        i += 1;
    }
    if number >= 10 {
        out[i] = b'0' as u16 + (number / 10) as u16;
        i += 1;
    }
    out[i] = b'0' as u16 + (number % 10) as u16;
    out
}

/// Longueur du gabarit des noms `WaveRender<n>` et `TopoRender<n>` : dix caractères de
/// préfixe, deux chiffres, un NUL.
pub const RENDER_NAME_LEN: usize = 13;
/// Longueur du gabarit des noms `WaveCapture<n>` et `TopoCapture<n>` : onze caractères
/// de préfixe, deux chiffres, un NUL.
pub const CAPTURE_NAME_LEN: usize = 14;

/// Les [`CABLE_COUNT`] noms bâtis sur `prefix` : `prefix0`, `prefix1`, … dans l'ordre des
/// câbles.
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn numbered_names<const N: usize>(prefix: &str) -> [[u16; N]; CABLE_COUNT] {
    let mut out = [[0u16; N]; CABLE_COUNT];
    let mut i = 0;
    while i < CABLE_COUNT {
        out[i] = utf16z_numbered(prefix, i);
        i = i.wrapping_add(1);
    }
    out
}

/// Noms des sous-périphériques WaveRT rendu (`WaveRender<n>`), un par câble.
pub const WAVE_RENDER_NAMES: [[u16; RENDER_NAME_LEN]; CABLE_COUNT] = numbered_names("WaveRender");
/// Noms des sous-périphériques topologie rendu (`TopoRender<n>`), un par câble.
pub const TOPO_RENDER_NAMES: [[u16; RENDER_NAME_LEN]; CABLE_COUNT] = numbered_names("TopoRender");
/// Noms des sous-périphériques WaveRT capture (`WaveCapture<n>`), un par câble.
pub const WAVE_CAPTURE_NAMES: [[u16; CAPTURE_NAME_LEN]; CABLE_COUNT] =
    numbered_names("WaveCapture");
/// Noms des sous-périphériques topologie capture (`TopoCapture<n>`), un par câble.
pub const TOPO_CAPTURE_NAMES: [[u16; CAPTURE_NAME_LEN]; CABLE_COUNT] =
    numbered_names("TopoCapture");

/// Les quatre noms de sous-périphérique du câble `cable`, dans l'ordre d'enregistrement
/// (`WaveRender<n>`, `TopoRender<n>`, `WaveCapture<n>`, `TopoCapture<n>`) ; `None`
/// au-delà du dernier câble.
///
/// Un seul point d'accès plutôt que quatre indexations chez l'appelant : le `None` y
/// devient le garde-fou unique qui remplace le « seul le câble 0 a des noms » de M1a.
#[must_use]
pub fn subdevice_names(cable: u32) -> Option<[&'static [u16]; 4]> {
    let index = usize::try_from(cable).ok()?;
    Some([
        WAVE_RENDER_NAMES.get(index)?.as_slice(),
        TOPO_RENDER_NAMES.get(index)?.as_slice(),
        WAVE_CAPTURE_NAMES.get(index)?.as_slice(),
        TOPO_CAPTURE_NAMES.get(index)?.as_slice(),
    ])
}

/// GUID de base des **noms de broche** des câbles (driver-design.md §4.2), dernier octet
/// à zéro : [`pin_name_guid`] y écrit le numéro du câble.
///
/// Généré par `[guid]::NewGuid()`, propre à Conduit : aucune catégorie KS ne décrit un
/// câble virtuel, et c'est le GUID `KsPinDescriptor.Name` — pas la catégorie — que KS
/// interroge en premier pour répondre à `KSPROPERTY_PIN_NAME`.
const PIN_NAME_BASE: GUID = GUID {
    Data1: 0xCAA7_4E3D,
    Data2: 0x9BD5,
    Data3: 0x4F78,
    Data4: [0x8E, 0xAC, 0x26, 0xA5, 0xA1, 0xAE, 0x0F, 0x00],
};

/// GUID de nom de broche du câble `cable` : [`PIN_NAME_BASE`] dont le dernier octet vaut
/// `cable`. C'est la valeur de `KsPinDescriptor.Name` des broches endpoint des filtres
/// topologie, et la clé que l'INF associe à « Conduit *n+1* » sous
/// `HKR\MediaCategories` (`conduit_kmd.inx`, `GUID.PinName.Cable<n>`).
///
/// `portcls/tests/inf.rs` vérifie que l'INF et cette fonction ne divergent pas.
#[allow(clippy::indexing_slicing)] // index constant dans un tableau de 8 octets
#[must_use]
pub const fn pin_name_guid(cable: u8) -> GUID {
    let mut guid = PIN_NAME_BASE;
    guid.Data4[7] = cable;
    guid
}

/// GUID de nom de broche de chaque câble, dans l'ordre : `PIN_NAME_GUIDS[n]` vaut
/// `pin_name_guid(n)` et l'INF l'associe à « Conduit *n+1* ».
///
/// `const` : c'est cette valeur que les assertions `const` de
/// `conduit_kmd::descriptors` lisent (l'évaluation `const` ne lit pas les `static`) ;
/// c'est aussi elle que le `static` **adressable** du pilote recopie, puisque
/// `KsPinDescriptor.Name` prend l'adresse du GUID et que PortCls la conserve.
pub const PIN_NAME_GUIDS: [GUID; CABLE_COUNT] = pin_name_guids();

/// Les [`CABLE_COUNT`] GUID de nom de broche, du câble 0 au dernier.
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn pin_name_guids() -> [GUID; CABLE_COUNT] {
    let mut out = [const { PIN_NAME_BASE }; CABLE_COUNT];
    let mut i = 0;
    while i < CABLE_COUNT {
        out[i] = pin_name_guid(i as u8);
        i = i.wrapping_add(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le nom, coupé au **premier** NUL : les gabarits de [`utf16z_numbered`] sont
    /// remplis de zéros et `split_last` n'en verrait que le dernier.
    fn texte(nom: &[u16]) -> std::string::String {
        let unites: std::vec::Vec<u16> = nom.iter().copied().take_while(|u| *u != 0).collect();
        std::string::String::from_utf16(&unites).unwrap_or_default()
    }

    #[test]
    fn guid_de_nom_de_broche_porte_le_numero_de_cable() {
        assert_eq!(pin_name_guid(0).Data4[7], 0);
        assert_eq!(pin_name_guid(15).Data4[7], 15);
        // Seul le dernier octet varie : le reste identifie Conduit.
        assert_eq!(pin_name_guid(15).Data1, pin_name_guid(0).Data1);
        assert_eq!(pin_name_guid(15).Data4[..7], pin_name_guid(0).Data4[..7]);
    }

    #[test]
    fn la_table_des_guids_de_nom_de_broche_suit_les_cables() {
        assert_eq!(PIN_NAME_GUIDS.len(), CABLE_COUNT);
        for (index, guid) in PIN_NAME_GUIDS.iter().enumerate() {
            assert_eq!(usize::from(guid.Data4[7]), index);
        }
    }

    #[test]
    fn noms_utf16_numerotes_et_termines_par_nul() {
        let render: std::vec::Vec<(&[u16], &str)> = WAVE_RENDER_NAMES
            .iter()
            .map(|n| (n.as_slice(), "WaveRender"))
            .chain(
                TOPO_RENDER_NAMES
                    .iter()
                    .map(|n| (n.as_slice(), "TopoRender")),
            )
            .chain(
                WAVE_CAPTURE_NAMES
                    .iter()
                    .map(|n| (n.as_slice(), "WaveCapture")),
            )
            .chain(
                TOPO_CAPTURE_NAMES
                    .iter()
                    .map(|n| (n.as_slice(), "TopoCapture")),
            )
            .collect();
        assert_eq!(render.len(), 4 * CABLE_COUNT);
        for (index, (nom, prefixe)) in render.iter().enumerate() {
            let numero = index % CABLE_COUNT;
            assert_eq!(nom.last(), Some(&0), "nom non terminé par NUL");
            assert_eq!(texte(nom), std::format!("{prefixe}{numero}"));
        }
    }

    #[test]
    fn les_noms_du_cable_sont_ceux_des_tables() {
        for cable in 0..CABLE_COUNT {
            let index = u32::try_from(cable).unwrap_or_default();
            let noms = subdevice_names(index);
            assert!(
                noms.is_some(),
                "le câble {cable} doit avoir ses quatre noms"
            );
            let attendus = [
                std::format!("WaveRender{cable}"),
                std::format!("TopoRender{cable}"),
                std::format!("WaveCapture{cable}"),
                std::format!("TopoCapture{cable}"),
            ];
            for (nom, attendu) in noms.into_iter().flatten().zip(attendus.iter()) {
                assert_eq!(&texte(nom), attendu);
            }
        }
        assert!(
            subdevice_names(u32::try_from(CABLE_COUNT).unwrap_or_default()).is_none(),
            "un câble au-delà du dernier n'a pas de nom"
        );
    }
}
