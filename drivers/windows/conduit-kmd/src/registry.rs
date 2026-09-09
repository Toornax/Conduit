//! Lecture des paramètres de registre au démarrage (M1b-01, driver-design.md §2.1) et
//! persistance de l'état actif des câbles (M1b-04, §6).
//!
//! `StartDevice` lit quatre `REG_DWORD` dans la **clé matérielle** du périphérique —
//! `ReserveSize`, `Channels`, `BufferMs`, `PacketMode` — les confronte à leurs bornes par
//! [`conduit_kmd_core::params::sanitize`], et journalise ce qu'il a corrigé. Il y lit
//! aussi le masque `ActiveCables` ([`read_active_cables`]), que le gestionnaire de
//! propriété privée réécrit ensuite à chaque changement ([`write_active_cables`]).
//!
//! # Trois natures, trois chemins
//!
//! Les quatre paramètres, le masque et les seize formats partagent la même clé et les mêmes
//! primitives, mais **pas la même nature**, et c'est pourquoi ils ne passent pas par la
//! même fonction :
//!
//! | | `ReserveSize`, `Channels`, `BufferMs`, `PacketMode` | `CableFormat<n>` | `ActiveCables` |
//! |---|---|---|---|
//! | Qui écrit | l'administrateur (et l'INF, une fois) | le **service**, puis redémarrage du devnode | le **pilote**, à chaque `SET` |
//! | Quand c'est lu | `StartDevice` | `StartDevice` | `StartDevice` |
//! | Ce que c'est | de la configuration | de la configuration | de l'**état** |
//!
//! Fondre `ActiveCables` dans [`read_params`] aurait demandé un `Param` de plus dans le
//! crate portable, donc un défaut de plus, un rang de code d'événement de plus et une
//! correction de plus — pour une valeur que l'administrateur n'est pas censé régler à la
//! main et que le pilote écrase à la première demande de l'utilisateur. Le prix de la
//! séparation est **une ouverture de clé de plus** au démarrage, négligeable devant les
//! seize câbles à enregistrer.
//!
//! Les seize `CableFormat<n>` (M1b-05), eux, sont lus **dans** [`read_params`] et sur la
//! même ouverture de clé : ce sont des paramètres de démarrage comme les trois autres,
//! simplement indexés par câble. La seule différence est ce qu'on en fait — ils ne
//! ressortent pas dans les [`Params`] mais sont déposés dans le magasin de
//! `crate::descriptors` ([`conduit_kmd::descriptors::apply_cable_format`]), le seul lecteur
//! qui compte, avant que le moindre sous-périphérique ne soit enregistré.
//!
//! # La règle qui gouverne tout ce module : **on charge quand même**
//!
//! Aucun chemin d'ici ne rend d'erreur. Clé impossible à ouvrir, valeur absente, valeur
//! d'un autre type, taille incohérente, valeur hors bornes : chacun se journalise et se
//! replie sur la valeur par défaut. La raison est écrite en tête de
//! [`conduit_kmd_core::params`] et vaut d'être répétée ici, parce que c'est ce module qui
//! pourrait la trahir : refuser de charger pour une faute de frappe dans un `REG_DWORD`
//! coûterait à l'administrateur **toutes** les cartes son virtuelles du poste, et le
//! symptôme (« le pilote ne démarre pas, code 10 ») ne désignerait pas la valeur fautive.
//!
//! Une valeur absente est un repli **normal**, pas une faute : clé neuve, pilote mis à
//! jour, valeur retirée à la main. Elle est tout de même consignée, parce que l'INF en
//! écrit les trois (`[ConduitCable_HW_AddReg]`) : sur un poste installé proprement,
//! l'absence signifie que quelqu'un ou quelque chose les a supprimées, et ça se dit.
//!
//! # Pourquoi le PDO et pas l'objet que PortCls nous remet
//!
//! `IoOpenDeviceRegistryKey` veut le **PDO** — « Pointer to the physical device object
//! (PDO) of the device instance » — et `StartDevice` reçoit l'objet *fonctionnel* créé
//! par `PcAddAdapterDevice`. Passer ce dernier rendrait `STATUS_INVALID_PARAMETER`, et
//! l'échec ressemblerait à s'y méprendre à « la clé n'existe pas ».
//!
//! `IoGetDeviceAttachmentBaseRef` rend l'objet du bas de la pile d'attachement, c'est-à-
//! dire le PDO, en prenant une référence dessus qu'il faut rendre. Deux solutions plus
//! simples ont été écartées : mémoriser le PDO reçu par `AddDevice` dans une `static`
//! supposerait une instance unique du périphérique, ce que rien ne garantit ;
//! `IoGetLowerDeviceObject` ne descendrait que d'un cran et se tromperait dès qu'un
//! pilote de filtre inférieur s'intercale.
//!
//! # `OBJ_KERNEL_HANDLE`, et ce qu'on peut réellement en dire
//!
//! `IoOpenDeviceRegistryKey` n'a **pas** de paramètre `OBJECT_ATTRIBUTES` : sa signature
//! est `(DeviceObject, DevInstKeyType, DesiredAccess, DeviceRegKey)`. Il n'existe donc
//! aucun moyen de lui demander `OBJ_KERNEL_HANDLE`, et sa documentation ne dit pas si le
//! descripteur rendu en est un. Ce qu'elle exige, en revanche, est ce qui compte ici :
//! l'appelant doit être « at IRQL = PASSIVE_LEVEL **in the context of a system thread** ».
//! `StartDevice` est appelé sur `IRP_MN_START_DEVICE`, dans un fil du gestionnaire PnP,
//! donc dans le processus System : le descripteur atterrit dans la table de ce processus,
//! là même où `OBJ_KERNEL_HANDLE` l'aurait placé.
//!
//! À quoi s'ajoute la mesure de prudence qui rend la question sans objet : le descripteur
//! est ouvert, utilisé et fermé dans le même appel de [`read_params`]. Il n'est stocké
//! nulle part, ne survit pas au retour, et aucun code utilisateur ne s'exécute entre les
//! deux.
//!
//! # L'écriture, et pourquoi elle ne peut pas faire échouer une propriété
//!
//! [`write_active_cables`] rouvre la clé en `KEY_WRITE` et pose la valeur par
//! `ZwSetValueKey`. Elle peut échouer — registre saturé, ruche en lecture seule, clé
//! retirée sous nos pieds pendant un retrait de périphérique — et **ce n'est pas une raison
//! de refuser la demande de l'utilisateur** : l'état en mémoire est déjà appliqué quand
//! elle est appelée, l'échec part au journal d'événements, et le seul effet est que le
//! prochain démarrage repartira sur la valeur persistée précédemment. Un disque plein ne
//! doit pas empêcher d'activer un câble.
//!
//! IRQL : `PASSIVE_LEVEL` (contexte de `IRP_MN_START_DEVICE` pour la lecture, contexte du
//! gestionnaire de propriété pour l'écriture), exigé par `IoOpenDeviceRegistryKey` comme
//! par `ZwQueryValueKey` et `ZwSetValueKey`.

use core::sync::atomic::{AtomicU32, Ordering};

use conduit_kmd_core::config::{
    ACTIVE_CABLES_DEFAULT, ACTIVE_CABLES_LABEL, ACTIVE_CABLES_VALUE_NAME, CABLE_FORMAT_DEFAULT,
    CABLE_FORMAT_LABEL, CABLE_FORMAT_VALUE_NAMES, CableFormat, sanitize_mask,
};
use conduit_kmd_core::params::{self, Param, Params, RawParams};
use portcls::conduit_com::{NtStatus, nt_success};
use portcls_sys::PDEVICE_OBJECT;
use wdk_sys::_KEY_VALUE_INFORMATION_CLASS::KeyValuePartialInformation;
use wdk_sys::ntddk::{
    IoGetDeviceAttachmentBaseRef, IoOpenDeviceRegistryKey, ObfDereferenceObject, ZwClose,
    ZwQueryValueKey, ZwSetValueKey,
};
use wdk_sys::{
    HANDLE, KEY_READ, KEY_VALUE_PARTIAL_INFORMATION, KEY_WRITE, PLUGPLAY_REGKEY_DEVICE,
    STATUS_SUCCESS, ULONG, UNICODE_STRING, USHORT, WCHAR,
};

use crate::descriptors::apply_cable_format;
use crate::eventlog::{EventLog, kmd_event};

/// Le `REG_DWORD` du décodeur portable est bien celui du WDK.
///
/// [`conduit_kmd_core`] recopie la constante pour rester sans dépendance et testable
/// depuis Linux ; si les deux divergeaient, le pilote rejetterait **toutes** les valeurs
/// et prendrait silencieusement ses défauts, sans qu'aucun test de l'hôte ne bronche.
const _: () = assert!(params::REG_DWORD == wdk_sys::REG_DWORD);

/// `STATUS_OBJECT_NAME_NOT_FOUND` : la valeur n'existe pas dans la clé.
const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xC000_0034_u32 as NtStatus;

/// `STATUS_BUFFER_OVERFLOW` : la valeur existe mais dépasse notre tampon — donc elle
/// n'est pas un `REG_DWORD` de quatre octets.
const STATUS_BUFFER_OVERFLOW: NtStatus = 0x8000_0005_u32 as NtStatus;

/// `STATUS_BUFFER_TOO_SMALL` : même conclusion, autre code selon le chemin du noyau.
const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xC000_0023_u32 as NtStatus;

/// Décalage de la charge utile dans `KEY_VALUE_PARTIAL_INFORMATION`.
const DATA_OFFSET: usize = core::mem::offset_of!(KEY_VALUE_PARTIAL_INFORMATION, Data);

/// Octets du tampon de pile qui reçoit `KEY_VALUE_PARTIAL_INFORMATION`.
///
/// Bien plus large que les quatre octets d'un `REG_DWORD` : une valeur d'un autre type
/// qui **tient** dans le tampon se lit et se signale comme « type inattendu », ce qui
/// nomme la faute ; une valeur qui déborde ne donne plus qu'un `STATUS_BUFFER_OVERFLOW`.
/// Quatre-vingts octets de charge utile couvrent la faute courante — un `REG_SZ` « 16 »
/// saisi dans `regedit` — et n'importe quel `REG_QWORD` ou `REG_BINARY` plausible.
const PARTIAL_INFO_BYTES: usize = DATA_OFFSET.saturating_add(80);

/// Unités UTF-16 réservées à un nom de valeur.
const NAME_UNITS: usize = 32;

// Les cinq noms tiennent dans le tampon (noms ASCII : un octet par unité UTF-16).
const _: () = assert!(Param::Reserve.value_name().len() < NAME_UNITS);
const _: () = assert!(Param::Channels.value_name().len() < NAME_UNITS);
const _: () = assert!(Param::BufferMs.value_name().len() < NAME_UNITS);
const _: () = assert!(Param::PacketMode.value_name().len() < NAME_UNITS);
const _: () = assert!(ACTIVE_CABLES_VALUE_NAME.len() < NAME_UNITS);
// Les seize noms de format aussi : « CableFormat15 » fait treize caractères ASCII, mais la
// vérifier plutôt que la compter est ce qui tiendra le jour où `CABLE_MAX` passera à trois
// chiffres. Motif de tranche : ni indexation ni arithmétique.
const _: () = {
    let mut noms: &[&str] = &CABLE_FORMAT_VALUE_NAMES;
    while let [premier, reste @ ..] = noms {
        assert!(premier.len() < NAME_UNITS);
        noms = reste;
    }
};

/// Codes portés par `UniqueErrorValue` : la seule information qui survivrait si le texte
/// de l'entrée n'arrivait pas jusqu'à l'Observateur (voir [`crate::eventlog`]).
///
/// L'octet de poids faible est le rang du paramètre dans [`Param::ALL`] ; l'octet
/// au-dessus dit la nature de l'anomalie.
pub(crate) mod code {
    /// La clé matérielle du périphérique n'a pas pu être ouverte.
    pub(super) const CLE: u32 = 0x0001_0000;
    /// Valeur absente de la clé.
    pub(super) const ABSENTE: u32 = 0x0002_0000;
    /// Valeur illisible (échec de `ZwQueryValueKey` autre qu'une absence).
    pub(super) const ILLISIBLE: u32 = 0x0003_0000;
    /// Valeur présente, mais d'un type ou d'une taille inexploitables.
    pub(super) const INEXPLOITABLE: u32 = 0x0004_0000;
    /// Valeur lue, mais hors de ses bornes : écrêtée.
    pub(super) const HORS_BORNES: u32 = 0x0005_0000;
    /// La clé matérielle n'a pas pu être ouverte **en écriture** (M1b-04).
    pub(super) const CLE_ECRITURE: u32 = 0x0006_0000;
    /// `ZwSetValueKey` a échoué : l'état actif ne survivra pas au redémarrage.
    pub(super) const ECRITURE: u32 = 0x0007_0000;
    /// La broche système d'un câble ne déclare pas le format que sa clé annonce
    /// (`descriptors::check_cable_pins`, correction de M1b-05).
    ///
    /// Le seul code de ce module qui ne parle pas du registre mais de ce que le registre a
    /// produit : il se compose avec [`super::RANG_FORMAT`] et le numéro du câble, comme les
    /// codes de `CableFormat<n>`, pour qu'une entrée du journal désigne le câble fautif
    /// sans son texte.
    pub(crate) const DESCRIPTEUR: u32 = 0x0008_0000;
    /// La topologie d'un câble ne déclare pas le même nombre de canaux que sa broche wave
    /// (`topo::check_cable_topology`, M1b-05).
    ///
    /// Se compose comme [`DESCRIPTEUR`], dont il est le pendant côté topologie : un endpoint
    /// naît de la connexion des deux filtres, et une divergence entre eux ne se voit ni dans
    /// les tables ni dans le côté wave.
    pub(crate) const TOPOLOGIE: u32 = 0x0009_0000;
    /// Le pilote tourne avec une **expérience** activée par le registre (lot 2 du mode
    /// paquets : `PacketMode = 1`).
    ///
    /// Le seul code de ce module qui ne signale ni une anomalie ni une correction, mais un
    /// pilote qui se comporte volontairement autrement qu'en service. Il se compose avec
    /// [`super::RANG_PACKET_MODE`], comme les autres codes de paramètre.
    pub(super) const EXPERIENCE: u32 = 0x000B_0000;
    /// Aucune intersection entre ce que Windows demande et ce que le câble déclare
    /// (`intersect::Negotiation::resolve`, M1b-21).
    ///
    /// Se compose comme [`DESCRIPTEUR`]. C'est le seul code du module qui ne soit pas émis
    /// au démarrage mais **pendant** l'énumération de l'endpoint : un refus numérique
    /// donne « aucun format » dans le panneau de son, et sans cette entrée rien n'en
    /// resterait. Un seul par miniport et par démarrage.
    pub(crate) const INTERSECTION: u32 = 0x000A_0000;
}

/// Rang du paramètre dans [`Param::ALL`], pour composer un `UniqueErrorValue`.
///
/// [`RANG_MASQUE`] prolonge cette numérotation pour `ActiveCables`, qui n'est pas un
/// `Param` mais partage les codes de lecture.
const fn rang(param: Param) -> u32 {
    match param {
        Param::Reserve => 0,
        Param::Channels => 1,
        Param::BufferMs => 2,
        // Surtout pas 3 : les rangs sont une numérotation **commune** à tout ce module, et 3
        // à 19 sont déjà pris par le masque et les seize formats. Un `PacketMode` glissé à 3
        // ferait porter le même `UniqueErrorValue` à deux pannes sans rapport, et le journal
        // d'événements est précisément ce qui doit rester lisible quand le texte de l'entrée
        // n'arrive pas jusqu'à l'Observateur.
        Param::PacketMode => RANG_PACKET_MODE,
    }
}

/// Rang de `ActiveCables` dans les codes d'événement, à la suite des trois premiers
/// paramètres.
const RANG_MASQUE: u32 = 3;

/// Rang du premier `CableFormat<n>` : les seize occupent 4 à 19, à la suite du masque.
///
/// L'octet de poids faible d'un `UniqueErrorValue` porte donc directement le numéro du
/// câble, décalé de 4 — c'est ce qui rend une entrée du journal exploitable sans le texte
/// (voir [`crate::eventlog`]).
pub(crate) const RANG_FORMAT: u32 = 4;

/// Rang de `PacketMode`, à la suite des seize formats : 20.
///
/// À la fin plutôt qu'à sa place dans [`Param::ALL`], parce que les rangs des formats et du
/// masque sont **déjà** posés et qu'un poste installé peut avoir des entrées de journal qui
/// les portent. Renuméroter ferait relire de travers une entrée d'hier.
const RANG_PACKET_MODE: u32 = RANG_FORMAT.saturating_add(CABLE_FORMAT_VALUE_NAMES.len() as u32);

// Les rangs des seize formats, puis celui du mode paquets, tiennent dans l'octet de poids
// faible du code d'événement : au-delà, ils déborderaient sur l'octet qui dit la nature de
// l'anomalie, et deux pannes différentes porteraient le même code.
const _: () = assert!(
    RANG_PACKET_MODE <= 0xFF,
    "les rangs de code d'événement débordent sur l'octet de nature"
);
// Et aucun rang n'en recouvre un autre : le masque après les trois premiers paramètres, les
// seize formats après le masque, le mode paquets après les formats.
const _: () = assert!(RANG_MASQUE == 3 && RANG_FORMAT == 4 && RANG_PACKET_MODE == 20);

/// Nom de valeur encodé en UTF-16 sur la pile, avec l'`UNICODE_STRING` qui le décrit.
///
/// `ZwQueryValueKey` veut un `PUNICODE_STRING`, c'est-à-dire une longueur en octets et un
/// pointeur — pas de terminateur nul exigé.
struct ValueName {
    units: [WCHAR; NAME_UNITS],
    /// Unités utiles, toujours ≤ `NAME_UNITS`.
    used: usize,
}

impl ValueName {
    /// Encode `nom` en UTF-16, en abandonnant ce qui dépasserait le tampon (impossible
    /// pour nos trois noms : une assertion à la compilation le vérifie).
    fn new(nom: &str) -> Self {
        let mut value = Self {
            units: [0; NAME_UNITS],
            used: 0,
        };
        for unite in nom.encode_utf16() {
            if let Some(slot) = value.units.get_mut(value.used) {
                *slot = unite;
                value.used = value.used.saturating_add(1);
            }
        }
        value
    }

    /// L'`UNICODE_STRING` qui décrit ce nom. Emprunte le tampon : la valeur rendue ne
    /// doit pas survivre à `self`.
    fn as_unicode_string(&mut self) -> UNICODE_STRING {
        // `used <= NAME_UNITS = 32`, donc la longueur en octets tient dans un `USHORT`.
        let octets = USHORT::try_from(self.used.saturating_mul(size_of::<WCHAR>())).unwrap_or(0);
        UNICODE_STRING {
            Length: octets,
            MaximumLength: octets,
            Buffer: self.units.as_mut_ptr(),
        }
    }
}

/// Tampon de pile aligné pour `KEY_VALUE_PARTIAL_INFORMATION` (trois `ULONG` en tête).
#[repr(C, align(8))]
struct PartialInfo([u8; PARTIAL_INFO_BYTES]);

/// Ce que la lecture d'une valeur a donné.
enum Lecture {
    /// Valeur lue et décodée.
    Valeur(u32),
    /// La valeur n'est pas dans la clé.
    Absente,
    /// `ZwQueryValueKey` a échoué pour une autre raison.
    Illisible(NtStatus),
    /// Valeur présente mais inexploitable : type `REG_*` et taille annoncée.
    Inexploitable { kind: ULONG, len: ULONG },
    /// Valeur présente mais plus grande que le tampon : ce n'est pas un `REG_DWORD`.
    TropGrande(ULONG),
}

/// Ouvre la clé matérielle du périphérique avec l'accès `access` (`KEY_READ` ou
/// `KEY_WRITE`). Voir la note sur le PDO et celle sur `OBJ_KERNEL_HANDLE` en tête de
/// module.
///
/// L'accès est un paramètre plutôt qu'un `KEY_READ` en dur : demander `KEY_WRITE` pour une
/// simple lecture ferait échouer l'ouverture sur une ruche en lecture seule, et demander
/// `KEY_READ` pour une écriture la ferait échouer dans `ZwSetValueKey`, plus loin et moins
/// clairement.
///
/// # Safety
///
/// `device` est un objet de périphérique vivant de ce pilote, valide le temps de l'appel,
/// et l'appelant est à `PASSIVE_LEVEL`.
unsafe fn open_device_key(device: PDEVICE_OBJECT, access: ULONG) -> Result<HANDLE, NtStatus> {
    // SAFETY: `device` est valide (contrat) ; la routine rend une référence sur l'objet
    // du bas de la pile, ou sur `device` lui-même s'il n'est attaché à rien.
    let pdo = unsafe { IoGetDeviceAttachmentBaseRef(device.cast()) };
    let mut key: HANDLE = core::ptr::null_mut();
    // SAFETY: `pdo` est référencé et vivant jusqu'au déréférencement ci-dessous ; `key`
    // est une variable locale inscriptible. Un `pdo` nul serait refusé par
    // `IoOpenDeviceRegistryKey` avec `STATUS_INVALID_PARAMETER`, jamais déréférencé.
    let status = unsafe { IoOpenDeviceRegistryKey(pdo, PLUGPLAY_REGKEY_DEVICE, access, &mut key) };
    if !pdo.is_null() {
        // SAFETY: rend la référence prise par `IoGetDeviceAttachmentBaseRef`, comme sa
        // documentation l'exige (« must be matched by a subsequent call to
        // ObDereferenceObject »). `pdo` est non nul et n'est plus utilisé ensuite.
        unsafe { ObfDereferenceObject(pdo.cast()) };
    }
    if nt_success(status) && !key.is_null() {
        Ok(key)
    } else {
        Err(status)
    }
}

/// Lit une valeur `REG_DWORD` de la clé ouverte.
///
/// # Safety
///
/// `key` est un descripteur de clé ouvert en `KEY_READ`, et l'appelant est à
/// `PASSIVE_LEVEL`.
unsafe fn read_dword(key: HANDLE, nom: &str) -> Lecture {
    // `tampon` doit vivre aussi longtemps que l'`UNICODE_STRING` qui le pointe.
    let mut tampon = ValueName::new(nom);
    let mut nom = tampon.as_unicode_string();
    let mut buffer = PartialInfo([0; PARTIAL_INFO_BYTES]);
    let mut result_len: ULONG = 0;
    // `PARTIAL_INFO_BYTES` est une constante bien inférieure à `u32::MAX`.
    let capacity = ULONG::try_from(PARTIAL_INFO_BYTES).unwrap_or(0);

    // SAFETY: `key` est ouvert en lecture (contrat) ; `nom`, `buffer` et `result_len`
    // sont des variables locales vivantes le temps de l'appel, et `capacity` est bien la
    // taille de `buffer`.
    let status = unsafe {
        ZwQueryValueKey(
            key,
            &mut nom,
            KeyValuePartialInformation,
            buffer.0.as_mut_ptr().cast(),
            capacity,
            &mut result_len,
        )
    };
    if !nt_success(status) {
        return match status {
            STATUS_OBJECT_NAME_NOT_FOUND => Lecture::Absente,
            // La valeur existe mais ne tient pas dans un tampon dimensionné pour vingt
            // fois un `REG_DWORD` : ce n'en est pas un. `ResultLength` est alors la taille
            // *totale* requise, en-tête compris — on n'annonce que la charge utile.
            STATUS_BUFFER_OVERFLOW | STATUS_BUFFER_TOO_SMALL => Lecture::TropGrande(
                result_len.saturating_sub(ULONG::try_from(DATA_OFFSET).unwrap_or(0)),
            ),
            _ => Lecture::Illisible(status),
        };
    }

    let info = buffer.0.as_ptr().cast::<KEY_VALUE_PARTIAL_INFORMATION>();
    // SAFETY: `ZwQueryValueKey` a réussi, donc `buffer` contient un
    // `KEY_VALUE_PARTIAL_INFORMATION` complet ; le tampon est aligné pour lui
    // (`#[repr(align(8))]`) et le dépasse largement en taille.
    let (kind, len) = unsafe { ((*info).Type, (*info).DataLength) };

    // Ce que la charge utile occupe *réellement* dans le tampon : ni plus que ce que la
    // valeur annonce, ni plus que ce que le noyau a écrit, ni plus que le tampon.
    let ecrit = usize::try_from(result_len)
        .unwrap_or(0)
        .min(PARTIAL_INFO_BYTES)
        .saturating_sub(DATA_OFFSET);
    let utile = usize::try_from(len).unwrap_or(0).min(ecrit);
    // SAFETY: `DATA_OFFSET + utile <= PARTIAL_INFO_BYTES` par construction de `utile` ;
    // les octets sont initialisés (le tampon l'est à zéro, et le noyau a écrit par-dessus).
    let data = unsafe { core::slice::from_raw_parts(buffer.0.as_ptr().add(DATA_OFFSET), utile) };

    match params::decode_dword(kind, data) {
        Some(valeur) => Lecture::Valeur(valeur),
        // On signale la taille **annoncée**, pas la tranche tronquée : c'est celle que
        // l'administrateur voit dans `regedit`.
        None => Lecture::Inexploitable { kind, len },
    }
}

/// Lit les trois paramètres **et les seize formats de câble** dans la clé matérielle du
/// périphérique, les valide, et journalise tout ce qui a été corrigé.
///
/// **Ne peut pas échouer** : rend toujours des [`Params`] utilisables, et laisse toujours
/// un format valide pour chacun des seize câbles. Voir la règle en tête de module.
///
/// `log` reçoit une entrée par anomalie : clé inaccessible (une seule, toutes les valeurs
/// partant alors sur leur défaut), valeur absente, illisible, inexploitable, ou hors
/// bornes. Une clé complète et correcte n'écrit rien.
///
/// # Ce que la fonction rend, et ce qu'elle dépose
///
/// Les formats ne ressortent pas dans les [`Params`] : ils sont **déposés** dans le magasin
/// de `crate::descriptors` par [`apply_cable_format`], qui est le seul endroit d'où
/// `wave::description` et `topo::channels` sauront les lire. La signature ne change donc
/// pas, et `adapter::start_device` n'a rien à faire de plus — c'est délibéré : le format
/// d'un câble n'intéresse pas l'adaptateur, qui n'enregistre que des sous-périphériques.
///
/// L'ordre compte : les formats sont posés **avant** le retour, donc avant le premier
/// `install_cable`, donc avant que PortCls ne demande le moindre `GetDescription`. Un câble
/// qui prendrait son format après coup ne pourrait plus en changer — PortCls retient le
/// pointeur du descripteur à vie.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique remis à `StartDevice`, valide le temps de
/// l'appel.
pub(crate) unsafe fn read_params(device: PDEVICE_OBJECT, log: EventLog) -> Params {
    // SAFETY: contrat de la fonction relayé.
    let raw = match unsafe { open_device_key(device, KEY_READ) } {
        Ok(key) => {
            // SAFETY: `key` vient d'être ouvert en `KEY_READ` et n'est fermé qu'après.
            let raw = unsafe { read_all(key, log) };
            // Les seize formats, sur la même ouverture de clé (M1b-05).
            // SAFETY: idem.
            unsafe { read_all_cable_formats(key, log) };
            // SAFETY: `key` est le descripteur rendu par `IoOpenDeviceRegistryKey`, que
            // sa documentation demande de fermer par `ZwClose` ; il n'est plus utilisé.
            let status = unsafe { ZwClose(key) };
            if status != STATUS_SUCCESS {
                kmd_log!("registre : ZwClose a échoué : {status:#010x}");
            }
            raw
        }
        Err(status) => {
            kmd_event!(
                log,
                code::CLE,
                "clé matérielle du périphérique illisible ({status:#010x}), \
                 paramètres par défaut"
            );
            // Les seize câbles gardent le format par défaut : le magasin est initialisé
            // dessus, mais le poser explicitement rend le cycle `StopDevice` puis
            // `StartDevice` idempotent — sans quoi un démarrage sans clé conserverait les
            // formats du démarrage précédent.
            defaut_pour_tous_les_cables();
            RawParams::MISSING
        }
    };

    let (params, report) = params::sanitize(raw);
    for correction in report.corrections() {
        kmd_log!("registre : {correction}");
        kmd_event!(
            log,
            code::HORS_BORNES.saturating_add(rang(correction.param)),
            "{correction}"
        );
    }
    // `BufferMs` devient effectif ici (M1b-05) : il était lu, validé et journalisé depuis
    // M1b-01, et n'agissait sur rien.
    BUFFER_MS.store(params.buffer_ms, Ordering::Relaxed);
    // `PacketMode` devient effectif ici (lot 2 du mode paquets), et c'est le **seul** endroit
    // où il est lu : `wave::open_stream` consulte l'atomique, pas le registre. Un changement
    // dans `regedit` ne prend donc effet qu'au prochain démarrage du périphérique, comme pour
    // tous les paramètres de ce module.
    PACKET_MODE.store(u32::from(params.packet_mode), Ordering::Relaxed);
    // `Channels` est journalisé « (ignoré) » plutôt que passé sous silence : il est encore
    // dans l'INF et dans `regedit`, et un administrateur qui vient de le régler doit lire
    // pourquoi rien n'a bougé, au lieu de le déduire (voir `conduit_kmd_core::params`).
    kmd_log!(
        "registre : réserve {} câbles, tampon {} ms, mode paquets {}, Channels = {} (ignoré, \
         supplanté par les CableFormat<n>)",
        params.reserve,
        params.buffer_ms,
        params.packet_mode,
        params.channels
    );
    // Un `PacketMode = 1` ne doit exister que sur une machine d'essai : le dire au journal
    // d'événements est ce qui le fait remarquer sur un poste où il n'aurait rien à faire.
    // Ce n'est pas une correction — la valeur est dans ses bornes — mais une **expérience en
    // cours**, et un pilote qui expose des interfaces qu'il ne sert pas doit l'annoncer.
    if params.packet_mode_actif() {
        kmd_event!(
            log,
            code::EXPERIENCE.saturating_add(RANG_PACKET_MODE),
            "{} ({}) = 1 : les interfaces du mode paquets seront EXPOSÉES sans être servies \
             (les quatre méthodes refusent). Expérience de mesure, à ne jamais laisser sur un \
             poste en service — remettre 0 et redémarrer le périphérique",
            Param::PacketMode.label(),
            Param::PacketMode.value_name()
        );
    }
    params
}

/// Plancher du tampon cyclique, en millisecondes : la valeur de `BufferMs` du dernier
/// `StartDevice`.
///
/// # Pourquoi une `static` plutôt qu'un champ du flux
///
/// `BufferMs` est un paramètre **du pilote**, pas du câble ni du flux : une seule valeur
/// pour tout le périphérique, comme `ReserveSize`. La faire descendre jusqu'à
/// `stream::WaveStream::allocate` par la chaîne `adapter` → `cable` → `wave` → `stream`
/// aurait fait traverser quatre modules à une constante de démarrage, dont deux qui n'en
/// ont que faire. Un atomique lu au moment de l'allocation dit la même chose en une ligne.
///
/// `Relaxed` : écrit une fois par `StartDevice`, avant qu'aucun flux n'existe, et lu à
/// `PASSIVE_LEVEL` par `AllocateAudioBuffer`. Aucune relation d'ordre à établir avec un
/// autre champ — le raisonnement de `cable::Cable::connected`, à l'identique.
static BUFFER_MS: AtomicU32 = AtomicU32::new(params::DEFAULT_BUFFER_MS);

/// Le plancher du tampon cyclique, en millisecondes (paramètre `BufferMs`).
///
/// Toujours dans `MIN_BUFFER_MS..=MAX_BUFFER_MS` : c'est `params::sanitize` qui l'écrête
/// avant l'écriture, et `conduit_kmd_core::buffer_bytes_with_floor` l'écrête de nouveau de
/// son côté. La valeur avant le premier `StartDevice` est le défaut.
///
/// IRQL : quelconque.
#[must_use]
pub(crate) fn buffer_ms() -> u32 {
    BUFFER_MS.load(Ordering::Relaxed)
}

/// Mode paquets demandé par le registre : la valeur de `PacketMode` du dernier
/// `StartDevice`, 0 ou 1.
///
/// # Une `static`, et lue au démarrage — pas à chaque flux
///
/// Même raisonnement que [`BUFFER_MS`] : c'est un paramètre **du pilote**, pas du câble ni du
/// flux, et le faire descendre par la chaîne `adapter` → `cable` → `wave` ferait traverser
/// trois modules à une constante de démarrage. `wave::open_stream` lit cet atomique.
///
/// La conséquence est celle de tous les paramètres de ce module et elle vaut d'être écrite :
/// **modifier `PacketMode` dans `regedit` ne change rien tant que le périphérique n'a pas
/// redémarré**. C'est aussi ce que le relevé rend
/// ([`conduit_kmd_core::config::CablePackets::packet_mode`]) : le mode **effectif**, celui
/// d'ici, et non ce que la clé contient à l'instant de la requête.
///
/// `Relaxed` : écrit une fois par `StartDevice`, avant qu'aucun flux n'existe, et lu à
/// `PASSIVE_LEVEL` par `NewStream`. Aucune relation d'ordre à établir avec un autre champ.
static PACKET_MODE: AtomicU32 = AtomicU32::new(params::DEFAULT_PACKET_MODE);

/// Les interfaces du mode paquets doivent-elles être **exposées** sur les nouveaux flux ?
///
/// Faux par défaut et sur tout poste livré. Vrai, elles sont exposées **sans être servies** :
/// c'est une expérience de mesure, et toute la réserve est écrite sur
/// [`conduit_kmd_core::params::DEFAULT_PACKET_MODE`].
///
/// IRQL : quelconque.
#[must_use]
pub(crate) fn packet_mode() -> bool {
    PACKET_MODE.load(Ordering::Relaxed) != 0
}

/// Lit les trois valeurs et journalise ce qui les a empêchées d'arriver.
///
/// # Safety
///
/// `key` est un descripteur de clé ouvert en `KEY_READ`, et l'appelant est à
/// `PASSIVE_LEVEL`.
unsafe fn read_all(key: HANDLE, log: EventLog) -> RawParams {
    let mut raw = RawParams::MISSING;
    for param in Param::ALL {
        // SAFETY: contrat de la fonction relayé.
        let lue = unsafe { read_dword(key, param.value_name()) };
        let valeur = match lue {
            Lecture::Valeur(valeur) => Some(valeur),
            Lecture::Absente => {
                // L'INF écrit les trois valeurs : leur absence est anormale, même si
                // elle se répare toute seule.
                kmd_event!(
                    log,
                    code::ABSENTE.saturating_add(rang(param)),
                    "{param} absente de la clé du périphérique, repli sur {}",
                    param.default_value()
                );
                None
            }
            Lecture::Illisible(status) => {
                kmd_event!(
                    log,
                    code::ILLISIBLE.saturating_add(rang(param)),
                    "{param} illisible ({status:#010x}), repli sur {}",
                    param.default_value()
                );
                None
            }
            Lecture::Inexploitable { kind, len } => {
                kmd_event!(
                    log,
                    code::INEXPLOITABLE.saturating_add(rang(param)),
                    "{param} : type {kind}, {len} octets (REG_DWORD attendu), \
                     repli sur {}",
                    param.default_value()
                );
                None
            }
            Lecture::TropGrande(taille) => {
                kmd_event!(
                    log,
                    code::INEXPLOITABLE.saturating_add(rang(param)),
                    "{param} : {taille} octets, trop grande pour un REG_DWORD, \
                     repli sur {}",
                    param.default_value()
                );
                None
            }
        };
        match param {
            Param::Reserve => raw.reserve = valeur,
            Param::Channels => raw.channels = valeur,
            Param::BufferMs => raw.buffer_ms = valeur,
            Param::PacketMode => raw.packet_mode = valeur,
        }
    }
    raw
}

// ---------------------------------------------------------------------------------
// `CableFormat<n>` : le format de chaque câble, lu au démarrage (M1b-05).
// ---------------------------------------------------------------------------------

/// Pose le format par défaut sur les seize câbles.
///
/// Appelée quand la clé est inaccessible, et pour la raison d'idempotence expliquée dans
/// [`read_params`] : sans elle, un `StartDevice` sans clé laisserait en place les formats
/// du démarrage précédent, et les descripteurs ne correspondraient plus à ce que le
/// registre décrit.
fn defaut_pour_tous_les_cables() {
    for cable in 0..CABLE_FORMAT_VALUE_NAMES.len() {
        apply_cable_format(cable as u32, CABLE_FORMAT_DEFAULT);
    }
}

/// Lit les seize `CableFormat<n>` et les dépose dans le magasin des descripteurs.
///
/// **Ne peut pas échouer**, comme tout ce module : chaque valeur absente, illisible,
/// inexploitable ou aberrante donne [`CABLE_FORMAT_DEFAULT`] et une entrée de journal qui
/// nomme le câble et le champ fautif.
///
/// Une valeur **absente** est consignée pour la même raison que les trois paramètres et
/// que le masque : l'INF écrit les seize (`[ConduitCable_HW_AddReg]`, engendré par
/// `portcls/tests/inf.rs`), donc sur un poste installé proprement leur absence signifie
/// qu'on les a supprimées — ou que le poste vient d'une version antérieure à M1b-05, cas
/// que le message nomme aussi.
///
/// # Safety
///
/// `key` est un descripteur de clé ouvert en `KEY_READ`, et l'appelant est à
/// `PASSIVE_LEVEL`.
unsafe fn read_all_cable_formats(key: HANDLE, log: EventLog) {
    for (index, nom) in CABLE_FORMAT_VALUE_NAMES.iter().enumerate() {
        let cable = index as u32;
        // SAFETY: contrat de la fonction relayé.
        let lue = unsafe { read_dword(key, nom) };
        let brut = match lue {
            Lecture::Valeur(valeur) => Some(valeur),
            Lecture::Absente => {
                kmd_event!(
                    log,
                    code::ABSENTE
                        .saturating_add(RANG_FORMAT)
                        .saturating_add(cable),
                    "{CABLE_FORMAT_LABEL} {cable} ({nom}) absent de la clé du périphérique \
                     (mise à jour depuis une version antérieure, ou valeur supprimée), \
                     repli sur {:#010x}",
                    CABLE_FORMAT_DEFAULT.encode()
                );
                None
            }
            Lecture::Illisible(status) => {
                kmd_event!(
                    log,
                    code::ILLISIBLE
                        .saturating_add(RANG_FORMAT)
                        .saturating_add(cable),
                    "{CABLE_FORMAT_LABEL} {cable} ({nom}) illisible ({status:#010x}), \
                     repli sur {:#010x}",
                    CABLE_FORMAT_DEFAULT.encode()
                );
                None
            }
            Lecture::Inexploitable { kind, len } => {
                kmd_event!(
                    log,
                    code::INEXPLOITABLE
                        .saturating_add(RANG_FORMAT)
                        .saturating_add(cable),
                    "{CABLE_FORMAT_LABEL} {cable} ({nom}) : type {kind}, {len} octets \
                     (REG_DWORD attendu), repli sur {:#010x}",
                    CABLE_FORMAT_DEFAULT.encode()
                );
                None
            }
            Lecture::TropGrande(taille) => {
                kmd_event!(
                    log,
                    code::INEXPLOITABLE
                        .saturating_add(RANG_FORMAT)
                        .saturating_add(cable),
                    "{CABLE_FORMAT_LABEL} {cable} ({nom}) : {taille} octets, trop grande \
                     pour un REG_DWORD, repli sur {:#010x}",
                    CABLE_FORMAT_DEFAULT.encode()
                );
                None
            }
        };
        let format = match brut {
            Some(brut) => {
                let (format, correction) = CableFormat::sanitize(cable, brut);
                if let Some(correction) = correction {
                    kmd_log!("registre : {correction}");
                    kmd_event!(
                        log,
                        code::HORS_BORNES
                            .saturating_add(RANG_FORMAT)
                            .saturating_add(cable),
                        "{correction}"
                    );
                }
                format
            }
            None => CABLE_FORMAT_DEFAULT,
        };
        apply_cable_format(cable, format);
        kmd_log!(
            "registre : {CABLE_FORMAT_LABEL} {cable} = {} Hz, {} canaux, profondeur {:?}",
            format.sample_rate,
            format.channels,
            format.depth
        );
    }
}

// ---------------------------------------------------------------------------------
// `ActiveCables` : l'état actif des câbles, persisté par le pilote (M1b-04).
// ---------------------------------------------------------------------------------

/// Lit le masque `ActiveCables` dans la clé matérielle du périphérique.
///
/// **Ne peut pas échouer** : rend toujours un masque utilisable, comme [`read_params`] rend
/// toujours des [`Params`]. Clé inaccessible, valeur absente, d'un autre type, illisible :
/// chacun se journalise et se replie sur [`ACTIVE_CABLES_DEFAULT`] (les câbles 1 et 2). Les
/// bits au-delà du dernier câble sont ignorés et signalés
/// ([`conduit_kmd_core::config::sanitize_mask`]).
///
/// Une valeur **absente** est consignée pour la même raison que les trois paramètres :
/// l'INF l'écrit (`[ConduitCable_HW_AddReg]`), donc sur un poste installé proprement son
/// absence signifie que quelqu'un ou quelque chose l'a supprimée. Sur un poste mis à jour
/// depuis une version antérieure à M1b-04, en revanche, elle est normale — d'où un message
/// qui nomme les deux cas plutôt qu'un seul.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique remis à `StartDevice`, valide le temps de l'appel.
pub(crate) unsafe fn read_active_cables(device: PDEVICE_OBJECT, log: EventLog) -> u32 {
    // SAFETY: contrat de la fonction relayé.
    let brut = match unsafe { open_device_key(device, KEY_READ) } {
        Ok(key) => {
            // SAFETY: `key` vient d'être ouvert en `KEY_READ` et n'est fermé qu'après.
            let lue = unsafe { read_dword(key, ACTIVE_CABLES_VALUE_NAME) };
            // SAFETY: `key` est le descripteur rendu par `IoOpenDeviceRegistryKey`, que sa
            // documentation demande de fermer par `ZwClose` ; il n'est plus utilisé.
            let status = unsafe { ZwClose(key) };
            if status != STATUS_SUCCESS {
                kmd_log!("registre : ZwClose a échoué : {status:#010x}");
            }
            lue
        }
        Err(status) => {
            kmd_event!(
                log,
                code::CLE.saturating_add(RANG_MASQUE),
                "clé matérielle illisible ({status:#010x}), {ACTIVE_CABLES_LABEL} \
                 ({ACTIVE_CABLES_VALUE_NAME}) par défaut ({ACTIVE_CABLES_DEFAULT:#06x})"
            );
            Lecture::Illisible(status)
        }
    };

    let valeur = match brut {
        Lecture::Valeur(valeur) => valeur,
        Lecture::Absente => {
            kmd_event!(
                log,
                code::ABSENTE.saturating_add(RANG_MASQUE),
                "{ACTIVE_CABLES_LABEL} ({ACTIVE_CABLES_VALUE_NAME}) absente de la clé du \
                 périphérique (mise à jour depuis une version antérieure, ou valeur \
                 supprimée), repli sur {ACTIVE_CABLES_DEFAULT:#06x}"
            );
            return ACTIVE_CABLES_DEFAULT;
        }
        Lecture::Illisible(status) => {
            kmd_event!(
                log,
                code::ILLISIBLE.saturating_add(RANG_MASQUE),
                "{ACTIVE_CABLES_LABEL} ({ACTIVE_CABLES_VALUE_NAME}) illisible \
                 ({status:#010x}), repli sur {ACTIVE_CABLES_DEFAULT:#06x}"
            );
            return ACTIVE_CABLES_DEFAULT;
        }
        Lecture::Inexploitable { kind, len } => {
            kmd_event!(
                log,
                code::INEXPLOITABLE.saturating_add(RANG_MASQUE),
                "{ACTIVE_CABLES_LABEL} ({ACTIVE_CABLES_VALUE_NAME}) : type {kind}, {len} \
                 octets (REG_DWORD attendu), repli sur {ACTIVE_CABLES_DEFAULT:#06x}"
            );
            return ACTIVE_CABLES_DEFAULT;
        }
        Lecture::TropGrande(taille) => {
            kmd_event!(
                log,
                code::INEXPLOITABLE.saturating_add(RANG_MASQUE),
                "{ACTIVE_CABLES_LABEL} ({ACTIVE_CABLES_VALUE_NAME}) : {taille} octets, \
                 trop grande pour un REG_DWORD, repli sur {ACTIVE_CABLES_DEFAULT:#06x}"
            );
            return ACTIVE_CABLES_DEFAULT;
        }
    };

    let (masque, correction) = sanitize_mask(valeur);
    if let Some(correction) = correction {
        kmd_log!("registre : {correction}");
        kmd_event!(
            log,
            code::HORS_BORNES.saturating_add(RANG_MASQUE),
            "{correction}"
        );
    }
    kmd_log!("registre : {ACTIVE_CABLES_LABEL} = {masque:#06x}");
    masque
}

/// Écrit le masque `ActiveCables` dans la clé matérielle du périphérique.
///
/// Rend le `NTSTATUS` de l'échec, **que l'appelant ne doit pas propager à sa propriété** :
/// l'état en mémoire est déjà appliqué quand cette fonction est appelée, et un registre
/// saturé ne doit pas empêcher d'activer un câble (voir la note d'écriture en tête de
/// module). L'échec part aussi au journal d'événements ici même, pour que l'administrateur
/// le retrouve sans débogueur — c'est le seul symptôme d'un réglage qui ne survivra pas au
/// redémarrage.
///
/// IRQL : `PASSIVE_LEVEL` (contexte du gestionnaire de propriété).
///
/// # Safety
///
/// `device` est un objet de périphérique vivant de ce pilote, valide le temps de l'appel.
pub(crate) unsafe fn write_active_cables(
    device: PDEVICE_OBJECT,
    masque: u32,
    log: EventLog,
) -> Result<(), NtStatus> {
    // SAFETY: contrat de la fonction relayé.
    let key = match unsafe { open_device_key(device, KEY_WRITE) } {
        Ok(key) => key,
        Err(status) => {
            kmd_event!(
                log,
                code::CLE_ECRITURE.saturating_add(RANG_MASQUE),
                "clé matérielle inaccessible en écriture ({status:#010x}) : \
                 {ACTIVE_CABLES_LABEL} = {masque:#06x} ne survivra pas au redémarrage"
            );
            return Err(status);
        }
    };

    // `tampon` doit vivre aussi longtemps que l'`UNICODE_STRING` qui le pointe.
    let mut tampon = ValueName::new(ACTIVE_CABLES_VALUE_NAME);
    let mut nom = tampon.as_unicode_string();
    // `REG_DWORD` est petit-boutiste par définition de `winnt.h`, comme le décodage de
    // `params::decode_dword` le suppose en lecture : les deux sens doivent employer la même
    // convention, sinon un masque relu vaudrait son propre miroir.
    let mut octets = masque.to_le_bytes();
    // `REG_DWORD_BYTES` est une constante de 4 : la conversion ne peut pas échouer.
    let taille = ULONG::try_from(params::REG_DWORD_BYTES).unwrap_or(0);

    // SAFETY: `key` est ouvert en `KEY_WRITE` ; `nom` et `octets` sont des variables
    // locales vivantes le temps de l'appel, et `taille` est bien la taille de `octets`.
    // `TitleIndex` est ignoré et doit valoir 0.
    let status = unsafe {
        ZwSetValueKey(
            key,
            &mut nom,
            0,
            params::REG_DWORD,
            octets.as_mut_ptr().cast(),
            taille,
        )
    };
    // SAFETY: `key` est le descripteur rendu par `IoOpenDeviceRegistryKey`, fermé une seule
    // fois et plus utilisé ensuite — succès comme échec de l'écriture.
    let fermeture = unsafe { ZwClose(key) };
    if fermeture != STATUS_SUCCESS {
        kmd_log!("registre : ZwClose a échoué : {fermeture:#010x}");
    }

    if nt_success(status) {
        kmd_log!("registre : {ACTIVE_CABLES_LABEL} = {masque:#06x} persisté");
        return Ok(());
    }
    kmd_event!(
        log,
        code::ECRITURE.saturating_add(RANG_MASQUE),
        "{ACTIVE_CABLES_LABEL} ({ACTIVE_CABLES_VALUE_NAME}) = {masque:#06x} non écrite \
         ({status:#010x}) : le réglage est actif mais ne survivra pas au redémarrage"
    );
    Err(status)
}
