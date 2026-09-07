//! Lecture des paramètres de registre au démarrage (M1b-01, driver-design.md §2.1).
//!
//! `StartDevice` lit trois `REG_DWORD` dans la **clé matérielle** du périphérique —
//! `ReserveSize`, `Channels`, `BufferMs` — les confronte à leurs bornes par
//! [`conduit_kmd_core::params::sanitize`], et journalise ce qu'il a corrigé.
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
//! IRQL : `PASSIVE_LEVEL` (contexte de `IRP_MN_START_DEVICE`), exigé par
//! `IoOpenDeviceRegistryKey` comme par `ZwQueryValueKey`.

use conduit_kmd_core::params::{self, Param, Params, RawParams};
use portcls::conduit_com::{NtStatus, nt_success};
use portcls_sys::PDEVICE_OBJECT;
use wdk_sys::_KEY_VALUE_INFORMATION_CLASS::KeyValuePartialInformation;
use wdk_sys::ntddk::{
    IoGetDeviceAttachmentBaseRef, IoOpenDeviceRegistryKey, ObfDereferenceObject, ZwClose,
    ZwQueryValueKey,
};
use wdk_sys::{
    HANDLE, KEY_READ, KEY_VALUE_PARTIAL_INFORMATION, PLUGPLAY_REGKEY_DEVICE, STATUS_SUCCESS, ULONG,
    UNICODE_STRING, USHORT, WCHAR,
};

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

// Les trois noms tiennent dans le tampon (noms ASCII : un octet par unité UTF-16).
const _: () = assert!(Param::Reserve.value_name().len() < NAME_UNITS);
const _: () = assert!(Param::Channels.value_name().len() < NAME_UNITS);
const _: () = assert!(Param::BufferMs.value_name().len() < NAME_UNITS);

/// Codes portés par `UniqueErrorValue` : la seule information qui survivrait si le texte
/// de l'entrée n'arrivait pas jusqu'à l'Observateur (voir [`crate::eventlog`]).
///
/// L'octet de poids faible est le rang du paramètre dans [`Param::ALL`] ; l'octet
/// au-dessus dit la nature de l'anomalie.
mod code {
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
}

/// Rang du paramètre dans [`Param::ALL`], pour composer un `UniqueErrorValue`.
const fn rang(param: Param) -> u32 {
    match param {
        Param::Reserve => 0,
        Param::Channels => 1,
        Param::BufferMs => 2,
    }
}

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

/// Ouvre la clé matérielle du périphérique en lecture. Voir la note sur le PDO et celle
/// sur `OBJ_KERNEL_HANDLE` en tête de module.
///
/// # Safety
///
/// `device` est l'objet de périphérique remis à `StartDevice`, valide le temps de
/// l'appel, et l'appelant est à `PASSIVE_LEVEL`.
unsafe fn open_device_key(device: PDEVICE_OBJECT) -> Result<HANDLE, NtStatus> {
    // SAFETY: `device` est valide (contrat) ; la routine rend une référence sur l'objet
    // du bas de la pile, ou sur `device` lui-même s'il n'est attaché à rien.
    let pdo = unsafe { IoGetDeviceAttachmentBaseRef(device.cast()) };
    let mut key: HANDLE = core::ptr::null_mut();
    // SAFETY: `pdo` est référencé et vivant jusqu'au déréférencement ci-dessous ; `key`
    // est une variable locale inscriptible. Un `pdo` nul serait refusé par
    // `IoOpenDeviceRegistryKey` avec `STATUS_INVALID_PARAMETER`, jamais déréférencé.
    let status =
        unsafe { IoOpenDeviceRegistryKey(pdo, PLUGPLAY_REGKEY_DEVICE, KEY_READ, &mut key) };
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

/// Lit les trois paramètres dans la clé matérielle du périphérique, les valide, et
/// journalise tout ce qui a été corrigé.
///
/// **Ne peut pas échouer** : rend toujours des [`Params`] utilisables. Voir la règle en
/// tête de module.
///
/// `log` reçoit une entrée par anomalie : clé inaccessible (une seule, les trois valeurs
/// partant alors sur leur défaut), valeur absente, illisible, inexploitable, ou hors
/// bornes. Une clé complète et correcte n'écrit rien.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `device` est l'objet de périphérique remis à `StartDevice`, valide le temps de
/// l'appel.
pub(crate) unsafe fn read_params(device: PDEVICE_OBJECT, log: EventLog) -> Params {
    // SAFETY: contrat de la fonction relayé.
    let raw = match unsafe { open_device_key(device) } {
        Ok(key) => {
            // SAFETY: `key` vient d'être ouvert en `KEY_READ` et n'est fermé qu'après.
            let raw = unsafe { read_all(key, log) };
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
    kmd_log!(
        "registre : réserve {} câbles, {} canaux, tampon {} ms",
        params.reserve,
        params.channels,
        params.buffer_ms
    );
    params
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
        }
    }
    raw
}
