//! Événements KS : trait sûr [`EventHandler`], thunk [`handler`], constructeur d'entrée
//! de table [`item`], et l'enveloppe [`PortEvents`] par laquelle un événement **part**.
//!
//! C'est le pendant de [`crate::property`] pour l'autre moitié du contrat d'automatisation
//! de PortCls — et la moitié qui se comporte différemment, ce qui est tout le risque de ce
//! module. Une propriété est une **question** du client à laquelle le pilote répond dans le
//! même appel ; un événement est un **abonnement** que le client dépose et que le pilote
//! honore plus tard, depuis un autre contexte. Trois conséquences que la suite détaille :
//! le miniport ne possède pas la liste d'abonnés, la requête n'a aucun tampon de valeur, et
//! **rien de ce que fait ce module n'est observable en retour**.
//!
//! # Ce que M1b-04 doit signaler, et pourquoi
//!
//! Windows **n'interroge pas `KSPROPERTY_JACK_DESCRIPTION` en boucle** : il la lit à la
//! construction de l'endpoint, puis n'y revient que si le pilote le lui dit. La dette est
//! écrite en toutes lettres en tête de [`crate::jack`] : dès que l'état de connexion d'un
//! câble devient modifiable, l'absence d'événement se manifeste par « la propriété KS rend
//! la bonne valeur mais le panneau de son ne bouge pas ».
//!
//! L'événement à émettre est `KSEVENT_PINCAPS_JACKINFOCHANGE` du jeu
//! `KSEVENTSETID_PinCapsChange` ([`jack_info_change_item`], [`PortEvents::jack_info_change`]).
//!
//! # Le miniport **accepte** l'abonnement, il ne le crée pas
//!
//! La liste d'événements est tenue par l'**objet port**, pas par le miniport
//! (documentation PortCls, « Hardware Events »). PortCls trouve le `PCEVENT_ITEM` de même
//! `Set`/`Id` dans la `PCAUTOMATION_TABLE` de la cible, alloue une `PCEVENT_REQUEST` et
//! appelle notre `Handler` avec l'un de trois verbes :
//!
//! | Verbe | Valeur | Ce que le miniport doit faire |
//! |---|---|---|
//! | `PCEVENT_VERB_SUPPORT` | 4 | réussir si l'événement est supporté pour cette cible, échouer sinon. Rien à écrire : une `PCEVENT_REQUEST` n'a **aucun tampon de valeur** |
//! | `PCEVENT_VERB_ADD` | 1 | valider, puis remettre `EventEntry` à [`PortEvents::add_to_event_list`]. C'est cet appel qui **vaut acquittement** de l'activation |
//! | `PCEVENT_VERB_REMOVE` | 2 | **rien**. Il n'existe aucune API de retrait : le port a déjà retiré l'entrée, il nous prévient |
//!
//! Les valeurs sont des **bits** (`NONE` 0, `ADD` 1, `REMOVE` 2, `SUPPORT` 4), pas un
//! énuméré dense — `SUPPORT` vaut 4, pas 3. `portcls-sys/tests/vtables.rs` le fige.
//!
//! `EventEntry` (`PKSEVENT_ENTRY`) est une **structure système opaque**, que la
//! documentation demande explicitement de traiter comme telle : elle commence par un
//! `LIST_ENTRY` par lequel le port la chaîne dans sa propre liste. La copier ou la déplacer
//! serait fatal. [`EventEntry`] ne l'expose donc que comme un pointeur qu'on rend tel quel.
//!
//! # Signaler : `GenerateEventList`, pas `AddEventToEventList`
//!
//! `IPortEvents` a deux méthodes et elles ne servent pas au même moment. Elles diffèrent
//! aussi par l'IRQL, et c'est déterminant :
//!
//! | Méthode | Quand | IRQL documenté |
//! |---|---|---|
//! | `AddEventToEventList` | pendant `PCEVENT_VERB_ADD`, pour accepter un abonnement | **`PASSIVE_LEVEL`** |
//! | `GenerateEventList` | quand l'état change, pour réveiller les abonnés | **quelconque**, avec une réserve |
//!
//! La réserve de `GenerateEventList` est documentée et vaut d'être connue : au-dessus de
//! `DISPATCH_LEVEL`, l'implémentation met en file un **DPC qui ne porte le contexte que
//! d'un seul appel**, et deux signalements rapprochés peuvent alors se perdre. À
//! `<= DISPATCH_LEVEL`, aucune restriction. Notre appelant — `Cable::set_connected`,
//! atteint depuis un gestionnaire de propriété — est à `PASSIVE_LEVEL` : le cas dégradé ne
//! nous concerne pas, mais [`PortEvents::generate`] le documente parce que rien dans le
//! type ne l'empêcherait.
//!
//! Les deux méthodes rendent **`void`** : le port ne dit jamais s'il a fait quelque chose.
//! C'est la propriété la plus gênante de tout ce module, et elle explique la forme des
//! tests : on peut vérifier que le bon slot est appelé avec les bons arguments (faux port
//! des tests), **jamais** qu'une notification est effectivement partie. Une erreur de
//! câblage ici est silencieuse des deux côtés.
//!
//! # Filtre ou broche : l'item sur le **filtre**, le ciblage dans le **signalement**
//!
//! Deux affirmations coexistent dans la documentation et se concilient :
//!
//! - la page de `PCEVENT_ITEM` dit qu'en audio WDM une **instance de filtre ne peut pas
//!   être la cible** d'une requête d'événement — la cible est une broche ou un nœud ;
//! - `KSEVENT_PINCAPS_JACKINFOCHANGE` est néanmoins déclaré, dans SYSVAD, sur la table
//!   d'automatisation du **filtre** de topologie
//!   (`DEFINE_PCAUTOMATION_TABLE_PROP_EVENT(AutomationSpeakerHpTopoFilter, …)`,
//!   `EndpointsCommon/speakerhptoptable.h`), les `PCPIN_DESCRIPTOR::AutomationTable` du
//!   même filtre restant **nuls**.
//!
//! La première phrase parle de la cible de la *requête KS*, la seconde de l'endroit où
//! l'item est *déclaré*. Le ciblage de la broche n'est donc pas porté par la table mais par
//! le **signalement** : `GenerateEventList(…, PinEvent = TRUE, PinId = <broche>, …)`. C'est
//! exactement la même géométrie que `KSPROPERTY_JACK_DESCRIPTION` (une propriété du filtre
//! qui décrit une broche, voir [`crate::jack`]) — mais ce n'est pas une déduction : c'est
//! ce que fait SYSVAD, et il fallait le vérifier plutôt que le supposer.
//!
//! **Ce qui reste incertain** : la règle de routage interne de PortCls — comment un item
//! déclaré au niveau filtre est retenu pour une requête dont la cible KS est une broche —
//! n'est décrite nulle part dans la documentation publique. Le comportement est **attesté
//! par SYSVAD, pas spécifié**. Si l'événement ne partait pas en machine, c'est le premier
//! endroit où regarder, et poser une seconde table sur la broche endpoint est l'expérience
//! à tenter — les tests unitaires de ce module ne peuvent rien en dire.
//!
//! # Frontière de confiance
//!
//! Plus étroite que celle de [`crate::property`], et pour une bonne raison : une
//! `PCEVENT_REQUEST` **ne porte aucun tampon venu du mode utilisateur**. Ni `Instance`, ni
//! `Value`, ni taille. Il ne reste donc à valider que des pointeurs :
//!
//! - `request` peut être nul (défense, comme dans [`crate::property`]) ;
//! - `MajorTarget` est notre miniport — d'où la **même garde de vtable** qu'en
//!   [`crate::property::handler`], pour la même raison : rien, au niveau des types, ne
//!   relie une `PCAUTOMATION_TABLE` au miniport auquel elle appartient ;
//! - `EventItem` porte le `Set` et l'`Id` de l'entrée trouvée par PortCls. Le gestionnaire
//!   les **revalide** ([`JackInfoChange`]) : la documentation ne garantit pas qu'un
//!   handler ne soit appelé que pour son propre item, et SYSVAD fait ce contrôle ;
//! - `EventEntry` peut être nul sur un `ADD` : `STATUS_UNSUCCESSFUL`, comme SYSVAD.
//!
//! Rien n'est déréférencé au-delà : ni l'`Irp`, ni l'entrée d'événement.

use core::ffi::c_void;
use core::fmt;

use conduit_com::{
    ComInterface, ComObject, ComRef, Guid, NtStatus, RawPtr, STATUS_INVALID_PARAMETER,
    STATUS_SUCCESS, STATUS_UNSUCCESSFUL, nt_success,
};
use portcls_sys::{
    BOOL, GUID, IID_IPortEvents, IPortEvents, KSEVENT_PINCAPS_CHANGENOTIFICATIONS,
    KSEVENT_TYPE_BASICSUPPORT, KSEVENT_TYPE_ENABLE, KSEVENTSETID_PinCapsChange, NTSTATUS,
    PCAUTOMATION_TABLE, PCEVENT_ITEM, PCEVENT_VERB_ADD, PCEVENT_VERB_REMOVE, PCEVENT_VERB_SUPPORT,
    PKSEVENT_ENTRY, PPCEVENT_REQUEST, ULONG,
};

use crate::property::TargetVtbl;
use crate::status::{STATUS_INVALID_DEVICE_REQUEST, STATUS_NOT_FOUND, STATUS_NOT_SUPPORTED};

// ---------------------------------------------------------------------------------
// L'entrée d'événement : un pointeur opaque, et rien d'autre.
// ---------------------------------------------------------------------------------

/// `PKSEVENT_ENTRY` : l'abonnement qu'un client vient de déposer, vu du miniport.
///
/// **Opaque par contrat** (documentation de `KSEVENT_ENTRY` : « system data structure »).
/// La structure commence par un `LIST_ENTRY` que le port utilise pour la chaîner dans sa
/// propre liste ; la copier, la déplacer ou en lire un champ n'aurait aucun sens et
/// casserait le chaînage. Le seul usage légitime est de la rendre telle quelle à
/// [`PortEvents::add_to_event_list`].
///
/// Une `EventEntry` construite par [`handler`] est toujours **non nulle** : le thunk refuse
/// un `ADD` sans entrée avant d'appeler le gestionnaire.
///
/// Ni `Send` ni `Sync`, et volontairement : l'entrée n'a de sens que dans l'appel où
/// PortCls nous la remet, et la garder au-delà serait un pointeur pendant.
#[derive(Clone, Copy)]
pub struct EventEntry(PKSEVENT_ENTRY);

impl EventEntry {
    /// Le pointeur brut, tel qu'il ira à `IPortEvents::AddEventToEventList`.
    pub fn as_raw(self) -> PKSEVENT_ENTRY {
        self.0
    }
}

impl fmt::Debug for EventEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Le pointeur, jamais son contenu : la structure est opaque.
        f.debug_tuple("EventEntry").field(&self.0).finish()
    }
}

// ---------------------------------------------------------------------------------
// La requête, en sûr.
// ---------------------------------------------------------------------------------

/// Ce qu'un gestionnaire d'événement reçoit d'une `PCEVENT_REQUEST`, en sûr.
///
/// Pas de tampon : une `PCEVENT_REQUEST` n'en porte aucun (voir la documentation du
/// module). L'entrée d'événement, elle, n'est passée qu'à [`EventHandler::add`], seul
/// verbe où elle a un sens.
pub struct EventRequest<'a, T> {
    /// Le miniport, retrouvé depuis `MajorTarget` après vérification de sa vtable.
    pub target: &'a T,
    /// `PCEVENT_REQUEST::Node` : le nœud visé, ou `ULONG(-1)` si l'événement porte sur le
    /// filtre ou une broche.
    pub node: u32,
    /// `EventItem->Set` : le jeu d'événement de l'entrée que PortCls a trouvée.
    ///
    /// À **revalider** par le gestionnaire : rien ne garantit qu'il ne soit appelé que
    /// pour son propre item (voir la frontière de confiance en tête de module).
    pub set: &'a GUID,
    /// `EventItem->Id` : l'identifiant dans le jeu.
    pub id: u32,
}

impl<T> fmt::Debug for EventRequest<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventRequest")
            .field("node", &self.node)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Deux `GUID` sont-ils égaux ? (`GUID` généré n'implémente pas `PartialEq`.)
///
/// Passe par `conduit_com::Guid`, qui a la même disposition et l'implémente : une seule
/// définition de l'égalité de GUID dans le pilote, plutôt qu'une comparaison champ par
/// champ recopiée à chaque usage.
pub(crate) fn guid_eq(a: &GUID, b: &GUID) -> bool {
    portcls_sys::com::guid(a) == portcls_sys::com::guid(b)
}

// ---------------------------------------------------------------------------------
// Le trait métier.
// ---------------------------------------------------------------------------------

/// Gestionnaire d'un événement KS, côté pilote : un type sans état, un verbe par méthode.
///
/// Les trois méthodes ont un défaut `Err(STATUS_NOT_SUPPORTED)`, comme
/// [`PropertyHandler`](crate::property::PropertyHandler) : un gestionnaire ne déclare que
/// ce qu'il sait faire, et les `Flags` du [`item`] correspondant doivent s'accorder avec.
///
/// # Ce que les trois verbes veulent dire
///
/// Aucun des trois ne rend de données — une `PCEVENT_REQUEST` n'a pas de tampon. Ils ne
/// rendent qu'un `NTSTATUS`, et c'est ce statut qui **est** la réponse : accepter ou
/// refuser l'abonnement.
pub trait EventHandler<T: Send + Sync + 'static>: 'static {
    /// `PCEVENT_VERB_SUPPORT` : « supportes-tu cet événement pour cette cible ? ».
    ///
    /// `Ok(())` veut dire oui, et c'est tout ce qu'il y a à faire — rien à écrire, rien à
    /// enregistrer.
    ///
    /// IRQL : `PASSIVE_LEVEL` (contrat PortCls, non mesuré).
    fn support(req: &EventRequest<'_, T>) -> Result<(), NtStatus> {
        let _ = req;
        Err(STATUS_NOT_SUPPORTED)
    }

    /// `PCEVENT_VERB_ADD` : un client active l'événement.
    ///
    /// Le gestionnaire valide (`set`, `id`, cible), puis remet `entry` au port par
    /// [`PortEvents::add_to_event_list`] : **c'est cet appel qui acquitte l'activation**,
    /// pas la valeur de retour. Rendre `Ok(())` sans avoir appelé le port laisserait le
    /// client abonné à un événement qui ne partira jamais.
    ///
    /// `entry` est toujours non nulle : [`handler`] refuse un `ADD` sans entrée
    /// (`STATUS_UNSUCCESSFUL`) avant d'arriver ici.
    ///
    /// IRQL : `PASSIVE_LEVEL` — exigé, pas seulement observé :
    /// `IPortEvents::AddEventToEventList` le demande.
    fn add(req: &EventRequest<'_, T>, entry: EventEntry) -> Result<(), NtStatus> {
        let _ = (req, entry);
        Err(STATUS_NOT_SUPPORTED)
    }

    /// `PCEVENT_VERB_REMOVE` : le port a retiré un abonnement de sa liste et nous le dit.
    ///
    /// **Il n'y a rien à faire** : `IPortEvents` n'expose aucune méthode de retrait, la
    /// liste appartient au port. Un gestionnaire peut s'en servir pour cesser de produire
    /// l'événement ; le nôtre ne le fait pas, produire un événement sans abonné étant sans
    /// conséquence.
    ///
    /// IRQL : `PASSIVE_LEVEL` (contrat PortCls, non mesuré).
    fn remove(req: &EventRequest<'_, T>) -> Result<(), NtStatus> {
        let _ = req;
        Err(STATUS_NOT_SUPPORTED)
    }
}

// ---------------------------------------------------------------------------------
// L'entrée de table et le thunk.
// ---------------------------------------------------------------------------------

/// Entrée de `PCAUTOMATION_TABLE` câblée sur `H` : `Set`, `Id`, `Flags` et le thunk
/// [`handler`] monomorphisé.
///
/// `PCEVENT_ITEM` a **exactement la forme** de `PCPROPERTY_ITEM` — `Set`, `Id`, `Flags`,
/// `Handler` aux mêmes décalages, 24 octets (mesuré : `portcls-sys/tests/layout.golden`) —
/// d'où un constructeur jumeau de [`crate::property::item`]. Ne pas en conclure que les
/// *requêtes* se ressemblent : `PCEVENT_REQUEST` porte un `EventEntry` que
/// `PCPROPERTY_REQUEST` n'a pas, et son `Verb` est au décalage **40**, pas 32.
///
/// `flags` est une combinaison de `PCEVENT_ITEM_FLAG_ENABLE`, `_ONESHOT` et
/// `_BASICSUPPORT` (les `KSEVENT_TYPE_*` homonymes).
///
/// `const fn` : les tables d'automatisation du pilote sont des `static`.
pub const fn item<V, T, H>(set: &'static GUID, id: u32, flags: u32) -> PCEVENT_ITEM
where
    V: TargetVtbl<T>,
    T: Send + Sync + 'static,
    H: EventHandler<T>,
{
    PCEVENT_ITEM {
        Set: set,
        Id: id,
        Flags: flags,
        Handler: Some(handler::<V, T, H>),
    }
}

/// `PCPFNEVENT_HANDLER` : le thunk de PortCls vers `H`.
///
/// Retrouve le miniport depuis `MajorTarget` (après la garde de vtable de
/// [`crate::property`], identique et pour la même raison), lit `Set`/`Id` dans
/// `EventItem`, aiguille sur `Verb` et traduit le `Result` en `NTSTATUS`.
///
/// # Aiguillage du verbe
///
/// `Verb` est composé de bits distincts, comme les `KSPROPERTY_TYPE_*` : il est donc testé
/// défensivement, un bit à la fois, dans cet ordre — **`SUPPORT`, puis `REMOVE`, puis
/// `ADD`**. L'ordre n'est pas arbitraire. `SUPPORT` d'abord parce que c'est le verbe
/// descriptif, comme `BASICSUPPORT` en [`crate::property::handler`]. `ADD` **en dernier**
/// parce que c'est le seul des trois qui ait un effet de bord hors du pilote (l'entrée
/// entre dans la liste du port) : sur une valeur composite qui ne devrait pas exister, le
/// thunk fait ainsi le moins conséquent des gestes possibles plutôt que le plus.
/// `PCEVENT_VERB_NONE` (0) et tout le reste sont `STATUS_INVALID_DEVICE_REQUEST`.
///
/// # Table des statuts
///
/// | Cas | Code |
/// |---|---|
/// | `request` nul | `STATUS_INVALID_PARAMETER` |
/// | `MajorTarget` nul | `STATUS_INVALID_DEVICE_REQUEST` |
/// | vtable inattendue | `STATUS_INVALID_DEVICE_REQUEST` |
/// | `EventItem` ou `EventItem->Set` nul | `STATUS_INVALID_DEVICE_REQUEST` |
/// | verbe hors SUPPORT/ADD/REMOVE | `STATUS_INVALID_DEVICE_REQUEST` |
/// | `ADD` sans `EventEntry` | `STATUS_UNSUCCESSFUL` |
/// | verbe non implémenté par `H` | `STATUS_NOT_SUPPORTED` |
/// | `Ok(())` | `STATUS_SUCCESS` |
/// | `Err(s)` | `s` |
///
/// `STATUS_INVALID_DEVICE_REQUEST` sur un verbe inconnu est la convention de ce crate
/// ([`crate::status`]), pas une prescription : SYSVAD répond `STATUS_INVALID_PARAMETER` au
/// même cas et la documentation ne tranche pas. La cohérence interne l'emporte ici, un
/// verbe inconnu n'ayant de toute façon aucun client.
///
/// IRQL : `PASSIVE_LEVEL`.
///
/// # Safety
///
/// `request`, s'il n'est pas nul, pointe une `PCEVENT_REQUEST` lisible le temps de l'appel,
/// dont `MajorTarget` est nul ou pointe un objet COM vivant (donc porteur d'un pointeur de
/// vtable à l'offset 0), et dont `EventItem`, s'il n'est pas nul, pointe un `PCEVENT_ITEM`
/// vivant — c'est exactement le contrat de PortCls.
pub unsafe extern "C" fn handler<V, T, H>(request: PPCEVENT_REQUEST) -> NTSTATUS
where
    V: TargetVtbl<T>,
    T: Send + Sync + 'static,
    H: EventHandler<T>,
{
    if request.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `request` est non nul et pointe une `PCEVENT_REQUEST` lisible le temps de
    // l'appel (contrat) ; PortCls ne la partage avec personne pendant ce temps. Le thunk
    // n'y écrit rien : une requête d'événement n'a aucun champ de sortie.
    let req = unsafe { &*request };

    let this: *mut c_void = req.MajorTarget.cast();
    if this.is_null() {
        return STATUS_INVALID_DEVICE_REQUEST;
    }

    // Garde de vtable : voir `crate::property`, en tête de module. Le premier mot de tout
    // objet COM est son pointeur de vtable ; celui d'un `ComObject<V, T>` est l'occurrence
    // canonique `V::vtbl()`. Lire ce mot ne suppose pas encore que l'objet est le nôtre.
    // SAFETY: `this` est non nul et pointe un objet COM vivant (contrat) : `lpVtbl` est à
    // l'offset 0 et fait la taille d'un pointeur (`ComObject` est `repr(C)`, avec une
    // assertion `offset_of!(…, vtbl) == 0` dans `conduit-com`).
    let observee = unsafe { *this.cast::<*const c_void>() };
    let attendue: *const c_void = (V::vtbl() as *const V).cast();
    if observee != attendue {
        return STATUS_INVALID_DEVICE_REQUEST;
    }
    // SAFETY: la vtable observée est celle que le constructeur d'objet du crate donne aux
    // `ComObject<V, T>` et à eux seuls (contrat de `TargetVtbl`) : `this` en est un, et il
    // reste vivant pendant l'appel puisque PortCls détient une référence dessus.
    let target = unsafe { ComObject::<V, T>::inner(this) };

    if req.EventItem.is_null() {
        return STATUS_INVALID_DEVICE_REQUEST;
    }
    // SAFETY: `EventItem` est non nul et pointe le `PCEVENT_ITEM` de la table
    // d'automatisation, une `static` du pilote vivante pour toute la vie du filtre
    // (contrat PortCls).
    let event_item = unsafe { &*req.EventItem };
    if event_item.Set.is_null() {
        return STATUS_INVALID_DEVICE_REQUEST;
    }
    // SAFETY: `Set` est non nul et pointe le `GUID` `'static` que `item` y a mis (ou celui
    // qu'une table écrite à la main y a mis : même contrat, un `&'static GUID`).
    let set = unsafe { &*event_item.Set };

    let req_sur = EventRequest {
        target,
        node: req.Node,
        set,
        id: event_item.Id,
    };

    let verb = req.Verb;
    let resultat = if verb & PCEVENT_VERB_SUPPORT != 0 {
        H::support(&req_sur)
    } else if verb & PCEVENT_VERB_REMOVE != 0 {
        H::remove(&req_sur)
    } else if verb & PCEVENT_VERB_ADD != 0 {
        // Une entrée nulle sur un ADD est la réponse de SYSVAD (`STATUS_UNSUCCESSFUL`) :
        // la requête est bien formée mais inutilisable, il n'y a rien à abonner.
        if req.EventEntry.is_null() {
            Err(STATUS_UNSUCCESSFUL)
        } else {
            H::add(&req_sur, EventEntry(req.EventEntry))
        }
    } else {
        return STATUS_INVALID_DEVICE_REQUEST;
    };

    match resultat {
        Ok(()) => STATUS_SUCCESS,
        Err(status) => status,
    }
}

// ---------------------------------------------------------------------------------
// La table d'automatisation qui porte propriétés **et** événements.
// ---------------------------------------------------------------------------------

/// Ajoute une table d'événements à une `PCAUTOMATION_TABLE` déjà construite.
///
/// Une `PCAUTOMATION_TABLE` porte trois triplets `(ItemSize, Count, pointeur)` — propriétés,
/// méthodes, événements — indépendants les uns des autres ; c'est exactement ce que fait la
/// macro `DEFINE_PCAUTOMATION_TABLE_PROP_EVENT` de `portcls.h`. Cette fonction remplit le
/// **troisième** sans toucher aux deux autres, pour que les tables existantes du pilote
/// (`one_property_automation` de `conduit-kmd`, qui fige `EventCount = 0`) gagnent leurs
/// événements par une seule ligne de plus :
///
/// ```ignore
/// static TOPO_RENDER_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(with_events(
///     one_property_automation(&RENDER_JACK_PROPERTIES),
///     RENDER_JACK_EVENTS.get(),
/// ));
/// ```
///
/// `EventItemSize` vaut `sizeof(PCEVENT_ITEM)` : la documentation permet une taille
/// **plus grande** (la structure PortCls suivie de données privées du miniport), exige
/// qu'elle soit un multiple de 8, et laisse à zéro le seul cas d'un compte nul. Nous
/// n'étendons pas l'item ; 24 est mesuré par `cl.exe` (`layout.golden`).
///
/// `const fn` : les tables d'automatisation du pilote sont des `static`. `N` est déduit du
/// tableau, ce qui rend impossible le désaccord classique entre le compte et la table.
pub const fn with_events<const N: usize>(
    table: PCAUTOMATION_TABLE,
    events: &'static [PCEVENT_ITEM; N],
) -> PCAUTOMATION_TABLE {
    let mut table = table;
    table.EventItemSize = size_of::<PCEVENT_ITEM>() as ULONG;
    table.EventCount = N as ULONG;
    table.Events = events.as_ptr();
    table
}

// ---------------------------------------------------------------------------------
// L'interface par laquelle l'événement part.
// ---------------------------------------------------------------------------------

/// `IPortEvents` : l'aide au signalement que le **port** offre au miniport.
///
/// Elle n'est pas remise dans `Init` comme `IPortTopology` : elle s'obtient par
/// [`from_port`](Self::from_port) — un `QueryInterface` sur le port avec `IID_IPortEvents`.
/// D'où sa place ici plutôt que dans [`crate::received`], qui enveloppe les interfaces
/// *reçues*.
///
/// Le miniport la garde pour toute sa vie (SYSVAD fait le `QueryInterface` dans `Init` et
/// relâche la référence au `Drop`) : c'est par elle que passera
/// [`jack_info_change`](Self::jack_info_change) quand l'état de connexion d'un câble
/// changera.
#[derive(Debug, Clone)]
pub struct PortEvents(ComRef<IPortEvents>);

impl PortEvents {
    /// Enveloppe une référence déjà possédée.
    pub fn from_ref(r: ComRef<IPortEvents>) -> Self {
        Self(r)
    }

    /// La référence sous-jacente.
    pub fn com_ref(&self) -> &ComRef<IPortEvents> {
        &self.0
    }

    /// `QueryInterface(IID_IPortEvents)` sur le port : l'appel à faire dans `Init`.
    ///
    /// `port` est n'importe quelle interface du **port** (`IPortTopology`, `IPortWaveRT`) :
    /// c'est le même objet COM, et c'est lui qui implémente `IPortEvents`, pas le miniport.
    /// Depuis une enveloppe de [`crate::received`], le paramètre est son
    /// `com_ref()`.
    ///
    /// `Err(STATUS_NOT_FOUND)` si le port ne répond pas à cet IID. SYSVAD traite ce cas
    /// différemment selon le miniport — la topologie fait échouer `Init`, le WaveRT se
    /// contente d'un pointeur nul ; ce module rend l'erreur et laisse l'appelant choisir.
    ///
    /// IRQL : `PASSIVE_LEVEL` (`Init`) ; `QueryInterface` lui-même tolère
    /// `<= DISPATCH_LEVEL`.
    pub fn from_port<I: ComInterface>(port: &ComRef<I>) -> Result<Self, NtStatus> {
        let iid: Guid = portcls_sys::com::guid(&IID_IPortEvents);
        let mut out: RawPtr = core::ptr::null_mut();
        // SAFETY: `port` détient une référence sur un objet COM vivant dont la vtable
        // commence par `IUnknownVtbl` (contrat `ComInterface`/`ComVtable`) ; `iid` et `out`
        // sont des variables locales. `QueryInterface` écrit toujours `out` (nul en cas
        // d'échec) et rend une référence comptée en cas de succès.
        let status = unsafe { (port.unknown().query_interface)(port.as_raw(), &iid, &mut out) };
        if !nt_success(status) {
            return Err(status);
        }
        // SAFETY: `QueryInterface` a réussi : `out` est non nul et porte une référence que
        // l'appelant possède désormais, sur un objet dont la vtable a la forme
        // `IPortEventsVtbl` (c'est l'IID qu'on a demandé).
        let r = unsafe { ComRef::<IPortEvents>::try_from_raw_owned(out.cast()) };
        r.map(Self).ok_or(STATUS_NOT_FOUND)
    }

    /// `IPortEvents::AddEventToEventList` : accepte l'abonnement que PortCls vient de nous
    /// présenter par un `PCEVENT_VERB_ADD`.
    ///
    /// C'est **l'acquittement** de l'activation ; l'entrée entre dans la liste tenue par le
    /// port, qui la retirera lui-même le moment venu (il n'y a pas de méthode inverse).
    ///
    /// La méthode rend `void` : `Ok(())` signifie « le slot a été appelé », pas « le port a
    /// accepté ». La seule erreur possible est un slot vide, ce qu'aucun objet COM réel ne
    /// présente.
    ///
    /// IRQL : **`PASSIVE_LEVEL`**, exigé par la documentation de la méthode. C'est le cas
    /// dans un gestionnaire d'événement, que PortCls appelle depuis le fil du client.
    pub fn add_to_event_list(&self, entry: EventEntry) -> Result<(), NtStatus> {
        let slot = self
            .0
            .vtbl()
            .AddEventToEventList
            .ok_or(conduit_com::STATUS_NOT_IMPLEMENTED)?;
        // SAFETY: `self.0` détient une référence sur un objet vivant dont la vtable a la
        // forme `IPortEventsVtbl` (contrat `ComInterface`) ; le slot est non nul ; `entry`
        // est l'entrée que PortCls vient de nous remettre, non nulle par construction de
        // `EventEntry` et valide le temps du verbe `ADD`.
        unsafe { slot(self.0.as_raw(), entry.as_raw()) };
        Ok(())
    }

    /// `IPortEvents::GenerateEventList` : réveille les abonnés d'un événement.
    ///
    /// `pin` et `node` portent le **ciblage** : `Some(id)` met le drapeau correspondant à
    /// `TRUE` et passe l'identifiant, `None` le met à `FALSE`. C'est ici, et non dans la
    /// table d'automatisation, que se décide la broche visée (voir la documentation du
    /// module).
    ///
    /// # `Set` non `const` dans le header
    ///
    /// Le prototype déclare `_In_opt_ GUID* Set` — un pointeur **non `const`** sur un
    /// paramètre annoté en entrée. C'est une facilité du header, comme les
    /// `PPCFILTER_DESCRIPTOR` non `const` de `GetDescription`. Le `GUID` est donc recopié
    /// dans une variable locale mutable dont l'adresse est passée : rien de `'static` ni de
    /// partagé ne peut ainsi être écrit, quoi que le port fasse du pointeur pendant
    /// l'appel.
    ///
    /// # IRQL
    ///
    /// **Quelconque**, mais avec une réserve documentée : au-dessus de `DISPATCH_LEVEL`,
    /// l'implémentation met en file un DPC qui ne porte le contexte que d'**un seul**
    /// appel, et deux signalements rapprochés peuvent se perdre. À `<= DISPATCH_LEVEL`,
    /// aucune restriction. L'appelant prévu (`Cable::set_connected`, atteint depuis un
    /// gestionnaire de propriété) est à `PASSIVE_LEVEL`.
    ///
    /// Ne pas appeler cette méthode **depuis** un gestionnaire d'événement : la
    /// documentation ne dit rien du verrou qui protège la liste du port, ni de la
    /// ré-entrance entre `AddEventToEventList` et `GenerateEventList`.
    pub fn generate(
        &self,
        set: &GUID,
        id: u32,
        pin: Option<u32>,
        node: Option<u32>,
    ) -> Result<(), NtStatus> {
        let slot = self
            .0
            .vtbl()
            .GenerateEventList
            .ok_or(conduit_com::STATUS_NOT_IMPLEMENTED)?;
        // Copie locale : le slot veut un `GUID*` non `const` (voir la doc ci-dessus).
        let mut set_local: GUID = *set;
        let (pin_event, pin_id) = match pin {
            Some(id) => (KS_TRUE, id),
            None => (KS_FALSE, 0),
        };
        let (node_event, node_id) = match node {
            Some(id) => (KS_TRUE, id),
            None => (KS_FALSE, 0),
        };
        // SAFETY: `self.0` détient une référence sur un objet vivant dont la vtable a la
        // forme `IPortEventsVtbl` (contrat `ComInterface`) ; le slot est non nul ;
        // `set_local` est une variable locale vivante pendant tout l'appel, et les quatre
        // autres arguments sont des scalaires.
        unsafe {
            slot(
                self.0.as_raw(),
                &raw mut set_local,
                id,
                pin_event,
                pin_id,
                node_event,
                node_id,
            );
        }
        Ok(())
    }

    /// `KSEVENT_PINCAPS_JACKINFOCHANGE` sur la broche `pin` : « relis la description de
    /// prise ».
    ///
    /// L'appel que `Cable::set_connected` devra faire, une fois par filtre de topologie du
    /// câble, avec le numéro de la **broche endpoint** de ce filtre — la même que
    /// [`JackInfo::jack_pin`](crate::jack::JackInfo::jack_pin), celle qui porte la prise.
    /// Se tromper de broche ne casse rien de visible : l'événement part, aucun abonné ne le
    /// reconnaît, l'interface reste figée. C'est exactement le symptôme que M1b-04 cherche
    /// à supprimer, d'où l'insistance.
    ///
    /// IRQL : voir [`generate`](Self::generate).
    pub fn jack_info_change(&self, pin: u32) -> Result<(), NtStatus> {
        self.generate(&SET_PIN_CAPS_CHANGE, JACK_INFO_CHANGE_ID, Some(pin), None)
    }
}

/// `BOOL` de Windows (`int`) : `TRUE` vaut 1.
const KS_TRUE: BOOL = 1;
/// `BOOL` de Windows : `FALSE`.
const KS_FALSE: BOOL = 0;

// ---------------------------------------------------------------------------------
// Le gestionnaire de `KSEVENT_PINCAPS_JACKINFOCHANGE`.
// ---------------------------------------------------------------------------------

/// `KSEVENTSETID_PinCapsChange` en `static` : `PCEVENT_ITEM::Set` veut une adresse
/// `'static`, et les GUID de `portcls-sys` sont des `const`.
static SET_PIN_CAPS_CHANGE: GUID = KSEVENTSETID_PinCapsChange;

/// `KSEVENT_PINCAPS_JACKINFOCHANGE` : **1**, deuxième valeur de l'énumération
/// `KSEVENT_PINCAPS_CHANGENOTIFICATIONS` (`KSEVENT_PINCAPS_FORMATCHANGE` est 0).
///
/// L'énumération est dense et l'identifiant n'est nulle part écrit en clair dans la
/// documentation : le recopier ici, et l'asserter, attrape un jour où `ks.h` insérerait une
/// valeur avant celle-ci. Signaler `FORMATCHANGE` à la place serait indétectable.
pub const JACK_INFO_CHANGE_ID: u32 =
    KSEVENT_PINCAPS_CHANGENOTIFICATIONS::KSEVENT_PINCAPS_JACKINFOCHANGE as u32;

/// `Flags` du `PCEVENT_ITEM` du jack : `ENABLE | BASICSUPPORT` (513).
///
/// Ce sont les flags de SYSVAD sur le même événement
/// (`KSEVENT_TYPE_ENABLE | KSEVENT_TYPE_BASICSUPPORT`,
/// `EndpointsCommon/speakerhptoptable.h`). `ENABLE` autorise les verbes `ADD`/`REMOVE`,
/// `BASICSUPPORT` le verbe `SUPPORT` ; **pas** `ONESHOT`, l'abonnement devant survivre à la
/// première notification.
pub const JACK_EVENT_FLAGS: u32 = KSEVENT_TYPE_ENABLE | KSEVENT_TYPE_BASICSUPPORT;

const _: () = assert!(JACK_INFO_CHANGE_ID == 1);
const _: () = assert!(JACK_EVENT_FLAGS == 513);
// `ONESHOT` retirerait l'abonnement après la première notification : une prise qu'on
// débranche puis rebranche ne serait plus signalée qu'une fois.
const _: () = assert!(JACK_EVENT_FLAGS & portcls_sys::KSEVENT_TYPE_ONESHOT == 0);

// ---------------------------------------------------------------------------------
// Trace.
// ---------------------------------------------------------------------------------

/// Ce qu'un gestionnaire d'événement vient de faire d'une requête, passé à
/// [`EventSource::trace`].
///
/// Même rôle que [`crate::jack::JackTrace`] : rendre vérifiable en une minute, en machine,
/// ce qu'aucun test unitaire ne peut voir — que Windows s'abonne bien (un `SUPPORT` puis un
/// `ADD` sur le filtre de topologie, au moment où l'endpoint se construit). Sans cette
/// trace, un abonnement qui n'arrive jamais et un abonnement refusé se ressemblent.
/// Contrairement à [`JackTrace`](crate::jack::JackTrace), rien à emprunter : une
/// `PCEVENT_REQUEST` n'a aucun tampon, tout ce qu'elle porte tient dans des scalaires.
#[derive(Debug)]
pub struct EventTrace {
    /// Le verbe : `"SUPPORT"`, `"ADD"` ou `"REMOVE"`.
    pub verb: &'static str,
    /// L'identifiant de l'événement (`EventItem->Id`).
    pub id: u32,
    /// Le jeu d'événement était-il celui qu'on attend ?
    pub set_reconnu: bool,
    /// Ce que le gestionnaire a rendu.
    pub resultat: Result<(), NtStatus>,
    /// Le nœud visé, tel quel (`ULONG(-1)` pour le filtre).
    pub node: u32,
}

/// Nom de verbe des traces de support.
const VERBE_SUPPORT: &str = "SUPPORT";
/// Nom de verbe des traces d'abonnement.
const VERBE_ADD: &str = "ADD";
/// Nom de verbe des traces de désabonnement.
const VERBE_REMOVE: &str = "REMOVE";

// ---------------------------------------------------------------------------------
// Le trait métier du miniport.
// ---------------------------------------------------------------------------------

/// Un miniport capable de signaler des événements : il détient l'`IPortEvents` de son port.
///
/// `TopoRender` et `TopoCapture` l'implémenteront sur la référence obtenue dans `Init` par
/// [`PortEvents::from_port`]. **Ce module ne stocke rien** : c'est le miniport qui garde la
/// référence, comme il garde déjà l'état du câble.
///
/// # Pourquoi `Option`
///
/// `Init` peut ne pas avoir eu lieu, ou le `QueryInterface` avoir échoué. `None` fait
/// répondre `STATUS_INVALID_DEVICE_REQUEST` au verbe `ADD` — un refus franc plutôt qu'un
/// `Ok(())` qui laisserait le client abonné à un événement qui ne partirait jamais.
///
/// IRQL : `PASSIVE_LEVEL` pour un `ADD` (voir [`EventHandler::add`]). Appels concurrents
/// possibles depuis plusieurs fils.
pub trait EventSource: Send + Sync + 'static {
    /// L'`IPortEvents` du port de ce miniport, si elle a pu être obtenue.
    fn port_events(&self) -> Option<PortEvents>;

    /// Point de trace, appelé une fois par requête, après coup.
    ///
    /// Défaut : ne fait rien. Ne doit ni allouer ni bloquer.
    fn trace(&self, trace: &EventTrace) {
        let _ = trace;
    }
}

/// `KSEVENT_PINCAPS_JACKINFOCHANGE` sur la table d'automatisation d'un filtre de topologie.
///
/// Le squelette à trois verbes de SYSVAD (`CMiniportWaveRT::EventHandler_PinCapsChange`),
/// posé sur la brique de ce module : revalider `Set` et `Id`, accepter le `SUPPORT`, rendre
/// l'entrée au port sur `ADD`, ne rien faire sur `REMOVE`.
#[derive(Debug)]
pub struct JackInfoChange;

impl JackInfoChange {
    /// La requête porte-t-elle bien **notre** événement ?
    ///
    /// PortCls a déjà trouvé l'entrée par son `Set`/`Id`, mais la documentation ne garantit
    /// pas qu'un gestionnaire ne soit appelé que pour son propre item, et SYSVAD refait le
    /// contrôle. Deux entrées du même jeu (`FORMATCHANGE` et `JACKINFOCHANGE` ne diffèrent
    /// que par l'`Id`) rendent ce contrôle utile plutôt que rituel.
    fn reconnait<T>(req: &EventRequest<'_, T>) -> bool {
        guid_eq(req.set, &SET_PIN_CAPS_CHANGE) && req.id == JACK_INFO_CHANGE_ID
    }
}

impl<T: EventSource> EventHandler<T> for JackInfoChange {
    fn support(req: &EventRequest<'_, T>) -> Result<(), NtStatus> {
        let reconnu = Self::reconnait(req);
        let resultat = if reconnu {
            Ok(())
        } else {
            Err(STATUS_INVALID_PARAMETER)
        };
        trace(req, VERBE_SUPPORT, reconnu, resultat);
        resultat
    }

    fn add(req: &EventRequest<'_, T>, entry: EventEntry) -> Result<(), NtStatus> {
        let reconnu = Self::reconnait(req);
        let resultat = if !reconnu {
            Err(STATUS_INVALID_PARAMETER)
        } else {
            match req.target.port_events() {
                // C'est cet appel qui acquitte l'abonnement, pas le `Ok(())` qui suit.
                Some(port) => port.add_to_event_list(entry),
                None => Err(STATUS_INVALID_DEVICE_REQUEST),
            }
        };
        trace(req, VERBE_ADD, reconnu, resultat);
        resultat
    }

    fn remove(req: &EventRequest<'_, T>) -> Result<(), NtStatus> {
        // Rien à faire : le port a déjà retiré l'entrée de sa liste, il nous prévient. Nous
        // continuons de produire l'événement — sans abonné, il ne réveille personne.
        let reconnu = Self::reconnait(req);
        let resultat = if reconnu {
            Ok(())
        } else {
            Err(STATUS_INVALID_PARAMETER)
        };
        trace(req, VERBE_REMOVE, reconnu, resultat);
        resultat
    }
}

/// Appelle [`EventSource::trace`] avec les champs de la requête.
fn trace<T: EventSource>(
    req: &EventRequest<'_, T>,
    verb: &'static str,
    set_reconnu: bool,
    resultat: Result<(), NtStatus>,
) {
    req.target.trace(&EventTrace {
        verb,
        id: req.id,
        set_reconnu,
        resultat,
        node: req.node,
    });
}

/// Entrée de `PCAUTOMATION_TABLE` du **filtre** de topologie :
/// `KSEVENTSETID_PinCapsChange`, `KSEVENT_PINCAPS_JACKINFOCHANGE`, `ENABLE | BASICSUPPORT`.
///
/// À poser dans `PCFILTER_DESCRIPTOR::AutomationTable` par [`with_events`], **pas** sur une
/// broche : voir la documentation du module (l'item va sur le filtre, la broche est
/// désignée au moment du signalement). `const fn` : les tables d'automatisation du pilote
/// sont des `static`. `V` est la vtable du miniport qui porte la table
/// (`IMiniportTopologyVtbl`), `T` son type — c'est ce couple que la garde de vtable de
/// [`handler`] vérifie.
pub const fn jack_info_change_item<V, T>() -> PCEVENT_ITEM
where
    V: TargetVtbl<T>,
    T: EventSource,
{
    item::<V, T, JackInfoChange>(&SET_PIN_CAPS_CHANGE, JACK_INFO_CHANGE_ID, JACK_EVENT_FLAGS)
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

    /// L'identifiant et les flags sont ceux de `ks.h` et de SYSVAD, pas des constantes
    /// choisies ici.
    #[test]
    fn identifiant_et_flags_de_l_evenement() {
        assert_eq!(JACK_INFO_CHANGE_ID, 1);
        assert_eq!(
            JACK_INFO_CHANGE_ID,
            KSEVENT_PINCAPS_CHANGENOTIFICATIONS::KSEVENT_PINCAPS_JACKINFOCHANGE as u32
        );
        assert_ne!(
            JACK_INFO_CHANGE_ID,
            KSEVENT_PINCAPS_CHANGENOTIFICATIONS::KSEVENT_PINCAPS_FORMATCHANGE as u32,
            "signaler FORMATCHANGE au lieu de JACKINFOCHANGE serait indétectable"
        );
        assert_eq!(JACK_EVENT_FLAGS, 513);
        assert_eq!(
            JACK_EVENT_FLAGS,
            KSEVENT_TYPE_ENABLE | KSEVENT_TYPE_BASICSUPPORT
        );
        assert_eq!(JACK_EVENT_FLAGS & portcls_sys::KSEVENT_TYPE_ONESHOT, 0);
    }

    /// Le jeu d'événement est bien celui de `ks.h`, et il se distingue de celui du jack.
    #[test]
    fn le_jeu_d_evenement_est_pincapschange() {
        assert!(guid_eq(&SET_PIN_CAPS_CHANGE, &KSEVENTSETID_PinCapsChange));
        assert!(!guid_eq(
            &SET_PIN_CAPS_CHANGE,
            &portcls_sys::KSPROPSETID_Jack
        ));
        // Valeur mesurée par cl.exe (layout.golden) : DD4F192E-3B78-49AD-A534-2C315B822000.
        assert_eq!(SET_PIN_CAPS_CHANGE.Data1, 0xDD4F_192E);
        assert_eq!(SET_PIN_CAPS_CHANGE.Data2, 0x3B78);
        assert_eq!(SET_PIN_CAPS_CHANGE.Data3, 0x49AD);
        assert_eq!(
            SET_PIN_CAPS_CHANGE.Data4,
            [0xA5, 0x34, 0x2C, 0x31, 0x5B, 0x82, 0x20, 0x00]
        );
    }

    /// Les verbes de PortCls sont des **bits** : `SUPPORT` vaut 4, pas 3. Un aiguillage
    /// écrit comme un `match` sur 0..=3 manquerait le verbe le plus fréquent.
    #[test]
    fn les_verbes_sont_des_bits() {
        assert_eq!(portcls_sys::PCEVENT_VERB_NONE, 0);
        assert_eq!(PCEVENT_VERB_ADD, 1);
        assert_eq!(PCEVENT_VERB_REMOVE, 2);
        assert_eq!(PCEVENT_VERB_SUPPORT, 4);
        // Les trois bits sont distincts deux à deux : c'est ce qui rend l'aiguillage par
        // masque du thunk correct.
        assert_eq!(PCEVENT_VERB_ADD & PCEVENT_VERB_REMOVE, 0);
        assert_eq!(PCEVENT_VERB_ADD & PCEVENT_VERB_SUPPORT, 0);
        assert_eq!(PCEVENT_VERB_REMOVE & PCEVENT_VERB_SUPPORT, 0);
    }

    /// `PCEVENT_ITEM` a la taille et la forme mesurées par `cl.exe` (`layout.golden`), et
    /// `EventItemSize` doit être un multiple de 8 (exigence de la documentation de
    /// `PCAUTOMATION_TABLE`).
    #[test]
    fn la_taille_d_item_est_celle_du_golden() {
        assert_eq!(size_of::<PCEVENT_ITEM>(), 24);
        assert_eq!(size_of::<PCEVENT_ITEM>() % 8, 0);
        // Même taille que `PCPROPERTY_ITEM` : les deux constructeurs sont jumeaux.
        assert_eq!(
            size_of::<PCEVENT_ITEM>(),
            size_of::<portcls_sys::PCPROPERTY_ITEM>()
        );
    }
}
