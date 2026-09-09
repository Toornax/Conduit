//! `StartDevice` de l'adaptateur (driver-design.md §4.1) : lecture des paramètres de
//! registre ([`crate::registry`], M1b-01), puis, pour chaque câble de la réserve, création
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
//! # Alimentation (M1b-06)
//!
//! `StartDevice` enregistre en outre l'objet `IAdapterPowerManagement` du pilote auprès de
//! PortCls ([`crate::power::register`]), **avant** la boucle d'enregistrement des câbles :
//! c'est un objet d'adaptateur, pas de câble, et il vaut mieux qu'il soit en place avant
//! qu'un endpoint n'existe. Son échec est **journalisé sans être propagé** : l'interface
//! est documentée comme optionnelle, et un pilote qui n'apprend pas les transitions
//! d'alimentation reste parfaitement fonctionnel (PortCls met les flux en pause de
//! lui-même). Refuser de démarrer pour cela échangerait un service dégradé contre aucun
//! service.
//!
//! IRQL : `PASSIVE_LEVEL` partout (contexte de `IRP_MN_START_DEVICE`).

use core::sync::atomic::AtomicBool;

use portcls::conduit_com::{
    ComRef, NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
    nt_success,
};
use portcls::{
    ResourceList, as_unknown, new_port, port_init, ref_as_unknown, register_physical_connection,
    register_subdevice, subdevice_names, try_new_topology_object, try_new_wavert_object,
};
use portcls_sys::{
    CLSID_PortTopology, CLSID_PortWaveRT, GUID, IUnknown, NTSTATUS, PDEVICE_OBJECT, PIRP,
    PRESOURCELIST,
};

use crate::cable;
use crate::descriptors::{
    TOPO_CAPTURE_PIN_BRIDGE, TOPO_RENDER_PIN_BRIDGE, WAVE_CAPTURE_PIN_BRIDGE,
    WAVE_RENDER_PIN_BRIDGE, cable_format, check_cable_pins,
};
use crate::eventlog::{EventLog, kmd_event};
use crate::power;
use crate::registry;
use crate::topo::{TopoCapture, TopoRender, check_cable_topology};
use crate::wave::{WaveCapture, WaveRender};

/// Un sous-périphérique enregistré : l'`IUnknown` de son port, tel que
/// `PcRegisterPhysicalConnection` l'attend.
type Subdevice = ComRef<IUnknown>;

// La réserve maximale que le registre peut demander ne dépasse pas le nombre de câbles
// statiques. Les deux constantes valent 16 et disent la même limite de SPEC F-06, mais
// depuis deux crates différents : si l'une bougeait sans l'autre, `start_device`
// demanderait un câble inexistant (à la hausse) ou en laisserait dormir (à la baisse).
const _: () = assert!(conduit_kmd_core::params::MAX_RESERVE <= cable::CABLE_COUNT);

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
/// `log` est le journal d'événements de l'adaptateur, que les deux miniports WaveRT
/// conservent : un refus d'intersection survient **après** `StartDevice`, pendant que
/// Windows énumère l'endpoint, et il n'y aurait sans cela aucune trace de ce que Windows
/// avait demandé (voir [`crate::intersect`]).
///
/// # Safety
///
/// `device` et `irp` sont ceux remis à `StartDevice`, valides le temps de l'appel.
unsafe fn install_cable(
    device: PDEVICE_OBJECT,
    irp: PIRP,
    resources: &ResourceList,
    n: u32,
    log: EventLog,
) -> Result<(), NtStatus> {
    let Some(cable) = cable::cable(n) else {
        return fail("câble inconnu", STATUS_INVALID_PARAMETER);
    };
    // Les quatre noms de référence du câble (`WaveRender<n>`…), tels que l'INF les publie
    // dans ses `AddInterface` : leur absence est le seul garde-fou de numéro à passer.
    let Some(
        [
            wave_render_name,
            topo_render_name,
            wave_capture_name,
            topo_capture_name,
        ],
    ) = subdevice_names(n)
    else {
        return fail("nom de sous-périphérique", STATUS_INVALID_PARAMETER);
    };
    // Oublie les flux d'un éventuel cycle précédent, mémorise l'objet de périphérique (par
    // lequel M1b-04 persistera l'état actif) et crée le timer haute résolution de la
    // boucle locale (§5.3) : sans lui, le câble ne transporterait rien.
    // SAFETY: `device` est l'objet de périphérique de `StartDevice` (contrat), vivant au
    // moins jusqu'au retrait du périphérique — donc au-delà de toute propriété KS routée
    // vers un miniport de ce câble.
    if let Err(status) = unsafe { cable.start(device) } {
        return fail("démarrage du câble (timer haute résolution)", status);
    }

    // 1. WaveRender<n>.
    let mini = try_new_wavert_object(WaveRender {
        n,
        cable,
        log,
        refus_consigne: AtomicBool::new(false),
    })
    .ok_or(STATUS_INSUFFICIENT_RESOURCES)
    .or_else(|status| fail("allocation de WaveRender", status))?;
    // SAFETY: contrat de la fonction relayé.
    let wave_render = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortWaveRT,
            wave_render_name,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    // 2. TopoRender<n>.
    let mini = try_new_topology_object(TopoRender { n, cable })
        .ok_or(STATUS_INSUFFICIENT_RESOURCES)
        .or_else(|status| fail("allocation de TopoRender", status))?;
    // SAFETY: idem.
    let topo_render = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortTopology,
            topo_render_name,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    // 3. WaveCapture<n> et TopoCapture<n>.
    let mini = try_new_wavert_object(WaveCapture {
        n,
        cable,
        log,
        refus_consigne: AtomicBool::new(false),
    })
    .ok_or(STATUS_INSUFFICIENT_RESOURCES)
    .or_else(|status| fail("allocation de WaveCapture", status))?;
    // SAFETY: idem.
    let wave_capture = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortWaveRT,
            wave_capture_name,
            &as_unknown(&mini),
        )
    }?;
    drop(mini);

    let mini = try_new_topology_object(TopoCapture { n, cable })
        .ok_or(STATUS_INSUFFICIENT_RESOURCES)
        .or_else(|status| fail("allocation de TopoCapture", status))?;
    // SAFETY: idem.
    let topo_capture = unsafe {
        install_subdevice(
            device,
            irp,
            resources,
            &CLSID_PortTopology,
            topo_capture_name,
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

/// `StartDevice` : lit les paramètres de registre, puis enregistre les
/// sous-périphériques des `reserve` premiers câbles auprès de `device`.
///
/// `resources` nul → `STATUS_INVALID_PARAMETER` (PortCls fournit toujours une liste,
/// vide pour un périphérique racine).
///
/// # Ce que la réserve commande, et ce qu'elle ne commande pas
///
/// La réserve lue au registre (M1b-01) décide **combien** des `cable::CABLE_COUNT` câbles
/// statiques sont enregistrés : la boucle va de 0 à `reserve`. Les seize jeux de
/// descripteurs et les seize jeux d'interfaces de l'INF restent en place ; ceux qu'on
/// n'enregistre pas ne produisent simplement aucun endpoint.
///
/// Elle ne touche **pas** à `crate::MAX_MINIPORTS`, qui doit rester le maximum statique :
/// voir la mise en garde portée par cette constante.
///
/// Le nombre de canaux, lui, est lu et validé mais **pas appliqué** : `descriptors`
/// scelle `CHANNELS` dans les tables KS et dans des assertions à la compilation, et le
/// rendre dynamique est le sujet de M1b-05.
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

    // SAFETY: `device` est l'objet de périphérique de `StartDevice` (contrat), vivant
    // pendant tout l'appel — donc au-delà du dernier usage de `log`.
    let log = unsafe { EventLog::new(device) };
    // SAFETY: idem ; `StartDevice` s'exécute à `PASSIVE_LEVEL`, ce qu'exigent
    // `IoOpenDeviceRegistryKey` et `ZwQueryValueKey`.
    let params = unsafe { registry::read_params(device, log) };

    // État actif des câbles (M1b-04, F-05) : relu du registre et appliqué **avant** le
    // moindre enregistrement de sous-périphérique, pour qu'un endpoint apparaisse d'emblée
    // dans le bon état plutôt que de basculer sous les yeux de l'utilisateur. La lecture ne
    // peut pas échouer : elle se replie sur le masque par défaut et journalise.
    // SAFETY: idem.
    let masque = unsafe { registry::read_active_cables(device, log) };
    cable::apply_active_mask(masque);

    // Alimentation (M1b-06) : l'objet d'alimentation de l'adaptateur, avant tout câble.
    // L'échec ne fait pas échouer `StartDevice` — voir « Alimentation » en tête de module.
    // SAFETY: `device` est celui de `StartDevice` (contrat) ; `PcRegisterAdapterPowerManagement`
    // exige `PASSIVE_LEVEL`, ce qu'est le contexte de `IRP_MN_START_DEVICE`.
    if let Err(status) = unsafe { power::register(device) } {
        kmd_log!("StartDevice : enregistrement de l'alimentation a échoué : {status:#010x}");
    }

    // `sanitize` a déjà écrêté la réserve dans `1..=MAX_RESERVE`, et une assertion à la
    // compilation aligne ce plafond sur le nombre de câbles statiques : le `min` est une
    // ceinture, pas une correction — il garantit que la boucle ne demande jamais un câble
    // que `cable::cable(n)` ne connaît pas.
    let reserve = u32::from(params.reserve).min(cable::CABLE_COUNT);

    // Le garde-fou de la correction de M1b-05, **avant** le premier `GetDescription`.
    //
    // Tout ce qui précède a posé le format de chaque câble dans le magasin de
    // `descriptors` ; tout ce qui suit va demander à ce magasin une rangée de tables KS,
    // dont PortCls gardera le pointeur à vie. Entre les deux, personne ne vérifiait rien :
    // une variante introuvable est avalée par les `unwrap_or_else` de `wave.rs`, et
    // `kmd_log!` est vide en release. `check_cable_pins` traverse la chaîne entière — par
    // les pointeurs que PortCls suivra — et une divergence part au journal d'événements,
    // lisible sans débogueur.
    //
    // Elle ne fait **pas** échouer `StartDevice` : un endpoint au mauvais format vaut mieux
    // que pas de carte son du tout, c'est la règle du module `registry` et elle vaut ici.
    for n in 0..reserve {
        match check_cable_pins(n) {
            Ok(()) => {
                let format = cable_format(n);
                kmd_log!(
                    "StartDevice : câble {n} servira {} Hz sur {} canaux (variante {:?})",
                    format.sample_rate,
                    format.channels,
                    format.variant()
                );
            }
            Err(ecart) => {
                kmd_log!("StartDevice : câble {n} — {ecart}");
                kmd_event!(
                    log,
                    registry::code::DESCRIPTEUR
                        .saturating_add(registry::RANG_FORMAT)
                        .saturating_add(n),
                    "câble {n} : {ecart}. L'endpoint apparaîtra, mais pas au format que la \
                     clé du périphérique annonce."
                );
            }
        }
        // Le même intervalle, refermé de l'autre côté : un endpoint naît de la connexion du
        // filtre wave et du filtre de topologie, et rien ne vérifiait que les deux
        // s'accordaient sur le nombre de canaux (voir `topo::check_cable_topology`). Même
        // règle qu'au-dessus : on le dit, on n'échoue pas.
        if let Err(ecart) = check_cable_topology(n) {
            kmd_log!("StartDevice : câble {n} — {ecart}");
            kmd_event!(
                log,
                registry::code::TOPOLOGIE
                    .saturating_add(registry::RANG_FORMAT)
                    .saturating_add(n),
                "câble {n} : {ecart}. L'endpoint apparaîtra, mais Windows pourrait ne pas \
                 savoir lui calculer de format."
            );
        }
    }

    for n in 0..reserve {
        // SAFETY: contrat de la fonction relayé.
        if let Err(status) = unsafe { install_cable(device, irp, &resources, n, log) } {
            return status;
        }
    }
    kmd_log!(
        "StartDevice : {reserve} câbles enregistrés sur {}",
        cable::CABLE_COUNT
    );
    STATUS_SUCCESS
}
