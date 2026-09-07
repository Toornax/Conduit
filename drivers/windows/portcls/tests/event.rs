//! Le thunk d'événement KS vu par un faux PortCls : une `PCEVENT_REQUEST` bâtie à la main
//! est passée au `Handler` d'un `PCEVENT_ITEM` produit par `event::item`, exactement comme
//! PortCls le ferait après avoir trouvé l'entrée dans la `PCAUTOMATION_TABLE` de la cible.
//!
//! Rien ici ne touche au noyau : le thunk ne fait que du calcul et de la mémoire
//! (`MajorTarget` est le `this` d'un `new_topology_object`, `Irp` est nul, l'entrée
//! d'événement est un pointeur factice jamais déréférencé).
//!
//! # Ce que ces tests peuvent, et ce qu'ils ne peuvent pas
//!
//! `IPortEvents::AddEventToEventList` et `GenerateEventList` rendent **`void`**. Un faux
//! port permet donc de vérifier *que le bon slot est appelé avec les bons arguments* — et
//! c'est tout ce que ce fichier vérifie. Qu'une notification parte réellement, qu'un client
//! Windows la reçoive et relise le jack : rien ici ne peut en dire quoi que ce soit, et
//! c'est écrit en tête de `portcls::event`. La vérification se fait en machine, dans les
//! réglages Son.

#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use common::This;
use conduit_com::{
    ComObject, ComPtr, ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
    STATUS_UNSUCCESSFUL,
};
use portcls::portcls_sys::{
    BOOL, GUID, IMiniportTopologyVtbl, IPortEvents, IPortEventsVtbl, IUnknown,
    KSEVENT_PINCAPS_CHANGENOTIFICATIONS, KSEVENT_TYPE_BASICSUPPORT, KSEVENT_TYPE_ENABLE,
    KSEVENTSETID_PinCapsChange, KSPROPSETID_Jack, NTSTATUS, PCAUTOMATION_TABLE, PCEVENT_ITEM,
    PCEVENT_REQUEST, PCEVENT_VERB_ADD, PCEVENT_VERB_NONE, PCEVENT_VERB_REMOVE,
    PCEVENT_VERB_SUPPORT, PCFILTER_DESCRIPTOR, PCPROPERTY_ITEM, PKSEVENT_ENTRY, ULONG,
};
use portcls::{
    EventEntry, EventHandler, EventRequest, EventSource, EventTrace, JACK_EVENT_FLAGS,
    JACK_INFO_CHANGE_ID, JackInfoChange, MiniportTopology, PortEvents, PortTopology, ResourceList,
    STATUS_INVALID_DEVICE_REQUEST, STATUS_NOT_SUPPORTED, event, jack_info_change_item,
    new_topology_object, unknown, with_events,
};

// ---------------------------------------------------------------------------------
// Faux `IPortEvents` : il ne fait rien de ses arguments, il les **note**. C'est la seule
// observation possible, les deux méthodes rendant `void`.
// ---------------------------------------------------------------------------------

/// Valeur du pointeur factice d'entrée d'événement (jamais déréférencé).
const FAUSSE_ENTREE: usize = 0x4B53_4556; // « KSEV »

/// Un appel à `GenerateEventList`, tel que le faux port l'a vu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Generation {
    set: Option<GUID2>,
    id: ULONG,
    pin_event: BOOL,
    pin_id: ULONG,
    node_event: BOOL,
    node_id: ULONG,
}

/// `GUID` comparable (le `GUID` généré n'implémente ni `PartialEq` ni `Debug` utilement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GUID2(u32, u16, u16, [u8; 8]);

impl GUID2 {
    fn de(g: &GUID) -> Self {
        Self(g.Data1, g.Data2, g.Data3, g.Data4)
    }
}

struct FauxPortEvents {
    /// Entrées reçues par `AddEventToEventList`, dans l'ordre.
    ajouts: Mutex<Vec<usize>>,
    /// Appels reçus par `GenerateEventList`, dans l'ordre.
    generations: Mutex<Vec<Generation>>,
}

impl FauxPortEvents {
    fn nouveau() -> Self {
        Self {
            ajouts: Mutex::new(Vec::new()),
            generations: Mutex::new(Vec::new()),
        }
    }
}

unsafe extern "C" fn faux_add_event(this: This, entry: PKSEVENT_ENTRY) {
    let me = unsafe { ComObject::<IPortEventsVtbl, FauxPortEvents>::inner(this) };
    me.ajouts.lock().unwrap().push(entry as usize);
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn faux_generate_event(
    this: This,
    set: *mut GUID,
    id: ULONG,
    pin_event: BOOL,
    pin_id: ULONG,
    node_event: BOOL,
    node_id: ULONG,
) {
    let me = unsafe { ComObject::<IPortEventsVtbl, FauxPortEvents>::inner(this) };
    let set = if set.is_null() {
        None
    } else {
        Some(GUID2::de(unsafe { &*set }))
    };
    me.generations.lock().unwrap().push(Generation {
        set,
        id,
        pin_event,
        pin_id,
        node_event,
        node_id,
    });
}

static FAUX_PORT_EVENTS_VTBL: IPortEventsVtbl = IPortEventsVtbl {
    QueryInterface: Some(unknown::query_interface::<IPortEventsVtbl, FauxPortEvents>),
    AddRef: Some(unknown::add_ref::<IPortEventsVtbl, FauxPortEvents>),
    Release: Some(unknown::release::<IPortEventsVtbl, FauxPortEvents>),
    AddEventToEventList: Some(faux_add_event),
    GenerateEventList: Some(faux_generate_event),
};

fn faux_port_events() -> ComPtr<IPortEventsVtbl, FauxPortEvents> {
    ComObject::new(&FAUX_PORT_EVENTS_VTBL, FauxPortEvents::nouveau())
}

// ---------------------------------------------------------------------------------
// Deux miniports topologie de test : `Cible`, celui que le gestionnaire attend, et
// `Intrus`, un autre type — donc une autre adresse de vtable (garde de vtable).
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour la
/// `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

/// Une trace journalisée, résumée : verbe, jeu reconnu, résultat.
type Trace = (&'static str, bool, Result<(), NtStatus>);

/// Le miniport visé : il porte (ou non) un faux `IPortEvents`, et journalise les traces.
struct Cible {
    port: Option<PortEvents>,
    /// Traces reçues, dans l'ordre.
    traces: Mutex<Vec<Trace>>,
}

impl MiniportTopology for Cible {
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        _port: PortTopology,
    ) -> NtStatus {
        STATUS_SUCCESS
    }

    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        &DESCRIPTION.0
    }
}

impl EventSource for Cible {
    fn port_events(&self) -> Option<PortEvents> {
        self.port.clone()
    }

    fn trace(&self, trace: &EventTrace) {
        self.traces
            .lock()
            .unwrap()
            .push((trace.verb, trace.set_reconnu, trace.resultat));
    }
}

/// Un autre miniport topologie : même vtable au sens du type, autre `T`, donc autre
/// adresse.
struct Intrus;

impl MiniportTopology for Intrus {
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        _port: PortTopology,
    ) -> NtStatus {
        STATUS_SUCCESS
    }

    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        &DESCRIPTION.0
    }
}

impl EventSource for Intrus {
    fn port_events(&self) -> Option<PortEvents> {
        None
    }
}

// ---------------------------------------------------------------------------------
// Un gestionnaire qui n'implémente rien : les trois défauts du trait.
// ---------------------------------------------------------------------------------

struct Muet;
impl EventHandler<Cible> for Muet {}

/// Un gestionnaire qui compte ses appels, pour vérifier l'aiguillage du verbe seul.
struct Compteur;

static APPELS_SUPPORT: AtomicUsize = AtomicUsize::new(0);
static APPELS_ADD: AtomicUsize = AtomicUsize::new(0);
static APPELS_REMOVE: AtomicUsize = AtomicUsize::new(0);

impl EventHandler<Cible> for Compteur {
    fn support(_req: &EventRequest<'_, Cible>) -> Result<(), NtStatus> {
        APPELS_SUPPORT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn add(_req: &EventRequest<'_, Cible>, entry: EventEntry) -> Result<(), NtStatus> {
        assert_eq!(
            entry.as_raw() as usize,
            FAUSSE_ENTREE,
            "l'entrée arrive telle que PortCls l'a posée"
        );
        APPELS_ADD.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn remove(_req: &EventRequest<'_, Cible>) -> Result<(), NtStatus> {
        APPELS_REMOVE.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------
// Entrées de table de test.
// ---------------------------------------------------------------------------------

/// `PCEVENT_ITEM` contient un `*const GUID` et un pointeur de fonction : `Sync` déclaré à
/// la main pour la `static`, comme le pilote le fera pour ses vraies tables.
struct SyncItem(PCEVENT_ITEM);
unsafe impl Sync for SyncItem {}

/// L'entrée réelle du pilote : `KSEVENTSETID_PinCapsChange` /
/// `KSEVENT_PINCAPS_JACKINFOCHANGE`, gestionnaire `JackInfoChange`.
static ITEM_JACK: SyncItem = SyncItem(jack_info_change_item::<IMiniportTopologyVtbl, Cible>());

/// La même entrée sur `Intrus` : c'est elle que le test de garde de vtable appellera avec
/// un `MajorTarget` de `Cible`.
static ITEM_JACK_INTRUS: SyncItem =
    SyncItem(jack_info_change_item::<IMiniportTopologyVtbl, Intrus>());

/// Un jeu d'événement qui n'est pas le nôtre : `KSPROPSETID_Jack` fait très bien l'affaire
/// (c'est un jeu de *propriétés*, jamais un jeu d'événements).
static SET_ETRANGER: GUID = KSPROPSETID_Jack;

/// L'entrée `JackInfoChange` posée, par erreur, sur un jeu étranger : c'est ce que le
/// gestionnaire doit refuser en revalidant le `Set`.
static ITEM_SET_ETRANGER: SyncItem =
    SyncItem(event::item::<IMiniportTopologyVtbl, Cible, JackInfoChange>(
        &SET_ETRANGER,
        JACK_INFO_CHANGE_ID,
        JACK_EVENT_FLAGS,
    ));

/// Le bon jeu mais le mauvais identifiant : `KSEVENT_PINCAPS_FORMATCHANGE` (0).
static ITEM_ID_ETRANGER: SyncItem =
    SyncItem(event::item::<IMiniportTopologyVtbl, Cible, JackInfoChange>(
        &SET_PIN_CAPS,
        KSEVENT_PINCAPS_CHANGENOTIFICATIONS::KSEVENT_PINCAPS_FORMATCHANGE as u32,
        JACK_EVENT_FLAGS,
    ));

static SET_PIN_CAPS: GUID = KSEVENTSETID_PinCapsChange;

static ITEM_MUET: SyncItem = SyncItem(event::item::<IMiniportTopologyVtbl, Cible, Muet>(
    &SET_PIN_CAPS,
    JACK_INFO_CHANGE_ID,
    JACK_EVENT_FLAGS,
));

static ITEM_COMPTEUR: SyncItem = SyncItem(event::item::<IMiniportTopologyVtbl, Cible, Compteur>(
    &SET_PIN_CAPS,
    JACK_INFO_CHANGE_ID,
    JACK_EVENT_FLAGS,
));

// ---------------------------------------------------------------------------------
// Outillage.
// ---------------------------------------------------------------------------------

/// Nœud visé par les requêtes de test : `ULONG(-1)`, comme pour un événement de filtre.
const NODE: ULONG = ULONG::MAX;

/// Un miniport `Cible` branché sur un faux port (gardé vivant par l'appelant).
fn cible_avec_port(port: &ComPtr<IPortEventsVtbl, FauxPortEvents>) -> This {
    // SAFETY: `port` est vivant et détenu par l'appelant ; `from_raw_add_ref` prend la
    // référence que la `PortEvents` rendra au `Drop`.
    let r = unsafe { ComRef::<IPortEvents>::from_raw_add_ref(port.as_raw().cast()) };
    new_topology_object(Cible {
        port: Some(PortEvents::from_ref(r)),
        traces: Mutex::new(Vec::new()),
    })
    .into_raw()
}

/// Un miniport `Cible` **sans** port (`Init` n'a pas eu lieu, ou le `QueryInterface` a
/// échoué).
fn cible_sans_port() -> This {
    new_topology_object(Cible {
        port: None,
        traces: Mutex::new(Vec::new()),
    })
    .into_raw()
}

fn intrus() -> This {
    new_topology_object(Intrus).into_raw()
}

/// Les traces journalisées par la `Cible` désignée par `this`.
fn traces(this: This) -> Vec<Trace> {
    let me = unsafe { ComObject::<IMiniportTopologyVtbl, Cible>::inner(this) };
    me.traces.lock().unwrap().clone()
}

/// Requête minimale : cible, verbe, entrée d'événement factice.
fn requete(item: &'static SyncItem, this: This, verb: ULONG) -> PCEVENT_REQUEST {
    PCEVENT_REQUEST {
        MajorTarget: this.cast(),
        MinorTarget: ptr::null_mut(),
        Node: NODE,
        EventItem: &item.0,
        EventEntry: FAUSSE_ENTREE as PKSEVENT_ENTRY,
        Verb: verb,
        Irp: ptr::null_mut(),
    }
}

/// L'appel que PortCls fait : `item.Handler.unwrap()(&mut req)`.
fn appeler(item: &'static SyncItem, req: &mut PCEVENT_REQUEST) -> NTSTATUS {
    let slot = item.0.Handler.expect("Handler renseigné par event::item");
    unsafe { slot(req) }
}

// ---------------------------------------------------------------------------------
// 1. Cohérence de l'entrée de table.
// ---------------------------------------------------------------------------------

/// L'entrée que le pilote posera porte le jeu, l'identifiant et les flags attendus, et un
/// `Handler` non nul.
///
/// C'est la seule vérification possible du câblage : si le jeu ou l'identifiant sont faux,
/// Windows ne s'abonnera jamais et **rien ne le dira** — ni ici, ni en machine, sinon par
/// une interface figée.
#[test]
fn l_entree_de_table_porte_le_bon_jeu_et_le_bon_identifiant() {
    let item = &ITEM_JACK.0;
    assert!(!item.Set.is_null());
    let set = unsafe { &*item.Set };
    // `KSEVENTSETID_PinCapsChange`, valeur mesurée par cl.exe (layout.golden).
    assert_eq!(GUID2::de(set), GUID2::de(&KSEVENTSETID_PinCapsChange));
    assert_eq!(
        GUID2::de(set),
        GUID2(
            0xDD4F_192E,
            0x3B78,
            0x49AD,
            [0xA5, 0x34, 0x2C, 0x31, 0x5B, 0x82, 0x20, 0x00]
        )
    );
    // `KSEVENT_PINCAPS_JACKINFOCHANGE` vaut 1 ; 0 serait `FORMATCHANGE`.
    assert_eq!(item.Id, JACK_INFO_CHANGE_ID);
    assert_eq!(item.Id, 1);
    // `ENABLE | BASICSUPPORT`, sans `ONESHOT`.
    assert_eq!(item.Flags, KSEVENT_TYPE_ENABLE | KSEVENT_TYPE_BASICSUPPORT);
    assert_eq!(item.Flags, 513);
    assert!(item.Handler.is_some());
}

/// `with_events` remplit le troisième triplet de la table **sans toucher** aux deux
/// autres : c'est ce qui permet à `conduit-kmd` d'ajouter ses événements à une table de
/// propriétés existante par une seule ligne.
#[test]
fn with_events_remplit_le_triplet_evenement_et_laisse_le_reste() {
    static PROPRIETES: SyncProps = SyncProps([unsafe { core::mem::zeroed() }]);
    static EVENEMENTS: SyncEvents = SyncEvents([
        jack_info_change_item::<IMiniportTopologyVtbl, Cible>(),
        jack_info_change_item::<IMiniportTopologyVtbl, Cible>(),
    ]);

    struct SyncProps([PCPROPERTY_ITEM; 1]);
    unsafe impl Sync for SyncProps {}
    struct SyncEvents([PCEVENT_ITEM; 2]);
    unsafe impl Sync for SyncEvents {}

    // Une table « à une propriété, sans événement », comme celle de `conduit-kmd`.
    let avant = PCAUTOMATION_TABLE {
        PropertyItemSize: size_of::<PCPROPERTY_ITEM>() as ULONG,
        PropertyCount: 1,
        Properties: PROPRIETES.0.as_ptr(),
        MethodItemSize: 0,
        MethodCount: 0,
        Methods: ptr::null(),
        EventItemSize: size_of::<PCEVENT_ITEM>() as ULONG,
        EventCount: 0,
        Events: ptr::null(),
        Reserved: 0,
    };
    let apres = with_events(avant, &EVENEMENTS.0);

    assert_eq!(apres.EventCount, 2, "le compte vient du tableau");
    assert_eq!(apres.EventItemSize, 24, "sizeof(PCEVENT_ITEM), golden");
    assert_eq!(
        apres.EventItemSize % 8,
        0,
        "multiple de 8, exigé par PortCls"
    );
    assert!(ptr::eq(apres.Events, EVENEMENTS.0.as_ptr()));

    // Le triplet des propriétés et celui des méthodes sont intacts.
    assert_eq!(apres.PropertyItemSize, avant.PropertyItemSize);
    assert_eq!(apres.PropertyCount, 1);
    assert!(ptr::eq(apres.Properties, avant.Properties));
    assert_eq!(apres.MethodCount, 0);
    assert!(apres.Methods.is_null());
    assert_eq!(apres.Reserved, 0);
}

/// La table complète bâtie en `static`, **exactement** comme `conduit-kmd` la bâtira :
/// `one_property_automation(&…)` enveloppé dans `with_events(…, ÉVÉNEMENTS.get())`.
///
/// Ce test existe pour une raison précise : il vérifie que `with_events` est appelable dans
/// un initialiseur de `static`, à travers l'enveloppe `Sync` du pilote (`Shared`, ici
/// `Enveloppe`) et son accesseur `const`. Une brique qui ne passerait ce cap qu'en `fn`
/// serait inutilisable là où PortCls l'exige — les tables sont des `static`, PortCls en
/// conserve les pointeurs pour toute la vie du filtre.
#[test]
fn la_table_complete_se_batit_en_static() {
    /// Copie de `conduit_kmd::descriptors::Shared` : `repr(transparent)`, `Sync` déclaré à
    /// la main, accesseur `const`.
    #[repr(transparent)]
    struct Enveloppe<T>(T);
    // SAFETY: table immuable, écrite à la compilation, jamais modifiée.
    unsafe impl<T> Sync for Enveloppe<T> {}
    impl<T> Enveloppe<T> {
        const fn get(&self) -> &T {
            &self.0
        }
    }

    /// La table de propriétés du filtre de topologie (une entrée, ici factice).
    static PROPRIETES: Enveloppe<[PCPROPERTY_ITEM; 1]> =
        Enveloppe([unsafe { core::mem::zeroed() }]);
    /// La table d'événements à ajouter : `KSEVENT_PINCAPS_JACKINFOCHANGE`, une entrée.
    static EVENEMENTS: Enveloppe<[PCEVENT_ITEM; 1]> =
        Enveloppe([jack_info_change_item::<IMiniportTopologyVtbl, Cible>()]);

    /// L'équivalent de `one_property_automation` de `conduit-kmd`.
    const fn une_propriete(p: &'static Enveloppe<[PCPROPERTY_ITEM; 1]>) -> PCAUTOMATION_TABLE {
        PCAUTOMATION_TABLE {
            PropertyItemSize: size_of::<PCPROPERTY_ITEM>() as ULONG,
            PropertyCount: 1,
            Properties: ptr::from_ref(p).cast::<PCPROPERTY_ITEM>(),
            MethodItemSize: 0,
            MethodCount: 0,
            Methods: ptr::null(),
            EventItemSize: size_of::<PCEVENT_ITEM>() as ULONG,
            EventCount: 0,
            Events: ptr::null(),
            Reserved: 0,
        }
    }

    // ↓ la ligne exacte que `descriptors.rs` aura à écrire.
    static TABLE: Enveloppe<PCAUTOMATION_TABLE> =
        Enveloppe(with_events(une_propriete(&PROPRIETES), EVENEMENTS.get()));

    let t = TABLE.get();
    assert_eq!(t.PropertyCount, 1);
    assert_eq!(t.EventCount, 1);
    assert!(ptr::eq(t.Events, EVENEMENTS.get().as_ptr()));
    // L'entrée pointée est bien celle du jack : c'est le bout de la chaîne.
    let item = unsafe { &*t.Events };
    assert_eq!(item.Id, JACK_INFO_CHANGE_ID);
    assert_eq!(
        GUID2::de(unsafe { &*item.Set }),
        GUID2::de(&KSEVENTSETID_PinCapsChange)
    );
}

// ---------------------------------------------------------------------------------
// 2. La table des statuts du thunk.
// ---------------------------------------------------------------------------------

#[test]
fn requete_nulle_est_un_parametre_invalide() {
    let slot = ITEM_JACK.0.Handler.expect("Handler");
    let status = unsafe { slot(ptr::null_mut()) };
    assert_eq!(status, STATUS_INVALID_PARAMETER);
}

#[test]
fn major_target_nul_est_une_requete_invalide() {
    let mut req = requete(&ITEM_JACK, ptr::null_mut(), PCEVENT_VERB_SUPPORT);
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_INVALID_DEVICE_REQUEST);
}

/// Confusion de type : une entrée monomorphisée sur `Intrus` appelée avec le `this` d'une
/// `Cible`. C'est exactement l'erreur de câblage qu'une `PCAUTOMATION_TABLE` posée sur le
/// mauvais filtre produirait — et sans la garde, `inner::<_, Intrus>` sur un objet `Cible`
/// serait un comportement indéfini.
#[test]
fn confusion_de_type_arretee_par_la_garde_de_vtable() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    for verbe in [PCEVENT_VERB_SUPPORT, PCEVENT_VERB_ADD, PCEVENT_VERB_REMOVE] {
        let mut req = requete(&ITEM_JACK_INTRUS, this, verbe);
        assert_eq!(
            appeler(&ITEM_JACK_INTRUS, &mut req),
            STATUS_INVALID_DEVICE_REQUEST,
            "verbe {verbe}"
        );
    }
    // Rien n'a atteint le port ni la trace.
    assert!(port.object().get().ajouts.lock().unwrap().is_empty());
    assert!(traces(this).is_empty());
    assert_eq!(common::release(this), 0);

    // Et dans l'autre sens : l'entrée de `Cible` appelée avec le `this` d'un `Intrus`.
    // C'est le `MajorTarget` étranger proprement dit.
    let etranger = intrus();
    let mut req = requete(&ITEM_JACK, etranger, PCEVENT_VERB_ADD);
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_INVALID_DEVICE_REQUEST);
    assert_eq!(common::release(etranger), 0);
}

/// L'invariant dont dépend la garde : les trois chemins qui nomment la vtable de `Cible`
/// donnent la même adresse. Répété ici parce que `tests/property.rs` ne couvre que son
/// propre thunk, et que **ce fichier doit aussi être rejoué en `--release`** (voir
/// `tools/check.ps1` : sans `#[inline(never)]` sur `vtbl_of`, la garde refuse des objets
/// légitimes dès que l'inlining est actif).
#[test]
fn la_vtable_attendue_a_une_seule_adresse() {
    let this = cible_sans_port();
    let dans_objet: *const IMiniportTopologyVtbl =
        unsafe { common::vtbl_de::<IMiniportTopologyVtbl>(this) };
    let par_trait: *const IMiniportTopologyVtbl =
        <IMiniportTopologyVtbl as portcls::TargetVtbl<Cible>>::vtbl();
    assert!(ptr::eq(dans_objet, par_trait));
    assert_eq!(common::release(this), 0);
}

#[test]
fn verbe_inconnu_est_une_requete_invalide() {
    let this = cible_sans_port();
    // `NONE` (0) et deux bits qui n'appartiennent à aucun verbe.
    for verbe in [PCEVENT_VERB_NONE, 8, 0x1000, ULONG::MAX & !7] {
        let mut req = requete(&ITEM_JACK, this, verbe);
        assert_eq!(
            appeler(&ITEM_JACK, &mut req),
            STATUS_INVALID_DEVICE_REQUEST,
            "verbe {verbe:#x}"
        );
    }
    assert!(traces(this).is_empty(), "un verbe inconnu n'atteint pas H");
    assert_eq!(common::release(this), 0);
}

#[test]
fn verbe_non_implemente_est_non_supporte() {
    let this = cible_sans_port();
    for verbe in [PCEVENT_VERB_SUPPORT, PCEVENT_VERB_ADD, PCEVENT_VERB_REMOVE] {
        let mut req = requete(&ITEM_MUET, this, verbe);
        assert_eq!(
            appeler(&ITEM_MUET, &mut req),
            STATUS_NOT_SUPPORTED,
            "verbe {verbe}"
        );
    }
    assert_eq!(common::release(this), 0);
}

/// `EventItem` nul, puis `EventItem->Set` nul : une requête qui ne peut pas être routée.
#[test]
fn event_item_nul_est_une_requete_invalide() {
    let this = cible_sans_port();

    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_SUPPORT);
    req.EventItem = ptr::null();
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_INVALID_DEVICE_REQUEST);

    static ITEM_SANS_SET: SyncItem = SyncItem(PCEVENT_ITEM {
        Set: ptr::null(),
        Id: JACK_INFO_CHANGE_ID,
        Flags: JACK_EVENT_FLAGS,
        Handler: None,
    });
    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_SUPPORT);
    req.EventItem = &ITEM_SANS_SET.0;
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_INVALID_DEVICE_REQUEST);

    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// 3. L'aiguillage des trois verbes.
// ---------------------------------------------------------------------------------

/// Chaque verbe atteint sa méthode, et elle seule.
#[test]
fn les_trois_verbes_atteignent_leur_methode() {
    let this = cible_sans_port();
    APPELS_SUPPORT.store(0, Ordering::SeqCst);
    APPELS_ADD.store(0, Ordering::SeqCst);
    APPELS_REMOVE.store(0, Ordering::SeqCst);

    for verbe in [PCEVENT_VERB_SUPPORT, PCEVENT_VERB_ADD, PCEVENT_VERB_REMOVE] {
        let mut req = requete(&ITEM_COMPTEUR, this, verbe);
        assert_eq!(appeler(&ITEM_COMPTEUR, &mut req), STATUS_SUCCESS);
    }
    assert_eq!(APPELS_SUPPORT.load(Ordering::SeqCst), 1);
    assert_eq!(APPELS_ADD.load(Ordering::SeqCst), 1);
    assert_eq!(APPELS_REMOVE.load(Ordering::SeqCst), 1);
    assert_eq!(common::release(this), 0);
}

/// `ADD` est testé **en dernier** par le thunk : sur un masque composite, le geste le
/// moins conséquent l'emporte, plutôt que l'entrée d'un abonné dans la liste du port.
#[test]
fn sur_un_masque_composite_add_ne_l_emporte_pas() {
    let this = cible_sans_port();
    APPELS_SUPPORT.store(0, Ordering::SeqCst);
    APPELS_ADD.store(0, Ordering::SeqCst);
    APPELS_REMOVE.store(0, Ordering::SeqCst);

    // ADD | REMOVE : c'est REMOVE qui gagne.
    let mut req = requete(&ITEM_COMPTEUR, this, PCEVENT_VERB_ADD | PCEVENT_VERB_REMOVE);
    assert_eq!(appeler(&ITEM_COMPTEUR, &mut req), STATUS_SUCCESS);
    assert_eq!(APPELS_REMOVE.load(Ordering::SeqCst), 1);
    assert_eq!(APPELS_ADD.load(Ordering::SeqCst), 0);

    // ADD | SUPPORT : c'est SUPPORT qui gagne, comme BASICSUPPORT côté propriétés.
    let mut req = requete(
        &ITEM_COMPTEUR,
        this,
        PCEVENT_VERB_ADD | PCEVENT_VERB_SUPPORT,
    );
    assert_eq!(appeler(&ITEM_COMPTEUR, &mut req), STATUS_SUCCESS);
    assert_eq!(APPELS_SUPPORT.load(Ordering::SeqCst), 1);
    assert_eq!(APPELS_ADD.load(Ordering::SeqCst), 0);

    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// 4. `JackInfoChange` : les trois verbes de bout en bout.
// ---------------------------------------------------------------------------------

/// Le verbe `SUPPORT` réussit sans rien toucher au port.
#[test]
fn support_reussit_sans_toucher_au_port() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_SUPPORT);
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_SUCCESS);
    assert!(port.object().get().ajouts.lock().unwrap().is_empty());
    assert!(port.object().get().generations.lock().unwrap().is_empty());
    assert_eq!(traces(this), vec![("SUPPORT", true, Ok(()))]);
    assert_eq!(common::release(this), 0);
}

/// Le verbe `ADD` remet l'entrée au port, **telle quelle**. C'est cet appel qui acquitte
/// l'abonnement ; un `STATUS_SUCCESS` sans lui laisserait le client abonné à un événement
/// qui ne partirait jamais.
#[test]
fn add_remet_l_entree_au_port() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_ADD);
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_SUCCESS);
    assert_eq!(
        *port.object().get().ajouts.lock().unwrap(),
        vec![FAUSSE_ENTREE],
        "l'entrée arrive au port sans être copiée ni transformée"
    );
    assert_eq!(traces(this), vec![("ADD", true, Ok(()))]);
    assert_eq!(common::release(this), 0);
}

/// Le verbe `REMOVE` réussit **sans rien faire** : le port a déjà retiré l'entrée de sa
/// liste, il n'existe aucune méthode inverse d'`AddEventToEventList`.
#[test]
fn remove_ne_fait_rien_et_reussit() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_REMOVE);
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_SUCCESS);
    assert!(port.object().get().ajouts.lock().unwrap().is_empty());
    assert_eq!(traces(this), vec![("REMOVE", true, Ok(()))]);
    assert_eq!(common::release(this), 0);
}

/// `ADD` sans entrée d'événement : `STATUS_UNSUCCESSFUL`, la réponse de SYSVAD au même cas.
/// Distinct de `STATUS_INVALID_PARAMETER` (le jeu est bon, c'est l'entrée qui manque) et
/// surtout : le port n'est **pas** appelé.
#[test]
fn add_sans_entree_est_un_echec_et_n_appelle_pas_le_port() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_ADD);
    req.EventEntry = ptr::null_mut();
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_UNSUCCESSFUL);
    assert!(port.object().get().ajouts.lock().unwrap().is_empty());
    assert!(traces(this).is_empty(), "le gestionnaire n'est pas atteint");
    assert_eq!(common::release(this), 0);
}

/// `ADD` sans `IPortEvents` (`Init` pas passé, ou `QueryInterface` échoué) : refus franc.
/// Répondre `STATUS_SUCCESS` laisserait le client croire qu'il est abonné.
#[test]
fn add_sans_port_refuse_l_abonnement() {
    let this = cible_sans_port();
    let mut req = requete(&ITEM_JACK, this, PCEVENT_VERB_ADD);
    assert_eq!(appeler(&ITEM_JACK, &mut req), STATUS_INVALID_DEVICE_REQUEST);
    assert_eq!(
        traces(this),
        vec![("ADD", true, Err(STATUS_INVALID_DEVICE_REQUEST))]
    );
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// 5. Revalidation du jeu et de l'identifiant.
// ---------------------------------------------------------------------------------

/// Un **jeu d'événement inconnu** : refusé aux trois verbes, et le port n'est pas touché.
///
/// PortCls a pourtant routé la requête vers ce gestionnaire — c'est justement le cas que la
/// documentation ne garantit pas de ne jamais produire, et que SYSVAD contrôle aussi.
#[test]
fn un_jeu_d_evenement_inconnu_est_refuse() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    for verbe in [PCEVENT_VERB_SUPPORT, PCEVENT_VERB_ADD, PCEVENT_VERB_REMOVE] {
        let mut req = requete(&ITEM_SET_ETRANGER, this, verbe);
        assert_eq!(
            appeler(&ITEM_SET_ETRANGER, &mut req),
            STATUS_INVALID_PARAMETER,
            "verbe {verbe}"
        );
    }
    assert!(port.object().get().ajouts.lock().unwrap().is_empty());
    for (_, reconnu, resultat) in traces(this) {
        assert!(!reconnu, "le jeu n'est pas reconnu");
        assert_eq!(resultat, Err(STATUS_INVALID_PARAMETER));
    }
    assert_eq!(common::release(this), 0);
}

/// Le bon jeu mais le mauvais identifiant : `KSEVENT_PINCAPS_FORMATCHANGE` (0) est du même
/// jeu que `JACKINFOCHANGE` (1). C'est le cas que le contrôle du seul `Set` manquerait.
#[test]
fn un_identifiant_du_meme_jeu_est_refuse() {
    let port = faux_port_events();
    let this = cible_avec_port(&port);
    let mut req = requete(&ITEM_ID_ETRANGER, this, PCEVENT_VERB_ADD);
    assert_eq!(
        appeler(&ITEM_ID_ETRANGER, &mut req),
        STATUS_INVALID_PARAMETER
    );
    assert!(port.object().get().ajouts.lock().unwrap().is_empty());
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// 6. `PortEvents` : obtention et signalement.
// ---------------------------------------------------------------------------------

/// `from_port` fait un vrai `QueryInterface` : il réussit sur un objet qui répond à
/// `IID_IPortEvents`, et **échoue** sur un objet qui n'y répond pas.
#[test]
fn from_port_passe_par_query_interface() {
    let port = faux_port_events();
    let avant = port.refcount();
    // SAFETY: `port` est vivant.
    let r = unsafe { ComRef::<IPortEvents>::from_raw_add_ref(port.as_raw().cast()) };
    let events = PortEvents::from_port(&r).expect("le faux port répond à IID_IPortEvents");
    // Le `QueryInterface` a pris une référence de plus (celle de `r`, plus celle d'events).
    assert!(port.refcount() > avant);
    drop(events);
    drop(r);
    assert_eq!(port.refcount(), avant);

    // Un objet d'une autre interface : `IID_IPortEvents` n'est pas dans ses IID.
    let autre = common::faux_port_stream();
    // SAFETY: `autre` est vivant.
    let r = unsafe {
        ComRef::<portcls::portcls_sys::IPortWaveRTStream>::from_raw_add_ref(autre.as_raw().cast())
    };
    assert!(
        PortEvents::from_port(&r).is_err(),
        "un port qui n'implémente pas IPortEvents doit être refusé"
    );
}

/// `jack_info_change` appelle `GenerateEventList` avec le jeu, l'identifiant et le ciblage
/// de broche attendus — c'est l'appel que `Cable::set_connected` fera.
///
/// `PinEvent = TRUE` alors que l'item est déclaré sur la table du **filtre** : c'est la
/// géométrie de SYSVAD, vérifiée et non déduite (voir la documentation de `portcls::event`).
#[test]
fn jack_info_change_cible_la_broche_par_le_signalement() {
    let port = faux_port_events();
    // SAFETY: `port` est vivant.
    let r = unsafe { ComRef::<IPortEvents>::from_raw_add_ref(port.as_raw().cast()) };
    let events = PortEvents::from_ref(r);

    events.jack_info_change(1).unwrap();

    let generations = port.object().get().generations.lock().unwrap().clone();
    assert_eq!(generations.len(), 1);
    let g = generations[0];
    assert_eq!(
        g.set,
        Some(GUID2::de(&KSEVENTSETID_PinCapsChange)),
        "le jeu est passé, pas NULL (qui serait un joker sur tous les événements)"
    );
    assert_eq!(g.id, JACK_INFO_CHANGE_ID);
    assert_eq!(g.pin_event, 1, "PinEvent = TRUE");
    assert_eq!(g.pin_id, 1);
    assert_eq!(g.node_event, 0, "NodeEvent = FALSE");
}

/// `generate` traduit `Option` en couple `(drapeau, identifiant)`, dans les deux sens.
#[test]
fn generate_traduit_les_options_en_drapeaux() {
    let port = faux_port_events();
    // SAFETY: `port` est vivant.
    let r = unsafe { ComRef::<IPortEvents>::from_raw_add_ref(port.as_raw().cast()) };
    let events = PortEvents::from_ref(r);

    events
        .generate(&KSEVENTSETID_PinCapsChange, 7, None, Some(3))
        .unwrap();
    events
        .generate(&KSEVENTSETID_PinCapsChange, 7, Some(2), Some(3))
        .unwrap();

    let g = port.object().get().generations.lock().unwrap().clone();
    assert_eq!((g[0].pin_event, g[0].pin_id), (0, 0), "None ⇒ FALSE et 0");
    assert_eq!((g[0].node_event, g[0].node_id), (1, 3));
    assert_eq!((g[1].pin_event, g[1].pin_id), (1, 2));
    assert_eq!((g[1].node_event, g[1].node_id), (1, 3));
}

/// Le `GUID` passé à `GenerateEventList` est une **copie locale** : le prototype veut un
/// `GUID*` non `const`, et rien de `'static` ne doit être exposé à l'écriture.
#[test]
fn le_guid_passe_au_port_n_est_pas_la_static() {
    /// Un faux port qui retient l'**adresse** du `GUID` reçu.
    static ADRESSE: OnceLock<usize> = OnceLock::new();

    unsafe extern "C" fn note_adresse(
        _this: This,
        set: *mut GUID,
        _id: ULONG,
        _pe: BOOL,
        _pi: ULONG,
        _ne: BOOL,
        _ni: ULONG,
    ) {
        let _ = ADRESSE.set(set as usize);
    }

    static VTBL: IPortEventsVtbl = IPortEventsVtbl {
        QueryInterface: Some(unknown::query_interface::<IPortEventsVtbl, Marqueur>),
        AddRef: Some(unknown::add_ref::<IPortEventsVtbl, Marqueur>),
        Release: Some(unknown::release::<IPortEventsVtbl, Marqueur>),
        AddEventToEventList: None,
        GenerateEventList: Some(note_adresse),
    };
    /// Objet sans état : seul le slot `GenerateEventList` compte ici.
    struct Marqueur;

    let port = ComObject::new(&VTBL, Marqueur);
    // SAFETY: `port` est vivant.
    let r = unsafe { ComRef::<IPortEvents>::from_raw_add_ref(port.as_raw().cast()) };
    PortEvents::from_ref(r).jack_info_change(0).unwrap();

    let vue = *ADRESSE.get().expect("GenerateEventList appelée");
    let statique = ptr::from_ref(&KSEVENTSETID_PinCapsChange) as usize;
    assert_ne!(
        vue, statique,
        "le port reçoit une copie sur la pile, pas la constante du crate"
    );
}
