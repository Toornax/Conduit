//! Client **espace utilisateur** du jeu de propriétés KS privé `KSPROPSETID_Conduit`
//! (M1b-04, `docs/driver-design.md` §6) : ouvrir le filtre de topologie d'un câble et
//! lui parler.
//!
//! Le pilote expose `KSPROPERTY_CONDUIT_CABLE_STATE` (lecture/écriture) et
//! `KSPROPERTY_CONDUIT_VERSION` (lecture) sur la table d'automation du **filtre** de
//! topologie de chaque côté de câble. Ce module est le seul endroit du dépôt qui sache
//! les atteindre depuis l'espace utilisateur ; il servira à la mesure de M1b-04
//! aujourd'hui et au `CableControl` de M1b-34 demain, d'où le découpage :
//! [`TopologyFilter`] est un **transport** (ouvrir, envoyer, lire), pas une politique.
//!
//! # Le contrat vient de `conduit-kmd-core`, il n'est pas recopié
//!
//! Le GUID du jeu, les identifiants de propriété, la disposition des seize octets de
//! [`CableState`] et **tout** son parseur vivent dans
//! [`conduit_kmd_core::config`](conduit_kmd_core::config), que le pilote utilise aussi.
//! Ce module sérialise l'en-tête `KSPROPERTY` (qui, lui, appartient à KS et non à
//! Conduit) et délègue le reste. Un second jeu de décalages ici finirait par diverger
//! en silence de celui du pilote, et la panne se lirait « `STATUS_INVALID_PARAMETER`
//! sur un tampon pourtant correct ».
//!
//! # Comment on trouve le bon filtre
//!
//! Chaque côté de câble publie une interface de périphérique de classe
//! `KSCATEGORY_TOPOLOGY` dont la **chaîne de référence** est le nom passé à
//! `PcRegisterSubdevice` : `TopoRender<n>` et `TopoCapture<n>`, `n` de 0 à
//! [`CABLE_MAX`] − 1 (`portcls::adapter::TOPO_RENDER_NAMES`). L'énumération
//! ([`topology_interfaces`]) rend des chemins de la forme
//!
//! ```text
//! \\?\root#media#0000#{dda54a40-1e4c-11d1-a050-405705c10000}\toporender0
//! ```
//!
//! et [`matches_reference`] retient celui dont la chaîne de référence est **exactement**
//! celle attendue. Voir la documentation de cette fonction pour ce que coûterait un
//! `ends_with`.
//!
//! # L'écriture demande un privilège **armé**, et c'est au client de l'armer
//!
//! Le pilote contrôle toute écriture par
//! `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`, qui exige le privilège
//! **actif** dans le jeton de l'appelant. Or Windows livre un jeton avec ses
//! privilèges *présents mais désactivés* : c'est vrai d'un processus élevé comme du
//! jeton `LocalSystem`, et les deux se font refuser `ERROR_PRIVILEGE_NOT_HELD` (1314)
//! tant qu'ils n'ont rien armé — mesuré en machine virtuelle, `whoami /priv` montrant
//! « `SeLoadDriverPrivilege` … Désactivé » dans les trois contextes essayés.
//!
//! [`armer_privilege`] fait donc le travail que le pilote attend : ouvrir le jeton du
//! processus, y chercher le privilège, l'activer, et rendre un garde qui **restaure**
//! l'état précédent à sa destruction. [`etat_privilege`] le lit sans rien changer.
//!
//! # Ce que ce module n'ouvre pas
//!
//! Aucun flux audio, aucun `IAudioClient`, aucun son : `IOCTL_KS_PROPERTY` est une
//! requête de contrôle sur le filtre de topologie, au même titre que la lecture du
//! volume d'un endpoint (module `volume`). Les seules interfaces atteintes sont celles
//! du pilote Conduit, qui n'existent pas sur une machine sans lui.

use std::fmt;

use conduit_backend::CableId;
/// Le contrat lui-même, ré-exporté : les clients de ce module (l'outil de diagnostic
/// aujourd'hui, `CableControl` demain) lisent la structure d'échange et le nombre de
/// câbles **ici**, sans dépendre du crate du pilote ni en recopier quoi que ce soit.
pub use conduit_kmd_core::config::{CableState, CABLE_MAX};
use conduit_kmd_core::config::{
    ConfigError, ConfigGuid, CABLE_STATE_BYTES, CONFIG_VERSION, KSPROPERTY_CONDUIT_CABLE_STATE,
    KSPROPERTY_CONDUIT_VERSION, KSPROPSETID_CONDUIT,
};
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CONFIGRET, CR_BUFFER_SMALL, CR_SUCCESS,
};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED as WIN32_NOT_ALL_ASSIGNED, GENERIC_READ,
    GENERIC_WRITE, HANDLE, LUID,
};
use windows::Win32::Media::KernelStreaming::{
    IOCTL_KS_PROPERTY, KSCATEGORY_TOPOLOGY, KSPROPERTY_TYPE_GET, KSPROPERTY_TYPE_SET,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenPrivileges,
    LUID_AND_ATTRIBUTES, SE_LOAD_DRIVER_NAME, SE_PRIVILEGE_ENABLED, TOKEN_ACCESS_MASK,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::IO::DeviceIoControl;

// ---------------------------------------------------------------------------------
// La requête KS : un en-tête de 24 octets devant la valeur.
// ---------------------------------------------------------------------------------

/// Taille d'un `KSPROPERTY` (`KSIDENTIFIER`) : le GUID du jeu, l'identifiant, les
/// drapeaux.
pub const KSPROPERTY_BYTES: usize = 24;

/// Décalage du GUID du jeu dans un `KSPROPERTY`.
const O_SET: usize = 0;
/// Décalage de `Id: ULONG`.
const O_ID: usize = 16;
/// Décalage de `Flags: ULONG`.
const O_FLAGS: usize = 20;

// La structure de `ks.h` fait bien 24 octets : GUID (16) + deux `ULONG`.
const _: () = assert!(O_FLAGS + 4 == KSPROPERTY_BYTES);
const _: () = assert!(size_of::<ConfigGuid>() == 16);

/// Sérialise un `GUID` de `guiddef.h` en ses seize octets mémoire.
///
/// Boutisme **natif**, comme [`CableState::to_bytes`] : ce tampon ne traverse qu'une
/// frontière mémoire (l'espace utilisateur vers le noyau, même machine), pas une
/// frontière de format.
#[must_use]
fn guid_bytes(guid: &ConfigGuid) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&guid.data1.to_ne_bytes());
    out[4..6].copy_from_slice(&guid.data2.to_ne_bytes());
    out[6..8].copy_from_slice(&guid.data3.to_ne_bytes());
    out[8..16].copy_from_slice(&guid.data4);
    out
}

/// L'en-tête `KSPROPERTY` d'une requête : le jeu, la propriété, le verbe.
///
/// Recopié octet par octet plutôt que transtypé depuis une structure : c'est la règle
/// que le pilote s'applique déjà de l'autre côté (`portcls::property`), et elle rend la
/// fonction **pure**, donc vérifiable en table de cas sans machine virtuelle.
#[must_use]
pub fn ksproperty_bytes(set: &ConfigGuid, id: u32, flags: u32) -> [u8; KSPROPERTY_BYTES] {
    let mut out = [0u8; KSPROPERTY_BYTES];
    out[O_SET..O_SET + 16].copy_from_slice(&guid_bytes(set));
    out[O_ID..O_ID + 4].copy_from_slice(&id.to_ne_bytes());
    out[O_FLAGS..O_FLAGS + 4].copy_from_slice(&flags.to_ne_bytes());
    out
}

// ---------------------------------------------------------------------------------
// Les deux côtés d'un câble, et le décalage de un.
// ---------------------------------------------------------------------------------

/// Le côté du câble dont on ouvre le filtre de topologie.
///
/// Les deux portent la **même** propriété et le même état : le câble est une entité,
/// pas un côté. Pouvoir choisir sert à vérifier justement cela — un `SET` sur
/// `TopoRender<n>` doit se relire à l'identique sur `TopoCapture<n>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterSide {
    /// `TopoRender<n>` : le côté où les applications **jouent**.
    Render,
    /// `TopoCapture<n>` : le côté où les applications **lisent**.
    Capture,
}

impl FilterSide {
    /// Les deux côtés, dans l'ordre d'enregistrement du pilote.
    pub const ALL: [Self; 2] = [Self::Render, Self::Capture];

    /// Le préfixe du nom de sous-périphérique, source de vérité côté pilote
    /// (`portcls::adapter::TOPO_RENDER_NAMES` / `TOPO_CAPTURE_NAMES`).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Render => "TopoRender",
            Self::Capture => "TopoCapture",
        }
    }

    /// Nom du côté, en français, pour l'affichage.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Render => "rendu",
            Self::Capture => "capture",
        }
    }

    /// Le sens d'endpoint MMDevice correspondant.
    #[must_use]
    pub const fn direction(self) -> conduit_backend::DeviceDirection {
        match self {
            Self::Render => conduit_backend::DeviceDirection::Render,
            Self::Capture => conduit_backend::DeviceDirection::Capture,
        }
    }

    /// La chaîne de référence de l'interface du câble d'**index pilote** `index`
    /// (`TopoRender0`, `TopoCapture15`, …) ; `None` au-delà du dernier câble.
    #[must_use]
    pub fn reference(self, index: u32) -> Option<String> {
        (index < CABLE_MAX).then(|| format!("{}{index}", self.prefix()))
    }
}

/// L'**index pilote** du câble que l'utilisateur appelle « Conduit *n* ».
///
/// **Décalage de un, et c'est ici qu'il se fait.** [`CableId`] est le numéro affiché
/// (`CableId(1)` = « Conduit 1 »), tandis que le pilote numérote ses câbles à partir de
/// zéro : c'est l'index qui suffixe `TopoRender<n>` et celui qui voyage dans
/// [`CableState::cable`]. « Conduit 1 » est donc l'index **0**. `None` pour un numéro
/// nul ou au-delà du dernier câble.
#[must_use]
pub fn driver_index(cable: CableId) -> Option<u32> {
    let index = cable.0.checked_sub(1)?;
    (index < CABLE_MAX).then_some(index)
}

/// L'inverse de [`driver_index`] : le numéro affiché du câble d'index pilote `index`.
#[must_use]
pub fn cable_id(index: u32) -> Option<CableId> {
    (index < CABLE_MAX).then(|| CableId(index.saturating_add(1)))
}

// ---------------------------------------------------------------------------------
// Appariement d'une chaîne de référence.
// ---------------------------------------------------------------------------------

/// La chaîne de référence d'un chemin d'interface de périphérique : ce qui suit le
/// **dernier** `\`. `None` si le chemin n'en porte pas.
///
/// Un chemin d'interface est de la forme
/// `\\?\<identifiant d'instance>#{<GUID de classe>}\<chaîne de référence>` ; ni
/// l'identifiant d'instance ni le GUID ne contiennent de `\`, et SetupAPI interdit le
/// `\` dans une chaîne de référence. Le dernier séparateur est donc bien celui-là.
///
/// Un chemin **sans** chaîne de référence rend le segment `<instance>#{<GUID>}`, qui ne
/// peut être confondu avec aucun de nos noms : la comparaison de [`matches_reference`]
/// le rejette.
#[must_use]
pub fn reference_string(path: &str) -> Option<&str> {
    let (_, last) = path.rsplit_once('\\')?;
    (!last.is_empty()).then_some(last)
}

/// Le chemin `path` désigne-t-il l'interface dont la chaîne de référence est
/// **exactement** `reference` ?
///
/// # Pourquoi une égalité et jamais un `ends_with`
///
/// `path.ends_with("TopoRender1")` est vrai pour `…\TopoRender1` **et** pour
/// `…\TopoRender11` — et un `starts_with` par l'autre bout ferait attraper à
/// `TopoRender1` les câbles `TopoRender10` à `TopoRender15`. C'est très exactement le
/// défaut qui a été corrigé côté endpoints, où `starts_with("Conduit 1")` retenait
/// « Conduit 10 » à « Conduit 16 » (voir `devices::cable_id_from_name`). On isole donc
/// le **segment entier** et on l'égale : `TopoRender1` ≠ `TopoRender11`, quel que soit
/// le nombre de câbles.
///
/// La comparaison ignore la casse ASCII : Windows normalise couramment les chemins
/// d'interface en minuscules (`…\toporender0`) alors que le pilote enregistre
/// `TopoRender0`. Les noms de sous-périphérique sont ASCII par construction
/// (`portcls::adapter::utf16z_numbered` refuse tout autre octet à la compilation), donc
/// aucune subtilité Unicode n'entre ici.
#[must_use]
pub fn matches_reference(path: &str, reference: &str) -> bool {
    reference_string(path).is_some_and(|found| found.eq_ignore_ascii_case(reference))
}

/// Le premier chemin de `paths` dont la chaîne de référence est exactement
/// `reference`.
#[must_use]
pub fn find_reference<'a>(paths: &'a [String], reference: &str) -> Option<&'a str> {
    paths
        .iter()
        .find(|path| matches_reference(path, reference))
        .map(String::as_str)
}

// ---------------------------------------------------------------------------------
// Le code d'erreur, tel quel.
// ---------------------------------------------------------------------------------

/// L'erreur que le système a rendue, **non traduite**.
///
/// C'est le renseignement qui compte quand on éprouve la validation du pilote : un
/// `NTSTATUS` remonte à l'espace utilisateur traduit en code Win32 par le gestionnaire
/// d'entrées-sorties, et c'est ce code-là qu'il faut lire pour reconnaître le refus.
/// On l'affiche donc brut, avec sa traduction usuelle en `NTSTATUS` **présentée comme
/// une hypothèse** — la correspondance n'est pas bijective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsError {
    /// Un code de `GetLastError`, tel quel.
    Win32(u32),
    /// Un `HRESULT` qui n'encode pas un code Win32 (aucune traduction possible).
    Hresult(i32),
}

/// Les codes Win32 qu'on rencontre en éprouvant le jeu de propriétés, avec leur nom et
/// le `NTSTATUS` dont ils sont la traduction usuelle.
///
/// La table est courte **à dessein** : elle ne nomme que ce que le gestionnaire de
/// propriété peut rendre (`portcls::config`) et ce que l'ouverture du filtre peut
/// refuser. Un code absent s'affiche par son numéro, ce qui reste lisible.
const CODES: [(u32, &str, Option<&str>); 12] = [
    (
        1,
        "ERROR_INVALID_FUNCTION",
        Some("STATUS_INVALID_DEVICE_REQUEST"),
    ),
    (2, "ERROR_FILE_NOT_FOUND", None),
    (5, "ERROR_ACCESS_DENIED", Some("STATUS_ACCESS_DENIED")),
    (6, "ERROR_INVALID_HANDLE", None),
    (13, "ERROR_INVALID_DATA", Some("STATUS_DATA_ERROR")),
    (50, "ERROR_NOT_SUPPORTED", Some("STATUS_NOT_SUPPORTED")),
    (
        87,
        "ERROR_INVALID_PARAMETER",
        Some("STATUS_INVALID_PARAMETER"),
    ),
    (
        122,
        "ERROR_INSUFFICIENT_BUFFER",
        Some("STATUS_BUFFER_TOO_SMALL"),
    ),
    (234, "ERROR_MORE_DATA", Some("STATUS_BUFFER_OVERFLOW")),
    (1168, "ERROR_NOT_FOUND", Some("STATUS_NOT_FOUND")),
    // Pas un refus du pilote : le code qu'`AdjustTokenPrivileges` pose quand elle rend
    // `TRUE` sans avoir rien armé. Il n'a pas de `NTSTATUS` derrière lui.
    (1300, "ERROR_NOT_ALL_ASSIGNED", None),
    (
        1314,
        "ERROR_PRIVILEGE_NOT_HELD",
        Some("STATUS_PRIVILEGE_NOT_HELD"),
    ),
];

/// `FACILITY_WIN32` dans un `HRESULT` : `0x8007xxxx`.
const HRESULT_FACILITY_WIN32: u32 = 0x8007_0000;

impl OsError {
    /// Décode un `HRESULT` rendu par la caisse `windows`.
    ///
    /// Les enveloppes de la caisse `windows` capturent `GetLastError` et le rangent
    /// dans un `HRESULT` par `HRESULT_FROM_WIN32` : `0x8007` suivi des seize bits de
    /// poids faible du code. On refait le chemin inverse ; ce qui n'a pas cette forme
    /// reste un `HRESULT`, faute de code Win32 à en extraire.
    #[must_use]
    pub const fn from_hresult(hr: i32) -> Self {
        let bits = hr as u32;
        if bits & 0xFFFF_0000 == HRESULT_FACILITY_WIN32 {
            Self::Win32(bits & 0x0000_FFFF)
        } else {
            Self::Hresult(hr)
        }
    }

    /// Le code Win32, s'il y en a un.
    #[must_use]
    pub const fn win32(self) -> Option<u32> {
        match self {
            Self::Win32(code) => Some(code),
            Self::Hresult(_) => None,
        }
    }

    /// Le nom symbolique du code Win32 (`ERROR_INVALID_PARAMETER`), s'il est connu.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        let code = self.win32()?;
        CODES
            .iter()
            .find(|(known, _, _)| *known == code)
            .map(|(_, name, _)| *name)
    }

    /// Le `NTSTATUS` dont ce code est la traduction **usuelle**, s'il est connu.
    ///
    /// Une hypothèse, pas une certitude : plusieurs `NTSTATUS` se traduisent par le
    /// même code Win32.
    #[must_use]
    pub fn ntstatus(self) -> Option<&'static str> {
        let code = self.win32()?;
        CODES
            .iter()
            .find(|(known, _, _)| *known == code)
            .and_then(|(_, _, status)| *status)
    }
}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(code) => {
                write!(f, "erreur Win32 {code}")?;
                match (self.name(), self.ntstatus()) {
                    (Some(name), Some(status)) => {
                        write!(f, " ({name}, sans doute {status} côté pilote)")
                    }
                    (Some(name), None) => write!(f, " ({name})"),
                    (None, _) => Ok(()),
                }
            }
            Self::Hresult(hr) => write!(f, "HRESULT {:#010x}", *hr as u32),
        }
    }
}

// ---------------------------------------------------------------------------------
// Erreurs du module.
// ---------------------------------------------------------------------------------

/// Ce qui a empêché de parler au jeu de propriétés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CableConfigError {
    /// Numéro de câble hors des câbles adressables (1 à [`CABLE_MAX`]).
    Numero(CableId),
    /// L'énumération des interfaces `KSCATEGORY_TOPOLOGY` a échoué.
    Enumeration {
        /// L'appel fautif.
        appel: &'static str,
        /// Le `CONFIGRET` rendu, tel quel.
        configret: u32,
    },
    /// Aucune interface ne porte cette chaîne de référence : le pilote Conduit n'est
    /// pas chargé, ou ce câble n'existe pas sur cette machine.
    FiltreAbsent {
        /// La chaîne de référence cherchée (`TopoRender0`, …).
        reference: String,
    },
    /// `CreateFileW` a refusé d'ouvrir le filtre.
    Ouverture {
        /// Le chemin d'interface visé.
        chemin: String,
        /// Le code rendu, tel quel.
        erreur: OsError,
    },
    /// `DeviceIoControl` a refusé la requête : **c'est le refus du pilote**, celui
    /// qu'on cherche à lire quand on éprouve la validation.
    Requete {
        /// La propriété visée (`KSPROPERTY_CONDUIT_*`).
        propriete: &'static str,
        /// Le verbe (`GET` ou `SET`).
        verbe: &'static str,
        /// Le code rendu, tel quel.
        erreur: OsError,
    },
    /// La requête a réussi mais la réponse n'a pas la forme attendue.
    Reponse {
        /// Ce qui cloche, en français.
        cause: String,
    },
    /// Un appel du système lié au **privilège** d'écriture a échoué : on ne sait donc
    /// même pas dire dans quel état est le jeton, ce qui est un diagnostic à part
    /// entière et non un refus du pilote.
    Privilege {
        /// L'appel fautif (`OpenProcessToken`, `LookupPrivilegeValueW`, …).
        appel: &'static str,
        /// Le code rendu, tel quel.
        erreur: OsError,
    },
}

impl fmt::Display for CableConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numero(cable) => write!(
                f,
                "câble {} inconnu : les câbles vont de 1 à {CABLE_MAX}",
                cable.0
            ),
            Self::Enumeration { appel, configret } => write!(
                f,
                "{appel} a échoué (CONFIGRET {configret}) : impossible d'énumérer les \
                 interfaces KSCATEGORY_TOPOLOGY"
            ),
            Self::FiltreAbsent { reference } => write!(
                f,
                "aucune interface KSCATEGORY_TOPOLOGY nommée « {reference} » : le pilote \
                 Conduit n'est pas chargé sur cette machine, ou ce câble n'est pas \
                 enregistré (voir docs/driver-dev.md)"
            ),
            Self::Ouverture { chemin, erreur } => {
                write!(f, "ouverture de {chemin} refusée : {erreur}")
            }
            Self::Requete {
                propriete,
                verbe,
                erreur,
            } => write!(f, "{verbe} {propriete} refusé : {erreur}"),
            Self::Reponse { cause } => write!(f, "réponse inattendue du pilote : {cause}"),
            Self::Privilege { appel, erreur } => write!(
                f,
                "{appel} a échoué : {erreur} — impossible de connaître ou d'armer \
                 SeLoadDriverPrivilege dans le jeton de ce processus"
            ),
        }
    }
}

impl std::error::Error for CableConfigError {}

// ---------------------------------------------------------------------------------
// Analyse des réponses (pur).
// ---------------------------------------------------------------------------------

/// Analyse la réponse d'un `GET` de [`KSPROPERTY_CONDUIT_CABLE_STATE`] :
/// `returned` octets utiles dans `buffer`.
///
/// La longueur **rendue** est vérifiée avant le contenu, puis le jugement sur les
/// octets est délégué à [`CableState::from_bytes`] — le parseur du contrat, celui-là
/// même que le pilote applique et que M1b-08 fuzzera. Rien n'est validé deux fois.
///
/// # Erreurs
///
/// [`CableConfigError::Reponse`] si le pilote a écrit un nombre d'octets inattendu ou
/// une valeur que le contrat refuse.
pub fn parse_cable_state(buffer: &[u8], returned: usize) -> Result<CableState, CableConfigError> {
    let utiles = buffer
        .get(..returned)
        .ok_or_else(|| CableConfigError::Reponse {
            cause: format!(
                "{returned} octets annoncés pour un tampon de {} : le pilote a débordé",
                buffer.len()
            ),
        })?;
    if utiles.len() != CABLE_STATE_BYTES {
        return Err(CableConfigError::Reponse {
            cause: format!(
                "{} octets rendus, {CABLE_STATE_BYTES} attendus pour un CableState",
                utiles.len()
            ),
        });
    }
    CableState::from_bytes(utiles).map_err(|e: ConfigError| CableConfigError::Reponse {
        cause: e.to_string(),
    })
}

/// Analyse la réponse d'un `GET` de [`KSPROPERTY_CONDUIT_VERSION`] : un `ULONG`.
///
/// # Erreurs
///
/// [`CableConfigError::Reponse`] si le pilote n'a pas écrit exactement quatre octets.
pub fn parse_version(buffer: &[u8], returned: usize) -> Result<u32, CableConfigError> {
    let utiles = buffer
        .get(..returned)
        .ok_or_else(|| CableConfigError::Reponse {
            cause: format!(
                "{returned} octets annoncés pour un tampon de {} : le pilote a débordé",
                buffer.len()
            ),
        })?;
    let mot: [u8; 4] = utiles.try_into().map_err(|_| CableConfigError::Reponse {
        cause: format!("{} octets rendus, 4 attendus pour un ULONG", utiles.len()),
    })?;
    Ok(u32::from_ne_bytes(mot))
}

/// La version du contrat que ce client connaît, à comparer à celle du pilote.
#[must_use]
pub const fn contract_version() -> u32 {
    CONFIG_VERSION
}

// ---------------------------------------------------------------------------------
// Le privilège d'écriture : présent ne veut pas dire actif.
// ---------------------------------------------------------------------------------

/// `ERROR_NOT_ALL_ASSIGNED` (1300) : **le seul signe** qu'`AdjustTokenPrivileges` n'a
/// pas activé ce qu'on lui a demandé.
///
/// La valeur vient de la caisse `windows`, pas d'un 1300 recopié.
pub const ERROR_NOT_ALL_ASSIGNED: u32 = WIN32_NOT_ALL_ASSIGNED.0;

/// Taille d'un `LUID_AND_ATTRIBUTES` (`winnt.h`) : deux `ULONG` et un `LONG`.
const LUID_ATTR_BYTES: usize = 12;
/// Décalage de `PrivilegeCount` dans un `TOKEN_PRIVILEGES`.
const O_PRIVILEGE_COUNT: usize = 0;
/// Décalage du premier `LUID_AND_ATTRIBUTES` dans un `TOKEN_PRIVILEGES`.
const O_PRIVILEGES: usize = 4;
/// Décalage de `Luid.LowPart` dans un `LUID_AND_ATTRIBUTES`.
const O_LUID_LOW: usize = 0;
/// Décalage de `Luid.HighPart` dans un `LUID_AND_ATTRIBUTES`.
const O_LUID_HIGH: usize = 4;
/// Décalage d'`Attributes` dans un `LUID_AND_ATTRIBUTES`.
const O_ATTRIBUTES: usize = 8;
/// `SE_PRIVILEGE_ENABLED` : le bit qui distingue « détenu » d'« actif ».
///
/// De la caisse `windows` là encore. `SE_PRIVILEGE_ENABLED_BY_DEFAULT` (1) est un bit
/// **différent** et ne veut pas dire actif : il dit seulement que l'ouverture de session
/// l'aurait activé, ce qui n'est pas le cas de `SeLoadDriverPrivilege`.
const ATTR_ENABLED: u32 = SE_PRIVILEGE_ENABLED.0;

// Les tailles de `winnt.h`, vérifiées contre les types de la caisse `windows` : un
// décalage faux ferait lire un privilège pour un autre, et le client annoncerait
// « absent » à un administrateur.
const _: () = assert!(size_of::<LUID_AND_ATTRIBUTES>() == LUID_ATTR_BYTES);
const _: () = assert!(O_LUID_HIGH + 4 == O_ATTRIBUTES && O_ATTRIBUTES + 4 == LUID_ATTR_BYTES);
const _: () = assert!(O_PRIVILEGE_COUNT + 4 == O_PRIVILEGES);
const _: () = assert!(ATTR_ENABLED == 2);

/// L'état de `SeLoadDriverPrivilege` dans un jeton.
///
/// Les trois cas ne se soignent pas de la même façon, et c'est tout l'intérêt de les
/// distinguer : [`Self::Absent`] est un problème de **compte**, [`Self::Desactive`] un
/// armement qui n'a pas été fait, [`Self::Actif`] le seul état que le pilote accepte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtatPrivilege {
    /// Le privilège n'est **pas** dans le jeton : ce compte ne le détient pas.
    Absent,
    /// Présent mais **désactivé**. C'est l'état normal au démarrage d'un processus, y
    /// compris élevé et y compris `LocalSystem` : Windows n'active rien tout seul.
    Desactive,
    /// Présent et **actif** : le seul état que `SeSinglePrivilegeCheck` accepte.
    Actif,
}

impl EtatPrivilege {
    /// Le privilège est-il dans le jeton, actif ou non ?
    #[must_use]
    pub const fn present(self) -> bool {
        !matches!(self, Self::Absent)
    }

    /// Le privilège est-il actif ?
    #[must_use]
    pub const fn actif(self) -> bool {
        matches!(self, Self::Actif)
    }

    /// L'état, en français, pour l'affichage.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Absent => "absent du jeton",
            Self::Desactive => "présent mais désactivé",
            Self::Actif => "présent et actif",
        }
    }
}

impl fmt::Display for EtatPrivilege {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Lit un `ULONG` à `decalage` ; 0 si les quatre octets n'y sont pas.
///
/// Boutisme **natif** : ces octets viennent du noyau de la même machine.
#[must_use]
fn lire_u32(octets: &[u8], decalage: usize) -> u32 {
    octets
        .get(decalage..decalage.saturating_add(4))
        .and_then(|mot| <[u8; 4]>::try_from(mot).ok())
        .map_or(0, u32::from_ne_bytes)
}

/// Cherche le privilège de LUID (`low`, `high`) dans les octets d'un `TOKEN_PRIVILEGES`
/// tel que `GetTokenInformation(TokenPrivileges)` le rend.
///
/// Disposition de `winnt.h` : `PrivilegeCount: ULONG`, puis `PrivilegeCount` entrées de
/// douze octets (`Luid.LowPart: ULONG`, `Luid.HighPart: LONG`, `Attributes: ULONG`).
///
/// Le tampon est lu **octet par octet** plutôt que transtypé vers un
/// `*const TOKEN_PRIVILEGES` : le type de la caisse `windows` déclare un tableau d'un
/// seul `LUID_AND_ATTRIBUTES`, et le parcourir au-delà par arithmétique de pointeur est
/// un comportement indéfini en Rust même là où le C le tolère. Ce faisant, la fonction
/// devient **pure**, donc vérifiable en table de cas sans toucher au jeton du processus.
///
/// Un tampon tronqué, un compte plus grand que le tampon, ou un privilège absent
/// rendent tous [`EtatPrivilege::Absent`] : c'est le verdict prudent, celui qui fait
/// dire au client « ce compte ne l'a pas » plutôt que de laisser croire à un armement.
#[must_use]
pub fn chercher_privilege(octets: &[u8], low: u32, high: i32) -> EtatPrivilege {
    if octets.len() < O_PRIVILEGES {
        return EtatPrivilege::Absent;
    }
    let compte = lire_u32(octets, O_PRIVILEGE_COUNT) as usize;
    let Some(corps) = octets.get(O_PRIVILEGES..) else {
        return EtatPrivilege::Absent;
    };
    corps
        .chunks_exact(LUID_ATTR_BYTES)
        .take(compte)
        .find(|entree| {
            lire_u32(entree, O_LUID_LOW) == low && lire_u32(entree, O_LUID_HIGH) as i32 == high
        })
        .map_or(EtatPrivilege::Absent, |entree| {
            if lire_u32(entree, O_ATTRIBUTES) & ATTR_ENABLED == ATTR_ENABLED {
                EtatPrivilege::Actif
            } else {
                EtatPrivilege::Desactive
            }
        })
}

/// Ce qu'une tentative d'armement a donné.
///
/// Trois issues, parce que trois conduites : changer de compte, signaler une anomalie,
/// ou poursuivre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Armement {
    /// Le privilège est **actif** : l'écriture peut partir.
    Arme,
    /// Le privilège n'est pas dans le jeton : ce compte ne l'obtiendra jamais, aucune
    /// élévation n'y changera rien.
    Absent,
    /// Présent, mais l'activation n'a pas pris. Cas rare, à signaler tel quel.
    NonActivable {
        /// Le code Win32 constaté : `ERROR_NOT_ALL_ASSIGNED` (1300) quand l'appel a
        /// « réussi » sans rien armer, celui du refus quand il a échoué franchement.
        code: u32,
    },
}

impl Armement {
    /// L'écriture peut-elle partir ?
    #[must_use]
    pub const fn arme(self) -> bool {
        matches!(self, Self::Arme)
    }
}

impl fmt::Display for Armement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arme => {
                f.write_str("SeLoadDriverPrivilege armé (actif dans le jeton de ce processus)")
            }
            Self::Absent => f.write_str(
                "SeLoadDriverPrivilege absent du jeton : ce compte ne le détient pas — il faut \
                 un compte administrateur, l'élévation seule n'y changera rien",
            ),
            Self::NonActivable { code } => write!(
                f,
                "SeLoadDriverPrivilege présent dans le jeton mais impossible à activer : \
                 AdjustTokenPrivileges a rendu {}",
                OsError::Win32(*code)
            ),
        }
    }
}

/// Classe ce qu'`AdjustTokenPrivileges` vient de faire, à partir du couple (valeur de
/// retour, code d'erreur) et de ce que le jeton portait avant l'appel.
///
/// # Le piège de cette fonction, et pourquoi il tient dans ce classement
///
/// `AdjustTokenPrivileges` **réussit** (`TRUE`) même quand elle n'a activé aucun des
/// privilèges demandés : le seul signe est `GetLastError() == ERROR_NOT_ALL_ASSIGNED`.
/// Un client qui ne regarde que la valeur de retour croit avoir armé le privilège et se
/// fait refuser l'écriture plus loin sans comprendre — c'est exactement le pas de côté
/// qui a coûté trois contextes d'essai en machine virtuelle.
///
/// `present` vient de [`chercher_privilege`], lu **avant** l'appel : c'est lui qui
/// sépare « ce compte n'a pas le privilège » de « il l'a, et l'activation a quand même
/// échoué ». Le couple (valeur de retour, code d'erreur) rend le même `TRUE` + 1300
/// dans les deux cas et ne peut donc pas les distinguer à lui seul.
///
/// `code` n'a de sens que si `reussi` : c'est le code que l'appel pose lui-même, à lire
/// immédiatement après lui.
#[must_use]
pub const fn classer_armement(present: bool, reussi: bool, code: u32) -> Armement {
    // Le seul cas où l'armement a pris : l'appel a réussi **et** n'a pas signalé qu'il
    // laissait un privilège de côté.
    if reussi && code != ERROR_NOT_ALL_ASSIGNED {
        return Armement::Arme;
    }
    // Sinon rien n'a été activé, et c'est le jeton — pas le code d'erreur — qui dit
    // lequel des deux diagnostics s'applique.
    if present {
        Armement::NonActivable { code }
    } else {
        Armement::Absent
    }
}

/// Le jeton du processus courant, fermé à la destruction.
struct Jeton(HANDLE);

impl Jeton {
    /// Ouvre le jeton du processus courant avec les accès demandés.
    fn ouvrir(acces: TOKEN_ACCESS_MASK) -> Result<Self, CableConfigError> {
        let mut handle = HANDLE::default();
        // SAFETY: `GetCurrentProcess` rend un pseudo-handle constant, valide sans être
        // fermé ; `handle` est une variable de cette pile, vivante pendant tout l'appel
        // et écrite par l'appelé seul.
        unsafe { OpenProcessToken(GetCurrentProcess(), acces, &mut handle) }.map_err(|e| {
            CableConfigError::Privilege {
                appel: "OpenProcessToken",
                erreur: OsError::from_hresult(e.code().0),
            }
        })?;
        Ok(Self(handle))
    }
}

impl Drop for Jeton {
    fn drop(&mut self) {
        // SAFETY: handle rendu par `OpenProcessToken`, fermé une seule fois (le type
        // n'est ni `Copy` ni clonable, et `Drop` ne s'exécute qu'une fois).
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Le `LUID` de `SeLoadDriverPrivilege` sur cette machine.
///
/// Les privilèges bien connus ont partout le même LUID, mais on le demande quand même
/// au système plutôt que d'écrire 10 : c'est l'appel que la documentation prescrit, et
/// il ne coûte rien.
fn luid_load_driver() -> Result<LUID, CableConfigError> {
    let mut luid = LUID::default();
    // SAFETY: `SE_LOAD_DRIVER_NAME` est une chaîne large constante terminée par NUL,
    // publiée par la caisse `windows` ; `PCWSTR::null` est la façon documentée de dire
    // « la machine locale » ; `luid` est une variable de cette pile, écrite par
    // l'appelé seul.
    unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_LOAD_DRIVER_NAME, &mut luid) }.map_err(
        |e| CableConfigError::Privilege {
            appel: "LookupPrivilegeValueW",
            erreur: OsError::from_hresult(e.code().0),
        },
    )?;
    Ok(luid)
}

/// Les octets du `TOKEN_PRIVILEGES` d'un jeton, par la paire d'appels documentée
/// (taille d'abord, contenu ensuite).
///
/// Un `Vec<u8>` suffit : [`chercher_privilege`] lit ces octets champ par champ et ne
/// transtype rien, donc l'alignement du tampon n'entre pas en jeu.
fn octets_des_privileges(jeton: &Jeton) -> Result<Vec<u8>, CableConfigError> {
    let mut taille: u32 = 0;
    // SAFETY: premier appel de la paire : aucun tampon transmis, la seule sortie est
    // `taille`, une variable de cette pile. L'échec est **attendu**
    // (`ERROR_INSUFFICIENT_BUFFER`), d'où le résultat retenu pour le seul cas où il
    // faudrait le rapporter.
    let premier = unsafe { GetTokenInformation(jeton.0, TokenPrivileges, None, 0, &mut taille) };
    if taille == 0 {
        return Err(CableConfigError::Privilege {
            appel: "GetTokenInformation",
            erreur: premier
                .err()
                .map_or(OsError::Win32(0), |e| OsError::from_hresult(e.code().0)),
        });
    }
    let mut tampon = vec![0u8; taille as usize];
    let mut rendus: u32 = 0;
    // SAFETY: `tampon` fait exactement `taille` octets — la taille que l'appel précédent
    // a demandée — et vit pendant tout l'appel ; la longueur transmise est la sienne,
    // c'est elle qui borne ce que le système écrit. `rendus` est une variable de cette
    // pile.
    unsafe {
        GetTokenInformation(
            jeton.0,
            TokenPrivileges,
            Some(tampon.as_mut_ptr().cast()),
            taille,
            &mut rendus,
        )
    }
    .map_err(|e| CableConfigError::Privilege {
        appel: "GetTokenInformation",
        erreur: OsError::from_hresult(e.code().0),
    })?;
    tampon.truncate(rendus.min(taille) as usize);
    Ok(tampon)
}

/// L'état de `SeLoadDriverPrivilege` dans le jeton du processus courant, **sans rien
/// modifier**.
///
/// C'est le diagnostic à afficher quand une écriture est refusée : il sépare « mauvais
/// compte » (privilège absent) de « privilège actif et le pilote refuse quand même »,
/// deux conclusions que le seul code 1314 ne distingue pas.
///
/// # Erreurs
///
/// [`CableConfigError::Privilege`] si le système refuse d'ouvrir le jeton ou d'en
/// rendre les privilèges.
pub fn etat_privilege() -> Result<EtatPrivilege, CableConfigError> {
    let jeton = Jeton::ouvrir(TOKEN_QUERY)?;
    let luid = luid_load_driver()?;
    let octets = octets_des_privileges(&jeton)?;
    Ok(chercher_privilege(&octets, luid.LowPart, luid.HighPart))
}

/// `SeLoadDriverPrivilege` tel qu'il était **avant** l'armement : la destruction du
/// garde restaure cet état.
///
/// Le garde tient le jeton ouvert, ce qui est nécessaire pour pouvoir restaurer. Il
/// doit donc vivre au moins aussi longtemps que les écritures qu'il autorise — et pas
/// plus : un privilège armé plus longtemps que nécessaire est une surface offerte pour
/// rien.
pub struct GardePrivilege {
    /// Le jeton ouvert en `TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY`.
    jeton: Jeton,
    /// L'état précédent, tel qu'`AdjustTokenPrivileges` l'a elle-même écrit.
    precedent: TOKEN_PRIVILEGES,
    /// Faux quand il n'y a rien à défaire : jeton déjà armé, ou armement sans effet.
    a_restaurer: bool,
}

impl fmt::Debug for GardePrivilege {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GardePrivilege")
            .field("a_restaurer", &self.a_restaurer)
            .field("privileges_precedents", &self.precedent.PrivilegeCount)
            .finish_non_exhaustive()
    }
}

impl Drop for GardePrivilege {
    fn drop(&mut self) {
        if !self.a_restaurer {
            return;
        }
        // SAFETY: `self.jeton.0` est ouvert avec `TOKEN_ADJUST_PRIVILEGES` et vit encore
        // (le champ n'est détruit qu'après ce corps) ; `self.precedent` est la structure
        // qu'`AdjustTokenPrivileges` a elle-même écrite, transmise par adresse et non
        // relue par nous. `bufferlength` peut valoir zéro puisqu'aucun état précédent
        // n'est redemandé.
        let _ = unsafe {
            AdjustTokenPrivileges(
                self.jeton.0,
                false,
                Some(&raw const self.precedent),
                0,
                None,
                None,
            )
        };
    }
}

/// Arme `SeLoadDriverPrivilege` dans le jeton du processus courant, et rend le garde
/// qui restaurera l'état précédent.
///
/// Le garde est rendu **dans tous les cas**, y compris quand l'armement n'a pas pris :
/// il ne restaure alors rien, et l'appelant n'a pas à distinguer deux formes de retour.
///
/// # Erreurs
///
/// [`CableConfigError::Privilege`] quand un appel du système échoue au point qu'on ne
/// sache plus rien dire du jeton. Un privilège simplement **absent** n'est pas une
/// erreur : c'est [`Armement::Absent`], un renseignement.
pub fn armer_privilege() -> Result<(Armement, GardePrivilege), CableConfigError> {
    let jeton = Jeton::ouvrir(TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY)?;
    let luid = luid_load_driver()?;
    let avant = chercher_privilege(&octets_des_privileges(&jeton)?, luid.LowPart, luid.HighPart);

    let demande = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let mut precedent = TOKEN_PRIVILEGES::default();
    let mut rendus: u32 = 0;
    let taille = u32::try_from(size_of::<TOKEN_PRIVILEGES>()).unwrap_or(u32::MAX);
    // SAFETY: `jeton` est ouvert avec `TOKEN_ADJUST_PRIVILEGES` et vit pendant tout
    // l'appel ; `demande` et `precedent` sont des structures de cette pile, transmises
    // par adresse avec la taille de `precedent` — c'est elle qui borne ce que le système
    // y écrit. `rendus` est une variable de cette pile.
    let resultat = unsafe {
        AdjustTokenPrivileges(
            jeton.0,
            false,
            Some(&raw const demande),
            taille,
            Some(&raw mut precedent),
            Some(&mut rendus),
        )
    };
    // **Le code d'erreur se lit ici et pas plus loin** : `AdjustTokenPrivileges` le pose
    // elle-même (`ERROR_SUCCESS` ou `ERROR_NOT_ALL_ASSIGNED`) alors qu'elle a rendu
    // `TRUE`, et tout autre appel du système l'écraserait. L'enveloppe de la caisse
    // `windows` ne le consulte pas sur un `TRUE` : elle rend `Ok(())` sans y toucher.
    let code = match &resultat {
        // SAFETY: `GetLastError` ne prend aucun paramètre et lit le code du fil courant.
        Ok(()) => unsafe { GetLastError() }.0,
        Err(e) => OsError::from_hresult(e.code().0).win32().unwrap_or(0),
    };
    let issue = classer_armement(avant.present(), resultat.is_ok(), code);
    let garde = GardePrivilege {
        jeton,
        precedent,
        // Rien à défaire si le jeton était **déjà** armé (on n'a rien changé) ni si
        // l'appel n'a pu écrire aucun état précédent.
        a_restaurer: issue.arme() && !avant.actif() && precedent.PrivilegeCount != 0,
    };
    Ok((issue, garde))
}

// ---------------------------------------------------------------------------------
// La batterie d'entrées invalides (M1b-04).
// ---------------------------------------------------------------------------------

/// Une entrée **volontairement invalide**, envoyée pour vérifier que le pilote la
/// refuse.
///
/// C'est la moitié du critère de M1b-04 (« entrée invalide → `STATUS_INVALID_PARAMETER`
/// sans effet ») : chaque variante vise un contrôle précis, et l'outil affiche le code
/// d'erreur brut pour qu'on lise **lequel** a joué.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadInput {
    /// Quinze octets : un [`CableState`] amputé de son dernier octet.
    TropCourt,
    /// Dix-sept octets : un [`CableState`] valide suivi d'un octet en trop. Le
    /// parseur refuse le préfixe valide, et c'est ce qui rend le format extensible.
    TropLong,
    /// Champ `reserved` non nul : la place réservée reste libre parce qu'on la refuse.
    ReserveNonNulle,
    /// `connected` valant 2 : le contrat n'accepte que 0 et 1, plus strict que le
    /// `BOOL` de KS.
    ConnecteDeux,
    /// Index de câble valide mais **différent** de celui du filtre ouvert : le
    /// gestionnaire compare l'écho à son propre câble et refuse.
    CableAutre,
    /// Index de câble hors du domaine du contrat : refusé par le parseur, avant même
    /// la comparaison.
    CableHorsDomaine,
}

impl BadInput {
    /// Toute la batterie, dans l'ordre d'exécution.
    pub const ALL: [Self; 6] = [
        Self::TropCourt,
        Self::TropLong,
        Self::ReserveNonNulle,
        Self::ConnecteDeux,
        Self::CableAutre,
        Self::CableHorsDomaine,
    ];

    /// Ce que l'entrée viole, en français, pour la ligne de compte rendu.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TropCourt => "charge utile de 15 octets",
            Self::TropLong => "charge utile de 17 octets",
            Self::ReserveNonNulle => "champ réservé non nul",
            Self::ConnecteDeux => "connected = 2",
            Self::CableAutre => "index de câble d'un autre câble",
            Self::CableHorsDomaine => "index de câble hors domaine",
        }
    }

    /// La charge utile à envoyer en `SET` sur le filtre du câble d'index `index`.
    ///
    /// Le reste de la structure est toujours **valide** : une entrée qui violerait deux
    /// règles à la fois ne dirait pas laquelle a fait refuser.
    #[must_use]
    pub fn payload(self, index: u32) -> Vec<u8> {
        let valide = CableState::new(index, true).to_bytes();
        match self {
            Self::TropCourt => valide[..CABLE_STATE_BYTES - 1].to_vec(),
            Self::TropLong => {
                let mut out = valide.to_vec();
                out.push(0);
                out
            }
            Self::ReserveNonNulle => CableState {
                reserved: 0xDEAD_BEEF,
                ..CableState::new(index, true)
            }
            .to_bytes()
            .to_vec(),
            Self::ConnecteDeux => CableState {
                connected: 2,
                ..CableState::new(index, true)
            }
            .to_bytes()
            .to_vec(),
            // Un autre câble **existant** : on éprouve la comparaison de l'écho, pas le
            // domaine. Le modulo garde l'index dans les câbles adressables.
            Self::CableAutre => CableState::new(index.wrapping_add(1) % CABLE_MAX, true)
                .to_bytes()
                .to_vec(),
            Self::CableHorsDomaine => CableState::new(CABLE_MAX, true).to_bytes().to_vec(),
        }
    }
}

// ---------------------------------------------------------------------------------
// Le transport.
// ---------------------------------------------------------------------------------

/// Tous les chemins d'interface de classe `KSCATEGORY_TOPOLOGY` **présents** sur la
/// machine, câbles Conduit et cartes son du poste confondus.
///
/// Une seule énumération suffit pour les trente-deux filtres des seize câbles : on
/// filtre ensuite par [`find_reference`], plutôt que d'interroger le gestionnaire de
/// configuration trente-deux fois.
///
/// # Erreurs
///
/// [`CableConfigError::Enumeration`] si le gestionnaire de configuration refuse.
pub fn topology_interfaces() -> Result<Vec<String>, CableConfigError> {
    // Une interface peut apparaître entre la mesure de la taille et la lecture ; trois
    // tours suffisent largement, et l'échec est franc plutôt qu'une boucle infinie.
    for _ in 0..3 {
        let mut taille: u32 = 0;
        // SAFETY: `taille` est un `u32` de cette pile, vivant pendant tout l'appel et
        // écrit par l'appelé seul ; le GUID est une constante statique ; `PCWSTR::null`
        // est la façon documentée de dire « tous les périphériques ».
        let ret = unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut taille,
                &KSCATEGORY_TOPOLOGY,
                PCWSTR::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if ret != CR_SUCCESS {
            return Err(erreur_configret("CM_Get_Device_Interface_List_SizeW", ret));
        }
        let mut tampon = vec![0u16; taille as usize];
        // SAFETY: `tampon` fait exactement `taille` `u16`, la taille que l'appel
        // précédent a demandée ; l'enveloppe de la caisse `windows` en transmet la
        // longueur. Le GUID est une constante statique.
        let ret = unsafe {
            CM_Get_Device_Interface_ListW(
                &KSCATEGORY_TOPOLOGY,
                PCWSTR::null(),
                &mut tampon,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if ret == CR_BUFFER_SMALL {
            continue;
        }
        if ret != CR_SUCCESS {
            return Err(erreur_configret("CM_Get_Device_Interface_ListW", ret));
        }
        return Ok(split_multi_sz(&tampon));
    }
    Err(CableConfigError::Enumeration {
        appel: "CM_Get_Device_Interface_ListW",
        configret: CR_BUFFER_SMALL.0,
    })
}

/// Traduit un `CONFIGRET` non nul en erreur du module.
fn erreur_configret(appel: &'static str, ret: CONFIGRET) -> CableConfigError {
    CableConfigError::Enumeration {
        appel,
        configret: ret.0,
    }
}

/// Découpe une liste `MULTI_SZ` d'UTF-16 en chaînes, en s'arrêtant au premier élément
/// vide (le NUL final du `MULTI_SZ`).
///
/// Fonction pure, testée en table : c'est le seul endroit où la forme de la réponse du
/// gestionnaire de configuration est interprétée.
#[must_use]
pub fn split_multi_sz(buffer: &[u16]) -> Vec<String> {
    buffer
        .split(|unite| *unite == 0)
        .take_while(|element| !element.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Un filtre de topologie de câble **ouvert**, prêt à recevoir des
/// `IOCTL_KS_PROPERTY`.
///
/// Le handle est fermé à la destruction. L'objet n'ouvre aucun flux audio et n'émet
/// aucun son : voir l'en-tête de module.
pub struct TopologyFilter {
    /// Handle du filtre, fermé par [`Drop`].
    handle: HANDLE,
    /// Le chemin d'interface qui l'a désigné, pour les messages.
    path: String,
    /// Le côté ouvert.
    side: FilterSide,
    /// L'index **pilote** du câble (0 pour « Conduit 1 »).
    index: u32,
}

impl fmt::Debug for TopologyFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TopologyFilter")
            .field("index", &self.index)
            .field("side", &self.side)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl TopologyFilter {
    /// Ouvre le filtre de topologie du côté `side` du câble « Conduit *n* », en
    /// énumérant les interfaces `KSCATEGORY_TOPOLOGY`.
    ///
    /// Pour ouvrir plusieurs filtres, préférer [`topology_interfaces`] suivi de
    /// [`Self::open_in`] : une seule énumération au lieu d'une par filtre.
    ///
    /// # Erreurs
    ///
    /// [`CableConfigError::Numero`], [`CableConfigError::Enumeration`],
    /// [`CableConfigError::FiltreAbsent`] ou [`CableConfigError::Ouverture`].
    pub fn open(cable: CableId, side: FilterSide) -> Result<Self, CableConfigError> {
        let paths = topology_interfaces()?;
        Self::open_in(&paths, cable, side)
    }

    /// Comme [`Self::open`], mais en cherchant dans une liste d'interfaces déjà
    /// énumérée.
    ///
    /// # Erreurs
    ///
    /// Voir [`Self::open`], sans le cas [`CableConfigError::Enumeration`].
    pub fn open_in(
        paths: &[String],
        cable: CableId,
        side: FilterSide,
    ) -> Result<Self, CableConfigError> {
        let index = driver_index(cable).ok_or(CableConfigError::Numero(cable))?;
        let reference = side
            .reference(index)
            .ok_or(CableConfigError::Numero(cable))?;
        let path = find_reference(paths, &reference)
            .ok_or(CableConfigError::FiltreAbsent { reference })?;
        let large: Vec<u16> = path.encode_utf16().chain(core::iter::once(0)).collect();
        // SAFETY: `large` est terminé par NUL et vit pendant tout l'appel. On demande
        // lecture et écriture parce qu'un `SET` doit pouvoir échouer sur le privilège
        // du pilote, et non sur le droit d'ouverture ; l'INF accorde `GRGWGX` à « Tout
        // le monde », comme tout adaptateur audio. Aucun `FILE_FLAG_OVERLAPPED` : les
        // `DeviceIoControl` de ce module sont synchrones.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(large.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| CableConfigError::Ouverture {
            chemin: path.to_string(),
            erreur: OsError::from_hresult(e.code().0),
        })?;
        Ok(Self {
            handle,
            path: path.to_string(),
            side,
            index,
        })
    }

    /// Le chemin d'interface ouvert.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Le côté ouvert.
    #[must_use]
    pub fn side(&self) -> FilterSide {
        self.side
    }

    /// L'index **pilote** du câble (0 pour « Conduit 1 »).
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Le numéro affiché du câble.
    #[must_use]
    pub fn cable(&self) -> CableId {
        cable_id(self.index).unwrap_or(CableId(0))
    }

    /// Lit [`KSPROPERTY_CONDUIT_CABLE_STATE`].
    ///
    /// # Erreurs
    ///
    /// [`CableConfigError::Requete`] si le pilote refuse, [`CableConfigError::Reponse`]
    /// si ce qu'il rend n'est pas un [`CableState`] valide.
    pub fn read_state(&self) -> Result<CableState, CableConfigError> {
        let mut valeur = [0u8; CABLE_STATE_BYTES];
        let rendus = self.property(
            KSPROPERTY_CONDUIT_CABLE_STATE,
            KSPROPERTY_TYPE_GET,
            &mut valeur,
            "KSPROPERTY_CONDUIT_CABLE_STATE",
            "GET",
        )?;
        parse_cable_state(&valeur, rendus)
    }

    /// Lit [`KSPROPERTY_CONDUIT_VERSION`], la version du contrat que le pilote sert.
    ///
    /// # Erreurs
    ///
    /// Voir [`Self::read_state`].
    pub fn read_version(&self) -> Result<u32, CableConfigError> {
        let mut valeur = [0u8; 4];
        let rendus = self.property(
            KSPROPERTY_CONDUIT_VERSION,
            KSPROPERTY_TYPE_GET,
            &mut valeur,
            "KSPROPERTY_CONDUIT_VERSION",
            "GET",
        )?;
        parse_version(&valeur, rendus)
    }

    /// Écrit [`KSPROPERTY_CONDUIT_CABLE_STATE`] : connecte ou déconnecte le câble.
    ///
    /// **L'appelant doit avoir armé [`armer_privilege`] avant** et en tenir le garde
    /// pendant l'appel. Le pilote contrôle l'écriture par
    /// `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`, qui exige le privilège
    /// **actif** : un jeton qui le détient sans l'avoir armé reçoit
    /// `ERROR_PRIVILEGE_NOT_HELD` **avant** toute validation du contenu — c'est vrai
    /// d'un processus élevé comme de `LocalSystem`. L'armement n'est pas fait ici parce
    /// que ce type est un **transport** : modifier le jeton du processus est une
    /// décision qui appartient au programme, pas à un envoi d'IOCTL, et le garde doit
    /// couvrir toute la série d'écritures plutôt que chacune.
    ///
    /// # Erreurs
    ///
    /// [`CableConfigError::Requete`], dont le code Win32 est rendu tel quel.
    pub fn write_state(&self, connected: bool) -> Result<(), CableConfigError> {
        let etat = CableState::new(self.index, connected);
        self.write_raw(&etat.to_bytes()).map(|_| ())
    }

    /// Envoie une charge utile **arbitraire** en `SET` de
    /// [`KSPROPERTY_CONDUIT_CABLE_STATE`], sans la valider.
    ///
    /// C'est le point d'entrée de la batterie de [`BadInput`] : le client doit pouvoir
    /// envoyer ce que le contrat refuse, sinon il ne prouve rien de la validation du
    /// pilote. Rend le nombre d'octets que le pilote annonce avoir traités.
    ///
    /// Même exigence que [`Self::write_state`] : sans [`armer_privilege`], la batterie
    /// entière se solde par des `ERROR_PRIVILEGE_NOT_HELD` et n'éprouve **aucune** des
    /// validations qu'elle vise, le contrôle d'accès passant avant elles.
    ///
    /// # Erreurs
    ///
    /// [`CableConfigError::Requete`] — c'est le cas **attendu** pour une entrée
    /// invalide.
    pub fn write_raw(&self, payload: &[u8]) -> Result<usize, CableConfigError> {
        let mut valeur = payload.to_vec();
        self.property(
            KSPROPERTY_CONDUIT_CABLE_STATE,
            KSPROPERTY_TYPE_SET,
            &mut valeur,
            "KSPROPERTY_CONDUIT_CABLE_STATE",
            "SET",
        )
    }

    /// Envoie une requête `IOCTL_KS_PROPERTY` et rend le nombre d'octets traités.
    ///
    /// L'en-tête `KSPROPERTY` est le tampon d'**entrée** ; la valeur — lue pour un
    /// `GET`, écrite pour un `SET` — est le tampon de **sortie**. C'est la convention
    /// de KS, la même que celle de `ksproxy`.
    fn property(
        &self,
        id: u32,
        flags: u32,
        valeur: &mut [u8],
        propriete: &'static str,
        verbe: &'static str,
    ) -> Result<usize, CableConfigError> {
        let requete = ksproperty_bytes(&KSPROPSETID_CONDUIT, id, flags);
        let mut rendus: u32 = 0;
        // SAFETY: `self.handle` est valide tant que `self` vit (fermé par `Drop`
        // seulement). Les deux tampons sont des tranches de cet appel, vivantes pendant
        // toute sa durée, et les longueurs transmises sont exactement les leurs — c'est
        // ce qui borne ce que le pilote lit et écrit. `rendus` est un `u32` de cette
        // pile. Aucun `OVERLAPPED` : le handle est synchrone (`Self::open_in`).
        let resultat = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_KS_PROPERTY,
                Some(requete.as_ptr().cast()),
                u32::try_from(requete.len()).unwrap_or(u32::MAX),
                (!valeur.is_empty()).then(|| valeur.as_mut_ptr().cast()),
                u32::try_from(valeur.len()).unwrap_or(u32::MAX),
                Some(&mut rendus),
                None,
            )
        };
        match resultat {
            Ok(()) => Ok(rendus as usize),
            Err(e) => Err(CableConfigError::Requete {
                propriete,
                verbe,
                erreur: OsError::from_hresult(e.code().0),
            }),
        }
    }
}

impl Drop for TopologyFilter {
    fn drop(&mut self) {
        // SAFETY: handle rendu par `CreateFileW`, fermé une seule fois (le type n'est
        // ni `Copy` ni clonable, et `Drop` ne s'exécute qu'une fois).
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un chemin d'interface tel que le gestionnaire de configuration le rend, avec la
    /// chaîne de référence `reference`.
    fn chemin(reference: &str) -> String {
        format!("\\\\?\\root#media#0000#{{dda54a40-1e4c-11d1-a050-405705c10000}}\\{reference}")
    }

    #[test]
    fn conduit_1_est_l_index_0_du_pilote() {
        assert_eq!(driver_index(CableId(1)), Some(0));
        assert_eq!(driver_index(CableId(16)), Some(15));
        assert_eq!(driver_index(CableId(0)), None);
        assert_eq!(driver_index(CableId(17)), None);
        assert_eq!(driver_index(CableId(u32::MAX)), None);
        assert_eq!(cable_id(0), Some(CableId(1)));
        assert_eq!(cable_id(15), Some(CableId(16)));
        assert_eq!(cable_id(16), None);
        for n in 1..=CABLE_MAX {
            let index = driver_index(CableId(n)).expect("câble adressable");
            assert_eq!(cable_id(index), Some(CableId(n)));
        }
    }

    #[test]
    fn les_chaines_de_reference_suivent_le_pilote() {
        assert_eq!(
            FilterSide::Render.reference(0).as_deref(),
            Some("TopoRender0")
        );
        assert_eq!(
            FilterSide::Capture.reference(15).as_deref(),
            Some("TopoCapture15")
        );
        assert_eq!(FilterSide::Render.reference(16), None);
        // « Conduit 1 » → index 0 → TopoRender0 : le décalage de un, de bout en bout.
        let index = driver_index(CableId(1)).expect("Conduit 1");
        assert_eq!(
            FilterSide::Render.reference(index).as_deref(),
            Some("TopoRender0")
        );
    }

    #[test]
    fn la_chaine_de_reference_est_le_dernier_segment() {
        assert_eq!(
            reference_string(&chemin("TopoRender0")),
            Some("TopoRender0")
        );
        // Sans chaîne de référence : le dernier segment est l'identifiant d'instance.
        assert_eq!(
            reference_string("\\\\?\\root#media#0000#{dda54a40-1e4c-11d1-a050-405705c10000}"),
            Some("root#media#0000#{dda54a40-1e4c-11d1-a050-405705c10000}")
        );
        assert_eq!(reference_string("sans-separateur"), None);
        assert_eq!(reference_string("finit-par-un-separateur\\"), None);
    }

    /// Le défaut à ne pas réintroduire : `TopoRender1` ne doit **jamais** attraper
    /// `TopoRender10` à `TopoRender15`, ni l'inverse.
    #[test]
    fn l_appariement_est_exact_et_pas_un_prefixe() {
        let cible = chemin("TopoRender1");
        assert!(matches_reference(&cible, "TopoRender1"));
        for n in 10..16 {
            let voisin = chemin(&format!("TopoRender{n}"));
            assert!(
                !matches_reference(&voisin, "TopoRender1"),
                "TopoRender1 a attrapé TopoRender{n}"
            );
            assert!(
                !matches_reference(&cible, &format!("TopoRender{n}")),
                "TopoRender{n} a attrapé TopoRender1"
            );
        }
        // Les seize références d'un côté sont deux à deux exclusives.
        let chemins: Vec<String> = (0..CABLE_MAX)
            .map(|n| chemin(&format!("TopoRender{n}")))
            .collect();
        for n in 0..CABLE_MAX {
            let reference = FilterSide::Render.reference(n).expect("câble adressable");
            let retenus: Vec<&String> = chemins
                .iter()
                .filter(|p| matches_reference(p, &reference))
                .collect();
            assert_eq!(retenus.len(), 1, "{reference} en a retenu {retenus:?}");
        }
    }

    #[test]
    fn l_appariement_ignore_la_casse_mais_rien_d_autre() {
        assert!(matches_reference(&chemin("toporender0"), "TopoRender0"));
        assert!(matches_reference(&chemin("TOPORENDER0"), "TopoRender0"));
        assert!(!matches_reference(&chemin("TopoCapture0"), "TopoRender0"));
        assert!(!matches_reference(&chemin("TopoRender00"), "TopoRender0"));
        assert!(!matches_reference(&chemin("TopoRender0 "), "TopoRender0"));
        assert!(!matches_reference(&chemin("XTopoRender0"), "TopoRender0"));
    }

    #[test]
    fn on_retient_le_bon_chemin_dans_une_liste() {
        let paths: Vec<String> = (0..CABLE_MAX)
            .flat_map(|n| {
                [
                    chemin(&format!("TopoRender{n}")),
                    chemin(&format!("TopoCapture{n}")),
                ]
            })
            // Une carte son du poste : elle aussi publie du KSCATEGORY_TOPOLOGY.
            .chain(core::iter::once(chemin("Topology")))
            .collect();
        for attendu in ["TopoRender0", "TopoCapture15", "Topology"] {
            let voulu = chemin(attendu);
            assert_eq!(find_reference(&paths, attendu), Some(voulu.as_str()));
        }
        assert_eq!(find_reference(&paths, "TopoRender16"), None);
    }

    #[test]
    fn l_en_tete_ksproperty_est_a_sa_place() {
        let octets = ksproperty_bytes(&KSPROPSETID_CONDUIT, KSPROPERTY_CONDUIT_CABLE_STATE, 1);
        assert_eq!(octets.len(), KSPROPERTY_BYTES);
        // Le GUID, dans la disposition mémoire de `guiddef.h`.
        assert_eq!(&octets[0..4], &0x3F1B_27A4u32.to_ne_bytes());
        assert_eq!(&octets[4..6], &0x8C6Eu16.to_ne_bytes());
        assert_eq!(&octets[6..8], &0x4D02u16.to_ne_bytes());
        assert_eq!(
            &octets[8..16],
            &[0x9B, 0x75, 0xE4, 0xA0, 0xD6, 0x1C, 0x8F, 0x3B]
        );
        assert_eq!(&octets[16..20], &0u32.to_ne_bytes());
        assert_eq!(&octets[20..24], &1u32.to_ne_bytes());

        let version = ksproperty_bytes(&KSPROPSETID_CONDUIT, KSPROPERTY_CONDUIT_VERSION, 2);
        assert_eq!(&version[16..20], &1u32.to_ne_bytes());
        assert_eq!(&version[20..24], &2u32.to_ne_bytes());
        // Seuls l'identifiant et les drapeaux changent d'une requête à l'autre.
        assert_eq!(&version[0..16], &octets[0..16]);
    }

    /// `CTL_CODE` de `devioctl.h`, recomposé pour dire d'où vient la constante que la
    /// caisse `windows` publie : `IOCTL_KS_PROPERTY` est un `METHOD_NEITHER` sur le
    /// type de périphérique de Kernel Streaming, fonction 0, accès quelconque.
    fn ctl_code(device: u32, function: u32, method: u32, access: u32) -> u32 {
        (device << 16) | (access << 14) | (function << 2) | method
    }

    #[test]
    fn l_ioctl_est_celui_de_ks() {
        const FILE_DEVICE_KS: u32 = 0x0000_002F;
        const METHOD_NEITHER: u32 = 3;
        const FILE_ANY_ACCESS: u32 = 0;
        assert_eq!(
            IOCTL_KS_PROPERTY,
            ctl_code(FILE_DEVICE_KS, 0, METHOD_NEITHER, FILE_ANY_ACCESS)
        );
        assert_eq!(IOCTL_KS_PROPERTY, 0x002F_0003);
    }

    #[test]
    fn la_reponse_doit_faire_exactement_seize_octets() {
        let valide = CableState::new(2, true).to_bytes();
        let etat = parse_cable_state(&valide, CABLE_STATE_BYTES).expect("état valide");
        assert_eq!(etat.cable, 2);
        assert!(etat.is_connected());

        for rendus in [0, 4, 15] {
            let erreur = parse_cable_state(&valide, rendus).expect_err("longueur {rendus}");
            assert!(matches!(erreur, CableConfigError::Reponse { .. }));
        }
        // Plus d'octets que le tampon n'en contient : le pilote a débordé.
        assert!(matches!(
            parse_cable_state(&valide, 17),
            Err(CableConfigError::Reponse { .. })
        ));
        // Contenu refusé par le contrat : le message vient de `ConfigError`.
        let reserve = CableState {
            reserved: 1,
            ..CableState::new(0, true)
        }
        .to_bytes();
        let erreur = parse_cable_state(&reserve, CABLE_STATE_BYTES).expect_err("réservé non nul");
        assert!(erreur.to_string().contains("réservé"), "{erreur}");
    }

    #[test]
    fn la_version_est_un_ulong() {
        assert_eq!(parse_version(&7u32.to_ne_bytes(), 4).expect("ULONG"), 7);
        assert!(parse_version(&[0u8; 4], 3).is_err());
        assert!(parse_version(&[0u8; 4], 5).is_err());
        assert_eq!(contract_version(), CONFIG_VERSION);
    }

    #[test]
    fn les_entrees_invalides_ne_violent_qu_une_regle_chacune() {
        // Sur le filtre du câble d'index 3.
        assert_eq!(BadInput::TropCourt.payload(3).len(), 15);
        assert_eq!(BadInput::TropLong.payload(3).len(), 17);

        // Toutes les autres font la bonne taille : c'est bien le *contenu* qu'on éprouve.
        for mauvaise in [
            BadInput::ReserveNonNulle,
            BadInput::ConnecteDeux,
            BadInput::CableAutre,
            BadInput::CableHorsDomaine,
        ] {
            assert_eq!(
                mauvaise.payload(3).len(),
                CABLE_STATE_BYTES,
                "{}",
                mauvaise.label()
            );
        }

        // Chacune est refusée par le contrat, sauf `CableAutre` que seul le pilote peut
        // rejeter (l'index est valide, mais ce n'est pas celui du filtre ouvert).
        let refus = |mauvaise: BadInput| CableState::from_bytes(&mauvaise.payload(3));
        assert!(matches!(
            refus(BadInput::TropCourt),
            Err(ConfigError::Longueur { recus: 15 })
        ));
        assert!(matches!(
            refus(BadInput::TropLong),
            Err(ConfigError::Longueur { recus: 17 })
        ));
        assert!(matches!(
            refus(BadInput::ReserveNonNulle),
            Err(ConfigError::Reserved(0xDEAD_BEEF))
        ));
        assert!(matches!(
            refus(BadInput::ConnecteDeux),
            Err(ConfigError::Connected(2))
        ));
        assert!(matches!(
            refus(BadInput::CableHorsDomaine),
            Err(ConfigError::Cable(CABLE_MAX))
        ));
        let autre = refus(BadInput::CableAutre).expect("contrat satisfait");
        assert_eq!(autre.cable, 4);
        assert_ne!(autre.cable, 3);

        // Le modulo garde l'index dans les câbles adressables, même sur le dernier.
        let dernier = refus(BadInput::CableAutre).expect("contrat satisfait");
        assert!(dernier.cable < CABLE_MAX);
        let boucle = CableState::from_bytes(&BadInput::CableAutre.payload(CABLE_MAX - 1))
            .expect("contrat satisfait");
        assert_eq!(boucle.cable, 0);

        assert_eq!(BadInput::ALL.len(), 6);
    }

    #[test]
    fn le_code_win32_est_extrait_du_hresult() {
        // HRESULT_FROM_WIN32(87) = 0x80070057.
        let invalide = OsError::from_hresult(0x8007_0057u32 as i32);
        assert_eq!(invalide.win32(), Some(87));
        assert_eq!(invalide.name(), Some("ERROR_INVALID_PARAMETER"));
        assert_eq!(invalide.ntstatus(), Some("STATUS_INVALID_PARAMETER"));

        // HRESULT_FROM_WIN32(1314) = 0x80070522.
        let privilege = OsError::from_hresult(0x8007_0522u32 as i32);
        assert_eq!(privilege.win32(), Some(1314));
        assert_eq!(privilege.ntstatus(), Some("STATUS_PRIVILEGE_NOT_HELD"));

        // Ce qui n'est pas un code Win32 reste un HRESULT, sans traduction inventée.
        let autre = OsError::from_hresult(0x8000_4005u32 as i32);
        assert_eq!(autre.win32(), None);
        assert_eq!(autre.name(), None);
        assert_eq!(autre.ntstatus(), None);
        assert!(autre.to_string().contains("HRESULT"), "{autre}");

        // Un code Win32 inconnu de la table s'affiche par son numéro.
        let inconnu = OsError::from_hresult(0x8007_1234u32 as i32);
        assert_eq!(inconnu.win32(), Some(0x1234));
        assert_eq!(inconnu.name(), None);
        assert_eq!(inconnu.to_string(), "erreur Win32 4660");
    }

    #[test]
    fn le_message_d_erreur_porte_le_code_brut() {
        let erreur = CableConfigError::Requete {
            propriete: "KSPROPERTY_CONDUIT_CABLE_STATE",
            verbe: "SET",
            erreur: OsError::Win32(1314),
        };
        let texte = erreur.to_string();
        assert!(texte.contains("1314"), "{texte}");
        assert!(texte.contains("ERROR_PRIVILEGE_NOT_HELD"), "{texte}");
        assert!(texte.contains("STATUS_PRIVILEGE_NOT_HELD"), "{texte}");

        let absent = CableConfigError::FiltreAbsent {
            reference: "TopoRender0".to_string(),
        };
        assert!(absent.to_string().contains("TopoRender0"));
        assert!(CableConfigError::Numero(CableId(0))
            .to_string()
            .contains('1'));
    }

    #[test]
    fn le_multi_sz_se_decoupe_au_premier_element_vide() {
        let brut: Vec<u16> = "a\0bb\0\0".encode_utf16().collect();
        assert_eq!(
            split_multi_sz(&brut),
            vec!["a".to_string(), "bb".to_string()]
        );
        // Liste vide : un seul NUL final.
        assert!(split_multi_sz(&[0]).is_empty());
        assert!(split_multi_sz(&[]).is_empty());
        // Ce qui suit le NUL final est ignoré, même si le tampon est plus grand.
        let avec_reste: Vec<u16> = "x\0\0poubelle\0"
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();
        assert_eq!(split_multi_sz(&avec_reste), vec!["x".to_string()]);
    }

    /// Vérification du **branchement** de l'énumération, à lancer à la main.
    ///
    /// `#[ignore]` parce qu'elle interroge le gestionnaire de configuration de la
    /// machine : elle n'ouvre aucun périphérique et n'émet aucun son, mais son
    /// résultat dépend du poste, ce qui n'a pas sa place dans une suite déterministe.
    /// Toute machine ayant une carte son publie au moins une interface
    /// `KSCATEGORY_TOPOLOGY` ; sur une machine sans pilote Conduit, aucune ne porte
    /// nos chaînes de référence, et c'est le résultat attendu.
    #[test]
    #[ignore = "interroge le gestionnaire de configuration de la machine"]
    fn l_enumeration_des_interfaces_de_topologie_repond() {
        let paths = topology_interfaces().expect("énumération KSCATEGORY_TOPOLOGY");
        for path in &paths {
            std::println!("{path}");
        }
        assert!(
            !paths.is_empty(),
            "aucune interface KSCATEGORY_TOPOLOGY : machine sans carte son ?"
        );
        assert!(paths.iter().all(|p| reference_string(p).is_some()));
    }

    /// Un `TOKEN_PRIVILEGES` en octets, tel que `GetTokenInformation` le rend :
    /// le compte, puis les entrées `(LowPart, HighPart, Attributes)`.
    fn token_privileges(entrees: &[(u32, i32, u32)]) -> Vec<u8> {
        let mut out = (entrees.len() as u32).to_ne_bytes().to_vec();
        for (low, high, attributs) in entrees {
            out.extend_from_slice(&low.to_ne_bytes());
            out.extend_from_slice(&high.to_ne_bytes());
            out.extend_from_slice(&attributs.to_ne_bytes());
        }
        out
    }

    /// Le LUID de `SeLoadDriverPrivilege` : partie basse 10, partie haute nulle.
    const LOAD_DRIVER: (u32, i32) = (10, 0);

    #[test]
    fn le_privilege_se_trouve_et_son_activation_se_lit() {
        let (low, high) = LOAD_DRIVER;

        // Absent du jeton : ce compte ne le détient pas.
        let sans = token_privileges(&[(19, 0, 2), (23, 0, 3)]);
        assert_eq!(chercher_privilege(&sans, low, high), EtatPrivilege::Absent);

        // Présent mais désactivé : c'est l'état mesuré dans les trois contextes
        // essayés, `LocalSystem` compris.
        let dormant = token_privileges(&[(19, 0, 2), (low, high, 0), (23, 0, 3)]);
        assert_eq!(
            chercher_privilege(&dormant, low, high),
            EtatPrivilege::Desactive
        );

        // `SE_PRIVILEGE_ENABLED_BY_DEFAULT` (1) **seul** ne veut pas dire actif.
        let par_defaut = token_privileges(&[(low, high, 1)]);
        assert_eq!(
            chercher_privilege(&par_defaut, low, high),
            EtatPrivilege::Desactive
        );

        // Armé : le bit 2, seul ou accompagné.
        for attributs in [ATTR_ENABLED, ATTR_ENABLED | 1, ATTR_ENABLED | 0x8000_0000] {
            let arme = token_privileges(&[(low, high, attributs)]);
            assert_eq!(
                chercher_privilege(&arme, low, high),
                EtatPrivilege::Actif,
                "attributs {attributs:#x}"
            );
        }

        // La partie haute compte : un LUID voisin n'est pas le nôtre.
        let voisin = token_privileges(&[(low, 1, ATTR_ENABLED)]);
        assert_eq!(
            chercher_privilege(&voisin, low, high),
            EtatPrivilege::Absent
        );
        let partie_haute_negative = token_privileges(&[(low, -1, ATTR_ENABLED)]);
        assert_eq!(
            chercher_privilege(&partie_haute_negative, low, -1),
            EtatPrivilege::Actif
        );

        // Les états se résument sans ambiguïté.
        assert!(!EtatPrivilege::Absent.present());
        assert!(EtatPrivilege::Desactive.present() && !EtatPrivilege::Desactive.actif());
        assert!(EtatPrivilege::Actif.present() && EtatPrivilege::Actif.actif());
    }

    /// Un tampon abîmé ne doit ni paniquer ni faire croire à un armement.
    #[test]
    fn un_tampon_de_privileges_abime_est_prudent() {
        let (low, high) = LOAD_DRIVER;
        let complet = token_privileges(&[(low, high, ATTR_ENABLED)]);

        for taille in 0..complet.len() {
            assert_eq!(
                chercher_privilege(&complet[..taille], low, high),
                EtatPrivilege::Absent,
                "tampon de {taille} octets"
            );
        }
        // Un compte plus grand que le tampon : `chunks_exact` s'arrête au dernier
        // groupe complet, rien n'est lu au-delà.
        let mut menteur = complet.clone();
        menteur[0..4].copy_from_slice(&99u32.to_ne_bytes());
        assert_eq!(
            chercher_privilege(&menteur, low, high),
            EtatPrivilege::Actif
        );
        // Un compte nul : les entrées présentes sont ignorées.
        let mut muet = complet.clone();
        muet[0..4].copy_from_slice(&0u32.to_ne_bytes());
        assert_eq!(chercher_privilege(&muet, low, high), EtatPrivilege::Absent);
        // Une entrée tronquée à la fin n'est pas lue à moitié.
        let tronque = &complet[..complet.len() - 1];
        assert_eq!(
            chercher_privilege(tronque, low, high),
            EtatPrivilege::Absent
        );
        assert_eq!(chercher_privilege(&[], low, high), EtatPrivilege::Absent);
    }

    /// **Le piège d'`AdjustTokenPrivileges`** : `TRUE` ne veut pas dire armé.
    #[test]
    fn le_succes_d_adjust_token_privileges_ne_suffit_pas() {
        // Réussi et silencieux : armé, le seul cas où l'écriture peut partir.
        assert_eq!(classer_armement(true, true, 0), Armement::Arme);
        // Réussi (`TRUE`) mais 1300 : rien n'a été armé, malgré la valeur de retour.
        // C'est ici que se joue tout l'intérêt de lire `GetLastError`.
        assert_eq!(
            classer_armement(true, true, ERROR_NOT_ALL_ASSIGNED),
            Armement::NonActivable {
                code: ERROR_NOT_ALL_ASSIGNED
            }
        );
        // Même couple, jeton sans le privilège : ce n'est pas le même diagnostic.
        assert_eq!(
            classer_armement(false, true, ERROR_NOT_ALL_ASSIGNED),
            Armement::Absent
        );
        // L'appel a franchement échoué : le jeton tranche encore.
        assert_eq!(
            classer_armement(true, false, 5),
            Armement::NonActivable { code: 5 }
        );
        assert_eq!(classer_armement(false, false, 5), Armement::Absent);
        // Un code d'erreur résiduel d'un appel précédent ne doit pas faire croire à un
        // échec quand la fonction a réussi sans rien laisser de côté.
        assert_eq!(classer_armement(true, true, 87), Armement::Arme);

        assert!(Armement::Arme.arme());
        assert!(!Armement::Absent.arme());
        assert!(!Armement::NonActivable { code: 1300 }.arme());
    }

    /// Les trois messages doivent être distinguables par l'utilisateur : c'est ce qui
    /// sépare « mauvais compte » de « bogue du pilote ».
    #[test]
    fn les_trois_issues_se_lisent_differemment() {
        let arme = Armement::Arme.to_string();
        assert!(arme.contains("armé"), "{arme}");

        let absent = Armement::Absent.to_string();
        assert!(absent.contains("absent du jeton"), "{absent}");
        assert!(absent.contains("administrateur"), "{absent}");

        let rate = Armement::NonActivable {
            code: ERROR_NOT_ALL_ASSIGNED,
        }
        .to_string();
        assert!(rate.contains("impossible à activer"), "{rate}");
        assert!(rate.contains("ERROR_NOT_ALL_ASSIGNED"), "{rate}");
        assert!(rate.contains("1300"), "{rate}");

        // Les trois sont deux à deux différents.
        assert_ne!(arme, absent);
        assert_ne!(absent, rate);
        assert_ne!(arme, rate);

        // Et les états du jeton aussi.
        let etats: Vec<String> = [
            EtatPrivilege::Absent,
            EtatPrivilege::Desactive,
            EtatPrivilege::Actif,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(etats.len(), 3);
        for (rang, texte) in etats.iter().enumerate() {
            assert!(!texte.is_empty());
            assert!(
                !etats.iter().skip(rang + 1).any(|autre| autre == texte),
                "{texte} apparaît deux fois"
            );
        }
    }

    /// L'erreur d'un appel de privilège nomme l'appel fautif et le code brut.
    #[test]
    fn l_erreur_de_privilege_nomme_l_appel() {
        let erreur = CableConfigError::Privilege {
            appel: "OpenProcessToken",
            erreur: OsError::Win32(5),
        };
        let texte = erreur.to_string();
        assert!(texte.contains("OpenProcessToken"), "{texte}");
        assert!(texte.contains("ERROR_ACCESS_DENIED"), "{texte}");
        assert!(texte.contains("SeLoadDriverPrivilege"), "{texte}");
    }

    /// Vérification du **branchement** de la lecture du jeton, à lancer à la main.
    ///
    /// `#[ignore]` parce qu'elle interroge le jeton du processus courant : elle ne le
    /// modifie pas, mais son résultat dépend du compte qui lance la suite, ce qui n'a
    /// pas sa place dans une suite déterministe. Aucun flux audio, aucun son.
    #[test]
    #[ignore = "interroge le jeton du processus courant"]
    fn l_etat_du_privilege_se_lit_sur_cette_machine() {
        let etat = etat_privilege().expect("lecture du jeton du processus");
        std::println!("SeLoadDriverPrivilege : {etat}");
        assert!(matches!(
            etat,
            EtatPrivilege::Absent | EtatPrivilege::Desactive | EtatPrivilege::Actif
        ));
    }

    /// Vérification du **branchement** de l'armement, à lancer à la main.
    ///
    /// `#[ignore]` parce qu'elle **modifie** le jeton du processus courant — le garde le
    /// restaure aussitôt, ce que le test vérifie. Aucun flux audio, aucun son, et aucune
    /// écriture sur le pilote : seul le jeton de ce processus est touché.
    #[test]
    #[ignore = "modifie puis restaure le jeton du processus courant"]
    fn l_armement_se_restaure() {
        let avant = etat_privilege().expect("lecture du jeton du processus");
        {
            let (issue, garde) = armer_privilege().expect("armement");
            std::println!("avant : {avant} / issue : {issue} / {garde:?}");
            // Sur un compte qui le détient, l'armement doit prendre ; sur un compte qui
            // ne l'a pas, l'issue doit le dire — mais jamais « armé » à tort.
            assert_eq!(issue.arme(), etat_privilege().expect("relecture").actif());
        }
        assert_eq!(
            etat_privilege().expect("relecture après restauration"),
            avant,
            "le garde n'a pas restauré l'état précédent du jeton"
        );
    }

    #[test]
    fn les_deux_cotes_se_nomment_et_se_dirigent() {
        assert_eq!(FilterSide::ALL.len(), 2);
        assert_eq!(FilterSide::Render.label(), "rendu");
        assert_eq!(FilterSide::Capture.label(), "capture");
        assert_eq!(
            FilterSide::Render.direction(),
            conduit_backend::DeviceDirection::Render
        );
        assert_eq!(
            FilterSide::Capture.direction(),
            conduit_backend::DeviceDirection::Capture
        );
    }
}
