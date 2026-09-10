//! Côté **adaptateur** : ce que `StartDevice` appelle pour bâtir un câble
//! (driver-design.md §4) — création des ports (`PcNewPort`), initialisation
//! (`IPort::Init` avec le miniport), enregistrement des sous-périphériques
//! (`PcRegisterSubdevice`) et des connexions physiques
//! (`PcRegisterPhysicalConnection`) — et les conversions qui vont avec.
//!
//! Les fonctions `Pc*` sont des symboles de `portcls.sys` que seul le pilote lie
//! (`conduit-kmd/build.rs`) : leurs enveloppes ([`new_port`], [`register_subdevice`],
//! [`register_physical_connection`], [`register_adapter_power_management`]) n'existent
//! que sous la feature **`kernel`** du crate, activée par `conduit-kmd`. Le reste du
//! module ([`port_init`], [`as_unknown`], les noms) est un appel de vtable ou une
//! conversion, disponible et testé en mode utilisateur.
//!
//! Ordre d'appel par sous-périphérique (SYSVAD `InstallSubdevice`, structure) :
//! `new_port(&CLSID_PortWaveRT)` → `new_wavert_object(miniport)` → `port_init(…)` →
//! `register_subdevice(device, WAVE_RENDER_NAMES[n], &as_unknown(&port))`, puis, les quatre
//! sous-périphériques créés, `register_physical_connection` entre les pins bridge.
//! Toutes ces fonctions sont à `PASSIVE_LEVEL` (contexte de `IRP_MN_START_DEVICE`).
//! Celles qui reçoivent le `PDEVICE_OBJECT` (et l'`IRP`) de `StartDevice` sont `unsafe` :
//! PortCls les déréférence, et seul l'appelant sait qu'ils sont ceux du rappel en cours.

use conduit_com::{ComPtr, ComRef, ComVtable, NtStatus, STATUS_NOT_IMPLEMENTED};
use conduit_kmd_core::packetsize::{
    AUDIO_SIGNALPROCESSINGMODE_DEFAULT, PROCESSING_MODE_CONSTRAINT_COUNT, PacketConstraints,
    ProcessingModeConstraint,
};
use portcls_sys::{
    GUID, IPort, IUnknown, KSAUDIO_PACKETSIZE_CONSTRAINTS2,
    KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT, PDEVICE_OBJECT, PIRP, UNICODE_STRING,
};

use crate::config::en_guid;
use crate::received::ResourceList;

#[cfg(feature = "kernel")]
use conduit_com::{STATUS_INVALID_PARAMETER, nt_success};
#[cfg(feature = "kernel")]
use portcls_sys::{
    DEVPKEY_KsAudio_PacketSize_Constraints2, DEVPROP_TYPE_BINARY, IoRegisterDeviceInterface,
    IoSetDeviceInterfacePropertyData, KSCATEGORY_AUDIO, LOCALE_NEUTRAL, PPORT,
    PcGetPhysicalDeviceObject, PcNewPort, PcRegisterAdapterPowerManagement,
    PcRegisterPhysicalConnection, PcRegisterSubdevice, RtlFreeUnicodeString,
    packet_size_constraints_bytes,
};

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
    // SAFETY: `name` est une chaîne UTF-16 terminée par NUL (vérifié) que PortCls **ne
    // copie pas** : la documentation de `PcRegisterSubdevice` exige que le tampon reste
    // valide pendant toute la vie de l'objet périphérique. Les appelants passent des
    // tranches de `static`/`const` promus, jamais de tampon de pile. Le `cast_mut`
    // satisfait le prototype `PWSTR` sans qu'aucune écriture n'ait lieu ; `unknown` est
    // vivant le temps de l'appel ; `device` est celui de `StartDevice`.
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

/// `PcRegisterAdapterPowerManagement` : inscrit `unknown` — l'`IUnknown` d'un objet
/// [`crate::power::AdapterPowerManagement`], par [`as_unknown`] — comme destinataire des
/// messages d'alimentation de l'adaptateur `device`. Sans cet appel, l'objet construit
/// par [`crate::new_power_object`] n'est jamais appelé : c'est le seul chemin.
///
/// Prototype du WDK (`km/portcls.h` 10.0.26100, l. 4262-4273) :
/// `NTSTATUS PcRegisterAdapterPowerManagement(_In_ PUNKNOWN Unknown, _In_ PVOID
/// pvContext1)`. Le second paramètre est l'**objet de périphérique** de l'adaptateur :
/// c'est sur lui que la fonction inverse `PcUnregisterAdapterPowerManagement(_In_
/// PDEVICE_OBJECT)` (l. 4275-4287) est clavetée. L'enveloppe prend donc les deux
/// arguments dans l'ordre du reste du module — `device` d'abord — plutôt que dans celui
/// du prototype C.
///
/// # Propriété
///
/// Même contrat que [`register_subdevice`] : PortCls prend **sa propre référence** sur
/// l'objet, qu'il appellera longtemps après le retour ; l'appelant relâche la sienne.
/// C'est ce que fait SYSVAD de son `CAdapterCommon`.
///
/// # Quand appeler
///
/// L'en-tête est explicite et contre-intuitif (l. 2777-2785) : « If you want to fill in
/// the caps struct for your device, register the interface with PortCls **in or before
/// your AddDevice() function**. The OS queries devices before StartDevice() gets
/// called. » Autrement dit, un enregistrement fait dans `StartDevice` arrive **après**
/// l'interrogation des capacités : `QueryDeviceCapabilities` ne sera pas appelé, et le
/// défaut du trait (ne rien amender) est le seul comportement observable. Les deux
/// autres méthodes, elles, sont appelées normalement — ce sont les transitions
/// d'alimentation qui suivent le démarrage.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique fonctionnel remis à `StartDevice` (PortCls le
/// déréférence pour retrouver son extension).
#[cfg(feature = "kernel")]
pub unsafe fn register_adapter_power_management(
    device: PDEVICE_OBJECT,
    unknown: &ComRef<IUnknown>,
) -> NtStatus {
    // SAFETY: `unknown` est vivant le temps de l'appel et PortCls prend sa propre
    // référence ; `device` est celui de `StartDevice` (contrat), passé en `PVOID` comme
    // le veut le prototype.
    unsafe { PcRegisterAdapterPowerManagement(unknown.as_ptr(), device.cast()) }
}

/// La `KSAUDIO_PACKETSIZE_CONSTRAINTS2` que le pilote pose en valeur de
/// `DEVPKEY_KsAudio_PacketSize_Constraints2`, bâtie depuis les valeurs portables de
/// [`PacketConstraints`].
///
/// **Une** contrainte de mode de traitement, celle de
/// [`AUDIO_SIGNALPROCESSINGMODE_DEFAULT`] : `NumProcessingModeConstraints` vaut la
/// **longueur du tableau portable** ([`PROCESSING_MODE_CONSTRAINT_COUNT`]) et non un
/// littéral, de sorte que le compte et le contenu ne puissent pas diverger. La valeur posée
/// mesure donc 40 octets (16 + 1 × 24) et non plus 16 : c'est
/// [`set_packet_size_constraints`] qui la calcule, depuis ce même champ.
///
/// L'hypothèse que cette entrée teste, la sémantique des deux champs de durée et la réserve
/// « the mode-specific constraints need to be higher than the drivers minimum buffer size,
/// otherwise they're ignored by the audio stack » — à laquelle nous sommes à égalité, pas
/// au-dessus — sont écrites sur [`PacketConstraints`], là où les valeurs se calculent.
///
/// Le tableau se convertit par `array::map`, donc **entrée par entrée** : le type du champ
/// du WDK ayant un `ANYSIZE_ARRAY` de 1, cette écriture ne compile que tant que
/// [`PROCESSING_MODE_CONSTRAINT_COUNT`] vaut 1 lui aussi. C'est le compilateur, et non un
/// commentaire, qui tient l'égalité.
///
/// Pure et sans noyau : testable en mode utilisateur, comme tout ce qui n'appelle pas
/// `Pc*`.
#[must_use]
pub fn packet_size_constraints(contraintes: &PacketConstraints) -> KSAUDIO_PACKETSIZE_CONSTRAINTS2 {
    KSAUDIO_PACKETSIZE_CONSTRAINTS2 {
        MinPacketPeriodInHns: contraintes.min_packet_period_hns,
        PacketSizeFileAlignment: contraintes.packet_size_file_alignment,
        MaxPacketSizeInBytes: contraintes.max_packet_size_bytes,
        NumProcessingModeConstraints: MODES_DECLARES,
        ProcessingModeConstraints: contraintes.processing_modes.map(en_contrainte_de_mode),
    }
}

/// [`PROCESSING_MODE_CONSTRAINT_COUNT`] dans le type du champ du WDK (`ULONG`).
///
/// Converti une fois, en `const` : `u32::try_from` rendrait un `Result` à traiter dans une
/// fonction qui n'a rien à refuser, et un `as` silencieux tronquerait au lieu d'échouer.
/// L'assertion garde la conversion honnête si le tableau grandissait un jour au-delà de
/// 4 milliards d'entrées — ce qui n'arrivera pas, mais se lit en une ligne.
const MODES_DECLARES: u32 = {
    assert!(PROCESSING_MODE_CONSTRAINT_COUNT <= u32::MAX as usize);
    PROCESSING_MODE_CONSTRAINT_COUNT as u32
};

/// Une entrée portable vers le `KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT` du WDK,
/// champ par champ.
///
/// Jamais par transmutation, bien que les dispositions coïncident (les deux sont vérifiées
/// contre `layout.golden`) : c'est la seule forme qu'un renommage de champ ou un changement
/// de type casse à la compilation plutôt qu'en machine. Même règle que
/// [`crate::config::en_guid`], qu'elle appelle pour le mode.
fn en_contrainte_de_mode(
    mode: ProcessingModeConstraint,
) -> KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT {
    KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT {
        ProcessingMode: en_guid(&mode.mode),
        SamplesPerProcessingPacket: mode.samples_per_processing_packet,
        ProcessingPacketDurationInHns: mode.processing_packet_duration_hns,
    }
}

/// Le mode de traitement portable est **identiquement** celui que bindgen a extrait du
/// `DEFINE_GUIDSTRUCT` de `ksmedia.h` — les quatre champs, `Data4` octet par octet.
///
/// C'est l'assertion qui relie la constante portable de `conduit_kmd_core` à l'en-tête
/// réel, et elle ferme la chaîne : `portcls-sys/tests/layout.rs` confronte de son côté ce
/// même `AUDIO_SIGNALPROCESSINGMODE_DEFAULT` à ce que `cl.exe` lit de la macro
/// `STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT` (`layout.golden`). Sans elle, un chiffre de
/// travers dans la copie portable poserait la contrainte sur un mode que personne
/// n'emprunte : rien ne planterait, et la période resterait à 10 ms sans que rien ne le
/// dise. Exactement le raisonnement de `KSPROPSETID_CONDUIT` (`crate::config`).
const _: () = {
    let converti = en_guid(&AUDIO_SIGNALPROCESSINGMODE_DEFAULT);
    let reference = portcls_sys::AUDIO_SIGNALPROCESSINGMODE_DEFAULT;
    assert!(converti.Data1 == reference.Data1);
    assert!(converti.Data2 == reference.Data2);
    assert!(converti.Data3 == reference.Data3);
    assert!(converti.Data4[0] == reference.Data4[0]);
    assert!(converti.Data4[1] == reference.Data4[1]);
    assert!(converti.Data4[2] == reference.Data4[2]);
    assert!(converti.Data4[3] == reference.Data4[3]);
    assert!(converti.Data4[4] == reference.Data4[4]);
    assert!(converti.Data4[5] == reference.Data4[5]);
    assert!(converti.Data4[6] == reference.Data4[6]);
    assert!(converti.Data4[7] == reference.Data4[7]);
};

/// Unités UTF-16 **avant** le premier NUL d'une chaîne terminée par NUL, telle que
/// [`utf16z`] et [`utf16z_numbered`] en produisent.
///
/// Le premier NUL, et non le dernier : les noms numérotés sont des gabarits remplis de
/// zéros (« WaveRender0 » dans treize unités), et compter jusqu'au dernier ferait décrire à
/// l'`UNICODE_STRING` une chaîne avec un NUL au milieu — que PnP ne reconnaîtrait comme la
/// chaîne de référence d'aucune interface.
///
/// `None` si la tranche n'a pas de NUL, ou si la longueur ne tient pas dans le `USHORT`
/// d'une `UNICODE_STRING` (32 767 unités au plus, terminateur compris).
// Hors feature `kernel` et hors test, personne ne l'appelle : le seul consommateur est
// `set_packet_size_constraints`, qui n'existe que dans le pilote. Elle reste compilée et
// testée en mode utilisateur — c'est tout l'intérêt de l'avoir sortie de la partie noyau.
#[cfg_attr(not(feature = "kernel"), allow(dead_code))]
fn longueur_utf16z(nom: &[u16]) -> Option<u16> {
    let unites = nom.iter().position(|u| *u == 0)?;
    // Le tampon décrit compte une unité de plus (le NUL) : c'est `MaximumLength`.
    let octets = unites.checked_add(1)?.checked_mul(2)?;
    if octets > u16::MAX as usize {
        return None;
    }
    u16::try_from(unites).ok()
}

/// `UNICODE_STRING` décrivant `nom` (UTF-16 terminé par NUL) **sans le copier**.
///
/// `Length` compte les octets utiles, `MaximumLength` y ajoute le terminateur : c'est la
/// forme qu'attendent `IoRegisterDeviceInterface` et le reste de l'API NT. La structure
/// rendue **emprunte** le tampon de `nom` — elle ne doit pas lui survivre, ce que la durée
/// de vie du paramètre ne peut pas dire (`UNICODE_STRING` porte un pointeur nu) : les seuls
/// appelants passent des `static` promus.
// Même raison que [`longueur_utf16z`] pour l'`allow`.
#[cfg_attr(not(feature = "kernel"), allow(dead_code))]
fn unicode_string_from_utf16z(nom: &[u16]) -> Option<UNICODE_STRING> {
    let unites = longueur_utf16z(nom)?;
    Some(UNICODE_STRING {
        Length: unites.saturating_mul(2),
        MaximumLength: unites.saturating_add(1).saturating_mul(2),
        Buffer: nom.as_ptr().cast_mut(),
    })
}

/// `PcGetPhysicalDeviceObject` : l'objet de périphérique **physique** (PDO) que PortCls a
/// reçu à `PcAddAdapterDevice`, à partir de l'objet fonctionnel (FDO) de `StartDevice`.
///
/// C'est le PDO — et non le FDO — que `IoRegisterDeviceInterface` attend : les interfaces
/// de périphérique appartiennent au nœud PnP, pas à la pile de fonction.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique fonctionnel remis à `StartDevice` (PortCls le
/// déréférence pour retrouver son extension).
#[cfg(feature = "kernel")]
pub unsafe fn physical_device_object(device: PDEVICE_OBJECT) -> Result<PDEVICE_OBJECT, NtStatus> {
    let mut pdo: PDEVICE_OBJECT = core::ptr::null_mut();
    // SAFETY: `device` est celui de `StartDevice` (contrat) ; `pdo` est une variable locale.
    let status = unsafe { PcGetPhysicalDeviceObject(device, &mut pdo) };
    if !nt_success(status) {
        return Err(status);
    }
    if pdo.is_null() {
        return Err(STATUS_INVALID_PARAMETER);
    }
    Ok(pdo)
}

/// Déclare `contraintes` en valeur de `DEVPKEY_KsAudio_PacketSize_Constraints2` sur
/// l'interface `KSCATEGORY_AUDIO` de chaîne de référence `reference` du périphérique
/// physique `physical_device`.
///
/// # Ce que la fonction fait, dans l'ordre de SYSVAD
///
/// 1. `IoRegisterDeviceInterface(pdo, KSCATEGORY_AUDIO, reference, &lien)` — l'interface
///    existe déjà (l'INF la publie, PortCls l'enregistre), et rappeler la fonction sur la
///    même catégorie et la même chaîne de référence **rend le lien symbolique existant**
///    plutôt que d'en créer un second. C'est le seul moyen d'obtenir ce nom depuis un
///    pilote qui n'a pas créé l'interface lui-même ;
/// 2. `IoSetDeviceInterfacePropertyData(lien, DEVPKEY…, LOCALE_NEUTRAL, 0,
///    DEVPROP_TYPE_BINARY, taille, contraintes)` ;
/// 3. `RtlFreeUnicodeString(&lien)` — **toujours**, succès ou échec : le tampon du lien est
///    alloué par le noyau et nous en devenons propriétaires.
///
/// # La longueur passée
///
/// Exactement l'en-tête plus les contraintes de mode déclarées
/// (`packet_size_constraints_bytes`), **jamais** `size_of` de la structure : le
/// `ANYSIZE_ARRAY` du WDK vaut 1, donc la structure C mesure toujours une contrainte de
/// mode de plus qu'elle n'en porte, et les deux exemples de la documentation Microsoft
/// mesurent bien `16 + N × 24`. Une longueur qui déborderait la structure fournie —
/// c'est-à-dire plus d'une contrainte de mode dans une structure qui n'en loge qu'une — est
/// refusée sans appel : l'alternative serait de lire hors de l'objet.
///
/// Depuis que [`packet_size_constraints`] déclare l'entrée
/// [`AUDIO_SIGNALPROCESSINGMODE_DEFAULT`], le cas courant n'est plus l'en-tête seul mais
/// **40 octets** (16 + 1 × 24) — la longueur même de la variante capture de SYSVAD que le
/// paragraphe précédent cite. La coïncidence avec `size_of` de la structure C n'en est pas
/// une : les deux valent 40 tant qu'une seule contrainte est déclarée, et le calcul reste
/// celui de `packet_size_constraints_bytes` pour qu'une seconde entrée n'ait rien à
/// corriger ici.
///
/// # Catégorie, et une seule
///
/// `KSCATEGORY_AUDIO`, comme SYSVAD, alors que l'INF publie aussi chaque filtre wave sous
/// `KSCATEGORY_RENDER`/`KSCATEGORY_CAPTURE` et `KSCATEGORY_REALTIME`. La documentation
/// désigne « the PnP interface of the KS filter that has the streaming pins » sans
/// distinguer les catégories, et l'échantillon Microsoft n'en pose qu'une : poser la même
/// valeur sur trois interfaces multiplierait les chemins d'échec sans rien ajouter de
/// mesurable.
///
/// # Flags à zéro : une propriété volatile
///
/// SYSVAD passe `PLUGPLAY_PROPERTY_PERSISTENT` ; nous passons **0**. La valeur est
/// recalculée à chaque `StartDevice` depuis le registre (`BufferMs` la commande), et une
/// copie persistante survivrait à un changement de ce paramètre — c'est-à-dire annoncerait
/// au moteur audio une période que le pilote ne servirait plus. Volatile, elle est reposée
/// à chaque démarrage du périphérique et disparaît avec lui.
///
/// IRQL : `PASSIVE_LEVEL` (les trois appels l'exigent).
///
/// # Safety
///
/// `physical_device` est l'objet de périphérique **physique** du pilote (par
/// [`physical_device_object`]), valide le temps de l'appel ; `reference` est une chaîne
/// UTF-16 terminée par NUL dont le tampon reste valide pendant l'appel.
#[cfg(feature = "kernel")]
pub unsafe fn set_packet_size_constraints(
    physical_device: PDEVICE_OBJECT,
    reference: &[u16],
    contraintes: &KSAUDIO_PACKETSIZE_CONSTRAINTS2,
) -> NtStatus {
    let Some(mut reference) = unicode_string_from_utf16z(reference) else {
        return STATUS_INVALID_PARAMETER;
    };
    let Some(taille) = packet_size_constraints_bytes(contraintes.NumProcessingModeConstraints)
    else {
        return STATUS_INVALID_PARAMETER;
    };
    // La valeur est lue **dans** la structure fournie : une longueur qui la dépasserait
    // ferait lire hors de l'objet.
    if taille > size_of::<KSAUDIO_PACKETSIZE_CONSTRAINTS2>() {
        return STATUS_INVALID_PARAMETER;
    }
    let Ok(taille) = u32::try_from(taille) else {
        return STATUS_INVALID_PARAMETER;
    };

    let mut lien = UNICODE_STRING::default();
    // SAFETY: `physical_device` est le PDO du pilote (contrat) ; `KSCATEGORY_AUDIO` est une
    // constante lisible ; `reference` décrit un tampon vivant le temps de l'appel ; `lien`
    // est une variable locale que la fonction remplit.
    let status = unsafe {
        IoRegisterDeviceInterface(
            physical_device,
            &KSCATEGORY_AUDIO,
            &mut reference,
            &mut lien,
        )
    };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: `lien` a été rempli par l'appel qui précède ; la clé est une constante
    // lisible ; `contraintes` est vivant et `taille` ne dépasse pas sa taille (vérifié).
    let status = unsafe {
        IoSetDeviceInterfacePropertyData(
            &mut lien,
            &DEVPKEY_KsAudio_PacketSize_Constraints2,
            LOCALE_NEUTRAL,
            0,
            DEVPROP_TYPE_BINARY,
            taille,
            core::ptr::from_ref(contraintes).cast_mut().cast(),
        )
    };
    // SAFETY: `lien` porte un tampon alloué par `IoRegisterDeviceInterface`, dont nous
    // sommes propriétaires ; il n'est plus lu après cet appel. Libéré même en cas d'échec
    // de la pose : le lien a bien été rendu, c'est la propriété qui n'a pas pris.
    unsafe { RtlFreeUnicodeString(&mut lien) };
    status
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
    // Hors feature `kernel`, seul le test des quarante octets appelle ce calcul de
    // longueur : il n'est donc pas dans l'importation du module, qui est gardée.
    use portcls_sys::packet_size_constraints_bytes;

    /// Le nom, coupé au **premier** NUL : les gabarits de [`utf16z_numbered`] sont
    /// remplis de zéros et `split_last` n'en verrait que le dernier.
    fn texte(nom: &[u16]) -> std::string::String {
        let unites: std::vec::Vec<u16> = nom.iter().copied().take_while(|u| *u != 0).collect();
        std::string::String::from_utf16(&unites).unwrap_or_default()
    }

    /// Les valeurs du contrat portable arrivent aux bons champs, l'unique contrainte de
    /// mode comprise.
    #[test]
    fn les_contraintes_de_paquet_recopient_les_valeurs() {
        let portables = PacketConstraints::new(10);
        let ks = packet_size_constraints(&portables);
        assert_eq!(ks.MinPacketPeriodInHns, 50_000, "5 ms");
        assert_eq!(ks.PacketSizeFileAlignment, 0, "FILE_BYTE_ALIGNMENT");
        assert_eq!(ks.MaxPacketSizeInBytes, 30_720, "10 ms de 96 kHz × 8 × 4");
        assert_eq!(ks.NumProcessingModeConstraints, 1, "l'entrée DEFAULT");
        let mode = ks.ProcessingModeConstraints[0];
        // `GUID` du WDK ne dérive pas `PartialEq` : comparaison champ par champ, comme
        // partout ailleurs dans le crate.
        let attendu = portcls_sys::AUDIO_SIGNALPROCESSINGMODE_DEFAULT;
        assert_eq!(mode.ProcessingMode.Data1, attendu.Data1);
        assert_eq!(mode.ProcessingMode.Data2, attendu.Data2);
        assert_eq!(mode.ProcessingMode.Data3, attendu.Data3);
        assert_eq!(mode.ProcessingMode.Data4, attendu.Data4);
        // Zéro : la contrainte est exprimée par la durée, indépendamment de la fréquence.
        assert_eq!(mode.SamplesPerProcessingPacket, 0);
        assert_eq!(mode.ProcessingPacketDurationInHns, 50_000, "5 ms");
        // Un plancher de tampon plus large déplace la période — et la durée de l'entrée
        // avec elle, les deux valant `min_packet_period_hns`.
        let large = packet_size_constraints(&PacketConstraints::new(40));
        assert_eq!(large.MinPacketPeriodInHns, 200_000);
        assert_eq!(
            large.ProcessingModeConstraints[0].ProcessingPacketDurationInHns,
            200_000
        );
        assert_eq!(large.MaxPacketSizeInBytes, ks.MaxPacketSizeInBytes);
    }

    /// **Les quarante octets** que le moteur audio lira, en dur.
    ///
    /// C'est la seule vérification qui montre la valeur *telle qu'elle part* : les
    /// assertions de champ ci-dessus ne diraient rien d'un champ mal **placé**, la
    /// disposition venant du WDK et non de nous, ni d'un GUID dont deux moitiés seraient
    /// interverties — un GUID valide, différent, et une contrainte posée sur un mode que
    /// personne n'emprunte. Découpage de la valeur attendue :
    ///
    /// | Octets | Champ | Valeur |
    /// |---|---|---|
    /// | 0-3 | `MinPacketPeriodInHns` | 50 000 (5 ms) |
    /// | 4-7 | `PacketSizeFileAlignment` | 0 (`FILE_BYTE_ALIGNMENT`) |
    /// | 8-11 | `MaxPacketSizeInBytes` | 30 720 |
    /// | 12-15 | `NumProcessingModeConstraints` | 1 |
    /// | 16-31 | `ProcessingMode` | `{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}` |
    /// | 32-35 | `SamplesPerProcessingPacket` | 0 (la contrainte est en durée) |
    /// | 36-39 | `ProcessingPacketDurationInHns` | 50 000 (5 ms) |
    ///
    /// Les trois premiers champs et les deux derniers sont petit-boutistes ; le GUID, lui,
    /// l'est **par moitié** — ses trois premiers champs entiers le sont, ses huit derniers
    /// octets sont dans l'ordre du texte. C'est ce mélange que ce test fige.
    #[test]
    fn les_contraintes_de_paquet_partent_en_quarante_octets() {
        const ATTENDU: [u8; 40] = [
            0x50, 0xC3, 0x00, 0x00, // MinPacketPeriodInHns = 50 000
            0x00, 0x00, 0x00, 0x00, // PacketSizeFileAlignment = 0
            0x00, 0x78, 0x00, 0x00, // MaxPacketSizeInBytes = 30 720
            0x01, 0x00, 0x00, 0x00, // NumProcessingModeConstraints = 1
            0x7E, 0x2F, 0x8E, 0xC1, // ProcessingMode.Data1 = 0xC18E2F7E
            0x3D, 0x93, // Data2 = 0x933D
            0x65, 0x49, // Data3 = 0x4965
            0xB7, 0xD1, 0x1E, 0xEF, 0x22, 0x8D, 0x2A, 0xF3, // Data4
            0x00, 0x00, 0x00, 0x00, // SamplesPerProcessingPacket = 0
            0x50, 0xC3, 0x00, 0x00, // ProcessingPacketDurationInHns = 50 000
        ];

        let ks = packet_size_constraints(&PacketConstraints::new(10));
        // La longueur est celle que `set_packet_size_constraints` passera au noyau : 16 + 1
        // × 24. `unwrap_or(0)` plutôt qu'un `unwrap` interdit par les lints du workspace —
        // l'assertion qui suit dit la même chose, et le dit mieux.
        let taille = packet_size_constraints_bytes(ks.NumProcessingModeConstraints).unwrap_or(0);
        assert_eq!(taille, ATTENDU.len(), "16 + 1 × 24");

        // SAFETY: `ks` est vivant, `repr(C)` et entièrement constituée de `ULONG` et d'un
        // `GUID` — aucun pointeur, aucun rembourrage implicite (16 + 24 = 40 = `size_of`),
        // donc chacun de ses octets est initialisé et lisible. `taille` vaut exactement
        // cette taille, vérifiée juste au-dessus, et la tranche ne survit pas à `ks`.
        let octets =
            unsafe { core::slice::from_raw_parts(core::ptr::from_ref(&ks).cast::<u8>(), taille) };
        assert_eq!(octets, ATTENDU.as_slice());
    }

    /// La longueur d'une chaîne de référence s'arrête au **premier** NUL : les gabarits
    /// numérotés sont remplis de zéros, et compter jusqu'au dernier décrirait une chaîne
    /// avec un NUL au milieu.
    #[test]
    fn la_chaine_de_reference_s_arrete_au_premier_nul() {
        // « WaveRender0 » : onze unités utiles dans un gabarit de treize.
        let nom = WAVE_RENDER_NAMES[0];
        assert_eq!(nom.len(), RENDER_NAME_LEN);
        assert_eq!(longueur_utf16z(&nom), Some(11));
        // « WaveCapture15 » : treize unités utiles dans un gabarit de quatorze.
        let nom = WAVE_CAPTURE_NAMES[CABLE_COUNT - 1];
        assert_eq!(longueur_utf16z(&nom), Some(13));
        // Sans NUL, aucune longueur : mieux vaut refuser que décrire un tampon qui déborde.
        assert_eq!(longueur_utf16z(&[b'A' as u16, b'B' as u16]), None);
        // Une chaîne vide est une chaîne : zéro unité utile, un NUL.
        assert_eq!(longueur_utf16z(&[0]), Some(0));
    }

    /// L'`UNICODE_STRING` bâtie décrit bien le tampon : octets utiles, terminateur compris
    /// dans `MaximumLength`, et le pointeur est celui de la tranche.
    #[test]
    fn l_unicode_string_decrit_le_tampon_sans_le_copier() {
        let nom = WAVE_RENDER_NAMES[3];
        // `unwrap_or_default` plutôt qu'`expect` (interdit par les lints du workspace) : une
        // `UNICODE_STRING` par défaut est toute à zéro, et les assertions de longueur qui
        // suivent l'attrapent aussi sûrement qu'une panique.
        let chaine = unicode_string_from_utf16z(&nom).unwrap_or_default();
        assert_eq!(chaine.Length, 22, "onze unités utiles");
        assert_eq!(chaine.MaximumLength, 24, "plus le terminateur");
        assert!(core::ptr::eq(chaine.Buffer.cast_const(), nom.as_ptr()));
        assert_eq!(texte(&nom), "WaveRender3");
        assert!(unicode_string_from_utf16z(&[b'X' as u16]).is_none());
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
