//! `KSPROPERTY_JACK_DESCRIPTION` (jeu `KSPROPSETID_Jack`) : trait [`JackInfo`] que le
//! miniport topologie implémente, et le [`PropertyHandler`] [`JackDescription`] posé sur
//! la brique de [`crate::property`].
//!
//! **Ce module ne stocke aucun état**, comme [`crate::audio`] : il décode une requête,
//! sérialise une réponse champ par champ, et demande le reste au miniport. L'état de
//! connexion vit dans le câble, côté `conduit-kmd`, dans un atomique.
//!
//! # Une propriété du **filtre** qui parle d'une **broche**
//!
//! C'est le point où l'intuition se trompe, et la documentation Microsoft est explicite
//! (« *Jack Description Property* ») : « *Although the property value contains information
//! about a pin (or rather, the jack or jacks that are associated with the pin), the
//! property is a property of the filter, not of the pin.* » L'entrée
//! [`jack_description_item`] se pose donc dans la `PCAUTOMATION_TABLE` du **filtre**
//! ([`PCFILTER_DESCRIPTOR::AutomationTable`]), pas dans celle d'une broche ; le
//! `PCPIN_DESCRIPTOR::AutomationTable` du pilote reste nul.
//!
//! Et ce n'est pas qu'une question de conformité : une table posée sur la broche ne serait
//! **jamais atteinte**. Les broches concernées sont des broches bridge
//! (`KSPIN_COMMUNICATION_NONE`, zéro instance) : elles ne s'ouvrent pas, donc aucune
//! requête ne leur parvient jamais par une poignée d'instance de broche. Le client
//! interroge le **filtre** et désigne la broche par un `KSP_PIN`.
//!
//! # `Instance` porte le numéro de broche
//!
//! Le descripteur de propriété est un `KSP_PIN` (`KSPROPERTY` de 24 octets, puis
//! `PinId: ULONG`, puis `Reserved: ULONG`). PortCls en retire l'en-tête et ne laisse dans
//! `Instance` que la **queue** : `InstanceSize == 8`, `PinId` **en premier**. C'est
//! exactement la mécanique du `Channel` de [`crate::audio`], et le golden la démontre —
//! `offset KSP_PIN.PinId 24` vaut `sizeof(KSPROPERTY)`.
//!
//! [`Pin::decode`] applique donc les mêmes règles défensives : moins de 4 octets
//! d'instance, `STATUS_INVALID_DEVICE_REQUEST` ; numéro de broche au-delà du dernier,
//! `STATUS_INVALID_PARAMETER`. Ce sont les statuts de l'exemple MSVAD de la documentation
//! (« *If the client specifies an invalid pin ID (that identifies a nonexistent pin), the
//! handler returns status code STATUS_INVALID_PARAMETER* »).
//!
//! # La forme de la réponse
//!
//! Un `KSMULTIPLE_ITEM` suivi de **N `KSJACK_DESCRIPTION`**, où N est le nombre de
//! **prises** associées à la broche — pas le nombre de canaux. La documentation de
//! `KSPROPERTY_JACK_DESCRIPTION` le dit : « *N = the number of jacks associated with the
//! specified bridge pin* », et son exemple 5.1 (six canaux, **trois** prises stéréo) le
//! rend imparable. Un câble Conduit est une prise unique qui porte ses deux canaux :
//! **N vaut 1**, et les deux canaux sont dans `ChannelMapping`.
//!
//! | Champ | Valeur |
//! |---|---|
//! | `KSMULTIPLE_ITEM.Size` | `8 + N * 28` : 36 avec la prise, 8 sans |
//! | `KSMULTIPLE_ITEM.Count` | N |
//!
//! Une broche **existante mais sans prise** (chez nous : la broche bridge vers le filtre
//! WaveRT) reçoit `STATUS_SUCCESS` et un `KSMULTIPLE_ITEM` **seul**, `Count = 0`. C'est
//! littéralement ce que prescrit la documentation : « *the query succeeds (with status
//! code STATUS_SUCCESS), but the property handler returns an empty jack description
//! consisting of a KSMULTIPLE_ITEM structure and nothing else* ».
//!
//! # Lecture seule, et `BASICSUPPORT` obligatoire
//!
//! `Flags = GET | BASICSUPPORT` ([`JACK_ACCESS_FLAGS`], 513) : **pas de `SET`**, la table
//! de la documentation le donne (« *Get: Yes, Set: No* »). M1b-04 changera l'état de
//! connexion par une propriété KS **privée**, pas par un `SET` sur celle-ci.
//!
//! Comme pour le volume, dès que le bit `BASICSUPPORT` est déclaré PortCls ne répond plus
//! à notre place. La réponse est en paliers, et [`PropertyHandler::basic_support`] rend
//! les octets **écrits** (asymétrie documentée sur le trait) :
//!
//! | Place dans `Value` | Écrit | rendu |
//! |---|---|---|
//! | < 4 | rien | `Err(STATUS_BUFFER_TOO_SMALL)` |
//! | 4 à 39 | `AccessFlags: ULONG` | 4 |
//! | ≥ 40 | `KSPROPERTY_DESCRIPTION` seule | 40 |
//!
//! Aucun membre : la valeur est de **taille variable** (N prises), qu'une plage de
//! `KSPROPERTY_MEMBERSHEADER` ne saurait décrire. D'où `MembersListCount = 0` et un
//! `PropTypeSet` **nul** (`GUID_NULL`, `Id = 0`) — c'est ce que fait
//! `PropertyHandler_BasicSupport` de SYSVAD quand on lui passe `VT_ILLEGAL`, et cette
//! valeur-là ne se sérialise pas : elle veut dire « pas de type simple », donc on écrit
//! l'ensemble vide plutôt que la variante.
//!
//! # Les valeurs des autres champs, une par une
//!
//! Un câble virtuel n'a ni couleur, ni connecteur, ni emplacement. Chaque champ prend donc
//! la valeur que la documentation de `KSJACK_DESCRIPTION` réserve à ce cas, jamais un zéro
//! au hasard : voir [`JACK_COLOR`], [`JACK_CONNECTION_TYPE`], [`JACK_GEO_LOCATION`],
//! [`JACK_GEN_LOCATION`] et [`JACK_PORT_CONNECTION`]. `ChannelMapping` est le seul que le
//! miniport fournit ([`JackInfo::channel_mapping`]) : la documentation le veut non nul
//! **pour les seules broches de rendu analogique**, et nul pour les broches de capture.
//!
//! # La contrepartie obligatoire : `KSEVENT_PINCAPS_JACKINFOCHANGE`
//!
//! Windows **n'interroge pas le jack en boucle**. Il lit `KSPROPERTY_JACK_DESCRIPTION` à
//! la construction de l'endpoint, puis n'y revient que si le pilote le lui signale par
//! l'événement `KSEVENT_PINCAPS_JACKINFOCHANGE` (jeu `KSEVENTSETID_PinCapsChange`,
//! `PCEVENT_ITEM` dans la table d'automatisation du **filtre**,
//! `IPortEvents::GenerateEventList` pour l'émettre).
//!
//! M1b-03 ne l'implémentait pas, et **c'était cohérent** : rien ne changeait encore l'état
//! de connexion, un événement qui ne se produit jamais n'a pas de lecteur. Dès que l'état
//! devient modifiable, **cet événement devient obligatoire** : sans lui, l'interface
//! utilisateur reste figée sur l'état du démarrage, une déconnexion demandée par
//! l'utilisateur n'a aucun effet visible, et le symptôme est « la propriété KS rend la
//! bonne valeur mais le panneau de son ne bouge pas » — un faux mystère qui coûte une
//! demi-journée.
//!
//! La brique existe désormais : [`crate::event`] — [`jack_info_change_item`] pour l'entrée
//! de table, [`with_events`] pour la poser à côté de [`jack_description_item`] dans la même
//! `PCAUTOMATION_TABLE` de filtre, et [`PortEvents::jack_info_change`] pour émettre. Il
//! reste à câbler côté `conduit-kmd` : garder l'`IPortEvents` du port dans le miniport
//! topologie ([`EventSource`]), et appeler la notification depuis `Cable::set_connected`,
//! une fois par filtre de topologie du câble avec le numéro de sa broche endpoint — celle
//! que [`JackInfo::jack_pin`] désigne.
//!
//! [`jack_info_change_item`]: crate::event::jack_info_change_item
//! [`with_events`]: crate::event::with_events
//! [`PortEvents::jack_info_change`]: crate::event::PortEvents::jack_info_change
//! [`EventSource`]: crate::event::EventSource

use conduit_com::{NtStatus, STATUS_INVALID_PARAMETER};
use portcls_sys::{
    EPcxConnectionType, EPcxGenLocation, EPcxGeoLocation, EPxcPortConnection, GUID, GUID_NULL,
    KSPROPERTY_JACK, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET, KSPROPSETID_Jack,
    PCPROPERTY_ITEM,
};

use crate::property::{
    self, Champs, D_ACCESSFLAGS, PropertyHandler, Request, TAILLE_ACCESSFLAGS, TAILLE_DESCRIPTION,
    TargetVtbl, ecrire_description,
};
use crate::status::{STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST};

// ---------------------------------------------------------------------------------
// Les valeurs du descripteur de prise, justifiées une à une contre la documentation de
// `KSJACK_DESCRIPTION`.
// ---------------------------------------------------------------------------------

/// `Flags` du `PCPROPERTY_ITEM` du jack : `GET | BASICSUPPORT` (513), **sans `SET`**.
///
/// La table d'utilisation de `KSPROPERTY_JACK_DESCRIPTION` donne « Get: Yes, Set: No » ;
/// le bit `BASICSUPPORT` fait de nous le seul répondant à ce verbe.
pub const JACK_ACCESS_FLAGS: u32 = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT;

/// `KSJACK_DESCRIPTION::Color` : **0x00000000**.
///
/// « *If the jack color is unknown or the physical connector has no identifiable color,
/// the value of this member is 0x00000000, which represents black.* » Un câble virtuel n'a
/// pas de connecteur du tout : c'est la valeur prévue, pas un zéro par défaut.
pub const JACK_COLOR: u32 = 0x0000_0000;

/// `KSJACK_DESCRIPTION::ConnectionType` : `eConnTypeUnknown`.
///
/// Les onze autres valeurs nomment un connecteur physique (minijack 3,5 mm, RCA, XLR,
/// optique…) ; aucune ne décrit l'absence de connecteur. `eConnTypeUnknown` est la seule
/// qui n'affirme rien de faux.
pub const JACK_CONNECTION_TYPE: u32 = EPcxConnectionType::eConnTypeUnknown as u32;

/// `KSJACK_DESCRIPTION::GeoLocation` : `eGeoLocNotApplicable`.
///
/// Le champ le mieux documenté des six : « *When an audio device does not expose a
/// physically accessible jack, the audio device uses the eGeoLocNotApplicable value to
/// indicate to Windows and Windows-based apps that there is no physical jack.* » C'est
/// exactement notre cas.
pub const JACK_GEO_LOCATION: u32 = EPcxGeoLocation::eGeoLocNotApplicable as u32;

/// `KSJACK_DESCRIPTION::GenLocation` : `eGenLocOther`.
///
/// Les trois autres valeurs situent la prise par rapport au châssis (« on primary
/// chassis », « inside primary chassis », « on separate chassis ») ; un câble logiciel n'en
/// a aucun. `eGenLocOther` (« other location ») est le fourre-tout de l'énumération, et
/// l'énumération n'offre pas de « sans objet » comme celle de la géométrie.
pub const JACK_GEN_LOCATION: u32 = EPcxGenLocation::eGenLocOther as u32;

/// `KSJACK_DESCRIPTION::PortConnection` : `ePortConnUnknown`.
///
/// Ni une prise (`ePortConnJack`), ni un emplacement pour périphérique intégré
/// (`ePortConnIntegratedDevice`), ni les deux : il n'y a aucun port. `ePortConnUnknown`
/// est la quatrième valeur, et la seule vraie.
pub const JACK_PORT_CONNECTION: u32 = EPxcPortConnection::ePortConnUnknown as u32;

const _: () = assert!(JACK_ACCESS_FLAGS == 513);
// Les quatre énumérations valent ce que dit `ksmedia.h` ; les recopier ici attrape un
// renommage de variante qui changerait silencieusement la description rendue.
const _: () = assert!(JACK_CONNECTION_TYPE == 0 && JACK_GEO_LOCATION == 14);
const _: () = assert!(JACK_GEN_LOCATION == 3 && JACK_PORT_CONNECTION == 3);

// ---------------------------------------------------------------------------------
// Décalages recopiés de portcls-sys/tests/layout.golden (oracle cl.exe), en absolu
// depuis le début du tampon `Value` : le `KSMULTIPLE_ITEM` d'abord, la prise ensuite.
// ---------------------------------------------------------------------------------

/// `KSMULTIPLE_ITEM::Size` (0) : taille **totale** de la réponse, en-tête compris.
const MI_SIZE: usize = 0;
/// `KSMULTIPLE_ITEM::Count` (4) : le nombre de prises.
const MI_COUNT: usize = 4;
/// `sizeof(KSMULTIPLE_ITEM)` (golden).
const TAILLE_MULTIPLE_ITEM: usize = 8;

/// `KSJACK_DESCRIPTION::ChannelMapping`, à la suite du `KSMULTIPLE_ITEM` (8 + 0).
const J_CHANNELMAPPING: usize = 8;
/// `KSJACK_DESCRIPTION::Color` (8 + 4).
const J_COLOR: usize = 12;
/// `KSJACK_DESCRIPTION::ConnectionType` (8 + 8).
const J_CONNECTIONTYPE: usize = 16;
/// `KSJACK_DESCRIPTION::GeoLocation` (8 + 12).
const J_GEOLOCATION: usize = 20;
/// `KSJACK_DESCRIPTION::GenLocation` (8 + 16).
const J_GENLOCATION: usize = 24;
/// `KSJACK_DESCRIPTION::PortConnection` (8 + 20).
const J_PORTCONNECTION: usize = 28;
/// `KSJACK_DESCRIPTION::IsConnected` (8 + 24) : un `BOOL`, donc un `LONG`.
const J_ISCONNECTED: usize = 32;

/// `sizeof(KSJACK_DESCRIPTION)` (golden).
const TAILLE_JACK_DESCRIPTION: usize = 28;
/// Réponse complète d'une broche à prise : `KSMULTIPLE_ITEM` + une `KSJACK_DESCRIPTION`.
const TAILLE_REPONSE_PRISE: usize = TAILLE_MULTIPLE_ITEM + TAILLE_JACK_DESCRIPTION;

const _: () = assert!(TAILLE_MULTIPLE_ITEM == 8 && TAILLE_JACK_DESCRIPTION == 28);
const _: () = assert!(TAILLE_REPONSE_PRISE == 36 && J_ISCONNECTED == 32);
// Le dernier champ de la prise finit exactement à la fin de la réponse : c'est ce qui
// relie les décalages du golden à la taille annoncée dans `KSMULTIPLE_ITEM::Size`.
const _: () = assert!(J_ISCONNECTED + 4 == TAILLE_REPONSE_PRISE);

/// Taille d'un `ULONG`/`LONG`, l'unité de tout ce qui est sérialisé ici.
const TAILLE_MOT: usize = 4;

/// `BOOL` de KS : `TRUE` vaut 1, `FALSE` vaut 0.
const KS_TRUE: i32 = 1;
/// `BOOL` de KS : `FALSE`.
const KS_FALSE: i32 = 0;

// ---------------------------------------------------------------------------------
// La broche visée.
// ---------------------------------------------------------------------------------

/// Ce que le `PinId` de la requête désigne, une fois décodé et validé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    /// La broche qui porte la prise ([`JackInfo::jack_pin`]) : réponse complète.
    Jack,
    /// Une autre broche **existante** du filtre : réponse vide (`KSMULTIPLE_ITEM` seul,
    /// `Count = 0`), comme le prescrit la documentation.
    Sans,
}

impl Pin {
    /// Décode le `PinId: ULONG` en tête de `instance`, la queue de `KSP_PIN` que PortCls
    /// nous laisse (`InstanceSize == 8` : `PinId` puis `Reserved`).
    ///
    /// `instance` est hostile et **d'alignement quelconque** : les quatre premiers octets
    /// sont recopiés puis interprétés, jamais transtypés.
    ///
    /// - moins de 4 octets (`InstanceSize < 4`) → `STATUS_INVALID_DEVICE_REQUEST` ;
    /// - `pin >= pin_count` → `STATUS_INVALID_PARAMETER` ;
    /// - `pin == jack_pin` → [`Pin::Jack`] ;
    /// - toute autre broche existante → [`Pin::Sans`].
    ///
    /// Les deux statuts d'erreur sont ceux de l'exemple MSVAD de la documentation
    /// Microsoft, et ils se distinguent : une requête tronquée est une requête mal formée,
    /// un numéro de broche inexistant est un paramètre faux.
    pub fn decode(instance: &[u8], pin_count: u32, jack_pin: u32) -> Result<Self, NtStatus> {
        let Some(quatre) = instance.get(..TAILLE_MOT) else {
            return Err(STATUS_INVALID_DEVICE_REQUEST);
        };
        let mut mot = [0u8; TAILLE_MOT];
        mot.copy_from_slice(quatre);
        let pin = u32::from_ne_bytes(mot);

        if pin >= pin_count {
            return Err(STATUS_INVALID_PARAMETER);
        }
        if pin == jack_pin {
            Ok(Self::Jack)
        } else {
            Ok(Self::Sans)
        }
    }

    /// Nombre de `KSJACK_DESCRIPTION` que la réponse porte : 1 pour la broche à prise,
    /// 0 pour les autres.
    const fn prises(self) -> u32 {
        match self {
            Self::Jack => 1,
            Self::Sans => 0,
        }
    }

    /// Taille totale de la réponse, `KSMULTIPLE_ITEM` compris.
    const fn taille(self) -> usize {
        match self {
            Self::Jack => TAILLE_REPONSE_PRISE,
            Self::Sans => TAILLE_MULTIPLE_ITEM,
        }
    }
}

// ---------------------------------------------------------------------------------
// Trace.
// ---------------------------------------------------------------------------------

/// Ce qu'un gestionnaire vient de faire d'une requête de jack, passé à
/// [`JackInfo::trace`].
///
/// Même rôle que [`crate::audio::Trace`] : rendre vérifiable en une minute, au premier
/// essai en machine, le décalage du `PinId` dans `Instance`. `instance` porte les octets
/// **bruts**, `pin` ce qu'on en a tiré ; les comparer suffit. Attendu sur un filtre de
/// topologie Conduit : la broche endpoint interrogée, l'autre jamais ou en `Sans`.
#[derive(Debug)]
pub struct JackTrace<'a> {
    /// Le verbe : `"GET"` ou `"BASICSUPPORT"`.
    pub verb: &'static str,
    /// Les octets d'`Instance` tels que PortCls les a laissés : la queue de `KSP_PIN`,
    /// normalement 8 octets, `PinId` d'abord.
    pub instance: &'a [u8],
    /// La broche décodée de ces octets, ou le statut d'erreur rendu à l'appelant.
    pub pin: Result<Pin, NtStatus>,
    /// L'état de connexion rendu, `None` si la requête n'en a pas rendu (erreur, ou
    /// broche sans prise, ou `BASICSUPPORT`).
    pub connected: Option<bool>,
}

/// Nom de verbe des traces de lecture.
const VERBE_GET: &str = "GET";
/// Nom de verbe des traces de description.
const VERBE_BASICSUPPORT: &str = "BASICSUPPORT";

// ---------------------------------------------------------------------------------
// Le trait métier.
// ---------------------------------------------------------------------------------

/// La prise d'un miniport topologie, vue par le gestionnaire de propriété.
///
/// `TopoRender` et `TopoCapture` l'implémenteront sur l'état du câble (un atomique, côté
/// `conduit-kmd`) : **ce module ne stocke rien**.
///
/// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne dans le contexte du
/// fil appelant). Appels concurrents possibles depuis plusieurs fils : d'où un atomique,
/// et non un verrou.
pub trait JackInfo: Send + Sync + 'static {
    /// Nombre de broches du filtre, tel que son `PCFILTER_DESCRIPTOR` le déclare.
    ///
    /// Sert à distinguer « broche existante sans prise » (réponse vide, `STATUS_SUCCESS`)
    /// de « broche inexistante » (`STATUS_INVALID_PARAMETER`). Doit s'accorder avec
    /// `PCFILTER_DESCRIPTOR::PinCount`, sinon le pilote répondrait `SUCCESS` pour une
    /// broche qui n'existe pas, ou refuserait une broche qui existe.
    fn pin_count(&self) -> u32;

    /// Numéro de la broche qui porte la prise : celle qui **fait face à l'extérieur**,
    /// celle dont Windows tire l'endpoint.
    ///
    /// Attention au vocabulaire : la documentation appelle « bridge pin » la broche par
    /// laquelle un périphérique d'extrémité se branche, c'est-à-dire chez nous la broche
    /// *endpoint* (`KSNODETYPE_LINE_CONNECTOR`, celle qui porte le nom du câble), et non
    /// la broche qui relie le filtre de topologie à son filtre WaveRT. Se tromper de
    /// broche ici ne casse rien de visible : la propriété répond, mais toujours par une
    /// description vide, et l'endpoint garde l'état de connexion que Windows lui donne par
    /// défaut.
    fn jack_pin(&self) -> u32;

    /// `KSJACK_DESCRIPTION::ChannelMapping` : masque `KSAUDIO_SPEAKER_*` des positions de
    /// haut-parleur.
    ///
    /// « *ChannelMapping should be nonzero only for analog rendering pins. For capture
    /// pins or for digital rendering pins, set this member to 0.* » Un miniport de rendu
    /// stéréo rend donc `KSAUDIO_SPEAKER_STEREO`, un miniport de capture **0**.
    fn channel_mapping(&self) -> u32;

    /// `KSJACK_DESCRIPTION::IsConnected` : y a-t-il quelque chose au bout du câble ?
    ///
    /// `false` fait apparaître l'endpoint sous « Périphériques déconnectés » dans les
    /// réglages Son. La documentation prescrit `TRUE` **inconditionnel** aux pilotes
    /// dépourvus de détection de présence ; nous en avons une (l'utilisateur décide quels
    /// câbles existent), donc `false` y est légitime et voulu.
    ///
    /// IRQL : quelconque.
    fn is_connected(&self) -> bool;

    /// Point de trace, appelé une fois par requête, après coup.
    ///
    /// Défaut : ne fait rien. Ne doit ni allouer ni bloquer.
    fn trace(&self, trace: &JackTrace<'_>) {
        let _ = trace;
    }
}

// ---------------------------------------------------------------------------------
// Sérialisation.
// ---------------------------------------------------------------------------------

/// Écrit le `KSMULTIPLE_ITEM` puis, si `pin` porte une prise, la `KSJACK_DESCRIPTION`.
///
/// **Tout ou rien** : si `value` est plus court que la réponse, rien n'est écrit et le
/// thunk rendra `STATUS_BUFFER_TOO_SMALL` avec la taille requise. C'est le contrat de
/// [`PropertyHandler::get`] (« ne rien écrire si `value` est trop court ») et le
/// comportement de l'exemple MSVAD ; une réponse à demi écrite serait pire qu'aucune,
/// puisque son `KSMULTIPLE_ITEM` annoncerait une prise absente du tampon.
fn ecrire_reponse(value: &mut [u8], pin: Pin, mapping: u32, connecte: bool) {
    if value.len() < pin.taille() {
        return;
    }
    let mut champs = Champs { dest: value };
    champs.u32(MI_SIZE, pin.taille() as u32);
    champs.u32(MI_COUNT, pin.prises());
    if pin == Pin::Sans {
        return;
    }
    champs.u32(J_CHANNELMAPPING, mapping);
    champs.u32(J_COLOR, JACK_COLOR);
    champs.u32(J_CONNECTIONTYPE, JACK_CONNECTION_TYPE);
    champs.u32(J_GEOLOCATION, JACK_GEO_LOCATION);
    champs.u32(J_GENLOCATION, JACK_GEN_LOCATION);
    champs.u32(J_PORTCONNECTION, JACK_PORT_CONNECTION);
    champs.i32(J_ISCONNECTED, if connecte { KS_TRUE } else { KS_FALSE });
}

/// Réponse à `KSPROPERTY_TYPE_BASICSUPPORT`, en paliers (voir la documentation du module).
///
/// Renvoie les octets **écrits**, jamais une taille requise.
fn basic_support_ks(value: &mut [u8]) -> Result<u32, NtStatus> {
    if value.len() < TAILLE_ACCESSFLAGS {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }

    let mut champs = Champs { dest: value };
    if champs.dest.len() < TAILLE_DESCRIPTION {
        // Premier appel de KS : `sizeof(ULONG)`, les seuls `AccessFlags`.
        champs.u32(D_ACCESSFLAGS, JACK_ACCESS_FLAGS);
        return Ok(TAILLE_ACCESSFLAGS as u32);
    }

    // Description complète : 40 octets, et rien de plus. La valeur est de taille variable
    // (N prises), donc aucun membre et un `PropTypeSet` nul — l'équivalent du `VT_ILLEGAL`
    // de SYSVAD. `DescriptionSize` vaut la taille écrite : il n'y a pas de palier au-delà.
    ecrire_description(
        &mut champs,
        JACK_ACCESS_FLAGS,
        TAILLE_DESCRIPTION as u32,
        &GUID_NULL,
        0,
        0,
    );
    Ok(TAILLE_DESCRIPTION as u32)
}

// ---------------------------------------------------------------------------------
// Le gestionnaire.
// ---------------------------------------------------------------------------------

/// `KSPROPERTY_JACK_DESCRIPTION` sur la table d'automatisation d'un filtre de topologie :
/// l'état de la prise de la broche désignée par `Instance`.
#[derive(Debug)]
pub struct JackDescription;

impl<T: JackInfo> PropertyHandler<T> for JackDescription {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        let broche = Pin::decode(req.instance, req.target.pin_count(), req.target.jack_pin());
        let connecte = broche.map(|pin| {
            let etat = req.target.is_connected();
            ecrire_reponse(value, pin, req.target.channel_mapping(), etat);
            (pin, etat)
        });
        req.target.trace(&JackTrace {
            verb: VERBE_GET,
            instance: req.instance,
            pin: broche,
            connected: connecte
                .ok()
                .and_then(|(pin, etat)| (pin == Pin::Jack).then_some(etat)),
        });
        // Taille **requise**, écrite ou non : c'est le contrat de `get` (le thunk en
        // déduit SUCCESS / BUFFER_TOO_SMALL / BUFFER_OVERFLOW, exactement la négociation
        // que fait l'exemple MSVAD à la main).
        connecte.map(|(pin, _)| pin.taille() as u32)
    }

    fn basic_support(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Le numéro de broche est validé **avant** le verbe, comme dans l'exemple MSVAD :
        // décrire la propriété d'une broche inexistante n'aurait pas de sens.
        let broche = Pin::decode(req.instance, req.target.pin_count(), req.target.jack_pin());
        let ecrits = broche.and_then(|_| basic_support_ks(value));
        req.target.trace(&JackTrace {
            verb: VERBE_BASICSUPPORT,
            instance: req.instance,
            pin: broche,
            connected: None,
        });
        ecrits
    }
}

// ---------------------------------------------------------------------------------
// Entrée de table prête à poser.
// ---------------------------------------------------------------------------------

/// `KSPROPSETID_Jack` en `static` : `PCPROPERTY_ITEM::Set` veut une adresse `'static`, et
/// les GUID de `portcls-sys` sont des `const`.
static SET_JACK: GUID = KSPROPSETID_Jack;

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie : `KSPROPSETID_Jack`,
/// `KSPROPERTY_JACK_DESCRIPTION`, `GET | BASICSUPPORT`.
///
/// À poser dans `PCFILTER_DESCRIPTOR::AutomationTable`, pas sur une broche (voir la
/// documentation du module). `const fn` : les tables d'automatisation du pilote sont des
/// `static`. `V` est la vtable du miniport qui porte la table
/// (`IMiniportTopologyVtbl`), `T` son type — c'est ce couple que la garde de vtable de
/// [`crate::property::handler`] vérifie.
pub const fn jack_description_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: JackInfo,
{
    property::item::<V, T, JackDescription>(
        &SET_JACK,
        KSPROPERTY_JACK::KSPROPERTY_JACK_DESCRIPTION as u32,
        JACK_ACCESS_FLAGS,
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

    /// Les tailles sont bien celles du golden, et la réponse complète en est la somme.
    #[test]
    fn les_tailles_sont_celles_du_golden() {
        assert_eq!(TAILLE_MULTIPLE_ITEM, 8);
        assert_eq!(TAILLE_JACK_DESCRIPTION, 28);
        assert_eq!(
            TAILLE_REPONSE_PRISE,
            TAILLE_MULTIPLE_ITEM + TAILLE_JACK_DESCRIPTION
        );
        assert_eq!(TAILLE_DESCRIPTION, 40);
    }

    /// Les décalages de la prise se suivent de quatre en quatre, du `KSMULTIPLE_ITEM` à
    /// la fin de la réponse : sept `DWORD`, aucun trou, aucun recouvrement.
    #[test]
    fn les_decalages_de_la_prise_pavent_la_reponse() {
        let decalages = [
            MI_SIZE,
            MI_COUNT,
            J_CHANNELMAPPING,
            J_COLOR,
            J_CONNECTIONTYPE,
            J_GEOLOCATION,
            J_GENLOCATION,
            J_PORTCONNECTION,
            J_ISCONNECTED,
        ];
        for (i, decalage) in decalages.iter().enumerate() {
            assert_eq!(*decalage, i * TAILLE_MOT, "décalage n° {i}");
        }
        assert_eq!(
            decalages.len() * TAILLE_MOT,
            TAILLE_REPONSE_PRISE,
            "la réponse est exactement ces neuf mots"
        );
    }

    /// Les valeurs des champs constants sont celles que `ksmedia.h` nomme.
    #[test]
    fn les_valeurs_du_descripteur_sont_celles_de_ksmedia() {
        assert_eq!(JACK_COLOR, 0);
        assert_eq!(
            JACK_CONNECTION_TYPE,
            EPcxConnectionType::eConnTypeUnknown as u32
        );
        assert_eq!(
            JACK_GEO_LOCATION,
            EPcxGeoLocation::eGeoLocNotApplicable as u32
        );
        assert_eq!(JACK_GEN_LOCATION, EPcxGenLocation::eGenLocOther as u32);
        assert_eq!(
            JACK_PORT_CONNECTION,
            EPxcPortConnection::ePortConnUnknown as u32
        );
        // `GET | BASICSUPPORT`, et surtout **pas** `SET` : la propriété est en lecture
        // seule (M1b-04 passera par une propriété privée).
        assert_eq!(JACK_ACCESS_FLAGS, 513);
        assert_eq!(JACK_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET, 0);
    }

    /// Décodage de la broche : les quatre cas.
    #[test]
    fn decodage_de_la_broche() {
        let instance = |pin: u32| pin.to_ne_bytes();
        // Filtre à deux broches, la prise sur la broche 1 (cas `TopoRender`).
        assert_eq!(Pin::decode(&instance(1), 2, 1), Ok(Pin::Jack));
        assert_eq!(Pin::decode(&instance(0), 2, 1), Ok(Pin::Sans));
        // Cas `TopoCapture` : la prise est sur la broche 0.
        assert_eq!(Pin::decode(&instance(0), 2, 0), Ok(Pin::Jack));
        assert_eq!(Pin::decode(&instance(1), 2, 0), Ok(Pin::Sans));
        // Broche inexistante.
        assert_eq!(
            Pin::decode(&instance(2), 2, 1),
            Err(STATUS_INVALID_PARAMETER)
        );
        assert_eq!(
            Pin::decode(&instance(u32::MAX), 2, 1),
            Err(STATUS_INVALID_PARAMETER)
        );
        // Instance absente ou tronquée : requête mal formée, statut différent.
        assert_eq!(Pin::decode(&[], 2, 1), Err(STATUS_INVALID_DEVICE_REQUEST));
        assert_eq!(
            Pin::decode(&[0, 0, 0], 2, 1),
            Err(STATUS_INVALID_DEVICE_REQUEST)
        );
    }

    /// Le compte annoncé et la taille annoncée se déduisent l'un de l'autre.
    #[test]
    fn le_compte_et_la_taille_s_accordent() {
        assert_eq!(Pin::Jack.prises(), 1);
        assert_eq!(Pin::Sans.prises(), 0);
        assert_eq!(
            Pin::Jack.taille(),
            TAILLE_MULTIPLE_ITEM + Pin::Jack.prises() as usize * TAILLE_JACK_DESCRIPTION
        );
        assert_eq!(
            Pin::Sans.taille(),
            TAILLE_MULTIPLE_ITEM + Pin::Sans.prises() as usize * TAILLE_JACK_DESCRIPTION
        );
    }
}
