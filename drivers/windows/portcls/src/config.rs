//! Le jeu de propriétés KS **privé** de configuration (M1b-04, driver-design.md §6) :
//! trait [`CableConfig`] que le miniport topologie implémente, et les quatre
//! [`PropertyHandler`] posés sur la brique de [`crate::property`].
//!
//! **Ce module ne stocke aucun état et ne valide rien lui-même**, comme [`crate::audio`]
//! et [`crate::jack`] : il décode une requête, appelle
//! [`conduit_kmd_core::config`](conduit_kmd_core::config) pour tout ce qui est jugement
//! sur des octets, sérialise une réponse champ par champ, et demande le reste au
//! miniport. L'état de connexion vit dans le câble, côté `conduit-kmd`, dans un atomique ;
//! sa persistance vit dans `conduit_kmd::registry`.
//!
//! Cette séparation n'est pas cosmétique : le parseur est le morceau que M1b-08 fuzzera en
//! mode utilisateur, et il ne peut l'être que s'il ne dépend d'aucun type du WDK. Toute
//! validation écrite **ici** échapperait au fuzzer.
//!
//! # Cinq propriétés, un jeu
//!
//! | Propriété | Verbes | Valeur |
//! |---|---|---|
//! | [`KSPROPERTY_CONDUIT_CABLE_STATE`] | GET, SET, BASICSUPPORT | [`CableState`], 16 octets |
//! | [`KSPROPERTY_CONDUIT_VERSION`] | GET, BASICSUPPORT | un `ULONG` |
//! | [`KSPROPERTY_CONDUIT_COUNTERS`] | GET, BASICSUPPORT | [`CableCounters`], 56 octets |
//! | [`KSPROPERTY_CONDUIT_TRANSPORT`] | GET, BASICSUPPORT | [`CableTransport`], 80 octets |
//! | [`KSPROPERTY_CONDUIT_PACKETS`] | GET, BASICSUPPORT | [`CablePackets`], 248 octets |
//!
//! Une seule est modifiable, et c'est la première : la version est celle du binaire chargé,
//! les compteurs sont ce que la boucle locale a fait, le transport est ce que le moteur audio
//! a demandé, le relevé de paquets ce qu'il a fait des interfaces du mode paquets. **Ni un
//! compteur ni une observation n'est un réglage**, d'où l'absence de `SET` — et, faute
//! d'écriture, aucun contrôle de privilège à faire : seule l'écriture en demandait un.
//!
//! Les cinq se posent dans la `PCAUTOMATION_TABLE` du **filtre** de topologie
//! (`PCFILTER_DESCRIPTOR::AutomationTable`), à côté de `KSPROPERTY_JACK_DESCRIPTION`, et
//! non sur une broche ni sur un nœud : elles décrivent le câble entier, pas un point de
//! son graphe. Le service d'assistance (M1b-20) ouvre l'interface `KSCATEGORY_TOPOLOGY` du
//! câble et envoie `IOCTL_KS_PROPERTY` ; PortCls fait tout le routage.
//!
//! # Le câble visé est celui du filtre
//!
//! Le `MajorTarget` de la requête **est** le miniport topologie, donc le câble : c'est le
//! même chemin que le volume et le jack, et la garde de vtable de
//! [`crate::property::handler`] en vérifie déjà le type. Le champ [`CableState::cable`]
//! n'est donc pas un sélecteur mais un **écho** : au `GET` il dit au service quel câble il
//! vient de lire, au `SET` le gestionnaire le compare à
//! [`CableConfig::cable_index`] et refuse `STATUS_INVALID_PARAMETER` s'il diffère. Un
//! service qui se tromperait de descripteur déconnecterait sinon le mauvais câble sans que
//! rien ne le signale.
//!
//! # `Instance` n'est pas lue, et c'est mesurable
//!
//! Nos deux propriétés sont de simples `KSPROPERTY` sans extension : PortCls en retire
//! l'en-tête et ne laisse **rien** dans `Instance` (`InstanceSize == 0`), contrairement au
//! `KSP_PIN` de [`crate::jack`] et au `KSNODEPROPERTY_AUDIO_CHANNEL` de
//! [`crate::audio`]. Le gestionnaire l'ignore donc, plutôt que d'exiger qu'elle soit vide :
//! un refus sur ce critère transformerait une différence de routage KS en propriété muette
//! dans la VM, symptôme le plus coûteux à diagnostiquer de tout ce module. Les octets
//! bruts partent en revanche dans [`ConfigTrace::instance`] : une ligne de journal au
//! premier essai en machine dit ce qui arrive réellement, et c'est seulement une fois
//! **mesuré** qu'on pourra resserrer.
//!
//! # `BASICSUPPORT` : dès que le bit est déclaré, PortCls ne répond plus à notre place
//!
//! Même règle que pour le volume et le jack. Réponse en **paliers**, KS interrogeant deux
//! fois (d'abord `sizeof(ULONG)` pour les seuls `AccessFlags`, puis la taille complète), et
//! [`PropertyHandler::basic_support`] rend les octets **écrits**, jamais une taille requise
//! (asymétrie documentée sur le trait) :
//!
//! | Place dans `Value` | Écrit | rendu |
//! |---|---|---|
//! | < 4 | rien | `Err(STATUS_BUFFER_TOO_SMALL)` |
//! | 4 à 39 | `AccessFlags: ULONG` | 4 |
//! | ≥ 40 | `KSPROPERTY_DESCRIPTION` seule | 40 |
//!
//! Aucun membre dans les deux cas, mais pour deux raisons différentes, et le `PropTypeSet`
//! le dit : la **version** est un `ULONG`, donc `KSPROPTYPESETID_General` + `VT_UI4` ;
//! l'**état** est une structure de seize octets qu'aucune `VARENUM` ne nomme, donc un
//! `PropTypeSet` nul (`GUID_NULL`, `Id = 0`) — l'équivalent du `VT_ILLEGAL` que
//! `PropertyHandler_BasicSupport` de SYSVAD traite en écrivant l'ensemble vide. Une plage
//! de `KSPROPERTY_MEMBERSHEADER` ne décrirait de toute façon qu'un scalaire.
//!
//! # Contrôle d'accès : ce qui est établi, et ce qui ne l'est pas
//!
//! Toute écriture exige que [`CableConfig::may_configure`] rende vrai, sans quoi
//! `STATUS_PRIVILEGE_NOT_HELD` et **aucun effet**. Le pilote l'implémente par
//! `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)` (le service d'assistance tourne en
//! `LocalSystem`) ; ce crate n'a pas `wdk-sys` en dépendance, d'où le passage par le trait.
//!
//! Le contrôle est fait **avant** la validation du contenu, et volontairement : un appelant
//! sans privilège reçoit le même `STATUS_PRIVILEGE_NOT_HELD` quel que soit son tampon, et
//! n'apprend donc rien du format en le faisant varier. C'est aussi l'ordre le moins
//! surprenant — on refuse l'accès avant de commenter la demande.
//!
//! **La réserve est levée : c'est mesuré.** `SeSinglePrivilegeCheck` n'a de sens que si le
//! gestionnaire de propriété s'exécute dans le contexte du **fil appelant** : sur un fil
//! système, `ExGetPreviousMode()` rendrait `KernelMode`, la routine rendrait vrai
//! inconditionnellement, et le contrôle serait pire qu'absent — il donnerait l'illusion
//! d'une protection. **La documentation Microsoft ne dit toujours rien du contexte de fil
//! des gestionnaires PortCls** : `portcls.h` n'en parle pas, la page `PCPROPERTY_ITEM` a un
//! champ IRQL vide, et il n'existe pas de page dédiée à `PCPFNPROPERTY_HANDLER`. Le
//! raisonnement qui rendait le contrôle crédible — un `IOCTL_KS_PROPERTY` est une
//! `IRP_MJ_DEVICE_CONTROL` traitée en ligne par la routine de répartition, donc dans le fil
//! qui a appelé `DeviceIoControl` — reste non documenté, mais il n'est plus une supposition :
//! en machine virtuelle, un `SET` depuis l'espace utilisateur rend bien
//! `STATUS_PRIVILEGE_NOT_HELD` (`ERROR_PRIVILEGE_NOT_HELD`, 1314 côté client), dans trois
//! contextes dont `LocalSystem` par tâche planifiée. Le gestionnaire ne s'est donc **pas**
//! exécuté avec `ExGetPreviousMode() == KernelMode`, ni contre le jeton d'un fil système
//! privilégié : il a évalué le jeton de l'appelant, en mode utilisateur.
//!
//! Ce que la mesure prouve, **exactement** : le **refus** a lieu dans le contexte de
//! l'appelant. C'est le sens qui compte pour la sécurité — la panne redoutée était un
//! contrôle qui laisse tout passer, et elle est écartée. La réciproque, qu'un appelant
//! réellement privilégié soit accepté, se mesure séparément : elle demande un client qui
//! **arme** `SeLoadDriverPrivilege` avant d'écrire, les jetons Windows livrant leurs
//! privilèges présents mais désactivés (`conduit_backend_wasapi::cable::armer_privilege`,
//! `conduit-looptest --cable-privilege`). Le repli par `PCPROPERTY_REQUEST::Irp`
//! (`RequestorMode`, `Tail.Overlay.Thread`) n'a plus lieu d'être.
//!
//! # Ce que ce module ne fait pas
//!
//! - **`KSEVENT_PINCAPS_JACKINFOCHANGE`** : sans lui, Windows n'ira jamais relire
//!   `KSPROPERTY_JACK_DESCRIPTION` et l'interface restera figée sur l'état du démarrage
//!   (voir [`crate::jack`]). La propriété rendra la bonne valeur et le panneau de son ne
//!   bougera pas. La brique d'événements est construite à part ; le point d'insertion est
//!   marqué dans `conduit_kmd::cable::Cable::set_connected`.
//! - **Changer les canaux à chaud** : [`CableState::channels`] est validé par le contrat
//!   portable, puis le gestionnaire refuse toute valeur autre que celle que **ce câble**
//!   sert déjà ([`CableState::channels_appliquables`], contre [`CableConfig::channels`]).
//!   Depuis M1b-05 le pilote sait servir 1 à 8 canaux, mais pas en changer sans redémarrer
//!   le périphérique : les tables KS sont immuables et PortCls en retient les pointeurs
//!   pour toute la vie du filtre. Le réglage passe par le registre et un redémarrage du
//!   devnode, tous deux en espace utilisateur. Répondre `STATUS_SUCCESS` à un changement
//!   qui n.agirait sur rien avant le prochain démarrage serait pire qu.un refus.
//!
//! # Les deux champs comparés sont deux échos, et le client les reprend d'un `GET`
//!
//! `cable` et `channels` sont de même nature : le gestionnaire les **compare** à ce que le
//! miniport rapporte, il ne les applique pas. Un `SET` est donc toujours une
//! lecture-modification-écriture — [`CableState::avec_connexion`] est ce geste côté client
//! —, et un client qui fabriquerait sa requête de toutes pièces se ferait refuser dès que
//! le câble n'est pas au format d'usine. La décision et ce qu'elle écarte sont écrites sur
//! [`CableState::channels`] ; le refus, lui, est ci-dessous.

use conduit_com::{NtStatus, STATUS_INVALID_PARAMETER};
use conduit_kmd_core::config::{
    CABLE_COUNTERS_BYTES, CABLE_PACKETS_BYTES, CABLE_STATE_BYTES, CABLE_TRANSPORT_BYTES,
    CONFIG_VERSION, CableCounters, CablePackets, CableState, CableTransport, ConfigError,
    ConfigGuid, KSPROPERTY_CONDUIT_CABLE_STATE, KSPROPERTY_CONDUIT_COUNTERS,
    KSPROPERTY_CONDUIT_PACKETS, KSPROPERTY_CONDUIT_TRANSPORT, KSPROPERTY_CONDUIT_VERSION,
    KSPROPSETID_CONDUIT, O_CABLE, O_CHANNELS, O_CONNECTED, O_RESERVED, OC_CABLE, OC_COPIED,
    OC_DISCARDED_TICKS, OC_OVERRUNS, OC_RESERVED, OC_SILENCED_BEFORE_RENDER, OC_SILENCED_NO_RENDER,
    OC_TICKS, OCP_CABLE, OCP_CAPTURE, OCP_MODE, OCP_RENDER, OS_BUFFER_BYTES, OS_BUFFER_FRAMES,
    OS_KS_STATE, OS_MODE, OS_NOTIFICATION_COUNT, OS_NOTIFICATION_EVENTS, OS_REFUSED_ALLOCATIONS,
    OSP_EXPOSURE, OSP_FIRST_QPC, OSP_GET_READ_PACKET, OSP_IRQL_LAST, OSP_IRQL_MAX,
    OSP_LAST_PACKET_COUNT_RETURNED, OSP_LAST_QPC, OSP_LAST_WRITE_AT_LAST_COUNT, OSP_PACKET_COUNT,
    OSP_PACKETS_REACHED_AT_LAST_COUNT, OSP_PRESENTATION_POSITION, OSP_QUERIES, OSP_QUERIES_GRANTED,
    OSP_RESERVED, OSP_SET_WRITE_LATE, OSP_SET_WRITE_OVERRUN, OSP_SET_WRITE_PACKET, OT_CABLE,
    OT_CAPTURE, OT_CONSTRAINTS_CAPTURE, OT_CONSTRAINTS_RENDER, OT_RENDER, OT_RESERVED,
    STREAM_PACKETS_BYTES, STREAM_TRANSPORT_BYTES, StreamPackets, StreamTransport,
};
use portcls_sys::{
    GUID, GUID_NULL, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET, KSPROPERTY_TYPE_SET,
    KSPROPTYPESETID_General, PCPROPERTY_ITEM, VARENUM,
};

use crate::property::{
    self, Champs, D_ACCESSFLAGS, PropertyHandler, Request, TAILLE_ACCESSFLAGS, TAILLE_DESCRIPTION,
    TargetVtbl, ecrire_description,
};
use crate::status::{STATUS_BUFFER_TOO_SMALL, STATUS_PRIVILEGE_NOT_HELD};

// ---------------------------------------------------------------------------------
// Le GUID du jeu, converti une fois vers le type de `portcls-sys`.
// ---------------------------------------------------------------------------------

/// Convertit un [`ConfigGuid`] portable en `GUID` du WDK.
///
/// Les deux types ont la même disposition (`guiddef.h`), ce que des assertions `const`
/// vérifient des deux côtés ; la conversion reste néanmoins **champ par champ**, jamais par
/// transmutation : c'est la seule forme qu'un renommage de champ ou un changement de type
/// casse à la compilation plutôt qu'à l'exécution.
///
/// `pub(crate)` et non privée depuis que [`crate::adapter`] convertit lui aussi un
/// `ConfigGuid` — le mode de traitement de sa contrainte de taille de paquet : deux copies
/// de la même conversion finiraient par diverger d'un champ, et c'est exactement la panne
/// muette que l'assertion `const` ci-dessous existe pour empêcher.
pub(crate) const fn en_guid(guid: &ConfigGuid) -> GUID {
    GUID {
        Data1: guid.data1,
        Data2: guid.data2,
        Data3: guid.data3,
        Data4: guid.data4,
    }
}

/// `KSPROPSETID_Conduit` en `static` : `PCPROPERTY_ITEM::Set` veut une adresse `'static`.
static SET_CONDUIT: GUID = en_guid(&KSPROPSETID_CONDUIT);

/// La conversion préserve bien les quatre champs, dans le bon ordre.
///
/// Un `Data2` et un `Data3` intervertis donneraient un GUID valide, différent, et une
/// propriété que le service d'assistance ne trouverait jamais — sans le moindre message
/// d'erreur, la requête se soldant par « jeu de propriétés inconnu ».
const _: () = {
    let converti = en_guid(&KSPROPSETID_CONDUIT);
    assert!(converti.Data1 == KSPROPSETID_CONDUIT.data1);
    assert!(converti.Data2 == KSPROPSETID_CONDUIT.data2);
    assert!(converti.Data3 == KSPROPSETID_CONDUIT.data3);
    assert!(converti.Data4[0] == KSPROPSETID_CONDUIT.data4[0]);
    assert!(converti.Data4[7] == KSPROPSETID_CONDUIT.data4[7]);
    assert!(size_of::<GUID>() == size_of::<ConfigGuid>());
};

// ---------------------------------------------------------------------------------
// Drapeaux d'accès et tailles.
// ---------------------------------------------------------------------------------

/// `Flags` du `PCPROPERTY_ITEM` de l'état : `GET | SET | BASICSUPPORT` (515).
///
/// C'est la seule propriété **modifiable** du pilote qui ne soit pas un nœud audio ; le bit
/// `SET` est ce qui distingue M1b-04 de M1b-03, où `KSPROPERTY_JACK_DESCRIPTION` est en
/// lecture seule par prescription de la documentation.
pub const CABLE_STATE_ACCESS_FLAGS: u32 =
    KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT;

/// `Flags` du `PCPROPERTY_ITEM` de la version : `GET | BASICSUPPORT` (513), **sans `SET`**.
///
/// La version est celle du binaire chargé : elle se lit, elle ne se règle pas.
pub const VERSION_ACCESS_FLAGS: u32 = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT;

/// `Flags` du `PCPROPERTY_ITEM` des compteurs : `GET | BASICSUPPORT` (513), **sans `SET`**.
///
/// Même valeur que [`VERSION_ACCESS_FLAGS`], et pour une raison de même nature : un
/// compteur n'est pas un réglage. Deux constantes plutôt qu'une, parce qu'elles décrivent
/// deux propriétés dont rien ne garantit qu'elles resteront d'accord — et parce qu'un
/// `assert_eq!` entre les deux dirait quelque chose de faux, à savoir qu'elles dépendent
/// l'une de l'autre.
pub const COUNTERS_ACCESS_FLAGS: u32 = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT;

/// `Flags` du `PCPROPERTY_ITEM` du transport : `GET | BASICSUPPORT` (513), **sans `SET`**.
///
/// Troisième constante de même valeur, et pour la même raison que la deuxième : elles
/// décrivent trois propriétés dont rien ne garantit qu'elles resteront d'accord. Ce que le
/// transport décrit est une **observation** — ce que le moteur audio a demandé —, et une
/// observation ne se règle pas plus qu'un compteur.
pub const TRANSPORT_ACCESS_FLAGS: u32 = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT;

/// `Flags` du `PCPROPERTY_ITEM` du relevé de paquets : `GET | BASICSUPPORT` (513), **sans
/// `SET`**.
///
/// Quatrième constante de même valeur, et pour la même raison que la troisième. Ce que le
/// relevé de paquets décrit est une **observation** — ce que le moteur audio a fait
/// d'interfaces exposées — et le seul réglage du mode paquets est le paramètre de registre
/// `PacketMode`, lu au `StartDevice` : rendre modifiable par KS ce qui exige un redémarrage du
/// périphérique serait promettre un effet qui n'aurait pas lieu, exactement l'erreur que
/// [`CableState::channels`] écarte de son côté.
pub const PACKETS_ACCESS_FLAGS: u32 = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT;

/// Taille de la valeur de [`KSPROPERTY_CONDUIT_VERSION`] : un `ULONG`.
const TAILLE_VERSION: usize = 4;

const _: () = assert!(CABLE_STATE_ACCESS_FLAGS == 515);
const _: () = assert!(VERSION_ACCESS_FLAGS == 513);
const _: () = assert!(VERSION_ACCESS_FLAGS & KSPROPERTY_TYPE_SET == 0);
const _: () = assert!(COUNTERS_ACCESS_FLAGS == 513);
const _: () = assert!(COUNTERS_ACCESS_FLAGS & KSPROPERTY_TYPE_SET == 0);
const _: () = assert!(CABLE_STATE_BYTES == 16 && TAILLE_VERSION == 4);
// Les décalages de la structure d'échange sont ceux du contrat portable et pavent bien ses
// seize octets : la sérialisation ci-dessous n'en recopie pas une seconde définition.
const _: () = assert!(O_CABLE == 0 && O_CONNECTED == 4 && O_CHANNELS == 8 && O_RESERVED == 12);
const _: () = assert!(O_RESERVED + 4 == CABLE_STATE_BYTES);
// Idem pour les compteurs : deux `ULONG` puis six `ULONGLONG`, chacun aligné sur huit.
const _: () = assert!(CABLE_COUNTERS_BYTES == 56);
const _: () = assert!(OC_CABLE == 0 && OC_RESERVED == 4 && OC_TICKS == 8);
const _: () = assert!(OC_COPIED == 16 && OC_SILENCED_NO_RENDER == 24);
const _: () = assert!(OC_SILENCED_BEFORE_RENDER == 32 && OC_DISCARDED_TICKS == 40);
const _: () = assert!(OC_OVERRUNS == 48 && OC_OVERRUNS + 8 == CABLE_COUNTERS_BYTES);
const _: () = assert!(TRANSPORT_ACCESS_FLAGS == 513);
const _: () = assert!(TRANSPORT_ACCESS_FLAGS & KSPROPERTY_TYPE_SET == 0);
// Idem pour le transport : quatre `ULONG` puis deux blocs de sens de 32 octets, chacun
// aligné sur huit ; dans un bloc, six `ULONG` puis un `ULONGLONG`.
const _: () = assert!(CABLE_TRANSPORT_BYTES == 80 && STREAM_TRANSPORT_BYTES == 32);
const _: () = assert!(OT_CABLE == 0 && OT_RESERVED == 4);
const _: () = assert!(OT_CONSTRAINTS_RENDER == 8 && OT_CONSTRAINTS_CAPTURE == 12);
const _: () = assert!(OT_RENDER == 16 && OT_CAPTURE == 48);
const _: () = assert!(OT_CAPTURE + STREAM_TRANSPORT_BYTES == CABLE_TRANSPORT_BYTES);
const _: () = assert!(OS_MODE == 0 && OS_NOTIFICATION_COUNT == 4 && OS_BUFFER_BYTES == 8);
const _: () = assert!(OS_BUFFER_FRAMES == 12 && OS_NOTIFICATION_EVENTS == 16);
const _: () = assert!(OS_KS_STATE == 20 && OS_REFUSED_ALLOCATIONS == 24);
const _: () = assert!(OS_REFUSED_ALLOCATIONS + 8 == STREAM_TRANSPORT_BYTES);
const _: () = assert!(PACKETS_ACCESS_FLAGS == 513);
const _: () = assert!(PACKETS_ACCESS_FLAGS & KSPROPERTY_TYPE_SET == 0);
// Idem pour le relevé de paquets : deux `ULONG` puis deux blocs de sens de 120 octets, chacun
// aligné sur huit ; dans un bloc, quatre `ULONG` puis treize `ULONGLONG`.
const _: () = assert!(CABLE_PACKETS_BYTES == 248 && STREAM_PACKETS_BYTES == 120);
const _: () = assert!(OCP_CABLE == 0 && OCP_MODE == 4);
const _: () = assert!(OCP_RENDER == 8 && OCP_CAPTURE == 128);
const _: () = assert!(OCP_CAPTURE + STREAM_PACKETS_BYTES == CABLE_PACKETS_BYTES);
const _: () = assert!(OSP_EXPOSURE == 0 && OSP_IRQL_LAST == 4 && OSP_IRQL_MAX == 8);
const _: () = assert!(OSP_RESERVED == 12 && OSP_SET_WRITE_PACKET == 16);
const _: () = assert!(OSP_GET_READ_PACKET == 24 && OSP_PACKET_COUNT == 32);
const _: () = assert!(OSP_PRESENTATION_POSITION == 40 && OSP_QUERIES == 48);
const _: () = assert!(OSP_QUERIES_GRANTED == 56 && OSP_FIRST_QPC == 64 && OSP_LAST_QPC == 72);
const _: () = assert!(OSP_SET_WRITE_LATE == 80 && OSP_SET_WRITE_OVERRUN == 88);
const _: () =
    assert!(OSP_LAST_PACKET_COUNT_RETURNED == 96 && OSP_PACKETS_REACHED_AT_LAST_COUNT == 104);
const _: () = assert!(OSP_LAST_WRITE_AT_LAST_COUNT == 112);
const _: () = assert!(OSP_LAST_WRITE_AT_LAST_COUNT + 8 == STREAM_PACKETS_BYTES);

// ---------------------------------------------------------------------------------
// Trace.
// ---------------------------------------------------------------------------------

/// Ce qu'un gestionnaire vient de faire d'une requête de configuration, passé à
/// [`CableConfig::trace`].
///
/// Même rôle que [`crate::audio::Trace`] et [`crate::jack::JackTrace`] : rendre vérifiable
/// en une minute, au premier essai en machine, ce qui n'est pour l'instant qu'un
/// raisonnement. Deux choses en particulier — le contenu réel d'`Instance` (voir l'en-tête
/// de module) et le verdict du contrôle de privilège, dont le contexte de fil n'est pas
/// documenté. C'est cette trace qui a servi à établir le refus mesuré en machine virtuelle ;
/// il reste à lui faire montrer l'**acceptation** d'un appelant qui a armé son privilège.
#[derive(Debug)]
pub struct ConfigTrace<'a> {
    /// La propriété : `"état"` ou `"version"`.
    pub property: &'static str,
    /// Le verbe : `"GET"`, `"SET"` ou `"BASICSUPPORT"`.
    pub verb: &'static str,
    /// Les octets d'`Instance` tels que PortCls les a laissés — attendus **vides**.
    pub instance: &'a [u8],
    /// Le câble du miniport visé ([`CableConfig::cable_index`]).
    pub cable: u32,
    /// L'état lu (`GET`) ou demandé (`SET`), ou la cause portable du refus. `None` pour
    /// `BASICSUPPORT`, et pour un `SET` arrêté par le contrôle de privilège.
    pub state: Option<Result<CableState, ConfigError>>,
    /// Le `NTSTATUS` rendu à l'appelant.
    pub status: NtStatus,
    /// Ce que la persistance d'un `SET` appliqué a donné ; `None` si aucun `SET` n'a été
    /// appliqué. Un `Err` ici n'empêche **pas** la propriété de réussir.
    pub persisted: Option<Result<(), NtStatus>>,
}

/// Nom de propriété des traces de l'état.
const PROP_ETAT: &str = "état";
/// Nom de propriété des traces de la version.
const PROP_VERSION: &str = "version";
/// Nom de propriété des traces des compteurs.
const PROP_COMPTEURS: &str = "compteurs";
/// Nom de propriété des traces du transport.
const PROP_TRANSPORT: &str = "transport";
/// Nom de propriété des traces du relevé de paquets.
const PROP_PAQUETS: &str = "paquets";
/// Nom de verbe des traces de lecture.
const VERBE_GET: &str = "GET";
/// Nom de verbe des traces d'écriture.
const VERBE_SET: &str = "SET";
/// Nom de verbe des traces de description.
const VERBE_BASICSUPPORT: &str = "BASICSUPPORT";

// ---------------------------------------------------------------------------------
// Le trait métier.
// ---------------------------------------------------------------------------------

/// La configuration d'un câble, vue par les gestionnaires de propriété.
///
/// `TopoRender` et `TopoCapture` l'implémenteront sur l'état du câble (un atomique, côté
/// `conduit-kmd`) : **ce module ne stocke rien**. Les deux sens d'un même câble
/// implémentent le même état, un câble débranché l'étant de ses deux bouts.
///
/// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne par PortCls). Appels
/// concurrents possibles depuis plusieurs fils : d'où un atomique, et non un verrou.
pub trait CableConfig: Send + Sync + 'static {
    /// Numéro du câble que ce miniport sert, de 0 à `CABLE_MAX` − 1.
    ///
    /// C'est lui que [`CableState::cable`] doit répéter dans un `SET` : voir l'en-tête de
    /// module. Doit être constant pour la vie du miniport.
    fn cable_index(&self) -> u32;

    /// Nombre de canaux du câble, tel que ses descripteurs KS le déclarent.
    ///
    /// Sert à remplir [`CableState::channels`] au `GET`, et à **valider** le `SET` depuis
    /// M1b-05 : [`CableState::channels_appliquables`] compare la valeur demandée à
    /// celle-ci. Doit être constant pour la vie du miniport — un format ne change qu.au
    /// redémarrage du périphérique.
    fn channels(&self) -> u32;

    /// L'état actif du câble, celui que `KSJACK_DESCRIPTION::IsConnected` porte aussi.
    ///
    /// IRQL : quelconque.
    fn is_connected(&self) -> bool;

    /// Fixe l'état actif du câble et le persiste.
    ///
    /// **L'état en mémoire est toujours appliqué**, y compris quand le `Err` est rendu :
    /// celui-ci ne décrit que l'échec de la **persistance** (registre plein, clé
    /// inaccessible), et le gestionnaire l'ignore pour le statut qu'il rend à l'appelant.
    /// Un disque plein ne doit pas empêcher d'activer un câble ; l'implémentation
    /// journalise l'échec par `kmd_event!` et le pilote repartira sur la valeur persistée
    /// précédemment au prochain démarrage.
    ///
    /// L'appelant a déjà vérifié le privilège ([`Self::may_configure`]) et validé la
    /// valeur : cette méthode n'a rien à refuser.
    ///
    /// IRQL : `PASSIVE_LEVEL` (l'écriture au registre l'exige).
    fn set_connected(&self, connected: bool) -> Result<(), NtStatus>;

    /// L'appelant a-t-il le droit de modifier la configuration ?
    ///
    /// Implémenté par `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)` côté pilote (ce
    /// crate n'a pas `wdk-sys` en dépendance). Faux → `STATUS_PRIVILEGE_NOT_HELD`, sans
    /// effet.
    ///
    /// **Doit être évalué dans le contexte du fil appelant** pour vouloir dire quelque
    /// chose : lire la réserve en tête de module avant de s'y fier.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn may_configure(&self) -> bool;

    /// Les compteurs de la boucle locale du câble, pour
    /// [`KSPROPERTY_CONDUIT_COUNTERS`] (M1b-21).
    ///
    /// **Un instantané, pas une transaction** : l'implémentation lit ses compteurs
    /// atomiques indépendamment les uns des autres, sans verrou, et le résultat peut donc
    /// mélanger deux ticks. La raison tient en une phrase — prendre le verrou du câble à
    /// `PASSIVE_LEVEL` sérialiserait la boucle locale, celle qui doit tenir un budget de
    /// dix millisecondes, avec un simple diagnostic. Voir
    /// `conduit_kmd::cable::Cable::counters_snapshot`, où le détail est écrit.
    ///
    /// Le champ [`CableCounters::cable`] doit valoir [`Self::cable_index`] : c'est un écho,
    /// et il n'y a rien à comparer puisque cette propriété n'a pas de `SET`.
    ///
    /// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne par PortCls) ;
    /// l'implémentation, elle, doit se contenter de chargements atomiques.
    fn counters(&self) -> CableCounters;

    /// L'état du transport des **deux** sens du câble, pour
    /// [`KSPROPERTY_CONDUIT_TRANSPORT`] (lot 0 du mode paquets WaveRT).
    ///
    /// Les deux sens, et pas seulement celui du filtre interrogé : la question porte sur le
    /// câble, et un relevé des seize câbles ferait sinon trente-deux ouvertures de filtre au
    /// lieu de seize. Les deux miniports d'un même câble rendent donc la même valeur, comme
    /// pour [`Self::counters`].
    ///
    /// **Un instantané, pas une transaction** : l'implémentation lit les champs d'un sens
    /// sous le verrou de ce flux, mais les compteurs de refus hors de tout verrou. Voir
    /// `conduit_kmd::cable::Cable::transport_snapshot`, où le détail est écrit — et où est
    /// écrit aussi pourquoi ce verrou-là, contrairement à celui des compteurs, ne pouvait pas
    /// être évité.
    ///
    /// Le champ [`CableTransport::cable`] doit valoir [`Self::cable_index`] : c'est un écho,
    /// et il n'y a rien à comparer puisque cette propriété n'a pas de `SET`.
    ///
    /// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne par PortCls).
    fn transport(&self) -> CableTransport;

    /// Ce que le moteur audio a fait des interfaces du **mode paquets**, sur les deux sens du
    /// câble, pour [`KSPROPERTY_CONDUIT_PACKETS`] (lot 2).
    ///
    /// Les deux sens, comme [`Self::transport`] et pour la même raison. L'implémentation doit
    /// aussi remplir [`CablePackets::packet_mode`] — le paramètre `PacketMode` **effectif**,
    /// celui que le pilote a retenu au dernier `StartDevice` — parce qu'un relevé qui
    /// montrerait des compteurs sans dire dans quel mode ils ont été pris n'apprendrait rien :
    /// zéro appel est une conclusion sous `PacketMode = 1` et une évidence sous 0.
    ///
    /// **Un instantané, pas une transaction** : les compteurs sont des atomiques lus hors de
    /// tout verrou, l'exposition de chaque sens sous le verrou de son flux. Voir
    /// `conduit_kmd::cable::Cable::packets_snapshot`.
    ///
    /// Le champ [`CablePackets::cable`] doit valoir [`Self::cable_index`] : c'est un écho, et
    /// il n'y a rien à comparer puisque cette propriété n'a pas de `SET`.
    ///
    /// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne par PortCls).
    fn packets(&self) -> CablePackets;

    /// Point de trace, appelé une fois par requête, après coup.
    ///
    /// Défaut : ne fait rien. Ne doit ni allouer ni bloquer.
    fn trace(&self, trace: &ConfigTrace<'_>) {
        let _ = trace;
    }
}

// ---------------------------------------------------------------------------------
// Sérialisation.
// ---------------------------------------------------------------------------------

/// Écrit une [`CableState`] dans `value`, champ par champ, aux décalages du contrat
/// portable.
///
/// **Tout ou rien** : si `value` est plus court que [`CABLE_STATE_BYTES`], rien n'est écrit
/// et le thunk rendra `STATUS_BUFFER_TOO_SMALL` avec la taille requise. C'est le contrat de
/// [`PropertyHandler::get`] et le comportement de [`crate::jack`] ; une structure à demi
/// écrite serait pire qu'aucune, le client n'ayant aucun moyen de savoir quels champs sont
/// à lui.
///
/// Jamais de transtypage vers un `*mut CableState` : `value` n'est aligné sur rien
/// (frontière de confiance de [`crate::property`]), et une écriture désalignée par un
/// pointeur de structure est un comportement indéfini en Rust même là où x64 la tolère.
fn ecrire_etat(value: &mut [u8], etat: &CableState) {
    if value.len() < CABLE_STATE_BYTES {
        return;
    }
    let mut champs = Champs { dest: value };
    champs.u32(O_CABLE, etat.cable);
    champs.u32(O_CONNECTED, etat.connected);
    champs.u32(O_CHANNELS, etat.channels);
    // Toujours zéro : rien de la mémoire du noyau ne transite par le champ réservé.
    champs.u32(O_RESERVED, etat.reserved);
}

/// Écrit une [`CableCounters`] dans `value`, champ par champ, aux décalages du contrat
/// portable.
///
/// **Tout ou rien**, comme [`ecrire_etat`] et pour la même raison : un instantané à demi
/// écrit ferait lire au client des compteurs dont il ne saurait pas lesquels sont à lui.
/// Aucun transtypage vers un `*mut CableCounters` — `value` n'est aligné sur rien, et les
/// six `ULONGLONG` d'un `repr(C)` aligné sur huit sont exactement ce qu'une écriture par
/// pointeur casserait le plus sûrement.
fn ecrire_compteurs(value: &mut [u8], compteurs: &CableCounters) {
    if value.len() < CABLE_COUNTERS_BYTES {
        return;
    }
    let mut champs = Champs { dest: value };
    champs.u32(OC_CABLE, compteurs.cable);
    // Toujours zéro : rien de la mémoire du noyau ne transite par le champ réservé.
    champs.u32(OC_RESERVED, compteurs.reserved);
    champs.u64(OC_TICKS, compteurs.ticks);
    champs.u64(OC_COPIED, compteurs.copied);
    champs.u64(OC_SILENCED_NO_RENDER, compteurs.silenced_no_render);
    champs.u64(OC_SILENCED_BEFORE_RENDER, compteurs.silenced_before_render);
    champs.u64(OC_DISCARDED_TICKS, compteurs.discarded_ticks);
    champs.u64(OC_OVERRUNS, compteurs.overruns);
}

/// Écrit le bloc d'un sens dans `champs`, au décalage **absolu** `base`.
///
/// L'appelant a déjà vérifié que la place y est ([`ecrire_transport`]) : cette fonction ne
/// décide de rien, elle place sept champs à leurs décalages nommés, relatifs à `base`. Les
/// additions sont `checked_add` par principe — les décalages sont des constantes, mais un
/// débordement silencieux écrirait le champ ailleurs.
fn ecrire_sens(champs: &mut Champs<'_>, base: usize, sens: &StreamTransport) {
    let a = |offset: usize| base.checked_add(offset);
    if let Some(o) = a(OS_MODE) {
        champs.u32(o, sens.mode);
    }
    if let Some(o) = a(OS_NOTIFICATION_COUNT) {
        champs.u32(o, sens.notification_count);
    }
    if let Some(o) = a(OS_BUFFER_BYTES) {
        champs.u32(o, sens.buffer_bytes);
    }
    if let Some(o) = a(OS_BUFFER_FRAMES) {
        champs.u32(o, sens.buffer_frames);
    }
    if let Some(o) = a(OS_NOTIFICATION_EVENTS) {
        champs.u32(o, sens.notification_events);
    }
    if let Some(o) = a(OS_KS_STATE) {
        champs.u32(o, sens.ks_state);
    }
    if let Some(o) = a(OS_REFUSED_ALLOCATIONS) {
        champs.u64(o, sens.refused_allocations);
    }
}

/// Écrit une [`CableTransport`] dans `value`, champ par champ, aux décalages du contrat
/// portable.
///
/// **Tout ou rien**, comme [`ecrire_etat`] et [`ecrire_compteurs`], et pour la même raison :
/// un instantané à demi écrit ferait lire au client un mode d'allocation d'un sens et une
/// taille de tampon de l'autre — exactement le genre de relevé qui fait conclure de travers.
/// Aucun transtypage vers un `*mut CableTransport` : `value` n'est aligné sur rien.
fn ecrire_transport(value: &mut [u8], transport: &CableTransport) {
    if value.len() < CABLE_TRANSPORT_BYTES {
        return;
    }
    let mut champs = Champs { dest: value };
    champs.u32(OT_CABLE, transport.cable);
    // Toujours zéro : rien de la mémoire du noyau ne transite par le champ réservé.
    champs.u32(OT_RESERVED, transport.reserved);
    // Les deux `NTSTATUS` de la pose des contraintes de taille de paquet : ils datent du
    // dernier `StartDevice`, pas du flux courant, et c'est bien pour cela qu'ils sont sur le
    // câble et non dans un bloc de sens.
    champs.u32(OT_CONSTRAINTS_RENDER, transport.constraints_render);
    champs.u32(OT_CONSTRAINTS_CAPTURE, transport.constraints_capture);
    ecrire_sens(&mut champs, OT_RENDER, &transport.render);
    ecrire_sens(&mut champs, OT_CAPTURE, &transport.capture);
}

/// Écrit le bloc de paquets d'un sens dans `champs`, au décalage **absolu** `base`.
///
/// Même forme que [`ecrire_sens`] : l'appelant a déjà vérifié que la place y est
/// ([`ecrire_paquets`]), et les additions sont `checked_add` par principe — les décalages sont
/// des constantes, mais un débordement silencieux écrirait le champ ailleurs.
fn ecrire_sens_paquets(champs: &mut Champs<'_>, base: usize, sens: &StreamPackets) {
    let a = |offset: usize| base.checked_add(offset);
    if let Some(o) = a(OSP_EXPOSURE) {
        champs.u32(o, sens.exposure);
    }
    if let Some(o) = a(OSP_IRQL_LAST) {
        champs.u32(o, sens.irql_last);
    }
    if let Some(o) = a(OSP_IRQL_MAX) {
        champs.u32(o, sens.irql_max);
    }
    if let Some(o) = a(OSP_RESERVED) {
        // Toujours zéro : rien de la mémoire du noyau ne transite par le champ réservé.
        champs.u32(o, sens.reserved);
    }
    if let Some(o) = a(OSP_SET_WRITE_PACKET) {
        champs.u64(o, sens.set_write_packet);
    }
    if let Some(o) = a(OSP_GET_READ_PACKET) {
        champs.u64(o, sens.get_read_packet);
    }
    if let Some(o) = a(OSP_PACKET_COUNT) {
        champs.u64(o, sens.packet_count);
    }
    if let Some(o) = a(OSP_PRESENTATION_POSITION) {
        champs.u64(o, sens.presentation_position);
    }
    if let Some(o) = a(OSP_QUERIES) {
        champs.u64(o, sens.queries);
    }
    if let Some(o) = a(OSP_QUERIES_GRANTED) {
        champs.u64(o, sens.queries_granted);
    }
    if let Some(o) = a(OSP_FIRST_QPC) {
        champs.u64(o, sens.first_qpc);
    }
    if let Some(o) = a(OSP_LAST_QPC) {
        champs.u64(o, sens.last_qpc);
    }
    if let Some(o) = a(OSP_SET_WRITE_LATE) {
        champs.u64(o, sens.set_write_late);
    }
    if let Some(o) = a(OSP_SET_WRITE_OVERRUN) {
        champs.u64(o, sens.set_write_overrun);
    }
    if let Some(o) = a(OSP_LAST_PACKET_COUNT_RETURNED) {
        champs.u64(o, sens.last_packet_count_returned);
    }
    if let Some(o) = a(OSP_PACKETS_REACHED_AT_LAST_COUNT) {
        champs.u64(o, sens.packets_reached_at_last_count);
    }
    if let Some(o) = a(OSP_LAST_WRITE_AT_LAST_COUNT) {
        champs.u64(o, sens.last_write_at_last_count);
    }
}

/// Écrit une [`CablePackets`] dans `value`, champ par champ, aux décalages du contrat
/// portable.
///
/// **Tout ou rien**, comme les trois autres sérialiseurs et pour la même raison : un relevé à
/// demi écrit ferait lire au client des compteurs d'un sens et une exposition de l'autre —
/// exactement le genre de relevé qui fait conclure de travers. Aucun transtypage vers un
/// `*mut CablePackets` : `value` n'est aligné sur rien.
fn ecrire_paquets(value: &mut [u8], paquets: &CablePackets) {
    if value.len() < CABLE_PACKETS_BYTES {
        return;
    }
    let mut champs = Champs { dest: value };
    champs.u32(OCP_CABLE, paquets.cable);
    champs.u32(OCP_MODE, paquets.packet_mode);
    ecrire_sens_paquets(&mut champs, OCP_RENDER, &paquets.render);
    ecrire_sens_paquets(&mut champs, OCP_CAPTURE, &paquets.capture);
}

/// Écrit un `ULONG` en tête de `value` s'il y tient (sinon rien : le thunk rendra la taille
/// requise et `STATUS_BUFFER_TOO_SMALL`).
fn ecrire_u32(value: &mut [u8], valeur: u32) {
    if let Some(place) = value.get_mut(..TAILLE_VERSION) {
        place.copy_from_slice(&valeur.to_ne_bytes());
    }
}

/// Réponse à `KSPROPERTY_TYPE_BASICSUPPORT`, en paliers (voir l'en-tête de module).
///
/// `type_set`/`variante` décrivent le type de la valeur : `KSPROPTYPESETID_General` +
/// `VT_UI4` pour la version, `GUID_NULL` + 0 pour l'état (structure qu'aucune `VARENUM` ne
/// nomme). Aucun membre dans les deux cas, donc `DescriptionSize` vaut 40 : il n'y a pas de
/// palier au-delà.
///
/// Renvoie les octets **écrits**, jamais une taille requise.
fn basic_support_ks(
    value: &mut [u8],
    access_flags: u32,
    type_set: &GUID,
    variante: u32,
) -> Result<u32, NtStatus> {
    if value.len() < TAILLE_ACCESSFLAGS {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }

    let mut champs = Champs { dest: value };
    if champs.dest.len() < TAILLE_DESCRIPTION {
        // Premier appel de KS : `sizeof(ULONG)`, les seuls `AccessFlags`.
        champs.u32(D_ACCESSFLAGS, access_flags);
        return Ok(TAILLE_ACCESSFLAGS as u32);
    }

    ecrire_description(
        &mut champs,
        access_flags,
        TAILLE_DESCRIPTION as u32,
        type_set,
        variante,
        0,
    );
    Ok(TAILLE_DESCRIPTION as u32)
}

// ---------------------------------------------------------------------------------
// Le gestionnaire de l'état.
// ---------------------------------------------------------------------------------

/// L'état courant du câble, tel que le miniport le rapporte.
fn etat_courant<T: CableConfig>(cible: &T) -> CableState {
    CableState {
        cable: cible.cable_index(),
        connected: u32::from(cible.is_connected()),
        channels: cible.channels(),
        reserved: 0,
    }
}

/// Ce qu'un `SET` a décidé : la valeur retenue et le sort de sa persistance.
struct Applique {
    /// L'état demandé, validé.
    etat: CableState,
    /// Le résultat de la persistance : `Err` n'empêche pas la propriété de réussir.
    persiste: Result<(), NtStatus>,
}

/// Valide puis applique un `SET`, privilège déjà vérifié par l'appelant.
///
/// Les trois refus, tous en `STATUS_INVALID_PARAMETER` **sans effet** :
///
/// 1. le tampon n'est pas une [`CableState`] valide (longueur, domaines, champ réservé) —
///    c'est [`CableState::from_bytes`] qui tranche, dans le crate portable ;
/// 2. [`CableState::cable`] ne désigne pas le câble de ce filtre (écho vérifié) ;
/// 3. [`CableState::channels`] est dans le domaine du contrat mais n.est pas le nombre de
///    canaux que **ce câble** sert (M1b-05 : le format se change par le registre plus un
///    redémarrage du devnode, pas par cette propriété).
fn appliquer<T: CableConfig>(
    cible: &T,
    value: &[u8],
) -> Result<Applique, Result<CableState, ConfigError>> {
    let etat = match CableState::from_bytes(value) {
        Ok(etat) => etat,
        Err(err) => return Err(Err(err)),
    };
    if etat.cable != cible.cable_index() {
        return Err(Ok(etat));
    }
    if !etat.channels_appliquables(cible.channels()) {
        return Err(Ok(etat));
    }
    let persiste = cible.set_connected(etat.is_connected());
    Ok(Applique { etat, persiste })
}

/// [`KSPROPERTY_CONDUIT_CABLE_STATE`] sur la table d'automatisation d'un filtre de
/// topologie : l'état actif du câble, lisible par tous, modifiable par les seuls appelants
/// privilégiés.
#[derive(Debug)]
pub struct ConduitCableState;

impl<T: CableConfig> PropertyHandler<T> for ConduitCableState {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Lecture libre : connaître l'état d'un câble n'est pas un privilège, et le service
        // d'assistance comme un outil de diagnostic doivent pouvoir l'interroger sans être
        // élevés. Seule l'écriture est contrôlée.
        let etat = etat_courant(req.target);
        ecrire_etat(value, &etat);
        req.target.trace(&ConfigTrace {
            property: PROP_ETAT,
            verb: VERBE_GET,
            instance: req.instance,
            cable: etat.cable,
            state: Some(Ok(etat)),
            status: conduit_com::STATUS_SUCCESS,
            persisted: None,
        });
        // Taille **requise**, écrite ou non : contrat de `get` (le thunk en déduit
        // SUCCESS / BUFFER_TOO_SMALL / BUFFER_OVERFLOW).
        Ok(CABLE_STATE_BYTES as u32)
    }

    fn set(req: &Request<'_, T>, value: &[u8]) -> Result<(), NtStatus> {
        // Le privilège **avant** la validation : un appelant sans droit reçoit le même
        // statut quel que soit son tampon, et n'apprend donc rien du format en le faisant
        // varier (voir l'en-tête de module).
        if !req.target.may_configure() {
            req.target.trace(&ConfigTrace {
                property: PROP_ETAT,
                verb: VERBE_SET,
                instance: req.instance,
                cable: req.target.cable_index(),
                state: None,
                status: STATUS_PRIVILEGE_NOT_HELD,
                persisted: None,
            });
            return Err(STATUS_PRIVILEGE_NOT_HELD);
        }

        let issue = appliquer(req.target, value);
        let (status, state, persisted) = match &issue {
            Ok(applique) => (
                conduit_com::STATUS_SUCCESS,
                Some(Ok(applique.etat)),
                Some(applique.persiste),
            ),
            Err(refus) => (STATUS_INVALID_PARAMETER, Some(*refus), None),
        };
        req.target.trace(&ConfigTrace {
            property: PROP_ETAT,
            verb: VERBE_SET,
            instance: req.instance,
            cable: req.target.cable_index(),
            state,
            status,
            persisted,
        });
        // Un échec de persistance ne fait **pas** échouer la propriété : l'état en mémoire
        // est appliqué, et c'est ce que le client vient de demander.
        issue.map(|_| ()).map_err(|_| STATUS_INVALID_PARAMETER)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Structure de seize octets : aucune `VARENUM` ne la nomme, d'où un `PropTypeSet`
        // nul plutôt qu'une variante fausse (voir l'en-tête de module).
        let ecrits = basic_support_ks(value, CABLE_STATE_ACCESS_FLAGS, &GUID_NULL, 0);
        req.target.trace(&ConfigTrace {
            property: PROP_ETAT,
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            cable: req.target.cable_index(),
            state: None,
            status: ecrits.map_or_else(|status| status, |_| conduit_com::STATUS_SUCCESS),
            persisted: None,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Le gestionnaire de la version.
// ---------------------------------------------------------------------------------

/// [`KSPROPERTY_CONDUIT_VERSION`] : la version du contrat que ce pilote sert.
///
/// Un `ULONG`, en lecture seule. C'est la première chose que le service d'assistance
/// (M1b-20) interroge : sans elle, une inadéquation entre le service et le pilote se
/// manifesterait par un `STATUS_INVALID_PARAMETER` sur une longueur de tampon, et personne
/// ne ferait le lien.
#[derive(Debug)]
pub struct ConduitVersion;

impl<T: CableConfig> PropertyHandler<T> for ConduitVersion {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        ecrire_u32(value, CONFIG_VERSION);
        req.target.trace(&ConfigTrace {
            property: PROP_VERSION,
            verb: VERBE_GET,
            instance: req.instance,
            cable: req.target.cable_index(),
            state: None,
            status: conduit_com::STATUS_SUCCESS,
            persisted: None,
        });
        Ok(TAILLE_VERSION as u32)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Un `ULONG` : `KSPROPTYPESETID_General` + `VT_UI4`, et aucun membre — la version
        // n'a pas de plage à annoncer, seulement un type.
        let ecrits = basic_support_ks(
            value,
            VERSION_ACCESS_FLAGS,
            &KSPROPTYPESETID_General,
            VARENUM::VT_UI4 as u32,
        );
        req.target.trace(&ConfigTrace {
            property: PROP_VERSION,
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            cable: req.target.cable_index(),
            state: None,
            status: ecrits.map_or_else(|status| status, |_| conduit_com::STATUS_SUCCESS),
            persisted: None,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Le gestionnaire des compteurs.
// ---------------------------------------------------------------------------------

/// [`KSPROPERTY_CONDUIT_COUNTERS`] : les compteurs de la boucle locale du câble (M1b-21).
///
/// Une [`CableCounters`] de 56 octets, en **lecture seule** et **sans contrôle de
/// privilège**, comme la version. C'est le seul chemin par lequel les compteurs de M1b-07
/// se lisent en release : `kmd_log!` y est vide, et attacher le débogueur noyau fausse la
/// mesure de transport qu'on cherche justement à qualifier (17 passes sur 20 attaché contre
/// 20 sur 20 détaché, le 2026-09-08).
///
/// L'écho de câble n'est pas comparé — il n'y a rien à comparer sans `SET` — mais il est
/// **rempli par le miniport**, donc par le câble du filtre visé : un relevé des seize
/// câbles se relit sans se souvenir de l'ordre des descripteurs ouverts.
#[derive(Debug)]
pub struct ConduitCounters;

impl<T: CableConfig> PropertyHandler<T> for ConduitCounters {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Lecture libre : voir `ConduitCableState::get`. Un diagnostic qui exigerait
        // l'élévation ne servirait pas là où il sert.
        let compteurs = req.target.counters();
        ecrire_compteurs(value, &compteurs);
        req.target.trace(&ConfigTrace {
            property: PROP_COMPTEURS,
            verb: VERBE_GET,
            instance: req.instance,
            cable: compteurs.cable,
            // `ConfigTrace::state` porte une `CableState` ; les compteurs n'en sont pas
            // une, et les recopier dans la trace n'apprendrait rien que le tampon rendu ne
            // dise déjà.
            state: None,
            status: conduit_com::STATUS_SUCCESS,
            persisted: None,
        });
        // Taille **requise**, écrite ou non : contrat de `get`.
        Ok(CABLE_COUNTERS_BYTES as u32)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Structure de cinquante-six octets : aucune `VARENUM` ne la nomme, d'où un
        // `PropTypeSet` nul, exactement comme pour l'état.
        let ecrits = basic_support_ks(value, COUNTERS_ACCESS_FLAGS, &GUID_NULL, 0);
        req.target.trace(&ConfigTrace {
            property: PROP_COMPTEURS,
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            cable: req.target.cable_index(),
            state: None,
            status: ecrits.map_or_else(|status| status, |_| conduit_com::STATUS_SUCCESS),
            persisted: None,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Le gestionnaire du transport.
// ---------------------------------------------------------------------------------

/// [`KSPROPERTY_CONDUIT_TRANSPORT`] : ce que le moteur audio a demandé au câble, sens par
/// sens (lot 0 du mode paquets WaveRT).
///
/// Une [`CableTransport`] de 80 octets, en **lecture seule** et **sans contrôle de
/// privilège**, comme les compteurs et pour les mêmes raisons. Elle répond à une question
/// qu'aucune ligne de journal ne sait poser en release : le tampon courant a-t-il été alloué
/// par `AllocateAudioBuffer` (scrutation) ou par `AllocateBufferWithNotification` — seul mode
/// sur lequel un paquet WaveRT puisse exister — et combien d'allocations avons-nous
/// **refusées** ? Un refus fait retomber le moteur en scrutation sans une ligne d'erreur ;
/// c'est ce qui a coûté une journée de diagnostic le 2026-09-06.
///
/// L'écho de câble n'est pas comparé — il n'y a rien à comparer sans `SET` — mais il est
/// **rempli par le miniport**, donc par le câble du filtre visé.
#[derive(Debug)]
pub struct ConduitTransport;

impl<T: CableConfig> PropertyHandler<T> for ConduitTransport {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Lecture libre : voir `ConduitCableState::get`. Un diagnostic qui exigerait
        // l'élévation ne servirait pas là où il sert.
        let transport = req.target.transport();
        ecrire_transport(value, &transport);
        req.target.trace(&ConfigTrace {
            property: PROP_TRANSPORT,
            verb: VERBE_GET,
            instance: req.instance,
            cable: transport.cable,
            // `ConfigTrace::state` porte une `CableState` ; le transport n'en est pas une,
            // et le recopier dans la trace n'apprendrait rien que le tampon rendu ne dise
            // déjà.
            state: None,
            status: conduit_com::STATUS_SUCCESS,
            persisted: None,
        });
        // Taille **requise**, écrite ou non : contrat de `get`.
        Ok(CABLE_TRANSPORT_BYTES as u32)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Structure de soixante-douze octets : aucune `VARENUM` ne la nomme, d'où un
        // `PropTypeSet` nul, exactement comme pour l'état et les compteurs.
        let ecrits = basic_support_ks(value, TRANSPORT_ACCESS_FLAGS, &GUID_NULL, 0);
        req.target.trace(&ConfigTrace {
            property: PROP_TRANSPORT,
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            cable: req.target.cable_index(),
            state: None,
            status: ecrits.map_or_else(|status| status, |_| conduit_com::STATUS_SUCCESS),
            persisted: None,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Le gestionnaire du relevé de paquets.
// ---------------------------------------------------------------------------------

/// [`KSPROPERTY_CONDUIT_PACKETS`] : ce que le moteur audio a fait des interfaces du **mode
/// paquets** (lot 2).
///
/// Une [`CablePackets`] de 248 octets, en **lecture seule** et **sans contrôle de privilège**,
/// comme les compteurs et le transport, pour les mêmes raisons. Elle répond à la question que
/// le lot 0 a laissée ouverte : le moteur audio scrute-t-il par politique, ou parce qu'il ne
/// trouve pas les interfaces de paquets ? Le paramètre de registre `PacketMode` les expose sur
/// une machine d'essai — **sans les servir** —, et ce relevé dit ce qui s'est passé ensuite :
/// les `QueryInterface` reçus sur les deux IID, ceux auxquels on a répondu, les appels de
/// méthode par sens et par méthode, leur IRQL et leurs horodatages.
///
/// Le relevé est utile **même à `PacketMode = 0`**, où rien n'est exposé : les
/// `QueryInterface` sont comptés que l'IID soit rendu ou non, si bien qu'un compteur de
/// demandes non nul prouve que le moteur cherche le mode paquets sans qu'on ait rien promis.
///
/// L'écho de câble n'est pas comparé — il n'y a rien à comparer sans `SET` — mais il est
/// **rempli par le miniport**, donc par le câble du filtre visé.
#[derive(Debug)]
pub struct ConduitPackets;

impl<T: CableConfig> PropertyHandler<T> for ConduitPackets {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Lecture libre : voir `ConduitCableState::get`. Un diagnostic qui exigerait
        // l'élévation ne servirait pas là où il sert.
        let paquets = req.target.packets();
        ecrire_paquets(value, &paquets);
        req.target.trace(&ConfigTrace {
            property: PROP_PAQUETS,
            verb: VERBE_GET,
            instance: req.instance,
            cable: paquets.cable,
            // `ConfigTrace::state` porte une `CableState` ; le relevé de paquets n'en est pas
            // une, et le recopier dans la trace n'apprendrait rien que le tampon rendu ne dise
            // déjà.
            state: None,
            status: conduit_com::STATUS_SUCCESS,
            persisted: None,
        });
        // Taille **requise**, écrite ou non : contrat de `get`.
        Ok(CABLE_PACKETS_BYTES as u32)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Structure de deux cent quarante-huit octets : aucune `VARENUM` ne la nomme, d'où un
        // `PropTypeSet` nul, exactement comme pour l'état, les compteurs et le transport.
        let ecrits = basic_support_ks(value, PACKETS_ACCESS_FLAGS, &GUID_NULL, 0);
        req.target.trace(&ConfigTrace {
            property: PROP_PAQUETS,
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            cable: req.target.cable_index(),
            state: None,
            status: ecrits.map_or_else(|status| status, |_| conduit_com::STATUS_SUCCESS),
            persisted: None,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Entrées de table prêtes à poser.
// ---------------------------------------------------------------------------------

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie :
/// `KSPROPSETID_Conduit`, [`KSPROPERTY_CONDUIT_CABLE_STATE`], `GET | SET | BASICSUPPORT`.
///
/// À poser dans `PCFILTER_DESCRIPTOR::AutomationTable`, à côté de
/// [`crate::jack::jack_description_item`]. `const fn` : les tables d'automatisation du
/// pilote sont des `static`. `V` est la vtable du miniport qui porte la table
/// (`IMiniportTopologyVtbl`), `T` son type — c'est ce couple que la garde de vtable de
/// [`crate::property::handler`] vérifie.
pub const fn cable_state_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: CableConfig,
{
    property::item::<V, T, ConduitCableState>(
        &SET_CONDUIT,
        KSPROPERTY_CONDUIT_CABLE_STATE,
        CABLE_STATE_ACCESS_FLAGS,
    )
}

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie :
/// `KSPROPSETID_Conduit`, [`KSPROPERTY_CONDUIT_VERSION`], `GET | BASICSUPPORT`.
pub const fn version_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: CableConfig,
{
    property::item::<V, T, ConduitVersion>(
        &SET_CONDUIT,
        KSPROPERTY_CONDUIT_VERSION,
        VERSION_ACCESS_FLAGS,
    )
}

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie :
/// `KSPROPSETID_Conduit`, [`KSPROPERTY_CONDUIT_COUNTERS`], `GET | BASICSUPPORT` (M1b-21).
pub const fn counters_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: CableConfig,
{
    property::item::<V, T, ConduitCounters>(
        &SET_CONDUIT,
        KSPROPERTY_CONDUIT_COUNTERS,
        COUNTERS_ACCESS_FLAGS,
    )
}

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie :
/// `KSPROPSETID_Conduit`, [`KSPROPERTY_CONDUIT_TRANSPORT`], `GET | BASICSUPPORT` (lot 0 du
/// mode paquets WaveRT).
pub const fn transport_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: CableConfig,
{
    property::item::<V, T, ConduitTransport>(
        &SET_CONDUIT,
        KSPROPERTY_CONDUIT_TRANSPORT,
        TRANSPORT_ACCESS_FLAGS,
    )
}

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie :
/// `KSPROPSETID_Conduit`, [`KSPROPERTY_CONDUIT_PACKETS`], `GET | BASICSUPPORT` (lot 2 du mode
/// paquets WaveRT).
pub const fn packets_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: CableConfig,
{
    property::item::<V, T, ConduitPackets>(
        &SET_CONDUIT,
        KSPROPERTY_CONDUIT_PACKETS,
        PACKETS_ACCESS_FLAGS,
    )
}

#[cfg(test)]
mod tests {
    // Tests en mode utilisateur : les lints anti-panique du noyau y sont sans objet, une
    // assertion fausse doit arrêter le test.
    #![allow(
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used
    )]

    use super::*;

    /// Le GUID exposé à PortCls est bien celui du contrat portable, champ par champ.
    #[test]
    fn le_guid_expose_est_celui_du_contrat() {
        assert_eq!(SET_CONDUIT.Data1, KSPROPSETID_CONDUIT.data1);
        assert_eq!(SET_CONDUIT.Data2, KSPROPSETID_CONDUIT.data2);
        assert_eq!(SET_CONDUIT.Data3, KSPROPSETID_CONDUIT.data3);
        assert_eq!(SET_CONDUIT.Data4, KSPROPSETID_CONDUIT.data4);
        // Et surtout : ce n'est pas `GUID_NULL`, qu'une conversion ratée produirait.
        assert_ne!(SET_CONDUIT.Data1, GUID_NULL.Data1);
    }

    /// Les drapeaux d'accès : l'état s'écrit, la version, les compteurs, le transport et le
    /// relevé de paquets non.
    #[test]
    fn les_drapeaux_disent_qui_s_ecrit() {
        assert_eq!(CABLE_STATE_ACCESS_FLAGS, 515);
        assert_eq!(VERSION_ACCESS_FLAGS, 513);
        assert_eq!(COUNTERS_ACCESS_FLAGS, 513);
        assert_eq!(TRANSPORT_ACCESS_FLAGS, 513);
        assert_eq!(PACKETS_ACCESS_FLAGS, 513);
        assert_ne!(CABLE_STATE_ACCESS_FLAGS & KSPROPERTY_TYPE_SET, 0);
        assert_eq!(VERSION_ACCESS_FLAGS & KSPROPERTY_TYPE_SET, 0);
        assert_eq!(
            COUNTERS_ACCESS_FLAGS & KSPROPERTY_TYPE_SET,
            0,
            "un compteur n'est pas un réglage : le bit SET ne doit pas être déclaré"
        );
        assert_eq!(
            TRANSPORT_ACCESS_FLAGS & KSPROPERTY_TYPE_SET,
            0,
            "une observation n'est pas un réglage : le bit SET ne doit pas être déclaré"
        );
        assert_eq!(
            PACKETS_ACCESS_FLAGS & KSPROPERTY_TYPE_SET,
            0,
            "le mode paquets se règle par le registre et un redémarrage du périphérique, \
             pas par cette propriété : le bit SET ne doit pas être déclaré"
        );
        // Les cinq répondent à BASICSUPPORT : dès ce bit posé, PortCls ne répond plus à
        // notre place.
        assert_ne!(CABLE_STATE_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
        assert_ne!(VERSION_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
        assert_ne!(COUNTERS_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
        assert_ne!(TRANSPORT_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
        assert_ne!(PACKETS_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
    }

    /// Les cinq entrées de table portent le même jeu et cinq identifiants distincts.
    #[test]
    fn les_quatre_entrees_partagent_le_jeu_et_pas_l_identifiant() {
        let ids = [
            KSPROPERTY_CONDUIT_CABLE_STATE,
            KSPROPERTY_CONDUIT_VERSION,
            KSPROPERTY_CONDUIT_COUNTERS,
            KSPROPERTY_CONDUIT_TRANSPORT,
            KSPROPERTY_CONDUIT_PACKETS,
        ];
        for (i, gauche) in ids.iter().enumerate() {
            for droite in ids.iter().skip(i + 1) {
                assert_ne!(
                    gauche, droite,
                    "PortCls sert la première entrée de même Set/Id : deux identifiants \
                     égaux rendraient une des propriétés inatteignable"
                );
            }
        }
        assert_eq!(CABLE_STATE_BYTES, 16);
        assert_eq!(TAILLE_VERSION, 4);
        assert_eq!(CABLE_COUNTERS_BYTES, 56);
        assert_eq!(CABLE_TRANSPORT_BYTES, 80);
        assert_eq!(CABLE_PACKETS_BYTES, 248);
        // Les cinq longueurs sont distinctes : un client qui allouerait la mauvaise se
        // fait refuser au lieu de lire une structure pour une autre.
        let tailles = [
            CABLE_STATE_BYTES,
            TAILLE_VERSION,
            CABLE_COUNTERS_BYTES,
            CABLE_TRANSPORT_BYTES,
            CABLE_PACKETS_BYTES,
        ];
        for (i, gauche) in tailles.iter().enumerate() {
            for droite in tailles.iter().skip(i + 1) {
                assert_ne!(gauche, droite);
            }
        }
    }

    /// La sérialisation du relevé de paquets écrit chaque champ à son décalage, ou rien du
    /// tout.
    ///
    /// Les trente-six valeurs diffèrent d'un sens à l'autre, pour la raison de
    /// [`la_serialisation_du_transport_est_tout_ou_rien`].
    #[test]
    fn la_serialisation_des_paquets_est_tout_ou_rien() {
        let paquets = paquets_temoin();

        // Place suffisante : les trente-six champs, à leurs décalages.
        let mut tampon = [0xAAu8; CABLE_PACKETS_BYTES];
        ecrire_paquets(&mut tampon, &paquets);
        assert_eq!(tampon, paquets.to_bytes());
        assert_eq!(CablePackets::from_bytes(&tampon), Ok(paquets));

        // Un octet de trop peu : rien n'est écrit, le tampon reste tel quel.
        let mut court = [0xAAu8; CABLE_PACKETS_BYTES - 1];
        ecrire_paquets(&mut court, &paquets);
        assert_eq!(court, [0xAAu8; CABLE_PACKETS_BYTES - 1]);

        // Tampon vide (interrogation de taille) : rien non plus, et aucune panique.
        ecrire_paquets(&mut [], &paquets);

        // Plus grand que nécessaire : les deux cent quarante-huit premiers octets, et rien au-delà.
        let mut grand = [0xAAu8; CABLE_PACKETS_BYTES + 8];
        ecrire_paquets(&mut grand, &paquets);
        assert_eq!(&grand[..CABLE_PACKETS_BYTES], &paquets.to_bytes()[..]);
        assert_eq!(&grand[CABLE_PACKETS_BYTES..], &[0xAAu8; 8]);
    }

    /// Les deux champs réservés du relevé de paquets partent à zéro, comme ceux des trois
    /// autres structures : rien de la mémoire du noyau ne transite par eux.
    #[test]
    fn les_reserves_des_paquets_partent_a_zero() {
        let cible = Faux {
            cable: 3,
            connected: true,
            channels: 2,
        };
        let paquets = cible.packets();
        assert_eq!(paquets.render.reserved, 0);
        assert_eq!(paquets.capture.reserved, 0);
        assert_eq!(
            paquets.cable,
            cible.cable_index(),
            "l'écho de câble doit être celui du miniport"
        );
        let mut tampon = [0xFFu8; CABLE_PACKETS_BYTES];
        ecrire_paquets(&mut tampon, &paquets);
        for base in [OCP_RENDER, OCP_CAPTURE] {
            let debut = base + OSP_RESERVED;
            assert_eq!(&tampon[debut..debut + 4], &[0, 0, 0, 0], "bloc {base}");
        }
    }

    /// La sérialisation du transport écrit chaque champ à son décalage, ou rien du tout.
    ///
    /// Les quatorze valeurs sont toutes différentes d'un sens à l'autre : deux blocs
    /// intervertis passeraient un aller-retour sur des zéros sans que rien ne le signale, et
    /// c'est exactement l'erreur qui ferait lire « le rendu scrute » d'un câble où c'est la
    /// capture qui scrute.
    #[test]
    fn la_serialisation_du_transport_est_tout_ou_rien() {
        let transport = transport_temoin();

        // Place suffisante : les quatorze champs, à leurs décalages.
        let mut tampon = [0xAAu8; CABLE_TRANSPORT_BYTES];
        ecrire_transport(&mut tampon, &transport);
        assert_eq!(tampon, transport.to_bytes());
        assert_eq!(CableTransport::from_bytes(&tampon), Ok(transport));

        // Un octet de trop peu : rien n'est écrit, le tampon reste tel quel.
        let mut court = [0xAAu8; CABLE_TRANSPORT_BYTES - 1];
        ecrire_transport(&mut court, &transport);
        assert_eq!(court, [0xAAu8; CABLE_TRANSPORT_BYTES - 1]);

        // Tampon vide (interrogation de taille) : rien non plus, et aucune panique.
        ecrire_transport(&mut [], &transport);

        // Plus grand que nécessaire : les soixante-douze premiers octets, et rien au-delà.
        let mut grand = [0xAAu8; CABLE_TRANSPORT_BYTES + 8];
        ecrire_transport(&mut grand, &transport);
        assert_eq!(&grand[..CABLE_TRANSPORT_BYTES], &transport.to_bytes()[..]);
        assert_eq!(&grand[CABLE_TRANSPORT_BYTES..], &[0xAAu8; 8]);
    }

    /// Le champ réservé du transport part toujours à zéro, comme celui de l'état et celui
    /// des compteurs.
    #[test]
    fn le_reserve_du_transport_part_a_zero() {
        let cible = Faux {
            cable: 3,
            connected: true,
            channels: 2,
        };
        let transport = cible.transport();
        assert_eq!(transport.reserved, 0);
        assert_eq!(
            transport.cable,
            cible.cable_index(),
            "l'écho de câble doit être celui du miniport"
        );
        let mut tampon = [0xFFu8; CABLE_TRANSPORT_BYTES];
        ecrire_transport(&mut tampon, &transport);
        assert_eq!(&tampon[OT_RESERVED..OT_RESERVED + 4], &[0, 0, 0, 0]);
    }

    /// La sérialisation des compteurs écrit chaque champ à son décalage, ou rien du tout.
    ///
    /// Les six valeurs sont toutes différentes : deux compteurs intervertis passeraient un
    /// aller-retour sur des zéros sans que rien ne le signale.
    #[test]
    fn la_serialisation_des_compteurs_est_tout_ou_rien() {
        let compteurs = CableCounters {
            cable: 3,
            reserved: 0,
            ticks: 11,
            copied: 22,
            silenced_no_render: 33,
            silenced_before_render: 44,
            discarded_ticks: 55,
            overruns: 66,
        };

        // Place suffisante : les huit champs, à leurs décalages.
        let mut tampon = [0xAAu8; CABLE_COUNTERS_BYTES];
        ecrire_compteurs(&mut tampon, &compteurs);
        assert_eq!(tampon, compteurs.to_bytes());
        assert_eq!(CableCounters::from_bytes(&tampon), Ok(compteurs));

        // Un octet de trop peu : rien n'est écrit, le tampon reste tel quel.
        let mut court = [0xAAu8; CABLE_COUNTERS_BYTES - 1];
        ecrire_compteurs(&mut court, &compteurs);
        assert_eq!(court, [0xAAu8; CABLE_COUNTERS_BYTES - 1]);

        // Tampon vide (interrogation de taille) : rien non plus, et aucune panique.
        ecrire_compteurs(&mut [], &compteurs);

        // Plus grand que nécessaire : les cinquante-six premiers octets, et rien au-delà.
        let mut grand = [0xAAu8; CABLE_COUNTERS_BYTES + 8];
        ecrire_compteurs(&mut grand, &compteurs);
        assert_eq!(&grand[..CABLE_COUNTERS_BYTES], &compteurs.to_bytes()[..]);
        assert_eq!(&grand[CABLE_COUNTERS_BYTES..], &[0xAAu8; 8]);
    }

    /// Le champ réservé des compteurs part toujours à zéro, comme celui de l'état : rien de
    /// la mémoire du noyau ne transite par lui.
    #[test]
    fn le_reserve_des_compteurs_part_a_zero() {
        let cible = Faux {
            cable: 3,
            connected: true,
            channels: 2,
        };
        let compteurs = cible.counters();
        assert_eq!(compteurs.reserved, 0);
        assert_eq!(
            compteurs.cable,
            cible.cable_index(),
            "l'écho de câble doit être celui du miniport"
        );
        let mut tampon = [0xFFu8; CABLE_COUNTERS_BYTES];
        ecrire_compteurs(&mut tampon, &compteurs);
        assert_eq!(&tampon[OC_RESERVED..OC_RESERVED + 4], &[0, 0, 0, 0]);
    }

    /// La sérialisation de l'état écrit chaque champ à son décalage, ou rien du tout.
    #[test]
    fn la_serialisation_est_tout_ou_rien() {
        let etat = CableState {
            cable: 3,
            connected: 1,
            channels: 2,
            reserved: 0,
        };

        // Place suffisante : les quatre champs, à leurs décalages.
        let mut tampon = [0xAAu8; CABLE_STATE_BYTES];
        ecrire_etat(&mut tampon, &etat);
        assert_eq!(tampon, etat.to_bytes());

        // Un octet de trop peu : rien n'est écrit, le tampon reste tel quel.
        let mut court = [0xAAu8; CABLE_STATE_BYTES - 1];
        ecrire_etat(&mut court, &etat);
        assert_eq!(court, [0xAAu8; CABLE_STATE_BYTES - 1]);

        // Tampon vide (interrogation de taille) : rien non plus, et aucune panique.
        ecrire_etat(&mut [], &etat);

        // Plus grand que nécessaire : les seize premiers octets, et rien au-delà.
        let mut grand = [0xAAu8; CABLE_STATE_BYTES + 8];
        ecrire_etat(&mut grand, &etat);
        assert_eq!(&grand[..CABLE_STATE_BYTES], &etat.to_bytes()[..]);
        assert_eq!(&grand[CABLE_STATE_BYTES..], &[0xAAu8; 8]);
    }

    /// Le champ réservé part toujours à zéro, même si l'état en mémoire en portait un
    /// autre : rien de la mémoire du noyau ne transite par lui.
    #[test]
    fn le_champ_reserve_part_a_zero() {
        let etat = etat_courant(&Faux {
            cable: 0,
            connected: true,
            channels: 2,
        });
        assert_eq!(etat.reserved, 0);
        let mut tampon = [0xFFu8; CABLE_STATE_BYTES];
        ecrire_etat(&mut tampon, &etat);
        assert_eq!(&tampon[O_RESERVED..O_RESERVED + 4], &[0, 0, 0, 0]);
    }

    /// Les paliers de `BASICSUPPORT` : rien, les `AccessFlags`, la description.
    #[test]
    fn les_paliers_de_basic_support() {
        // Moins de quatre octets : refusé, rien d'écrit.
        for place in 0..TAILLE_ACCESSFLAGS {
            let mut tampon = std::vec![0u8; place];
            assert_eq!(
                basic_support_ks(&mut tampon, CABLE_STATE_ACCESS_FLAGS, &GUID_NULL, 0),
                Err(STATUS_BUFFER_TOO_SMALL),
                "place {place}"
            );
        }

        // De 4 à 39 : les seuls `AccessFlags`, et le compte rendu est celui des octets
        // **écrits**, pas la taille requise.
        for place in TAILLE_ACCESSFLAGS..TAILLE_DESCRIPTION {
            let mut tampon = std::vec![0u8; place];
            let ecrits =
                basic_support_ks(&mut tampon, CABLE_STATE_ACCESS_FLAGS, &GUID_NULL, 0).unwrap();
            assert_eq!(ecrits, TAILLE_ACCESSFLAGS as u32, "place {place}");
            assert_eq!(
                u32::from_ne_bytes(tampon[..4].try_into().unwrap()),
                CABLE_STATE_ACCESS_FLAGS
            );
        }

        // 40 et au-delà : la description entière, et `DescriptionSize` vaut 40 — il n'y a
        // pas de palier de membres.
        let mut tampon = [0u8; TAILLE_DESCRIPTION];
        let ecrits = basic_support_ks(
            &mut tampon,
            VERSION_ACCESS_FLAGS,
            &KSPROPTYPESETID_General,
            19,
        )
        .unwrap();
        assert_eq!(ecrits, TAILLE_DESCRIPTION as u32);
        assert_eq!(
            u32::from_ne_bytes(tampon[0..4].try_into().unwrap()),
            VERSION_ACCESS_FLAGS
        );
        assert_eq!(
            u32::from_ne_bytes(tampon[4..8].try_into().unwrap()),
            TAILLE_DESCRIPTION as u32
        );
        // `PropTypeSet.Id` est à 24, pas à 20 : `PropTypeSet` est un `KSIDENTIFIER` de 24
        // octets, et `MembersListCount` (0) est à 32.
        assert_eq!(u32::from_ne_bytes(tampon[24..28].try_into().unwrap()), 19);
        assert_eq!(u32::from_ne_bytes(tampon[32..36].try_into().unwrap()), 0);
    }

    /// Un miniport minimal, sans état partagé : ces tests ne visent que la sérialisation
    /// et la validation, le reste passe par le faux PortCls de `tests/config.rs`.
    struct Faux {
        cable: u32,
        connected: bool,
        /// Le nombre de canaux que ce câble sert. Réglable depuis M1b-05 : c'est lui que
        /// le `SET` confronte à la valeur demandée, et un test qui le fixerait à 2 en dur
        /// ne dirait plus rien d'un câble à six canaux.
        channels: u32,
    }

    impl CableConfig for Faux {
        fn cable_index(&self) -> u32 {
            self.cable
        }
        fn channels(&self) -> u32 {
            self.channels
        }
        fn is_connected(&self) -> bool {
            self.connected
        }
        fn set_connected(&self, _connected: bool) -> Result<(), NtStatus> {
            Ok(())
        }
        fn may_configure(&self) -> bool {
            true
        }
        fn counters(&self) -> CableCounters {
            // Le régime « rendu seul » de M1b-07 : des ticks jetés, rien d'écrit.
            CableCounters {
                ticks: 100,
                discarded_ticks: 100,
                ..CableCounters::new(self.cable)
            }
        }

        fn transport(&self) -> CableTransport {
            CableTransport {
                cable: self.cable,
                ..transport_temoin()
            }
        }

        fn packets(&self) -> CablePackets {
            CablePackets {
                cable: self.cable,
                ..paquets_temoin()
            }
        }
    }

    /// Le relevé de paquets de référence des tests : le mode exposé, un rendu qui a été
    /// demandé et emprunté, une capture demandée et refusée.
    ///
    /// Les deux sens portent des valeurs toutes distinctes, pour la raison de
    /// [`transport_temoin`] : deux blocs de quatre-vingts octets écrits l'un à la place de
    /// l'autre feraient lire « c'est la capture qui emprunte » d'un câble où c'est le rendu.
    fn paquets_temoin() -> CablePackets {
        use conduit_kmd_core::config::PacketExposure;

        CablePackets {
            cable: 3,
            packet_mode: 1,
            render: StreamPackets {
                exposure: PacketExposure::Output.code(),
                irql_last: 0,
                irql_max: 2,
                reserved: 0,
                set_write_packet: 13,
                get_read_packet: 17,
                packet_count: 19,
                presentation_position: 23,
                queries: 29,
                queries_granted: 31,
                first_qpc: 37,
                last_qpc: 41,
                set_write_late: 79,
                set_write_overrun: 83,
                last_packet_count_returned: 89,
                packets_reached_at_last_count: 97,
                last_write_at_last_count: 101,
            },
            capture: StreamPackets {
                exposure: PacketExposure::Input.code(),
                irql_last: 1,
                irql_max: 3,
                reserved: 0,
                set_write_packet: 43,
                get_read_packet: 47,
                packet_count: 53,
                presentation_position: 59,
                queries: 61,
                queries_granted: 67,
                first_qpc: 71,
                last_qpc: 73,
                set_write_late: 103,
                set_write_overrun: 107,
                last_packet_count_returned: 109,
                packets_reached_at_last_count: 113,
                last_write_at_last_count: 127,
            },
        }
    }

    /// L'état de transport de référence des tests : un rendu **avec notifications**, une
    /// capture en **scrutation**, et des refus des deux côtés.
    ///
    /// Les deux sens sont volontairement dans des modes différents et portent des valeurs
    /// toutes distinctes : c'est la seule façon d'attraper deux blocs de trente-deux octets
    /// écrits l'un à la place de l'autre. Le câble est celui du faux miniport.
    fn transport_temoin() -> CableTransport {
        use conduit_kmd_core::config::{AllocationMode, KsRunState};

        CableTransport {
            cable: 3,
            reserved: 0,
            // Posée d'un côté, refusée de l'autre : une inversion des deux sens se voit.
            constraints_render: 0,
            constraints_capture: 0xC000_000D,
            render: StreamTransport {
                mode: AllocationMode::Notifications.code(),
                notification_count: 2,
                buffer_bytes: 1_920,
                buffer_frames: 480,
                notification_events: 1,
                ks_state: KsRunState::Run.code(),
                refused_allocations: 7,
            },
            capture: StreamTransport {
                mode: AllocationMode::Polling.code(),
                notification_count: 0,
                buffer_bytes: 3_840,
                buffer_frames: 960,
                notification_events: 0,
                ks_state: KsRunState::Pause.code(),
                refused_allocations: 11,
            },
        }
    }

    /// `appliquer` refuse l'écho de câble faux et tout nombre de canaux qui n'est pas
    /// celui du câble visé.
    #[test]
    fn appliquer_refuse_l_echo_faux_et_les_canaux_non_servis() {
        let cible = Faux {
            cable: 3,
            connected: false,
            channels: 2,
        };

        // Le bon câble, deux canaux : appliqué.
        let bon = CableState::new(3, true);
        assert!(appliquer(&cible, &bon.to_bytes()).is_ok());

        // Un autre câble, pourtant dans le domaine du contrat : refusé.
        let autre = CableState::new(4, true);
        let refus = appliquer(&cible, &autre.to_bytes()).err().unwrap();
        assert_eq!(
            refus,
            Ok(autre),
            "le refus porte l'état demandé, pas un parse"
        );

        // Six canaux sur un câble stéréo : dans le domaine du contrat, hors de ce que ce
        // câble sert. Le format se change par le registre et un redémarrage du devnode.
        let six = CableState {
            channels: 6,
            ..CableState::new(3, true)
        };
        assert!(!six.channels_appliquables(cible.channels()));
        assert!(appliquer(&cible, &six.to_bytes()).is_err());

        // Un tampon d'un octet de trop : refusé par le parseur portable, pas ici.
        let mut trop_long = std::vec::Vec::from(bon.to_bytes());
        trop_long.push(0);
        let refus = appliquer(&cible, &trop_long).err().unwrap();
        assert!(matches!(refus, Err(ConfigError::Longueur { recus: 17 })));
    }

    /// **La régression de frontière, vue du gestionnaire** : l'état qu'un client relit puis
    /// modifie est accepté sur n'importe quel format, celui qu'il fabrique ne l'est que sur
    /// un câble stéréo.
    ///
    /// C'est le refus mesuré en machine virtuelle — `conduit-helper activer 3` sur un câble
    /// en 96 kHz / 6 canaux, `ERROR_INVALID_PARAMETER` (87) — reproduit sans machine, du
    /// côté qui refuse.
    #[test]
    fn brancher_le_jack_passe_sur_tous_les_formats() {
        use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};

        for canaux in MIN_CHANNELS..=MAX_CHANNELS {
            let cible = Faux {
                cable: 3,
                connected: false,
                channels: canaux,
            };
            // Ce que le client relit, puis modifie : accepté quel que soit le format.
            let lu = etat_courant(&cible);
            let voulu = lu.avec_connexion(true);
            assert!(
                appliquer(&cible, &voulu.to_bytes()).is_ok(),
                "brancher le jack d'un câble à {canaux} canaux doit passer"
            );
            // Ce que le client fabriquait : refusé dès que le câble n'est pas au format
            // d'usine, sans que rien ne dise pourquoi côté appelant.
            let fabrique = CableState::new(3, true);
            assert_eq!(
                appliquer(&cible, &fabrique.to_bytes()).is_ok(),
                canaux == fabrique.channels,
                "un état fabriqué sur un câble à {canaux} canaux"
            );
        }
    }

    /// La symétrie de M1b-05 : sur un câble à six canaux, c'est « six » qui passe et
    /// « deux » qui est refusé. Le gestionnaire ne connaît plus de valeur privilégiée.
    #[test]
    fn appliquer_suit_les_canaux_du_cable_et_non_une_constante() {
        for (canaux, autre) in [(1u32, 2u32), (2, 6), (6, 2), (8, 7)] {
            let cible = Faux {
                cable: 3,
                connected: false,
                channels: canaux,
            };
            let bon = CableState {
                channels: canaux,
                ..CableState::new(3, true)
            };
            assert!(
                appliquer(&cible, &bon.to_bytes()).is_ok(),
                "{canaux} canaux refusés sur un câble à {canaux} canaux"
            );
            let mauvais = CableState {
                channels: autre,
                ..CableState::new(3, true)
            };
            assert!(
                appliquer(&cible, &mauvais.to_bytes()).is_err(),
                "{autre} canaux acceptés sur un câble à {canaux} canaux"
            );
        }
    }
}
