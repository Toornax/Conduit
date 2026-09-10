//! Corrections manuelles des bindings là où le mode C des en-têtes du WDK 26100 est
//! incomplet (driver-design.md §2.2).
//!
//! Toutes les interfaces PortCls utilisées par Conduit (`IMiniportWaveRT`,
//! `IMiniportTopology`, `IPortWaveRT`, `IAdapterPowerManagement`…) recopient les slots
//! hérités (`DEFINE_ABSTRACT_UNKNOWN()`, `DEFINE_ABSTRACT_MINIPORT()`…) et sortent donc
//! de bindgen avec la disposition COM exacte. Quatre interfaces de `portcls.h` ne le
//! font pas :
//!
//! - `IPortClsVersion` : `GetVersion(THIS)` seul, sans les trois slots de `IUnknown`.
//!   La vtable générée (1 slot) est bloquée dans `build.rs` et redéfinie ici avec le
//!   préfixe `IUnknown` ; `tests/layout.golden` (`sizeof(IUnknownVtbl) +
//!   sizeof(IPortClsVersionVtbl)` côté C) et `tests/vtables.rs` la vérifient.
//! - `IPortClsPower`, `IPortClsRuntimePower`, `IPortClsEtwHelper` : méthodes sans
//!   `THIS_` ni `IUnknown` (déclarations C++ seulement) : slots et signatures faux en C,
//!   exclues des bindings. À écrire à la main, avec `This` en premier paramètre et le
//!   préfixe `IUnknown`, si une tâche ultérieure en a besoin.
//!
//! S'y ajoutent quelques macros que bindgen n'émet pas (transtypage, `sizeof`) ou qui
//! viennent d'un en-tête hors de la liste d'autorisation (`mmreg.h`), recopiées ici avec
//! leur valeur du WDK 26100 : [`PCFILTER_NODE`], [`WAVE_FORMAT_PCM`],
//! [`WAVE_FORMAT_IEEE_FLOAT`], [`WAVE_FORMAT_EXTENSIBLE`].
//!
//! # Propriétés d'interface de périphérique (`wdm.h`, `devpropdef.h`)
//!
//! Déclarer les contraintes de taille de paquet WaveRT demande trois choses que les
//! bindings n'ont pas, et pour trois raisons différentes :
//!
//! - la **clé** `DEVPKEY_KsAudio_PacketSize_Constraints2` : elle est bien dans `ksmedia.h`,
//!   mais sous la macro `DEFINE_DEVPROPKEY`, qui sans `INITGUID` ne produit qu'un
//!   `extern const` sans définition — un symbole que rien ne fournirait à l'édition de
//!   liens. `build.rs` met donc `DEVPKEY_.*` en liste de blocage, et la clé est recopiée
//!   ici, GUID et `pid`, depuis `ksmedia.h` l. 1046-1048. Le golden de disposition
//!   (`tests/layout.golden`, produit par `cl.exe` **avec** `INITGUID`) la vérifie ;
//! - le **type** `DEVPROP_TYPE_BINARY` et la locale `LOCALE_NEUTRAL` : `devpropdef.h` et
//!   `ntdef.h` ne sont pas dans la liste d'autorisation ;
//! - les **fonctions** `IoRegisterDeviceInterface`, `IoSetDeviceInterfacePropertyData` et
//!   `RtlFreeUnicodeString` : `wdm.h` non plus. Elles sont déclarées ici comme les `Pc*`
//!   le sont dans les bindings — sans `#[link]`, pour que le crate reste testable en mode
//!   utilisateur ; c'est `conduit-kmd` qui lie `ntoskrnl.lib` (par `wdk-build`).

use core::{ffi::c_void, mem::size_of};

use crate::bindings::{
    DEVPROPKEY, DWORD, GUID, IID, IUnknown, LCID, NTSTATUS, PDEVICE_OBJECT, PUNICODE_STRING, PVOID,
    ULONG, ULONG_PTR,
};

/// `PORT_CLASS_DEVICE_EXTENSION_SIZE` (`portcls.h`) : taille de l'extension de
/// périphérique réservée par PortCls, `64 * sizeof(ULONG_PTR)`.
///
/// Macro à `sizeof`, que bindgen n'évalue pas ; recopiée ici (512 octets sur x64).
pub const PORT_CLASS_DEVICE_EXTENSION_SIZE: usize = 64 * size_of::<ULONG_PTR>();

/// `PCFILTER_NODE` (`portcls.h`, alias de `KSFILTER_NODE` dans `ks.h`) : `((ULONG)-1)`,
/// le « nœud » qui désigne le filtre lui-même dans une `PCCONNECTION_DESCRIPTOR`
/// (connexion directe d'une broche du filtre à une autre).
///
/// Macro à transtypage, que bindgen n'émet pas ; recopiée ici.
pub const PCFILTER_NODE: ULONG = ULONG::MAX;

/// `WAVE_FORMAT_PCM` (`mmreg.h`, en-tête hors de la liste d'autorisation des bindings) :
/// `wFormatTag` d'un `WAVEFORMATEX` PCM entier.
pub const WAVE_FORMAT_PCM: u16 = 0x0001;

/// `WAVE_FORMAT_IEEE_FLOAT` (`mmreg.h`) : `wFormatTag` d'un `WAVEFORMATEX` flottant.
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;

/// `WAVE_FORMAT_EXTENSIBLE` (`mmreg.h`) : `wFormatTag` annonçant un
/// `WAVEFORMATEXTENSIBLE` (sous-format dans `SubFormat`, bits valides dans `Samples`).
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// `DEVPROPTYPE` (`devpropdef.h`, hors de la liste d'autorisation) : le type d'une valeur
/// de propriété d'interface de périphérique, un `ULONG`.
pub type DEVPROPTYPE = ULONG;

/// `DEVPROP_TYPE_BINARY` (`devpropdef.h`) : `DEVPROP_TYPE_BYTE | DEVPROP_TYPEMOD_ARRAY`,
/// soit `0x03 | 0x1000`. Le type d'une valeur binaire de longueur libre — celui que
/// `KSAUDIO_PACKETSIZE_CONSTRAINTS2` exige, la structure étant de longueur variable.
pub const DEVPROP_TYPE_BINARY: DEVPROPTYPE = 0x0000_1003;

/// `LOCALE_NEUTRAL` (`ntdef.h`) : `MAKELCID(MAKELANGID(LANG_NEUTRAL, SUBLANG_NEUTRAL),
/// SORT_DEFAULT)`, donc **zéro**. La locale que passent tous les appels de propriété
/// d'interface qui ne portent pas de texte traduisible — c'est celle de SYSVAD.
pub const LOCALE_NEUTRAL: LCID = 0;

/// `DEVPKEY_KsAudio_PacketSize_Constraints2` (`ksmedia.h` l. 1046-1048,
/// `{9404F781-7191-409B-8B0B-80BF6EC229AE},2`, `DEVPROP_TYPE_BINARY`).
///
/// La propriété d'interface PnP par laquelle un pilote audio déclare ses contraintes de
/// taille de paquet (`KSAUDIO_PACKETSIZE_CONSTRAINTS2`). « Low Latency Audio » la classe
/// parmi les obligations d'un pilote qui veut des périodes plus courtes que 10 ms, et la
/// documentation de la structure dit où la poser : « on the PnP interface of the KS filter
/// that has the streaming pins ».
///
/// Recopiée à la main, faute de `INITGUID` (voir l'en-tête de module) ; `tests/layout.rs`
/// la confronte à ce que `cl.exe` a lu dans le même en-tête.
#[allow(non_upper_case_globals)]
pub const DEVPKEY_KsAudio_PacketSize_Constraints2: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID {
        Data1: 0x9404_f781,
        Data2: 0x7191,
        Data3: 0x409b,
        Data4: [0x8b, 0x0b, 0x80, 0xbf, 0x6e, 0xc2, 0x29, 0xae],
    },
    pid: 2,
};

/// Décalage du tableau `ProcessingModeConstraints` dans `KSAUDIO_PACKETSIZE_CONSTRAINTS2`,
/// c'est-à-dire la taille de la structure **sans aucune** contrainte de mode : quatre
/// `ULONG`, seize octets.
///
/// C'est la longueur qu'il faut passer à `IoSetDeviceInterfacePropertyData` quand
/// `NumProcessingModeConstraints` vaut zéro, et [`packet_size_constraints_bytes`] la
/// prolonge pour les autres cas. `size_of` ne convient pas : le `ANYSIZE_ARRAY` du WDK
/// vaut 1, donc la structure C mesure toujours une contrainte de plus qu'elle n'en porte.
pub const PACKET_SIZE_CONSTRAINTS2_HEADER_BYTES: usize = core::mem::offset_of!(
    crate::bindings::KSAUDIO_PACKETSIZE_CONSTRAINTS2,
    ProcessingModeConstraints
);

/// Longueur, en octets, d'une valeur `DEVPKEY_KsAudio_PacketSize_Constraints2` portant
/// `count` contraintes de mode de traitement.
///
/// **Exactement** `PACKET_SIZE_CONSTRAINTS2_HEADER_BYTES + count × sizeof(contrainte)`, et
/// c'est ce que les deux exemples de la documentation Microsoft mesurent : la variante
/// capture de SYSVAD annonce une contrainte et 40 octets (16 + 24), la variante rendu en
/// annonce deux et 64 (16 + 2 × 24). Rien n'est arrondi à la taille de la structure C.
///
/// `None` si le produit déborde.
#[must_use]
pub const fn packet_size_constraints_bytes(count: u32) -> Option<usize> {
    let unite = size_of::<crate::bindings::KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT>();
    match (count as usize).checked_mul(unite) {
        Some(octets) => PACKET_SIZE_CONSTRAINTS2_HEADER_BYTES.checked_add(octets),
        None => None,
    }
}

// SAFETY: aucune de ces fonctions n'est appelée par le crate lui-même ; seul `conduit-kmd`
// les atteint, à travers les enveloppes sûres de `portcls::adapter`, et c'est lui qui lie
// `ntoskrnl.lib`.
unsafe extern "C" {
    /// `IoRegisterDeviceInterface` (`wdm.h`) : enregistre — ou **retrouve**, si elle existe
    /// déjà — l'interface de périphérique de classe `InterfaceClassGuid` et de chaîne de
    /// référence `ReferenceString` sur l'objet de périphérique **physique** (PDO)
    /// `PhysicalDeviceObject`, et rend son lien symbolique.
    ///
    /// C'est le second usage qui nous intéresse : PortCls enregistre lui-même l'interface
    /// de chaque sous-périphérique, et rappeler cette fonction sur la même catégorie et la
    /// même chaîne de référence rend le lien symbolique **existant** plutôt que d'en créer
    /// un second. C'est ainsi que SYSVAD obtient le nom sur lequel poser ses propriétés.
    ///
    /// `SymbolicLinkName` est alloué par le noyau : à libérer par [`RtlFreeUnicodeString`].
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    #[must_use]
    pub fn IoRegisterDeviceInterface(
        PhysicalDeviceObject: PDEVICE_OBJECT,
        InterfaceClassGuid: *const GUID,
        ReferenceString: PUNICODE_STRING,
        SymbolicLinkName: PUNICODE_STRING,
    ) -> NTSTATUS;

    /// `IoSetDeviceInterfacePropertyData` (`wdm.h`) : pose la valeur `Data` (`Size` octets,
    /// de type `Type`) sur la propriété `PropertyKey` de l'interface `SymbolicLinkName`.
    ///
    /// `Flags` vaut 0 pour une propriété **volatile**, `PLUGPLAY_PROPERTY_PERSISTENT` (1)
    /// pour une propriété qui survit au redémarrage.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    #[must_use]
    pub fn IoSetDeviceInterfacePropertyData(
        SymbolicLinkName: PUNICODE_STRING,
        PropertyKey: *const DEVPROPKEY,
        Lcid: LCID,
        Flags: ULONG,
        Type: DEVPROPTYPE,
        Size: ULONG,
        Data: PVOID,
    ) -> NTSTATUS;

    /// `RtlFreeUnicodeString` (`wdm.h`) : libère le tampon d'une `UNICODE_STRING` allouée
    /// par le noyau et remet la structure à zéro.
    ///
    /// IRQL : `PASSIVE_LEVEL` (le tampon vient du pool paginé).
    pub fn RtlFreeUnicodeString(UnicodeString: PUNICODE_STRING);
}

/// Vtable de `IPortClsVersion` (`portcls.h`) : les trois slots de `IUnknown` puis
/// `GetVersion`, qui renvoie une valeur de `EPcVersion` (`kVersionWinXP`…).
///
/// Même forme que les vtables générées par bindgen : pointeurs de fonction
/// `extern "C"` optionnels, dans l'ordre du header C++.
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
#[allow(non_snake_case)]
pub struct IPortClsVersionVtbl {
    pub QueryInterface: Option<
        unsafe extern "C" fn(This: *mut IUnknown, arg1: *const IID, arg2: *mut PVOID) -> NTSTATUS,
    >,
    pub AddRef: Option<unsafe extern "C" fn(This: *mut IUnknown) -> ULONG>,
    pub Release: Option<unsafe extern "C" fn(This: *mut IUnknown) -> ULONG>,
    pub GetVersion: Option<unsafe extern "C" fn(This: *mut c_void) -> DWORD>,
}
