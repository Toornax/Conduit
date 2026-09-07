//! Le jeu de propriétés KS **privé** de configuration (M1b-04, driver-design.md §6) :
//! trait [`CableConfig`] que le miniport topologie implémente, et les deux
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
//! # Deux propriétés, un jeu
//!
//! | Propriété | Verbes | Valeur |
//! |---|---|---|
//! | [`KSPROPERTY_CONDUIT_CABLE_STATE`] | GET, SET, BASICSUPPORT | [`CableState`], 16 octets |
//! | [`KSPROPERTY_CONDUIT_VERSION`] | GET, BASICSUPPORT | un `ULONG` |
//!
//! Les deux se posent dans la `PCAUTOMATION_TABLE` du **filtre** de topologie
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
//! **Réserve écrite, faute d'avoir pu l'établir.** `SeSinglePrivilegeCheck` n'a de sens que
//! si le gestionnaire de propriété s'exécute dans le contexte du **fil appelant** : sur un
//! fil système, `ExGetPreviousMode()` rendrait `KernelMode`, la routine rendrait vrai
//! inconditionnellement, et le contrôle serait pire qu'absent — il donnerait l'illusion
//! d'une protection. Or **la documentation Microsoft ne dit rien du contexte de fil des
//! gestionnaires PortCls** : `portcls.h` n'en parle pas, la page `PCPROPERTY_ITEM` a un
//! champ IRQL vide, et il n'existe pas de page dédiée à `PCPFNPROPERTY_HANDLER`. Le
//! raisonnement qui rend le contrôle crédible — un `IOCTL_KS_PROPERTY` est une
//! `IRP_MJ_DEVICE_CONTROL` traitée en ligne par la routine de répartition, donc dans le fil
//! qui a appelé `DeviceIoControl` — est solide mais **non documenté**, et rien n'interdit à
//! PortCls de différer une requête. Le contrôle reste donc en place, et la vérification en
//! machine virtuelle reste **à faire** : un `SET` depuis un processus non élevé doit rendre
//! `STATUS_PRIVILEGE_NOT_HELD`. Si ce n'était pas le cas, il faudrait passer par
//! `PCPROPERTY_REQUEST::Irp` (`RequestorMode`, `Tail.Overlay.Thread`) plutôt que par le fil
//! courant.
//!
//! # Ce que ce module ne fait pas
//!
//! - **`KSEVENT_PINCAPS_JACKINFOCHANGE`** : sans lui, Windows n'ira jamais relire
//!   `KSPROPERTY_JACK_DESCRIPTION` et l'interface restera figée sur l'état du démarrage
//!   (voir [`crate::jack`]). La propriété rendra la bonne valeur et le panneau de son ne
//!   bougera pas. La brique d'événements est construite à part ; le point d'insertion est
//!   marqué dans `conduit_kmd::cable::Cable::set_connected`.
//! - **Appliquer les canaux** : [`CableState::channels`] est validé par le contrat
//!   portable, puis le gestionnaire refuse toute valeur autre que celle que le pilote sait
//!   servir ([`CableState::channels_applicables`]). `descriptors::CHANNELS` est scellée
//!   dans les tables KS et dans des assertions à la compilation ; rendre le champ effectif
//!   est M1b-05. Répondre `STATUS_SUCCESS` à un réglage qui n'agit sur rien serait pire
//!   qu'un refus.

use conduit_com::{NtStatus, STATUS_INVALID_PARAMETER};
use conduit_kmd_core::config::{
    CABLE_STATE_BYTES, CONFIG_VERSION, CableState, ConfigError, ConfigGuid,
    KSPROPERTY_CONDUIT_CABLE_STATE, KSPROPERTY_CONDUIT_VERSION, KSPROPSETID_CONDUIT, O_CABLE,
    O_CHANNELS, O_CONNECTED, O_RESERVED,
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
const fn en_guid(guid: &ConfigGuid) -> GUID {
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

/// Taille de la valeur de [`KSPROPERTY_CONDUIT_VERSION`] : un `ULONG`.
const TAILLE_VERSION: usize = 4;

const _: () = assert!(CABLE_STATE_ACCESS_FLAGS == 515);
const _: () = assert!(VERSION_ACCESS_FLAGS == 513);
const _: () = assert!(VERSION_ACCESS_FLAGS & KSPROPERTY_TYPE_SET == 0);
const _: () = assert!(CABLE_STATE_BYTES == 16 && TAILLE_VERSION == 4);
// Les décalages de la structure d'échange sont ceux du contrat portable et pavent bien ses
// seize octets : la sérialisation ci-dessous n'en recopie pas une seconde définition.
const _: () = assert!(O_CABLE == 0 && O_CONNECTED == 4 && O_CHANNELS == 8 && O_RESERVED == 12);
const _: () = assert!(O_RESERVED + 4 == CABLE_STATE_BYTES);

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
/// documenté.
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
    /// Sert à remplir [`CableState::channels`] au `GET`. Le `SET` ne s'en sert pas pour
    /// valider — c'est [`CableState::channels_applicables`] qui tranche, contre la valeur
    /// que le pilote sait servir jusqu'à M1b-05.
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
/// 3. [`CableState::channels`] est dans le domaine du contrat mais pas dans ce que le
///    pilote sait servir (M1b-05).
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
    if !etat.channels_applicables() {
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

    /// Les drapeaux d'accès : l'état s'écrit, la version non.
    #[test]
    fn les_drapeaux_disent_qui_s_ecrit() {
        assert_eq!(CABLE_STATE_ACCESS_FLAGS, 515);
        assert_eq!(VERSION_ACCESS_FLAGS, 513);
        assert_ne!(CABLE_STATE_ACCESS_FLAGS & KSPROPERTY_TYPE_SET, 0);
        assert_eq!(VERSION_ACCESS_FLAGS & KSPROPERTY_TYPE_SET, 0);
        // Les deux répondent à BASICSUPPORT : dès ce bit posé, PortCls ne répond plus à
        // notre place.
        assert_ne!(CABLE_STATE_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
        assert_ne!(VERSION_ACCESS_FLAGS & KSPROPERTY_TYPE_BASICSUPPORT, 0);
    }

    /// Les deux entrées de table portent le même jeu et deux identifiants distincts.
    #[test]
    fn les_deux_entrees_partagent_le_jeu_et_pas_l_identifiant() {
        assert_ne!(
            KSPROPERTY_CONDUIT_CABLE_STATE, KSPROPERTY_CONDUIT_VERSION,
            "PortCls sert la première entrée de même Set/Id : deux identifiants égaux \
             rendraient une des deux propriétés inatteignable"
        );
        assert_eq!(CABLE_STATE_BYTES, 16);
        assert_eq!(TAILLE_VERSION, 4);
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
    }

    impl CableConfig for Faux {
        fn cable_index(&self) -> u32 {
            self.cable
        }
        fn channels(&self) -> u32 {
            2
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
    }

    /// `appliquer` refuse l'écho de câble faux et les canaux que M1b-04 ne sert pas.
    #[test]
    fn appliquer_refuse_l_echo_faux_et_les_canaux_non_servis() {
        let cible = Faux {
            cable: 3,
            connected: false,
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

        // Six canaux : dans le domaine du contrat, hors de ce que le pilote sert (M1b-05).
        let six = CableState {
            channels: 6,
            ..CableState::new(3, true)
        };
        assert!(!six.channels_applicables());
        assert!(appliquer(&cible, &six.to_bytes()).is_err());

        // Un tampon d'un octet de trop : refusé par le parseur portable, pas ici.
        let mut trop_long = std::vec::Vec::from(bon.to_bytes());
        trop_long.push(0);
        let refus = appliquer(&cible, &trop_long).err().unwrap();
        assert!(matches!(refus, Err(ConfigError::Longueur { recus: 17 })));
    }
}
