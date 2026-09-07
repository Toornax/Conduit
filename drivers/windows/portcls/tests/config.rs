//! Le jeu de propriétés privé `KSPROPSETID_Conduit` vu par un faux PortCls, sur le modèle
//! de `tests/jack.rs` : une `PCPROPERTY_REQUEST` bâtie à la main est passée au `Handler` du
//! `PCPROPERTY_ITEM` que `config::cable_state_item` et `config::version_item` produisent.
//!
//! Quatre points que ces tests verrouillent, et que les tests unitaires du module ne
//! peuvent pas atteindre parce qu'ils ne passent pas par le thunk :
//!
//! - **« sans effet » est vérifié, pas supposé** : chaque refus est encadré d'une lecture
//!   avant et d'une lecture après, et le faux miniport compte ses écritures. Un
//!   gestionnaire qui appliquerait puis refuserait passerait tous les tests de statut ;
//! - **le contrôle de privilège précède la validation** : un appelant sans droit reçoit
//!   `STATUS_PRIVILEGE_NOT_HELD` même avec un tampon parfaitement formé, et **le même**
//!   statut avec un tampon absurde — il n'apprend rien du format en le faisant varier ;
//! - **la négociation de taille de KS** : tampon vide, trop court, exact, trop grand, aux
//!   trois verbes ;
//! - **l'échec de persistance ne fait pas échouer la propriété** : le faux miniport sait
//!   refuser d'écrire, et le `SET` rend quand même `STATUS_SUCCESS` avec l'état appliqué.

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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use common::This;
use conduit_com::{ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use conduit_kmd_core::config::{
    CABLE_MAX, CABLE_STATE_BYTES, CONFIG_VERSION, CableState, KSPROPERTY_CONDUIT_CABLE_STATE,
    KSPROPERTY_CONDUIT_VERSION, KSPROPSETID_CONDUIT, O_CABLE, O_CHANNELS, O_CONNECTED, O_RESERVED,
};
use portcls::portcls_sys::{
    GUID, GUID_NULL, IMiniportTopologyVtbl, IUnknown, KSPROPERTY_TYPE_BASICSUPPORT,
    KSPROPERTY_TYPE_GET, KSPROPERTY_TYPE_SET, KSPROPTYPESETID_General, NTSTATUS,
    PCFILTER_DESCRIPTOR, PCPROPERTY_ITEM, PCPROPERTY_REQUEST, ULONG, VARENUM,
};
use portcls::{
    CABLE_STATE_ACCESS_FLAGS, CableConfig, ConfigTrace, MiniportTopology, PortTopology,
    ResourceList, STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NOT_SUPPORTED, STATUS_PRIVILEGE_NOT_HELD, VERSION_ACCESS_FLAGS, cable_state_item,
    new_topology_object, version_item,
};

// ---------------------------------------------------------------------------------
// Décalages de `KSPROPERTY_DESCRIPTION`, recopiés de portcls-sys/tests/layout.golden.
// ---------------------------------------------------------------------------------

/// `KSPROPERTY_DESCRIPTION::AccessFlags`.
const D_ACCESSFLAGS: usize = 0;
/// `KSPROPERTY_DESCRIPTION::DescriptionSize`.
const D_DESCRIPTIONSIZE: usize = 4;
/// `KSPROPERTY_DESCRIPTION::PropTypeSet.Set`.
const D_PROPTYPESET_SET: usize = 8;
/// `KSPROPERTY_DESCRIPTION::PropTypeSet.Id`.
const D_PROPTYPESET_ID: usize = 24;
/// `KSPROPERTY_DESCRIPTION::PropTypeSet.Flags`.
const D_PROPTYPESET_FLAGS: usize = 28;
/// `KSPROPERTY_DESCRIPTION::MembersListCount`.
const D_MEMBERSLISTCOUNT: usize = 32;
/// `KSPROPERTY_DESCRIPTION::Reserved`.
const D_RESERVED: usize = 36;
/// `sizeof(KSPROPERTY_DESCRIPTION)`.
const TAILLE_DESCRIPTION: usize = 40;
/// `sizeof(ULONG)` : le premier palier de `BASICSUPPORT`.
const TAILLE_ACCESSFLAGS: usize = 4;
/// Taille de la valeur de la version.
const TAILLE_VERSION: usize = 4;

/// Le câble que le faux miniport sert.
const CABLE: u32 = 3;
/// Nombre de canaux du faux miniport, celui que le pilote sait servir (M1b-05 exclue).
const CANAUX: u32 = 2;

// ---------------------------------------------------------------------------------
// Le faux miniport.
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour la
/// `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

struct Faux {
    /// L'état actif, celui qu'un `SET` modifie.
    connecte: AtomicBool,
    /// Faux tant qu'on veut refuser toute écriture (contrôle de privilège).
    privilegie: AtomicBool,
    /// Vrai pour faire échouer la **persistance**, l'état en mémoire restant appliqué.
    persistance_casse: AtomicBool,
    /// Nombre d'appels à [`CableConfig::set_connected`] : c'est lui qui démontre le
    /// « sans effet », un refus ne devant jamais l'incrémenter.
    ecritures: AtomicU32,
    /// Nombre d'appels à [`CableConfig::trace`].
    traces: AtomicU32,
    /// Statut de la dernière trace.
    dernier_statut: AtomicU32,
}

impl MiniportTopology for Faux {
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

/// `STATUS_DISK_FULL` : un échec de persistance plausible et sans rapport avec la requête.
const STATUS_DISK_FULL: NtStatus = 0xC000_007F_u32 as NtStatus;

impl CableConfig for Faux {
    fn cable_index(&self) -> u32 {
        CABLE
    }

    fn channels(&self) -> u32 {
        CANAUX
    }

    fn is_connected(&self) -> bool {
        self.connecte.load(Ordering::SeqCst)
    }

    fn set_connected(&self, connected: bool) -> Result<(), NtStatus> {
        self.ecritures.fetch_add(1, Ordering::SeqCst);
        // L'état en mémoire est appliqué **avant** toute considération de persistance :
        // c'est le contrat du trait, et c'est ce que le test de disque plein vérifie.
        self.connecte.store(connected, Ordering::SeqCst);
        if self.persistance_casse.load(Ordering::SeqCst) {
            return Err(STATUS_DISK_FULL);
        }
        Ok(())
    }

    fn may_configure(&self) -> bool {
        self.privilegie.load(Ordering::SeqCst)
    }

    fn trace(&self, trace: &ConfigTrace<'_>) {
        self.traces.fetch_add(1, Ordering::SeqCst);
        self.dernier_statut
            .store(trace.status as u32, Ordering::SeqCst);
        assert_eq!(trace.cable, CABLE, "la trace nomme le câble du miniport");
        assert!(
            trace.instance.is_empty(),
            "nos propriétés sont de simples KSPROPERTY : Instance doit rester vide"
        );
    }
}

fn faux(connecte: bool, privilegie: bool) -> This {
    new_topology_object(Faux {
        connecte: AtomicBool::new(connecte),
        privilegie: AtomicBool::new(privilegie),
        persistance_casse: AtomicBool::new(false),
        ecritures: AtomicU32::new(0),
        traces: AtomicU32::new(0),
        dernier_statut: AtomicU32::new(0),
    })
    .into_raw()
}

fn interieur(this: This) -> &'static Faux {
    unsafe { conduit_com::ComObject::<IMiniportTopologyVtbl, Faux>::inner(this) }
}

// ---------------------------------------------------------------------------------
// Tables d'automatisation de test.
// ---------------------------------------------------------------------------------

/// `PCPROPERTY_ITEM` contient un `*const GUID` et un pointeur de fonction : `Sync` déclaré
/// à la main pour la `static`, comme le pilote le fait pour sa vraie table de filtre.
struct SyncItem(PCPROPERTY_ITEM);
unsafe impl Sync for SyncItem {}

static ITEM_ETAT: SyncItem = SyncItem(cable_state_item::<IMiniportTopologyVtbl, Faux>());
static ITEM_VERSION: SyncItem = SyncItem(version_item::<IMiniportTopologyVtbl, Faux>());

/// `Node` d'une propriété de filtre : `ULONG(-1)`.
const NODE: ULONG = ULONG::MAX;

// ---------------------------------------------------------------------------------
// Outillage.
// ---------------------------------------------------------------------------------

fn requete(this: This, item: &'static SyncItem, verb: ULONG) -> PCPROPERTY_REQUEST {
    PCPROPERTY_REQUEST {
        MajorTarget: this.cast(),
        MinorTarget: ptr::null_mut(),
        Node: NODE,
        PropertyItem: &item.0,
        Verb: verb,
        InstanceSize: 0,
        Instance: ptr::null_mut(),
        ValueSize: 0,
        Value: ptr::null_mut(),
        Irp: ptr::null_mut(),
    }
}

fn appeler(item: &'static SyncItem, req: &mut PCPROPERTY_REQUEST) -> NTSTATUS {
    let slot = item.0.Handler.expect("Handler renseigné par l'item");
    unsafe { slot(req) }
}

fn avec_valeur(req: &mut PCPROPERTY_REQUEST, tampon: &mut [u8]) {
    req.Value = tampon.as_mut_ptr().cast();
    req.ValueSize = tampon.len() as ULONG;
}

/// `GET` de l'état avec `place` octets de tampon : `(statut, ValueSize, octets)`.
fn lire_etat(this: This, place: usize) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut tampon = vec![0xAAu8; place];
    let mut req = requete(this, &ITEM_ETAT, KSPROPERTY_TYPE_GET);
    if place > 0 {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(&ITEM_ETAT, &mut req);
    (status, req.ValueSize, tampon)
}

/// `GET` de l'état avec la place exacte, décodé. Panique si la lecture échoue : dans tous
/// les tests ci-dessous la lecture est censée réussir, y compris après un refus.
fn etat(this: This) -> CableState {
    let (status, taille, octets) = lire_etat(this, CABLE_STATE_BYTES);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille as usize, CABLE_STATE_BYTES);
    CableState::from_bytes(&octets).expect("le pilote ne rend que des états valides")
}

/// `SET` de l'état avec le tampon donné : `(statut, ValueSize)`.
fn ecrire(this: This, valeur: &[u8]) -> (NTSTATUS, ULONG) {
    let mut tampon = valeur.to_vec();
    let mut req = requete(this, &ITEM_ETAT, KSPROPERTY_TYPE_SET);
    if !tampon.is_empty() {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(&ITEM_ETAT, &mut req);
    (status, req.ValueSize)
}

/// `BASICSUPPORT` sur `item` avec `place` octets : `(statut, ValueSize, octets)`.
fn basic_support(this: This, item: &'static SyncItem, place: usize) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut tampon = vec![0u8; place];
    let mut req = requete(this, item, KSPROPERTY_TYPE_BASICSUPPORT);
    if place > 0 {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(item, &mut req);
    (status, req.ValueSize, tampon)
}

fn u32_a(octets: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(octets[offset..offset + 4].try_into().unwrap())
}

/// Les 16 octets d'un GUID tels qu'ils apparaissent en mémoire.
fn guid_octets(guid: &GUID) -> Vec<u8> {
    let mut v = Vec::with_capacity(16);
    v.extend_from_slice(&guid.Data1.to_ne_bytes());
    v.extend_from_slice(&guid.Data2.to_ne_bytes());
    v.extend_from_slice(&guid.Data3.to_ne_bytes());
    v.extend_from_slice(&guid.Data4);
    v
}

/// Une `CableState` sérialisée, câble et état donnés, canaux et réservé corrects.
fn valeur(cable: u32, connecte: bool) -> Vec<u8> {
    CableState::new(cable, connecte).to_bytes().to_vec()
}

// ---------------------------------------------------------------------------------
// Les entrées de table.
// ---------------------------------------------------------------------------------

/// Les deux `PCPROPERTY_ITEM` portent le jeu privé, leurs identifiants et leurs drapeaux.
#[test]
fn les_entrees_de_table_portent_le_jeu_prive() {
    let set_etat = unsafe { &*ITEM_ETAT.0.Set };
    let set_version = unsafe { &*ITEM_VERSION.0.Set };
    for (nom, set) in [("état", set_etat), ("version", set_version)] {
        assert_eq!(set.Data1, KSPROPSETID_CONDUIT.data1, "{nom}");
        assert_eq!(set.Data2, KSPROPSETID_CONDUIT.data2, "{nom}");
        assert_eq!(set.Data3, KSPROPSETID_CONDUIT.data3, "{nom}");
        assert_eq!(set.Data4, KSPROPSETID_CONDUIT.data4, "{nom}");
    }
    // Le même `static`, pas deux copies : `PCPROPERTY_ITEM::Set` est un pointeur que
    // PortCls conserve, et deux GUID identiques à deux adresses seraient un gaspillage
    // silencieux qui masquerait une divergence future.
    assert!(ptr::eq(set_etat, set_version));

    assert_eq!(ITEM_ETAT.0.Id, KSPROPERTY_CONDUIT_CABLE_STATE);
    assert_eq!(ITEM_VERSION.0.Id, KSPROPERTY_CONDUIT_VERSION);
    assert_eq!(ITEM_ETAT.0.Flags, CABLE_STATE_ACCESS_FLAGS);
    assert_eq!(ITEM_VERSION.0.Flags, VERSION_ACCESS_FLAGS);
    assert!(ITEM_ETAT.0.Handler.is_some());
    assert!(ITEM_VERSION.0.Handler.is_some());
}

// ---------------------------------------------------------------------------------
// GET de l'état.
// ---------------------------------------------------------------------------------

/// La lecture rend les quatre champs, chacun à son décalage du contrat.
#[test]
fn la_lecture_rend_les_quatre_champs_a_leurs_decalages() {
    let this = faux(true, false);
    let (status, taille, octets) = lire_etat(this, CABLE_STATE_BYTES);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille as usize, CABLE_STATE_BYTES);
    assert_eq!(u32_a(&octets, O_CABLE), CABLE);
    assert_eq!(u32_a(&octets, O_CONNECTED), 1);
    assert_eq!(u32_a(&octets, O_CHANNELS), CANAUX);
    assert_eq!(u32_a(&octets, O_RESERVED), 0);

    // Déconnecté : seul le champ d'état bouge.
    interieur(this).connecte.store(false, Ordering::SeqCst);
    let (_, _, octets) = lire_etat(this, CABLE_STATE_BYTES);
    assert_eq!(u32_a(&octets, O_CONNECTED), 0);
    assert_eq!(u32_a(&octets, O_CABLE), CABLE);
    assert_eq!(u32_a(&octets, O_CHANNELS), CANAUX);
    assert_eq!(u32_a(&octets, O_RESERVED), 0);

    common::release(this);
}

/// La lecture ne demande **aucun** privilège : connaître l'état d'un câble n'en est pas un.
#[test]
fn la_lecture_ne_demande_aucun_privilege() {
    let this = faux(true, false);
    assert!(!interieur(this).may_configure());
    let (status, _, _) = lire_etat(this, CABLE_STATE_BYTES);
    assert_eq!(
        status, STATUS_SUCCESS,
        "un outil de diagnostic non élevé doit pouvoir lire l'état"
    );
    common::release(this);
}

/// Négociation de taille du `GET` : vide, trop court, exact, trop grand.
#[test]
fn la_negociation_de_taille_de_la_lecture() {
    let this = faux(true, false);

    // Tampon absent : interrogation de taille. `STATUS_BUFFER_OVERFLOW` et la taille
    // requise dans `ValueSize` — c'est ainsi que le client sait quoi allouer.
    let (status, taille, _) = lire_etat(this, 0);
    assert_eq!(status, STATUS_BUFFER_OVERFLOW);
    assert_eq!(taille as usize, CABLE_STATE_BYTES);

    // Trop court, de un octet comme de quinze : rien n'est écrit, la taille requise est
    // rendue.
    for place in 1..CABLE_STATE_BYTES {
        let (status, taille, octets) = lire_etat(this, place);
        assert_eq!(status, STATUS_BUFFER_TOO_SMALL, "place {place}");
        assert_eq!(taille as usize, CABLE_STATE_BYTES, "place {place}");
        assert!(
            octets.iter().all(|o| *o == 0xAA),
            "place {place} : une réponse à demi écrite serait pire qu'aucune"
        );
    }

    // Exact, puis plus grand : succès, et rien n'est écrit au-delà des seize octets.
    let (status, taille, _) = lire_etat(this, CABLE_STATE_BYTES);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille as usize, CABLE_STATE_BYTES);

    let (status, taille, octets) = lire_etat(this, CABLE_STATE_BYTES + 8);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille as usize, CABLE_STATE_BYTES);
    assert!(octets[CABLE_STATE_BYTES..].iter().all(|o| *o == 0xAA));

    common::release(this);
}

/// Tampon **sur-aligné**, pour que le décalage de 1 ci-dessous soit un vrai désalignement
/// et non un hasard d'allocation : un `[u8; N]` nu est aligné sur 1, et son adresse + 1
/// peut très bien tomber sur un multiple de 4.
#[repr(align(4))]
struct Aligne4([u8; CABLE_STATE_BYTES + 3]);

/// Un tampon désaligné se lit et s'écrit comme un autre : le gestionnaire ne transtype
/// jamais vers un `*mut CableState`.
///
/// C'est la règle de `portcls::property` (« `Value` n'est aligné sur rien ») mise à
/// l'épreuve : une écriture désalignée par un pointeur de structure est un comportement
/// indéfini en Rust même là où x64 la tolère au niveau du processeur, donc invisible en
/// pratique — ce test ne l'attraperait pas non plus, mais il fixe le contrat et
/// `-Zsanitizer` ou Miri le lisent.
#[test]
fn un_tampon_desaligne_marche() {
    let this = faux(true, true);
    // Base alignée sur 4, tranche à l'octet 1 : garanti désaligné pour un `u32`.
    let mut brut = Aligne4([0u8; CABLE_STATE_BYTES + 3]);
    assert_eq!(
        brut.0.as_ptr() as usize % 4,
        0,
        "la base doit être alignée pour que le décalage de 1 désaligne vraiment"
    );
    let tranche = &mut brut.0[1..=CABLE_STATE_BYTES];
    assert_ne!(tranche.as_ptr() as usize % 4, 0);
    let mut req = requete(this, &ITEM_ETAT, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, tranche);
    assert_eq!(appeler(&ITEM_ETAT, &mut req), STATUS_SUCCESS);
    assert_eq!(u32_a(tranche, O_CABLE), CABLE);
    assert_eq!(u32_a(tranche, O_CONNECTED), 1);

    // Et en écriture : le même décalage impair, un état inversé.
    let mut brut = Aligne4([0u8; CABLE_STATE_BYTES + 3]);
    brut.0[1..=CABLE_STATE_BYTES].copy_from_slice(&valeur(CABLE, false));
    let tranche = &mut brut.0[1..=CABLE_STATE_BYTES];
    let mut req = requete(this, &ITEM_ETAT, KSPROPERTY_TYPE_SET);
    avec_valeur(&mut req, tranche);
    assert_eq!(appeler(&ITEM_ETAT, &mut req), STATUS_SUCCESS);
    assert!(!interieur(this).is_connected());

    common::release(this);
}

// ---------------------------------------------------------------------------------
// SET : le contrôle d'accès.
// ---------------------------------------------------------------------------------

/// Sans privilège : `STATUS_PRIVILEGE_NOT_HELD`, **sans effet**, quel que soit le tampon.
///
/// L'état est relu avant et après, et le compteur d'écritures du faux miniport reste à
/// zéro : un gestionnaire qui appliquerait puis refuserait passerait le seul contrôle de
/// statut.
#[test]
fn sans_privilege_le_set_est_refuse_sans_effet() {
    let this = faux(true, false);
    let avant = etat(this);
    assert!(avant.is_connected());

    // Un tampon parfaitement formé, et pourtant refusé.
    let (status, _) = ecrire(this, &valeur(CABLE, false));
    assert_eq!(status, STATUS_PRIVILEGE_NOT_HELD);

    // Le **même** statut avec des tampons absurdes : un appelant sans droit n'apprend rien
    // du format en le faisant varier.
    for mauvais in [
        vec![],
        vec![0u8; 1],
        vec![0xFFu8; CABLE_STATE_BYTES],
        vec![0u8; 4096],
        valeur(CABLE_MAX + 7, true),
    ] {
        let (status, _) = ecrire(this, &mauvais);
        assert_eq!(
            status,
            STATUS_PRIVILEGE_NOT_HELD,
            "tampon de {} octets : le privilège doit être vérifié AVANT la validation",
            mauvais.len()
        );
    }

    assert_eq!(etat(this), avant, "l'état n'a pas bougé");
    assert_eq!(
        interieur(this).ecritures.load(Ordering::SeqCst),
        0,
        "aucune écriture ne doit avoir été tentée"
    );
    common::release(this);
}

/// Avec privilège, un tampon valide s'applique — et se relit.
#[test]
fn avec_privilege_le_set_s_applique_et_se_relit() {
    let this = faux(true, true);
    assert!(etat(this).is_connected());

    let (status, _) = ecrire(this, &valeur(CABLE, false));
    assert_eq!(status, STATUS_SUCCESS);
    assert!(!etat(this).is_connected());
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 1);

    // Et retour : l'aller-retour complet, `GET` → `SET` → `GET`.
    let (status, _) = ecrire(this, &valeur(CABLE, true));
    assert_eq!(status, STATUS_SUCCESS);
    assert!(etat(this).is_connected());
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 2);

    // Ce que le `GET` rend se réécrit tel quel : le contrat est un aller-retour fidèle.
    let relu = etat(this);
    let (status, _) = ecrire(this, &relu.to_bytes());
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(etat(this), relu);

    common::release(this);
}

// ---------------------------------------------------------------------------------
// SET : la validation.
// ---------------------------------------------------------------------------------

/// Chaque entrée invalide rend `STATUS_INVALID_PARAMETER`, **sans effet**.
///
/// L'état est relu après chaque refus, et le compteur d'écritures reste à zéro sur toute la
/// table : c'est le critère de M1b-04, « entrée invalide → `STATUS_INVALID_PARAMETER` sans
/// effet ».
#[test]
fn toute_entree_invalide_est_refusee_sans_effet() {
    let this = faux(true, true);
    let avant = etat(this);

    // La bonne valeur, dont chaque cas ci-dessous n'écarte qu'un champ.
    let bonne = CableState::new(CABLE, false);

    let mut cas: Vec<(&str, Vec<u8>)> = vec![
        // Longueur : tronquée d'un octet, allongée d'un octet, vide, énorme.
        (
            "tronqué",
            bonne.to_bytes()[..CABLE_STATE_BYTES - 1].to_vec(),
        ),
        ("vide", vec![]),
        ("énorme", vec![0u8; 4096]),
        // Champ réservé non nul : la place doit rester libre pour une v2.
        (
            "réservé",
            CableState {
                reserved: 1,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        // État de connexion hors de {0, 1} : notre protocole est plus strict que le `BOOL`
        // de KS, et volontairement.
        (
            "connected = 2",
            CableState {
                connected: 2,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        (
            "connected = 0xFFFFFFFF",
            CableState {
                connected: u32::MAX,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        // Canaux hors du domaine du contrat.
        (
            "canaux = 0",
            CableState {
                channels: 0,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        (
            "canaux = 99",
            CableState {
                channels: 99,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        // Canaux dans le domaine du contrat mais que M1b-04 ne sait pas servir (M1b-05).
        (
            "canaux = 6 (M1b-05)",
            CableState {
                channels: 6,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        // Câble hors domaine, puis un câble valide mais qui n'est pas celui de ce filtre :
        // l'écho vérifié attrape un service qui se tromperait de descripteur.
        (
            "câble hors domaine",
            CableState {
                cable: CABLE_MAX,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        (
            "câble voisin",
            CableState {
                cable: CABLE + 1,
                ..bonne
            }
            .to_bytes()
            .to_vec(),
        ),
        (
            "câble 0",
            CableState { cable: 0, ..bonne }.to_bytes().to_vec(),
        ),
    ];
    // Un octet de trop derrière un préfixe **parfaitement valide** : le cas que seul un
    // parseur strict refuse, et celui qu'un fuzzer trouve en premier.
    let mut allonge = bonne.to_bytes().to_vec();
    allonge.push(0);
    cas.push(("préfixe valide + un octet", allonge));

    for (nom, tampon) in cas {
        let (status, _) = ecrire(this, &tampon);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "cas « {nom} »");
        assert_eq!(etat(this), avant, "cas « {nom} » : l'état a bougé");
    }
    assert_eq!(
        interieur(this).ecritures.load(Ordering::SeqCst),
        0,
        "aucun refus ne doit avoir touché à l'état"
    );

    // Et pour finir : la même valeur, correcte cette fois, passe. Sans quoi la table
    // ci-dessus pourrait « réussir » parce que le gestionnaire refuse tout.
    let (status, _) = ecrire(this, &bonne.to_bytes());
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 1);

    common::release(this);
}

/// Toutes les longueurs de 0 à 32 octets sauf la bonne sont refusées.
#[test]
fn seule_la_longueur_exacte_est_acceptee() {
    let this = faux(true, true);
    let avant = etat(this);
    for taille in 0..=32usize {
        let (status, _) = ecrire(this, &vec![0u8; taille]);
        // Un tampon de seize octets nuls est refusé lui aussi, mais par le **domaine**
        // (canaux = 0), pas par la longueur : les deux contrôles restent distincts.
        assert_eq!(status, STATUS_INVALID_PARAMETER, "taille {taille}");
    }
    assert_eq!(etat(this), avant);
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 0);
    common::release(this);
}

/// Un échec de **persistance** ne fait pas échouer la propriété : un disque plein ne doit
/// pas empêcher d'activer un câble.
#[test]
fn un_echec_de_persistance_ne_fait_pas_echouer_la_propriete() {
    let this = faux(false, true);
    interieur(this)
        .persistance_casse
        .store(true, Ordering::SeqCst);

    let (status, _) = ecrire(this, &valeur(CABLE, true));
    assert_eq!(
        status, STATUS_SUCCESS,
        "l'état en mémoire est appliqué ; l'échec d'écriture est journalisé, pas rendu"
    );
    assert!(etat(this).is_connected(), "l'état en mémoire a bien changé");
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 1);
    common::release(this);
}

// ---------------------------------------------------------------------------------
// La version.
// ---------------------------------------------------------------------------------

/// La version se lit, ne s'écrit pas, et vaut celle du contrat.
#[test]
fn la_version_se_lit_et_ne_s_ecrit_pas() {
    let this = faux(true, true);

    let mut tampon = [0u8; TAILLE_VERSION];
    let mut req = requete(this, &ITEM_VERSION, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_VERSION, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize as usize, TAILLE_VERSION);
    assert_eq!(u32_a(&tampon, 0), CONFIG_VERSION);

    // Interrogation de taille.
    let mut req = requete(this, &ITEM_VERSION, KSPROPERTY_TYPE_GET);
    assert_eq!(appeler(&ITEM_VERSION, &mut req), STATUS_BUFFER_OVERFLOW);
    assert_eq!(req.ValueSize as usize, TAILLE_VERSION);

    // Trop court : rien d'écrit, la taille requise rendue.
    for place in 1..TAILLE_VERSION {
        let mut tampon = vec![0xAAu8; place];
        let mut req = requete(this, &ITEM_VERSION, KSPROPERTY_TYPE_GET);
        avec_valeur(&mut req, &mut tampon);
        assert_eq!(appeler(&ITEM_VERSION, &mut req), STATUS_BUFFER_TOO_SMALL);
        assert_eq!(req.ValueSize as usize, TAILLE_VERSION);
        assert!(tampon.iter().all(|o| *o == 0xAA), "place {place}");
    }

    // `SET` : le gestionnaire ne l'implémente pas, et le dit — seconde ligne de défense
    // derrière les `Flags`, que PortCls filtre déjà.
    let mut tampon = CONFIG_VERSION.to_ne_bytes();
    let mut req = requete(this, &ITEM_VERSION, KSPROPERTY_TYPE_SET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_VERSION, &mut req), STATUS_NOT_SUPPORTED);

    common::release(this);
}

// ---------------------------------------------------------------------------------
// BASICSUPPORT.
// ---------------------------------------------------------------------------------

/// Les paliers de `BASICSUPPORT`, pour les deux propriétés, et le contenu de chacun.
#[test]
fn basic_support_repond_en_paliers() {
    let this = faux(true, false);

    for (nom, item, flags, type_set, variante) in [
        (
            "état",
            &ITEM_ETAT,
            CABLE_STATE_ACCESS_FLAGS,
            GUID_NULL,
            0_u32,
        ),
        (
            "version",
            &ITEM_VERSION,
            VERSION_ACCESS_FLAGS,
            KSPROPTYPESETID_General,
            VARENUM::VT_UI4 as u32,
        ),
    ] {
        // Moins de quatre octets : refusé. Répondre autre chose casserait la négociation.
        for place in 0..TAILLE_ACCESSFLAGS {
            let (status, _, _) = basic_support(this, item, place);
            assert_eq!(status, STATUS_BUFFER_TOO_SMALL, "{nom}, place {place}");
        }

        // Premier appel de KS : `sizeof(ULONG)`, les seuls `AccessFlags`, et `ValueSize`
        // porte les octets **écrits** (4), pas la taille requise (40).
        let (status, taille, octets) = basic_support(this, item, TAILLE_ACCESSFLAGS);
        assert_eq!(status, STATUS_SUCCESS, "{nom}");
        assert_eq!(taille as usize, TAILLE_ACCESSFLAGS, "{nom}");
        assert_eq!(u32_a(&octets, D_ACCESSFLAGS), flags, "{nom}");

        // Palier intermédiaire, celui qu'on oublie : la place des `AccessFlags` mais pas
        // d'une description.
        for place in TAILLE_ACCESSFLAGS..TAILLE_DESCRIPTION {
            let (status, taille, octets) = basic_support(this, item, place);
            assert_eq!(status, STATUS_SUCCESS, "{nom}, place {place}");
            assert_eq!(taille as usize, TAILLE_ACCESSFLAGS, "{nom}, place {place}");
            assert_eq!(u32_a(&octets, D_ACCESSFLAGS), flags, "{nom}, place {place}");
        }

        // Description complète : 40 octets, chaque champ à son décalage du golden.
        let (status, taille, octets) = basic_support(this, item, TAILLE_DESCRIPTION);
        assert_eq!(status, STATUS_SUCCESS, "{nom}");
        assert_eq!(taille as usize, TAILLE_DESCRIPTION, "{nom}");
        assert_eq!(u32_a(&octets, D_ACCESSFLAGS), flags, "{nom}");
        // `DescriptionSize` vaut la taille complète : aucun membre, donc 40, et le client
        // n'a rien de plus à redemander.
        assert_eq!(
            u32_a(&octets, D_DESCRIPTIONSIZE),
            TAILLE_DESCRIPTION as u32,
            "{nom}"
        );
        assert_eq!(
            &octets[D_PROPTYPESET_SET..D_PROPTYPESET_SET + 16],
            guid_octets(&type_set).as_slice(),
            "{nom}"
        );
        assert_eq!(u32_a(&octets, D_PROPTYPESET_ID), variante, "{nom}");
        assert_eq!(u32_a(&octets, D_PROPTYPESET_FLAGS), 0, "{nom}");
        // `MembersListCount` est à **32**, pas 24 ni 16 : `PropTypeSet` est un
        // `KSIDENTIFIER` de 24 octets, pas un `GUID` de 16.
        assert_eq!(u32_a(&octets, D_MEMBERSLISTCOUNT), 0, "{nom}");
        assert_eq!(u32_a(&octets, D_RESERVED), 0, "{nom}");

        // Plus grand : toujours 40 écrits, et rien au-delà.
        let (status, taille, octets) = basic_support(this, item, TAILLE_DESCRIPTION + 32);
        assert_eq!(status, STATUS_SUCCESS, "{nom}");
        assert_eq!(taille as usize, TAILLE_DESCRIPTION, "{nom}");
        assert!(
            octets[TAILLE_DESCRIPTION..].iter().all(|o| *o == 0),
            "{nom}"
        );
    }

    common::release(this);
}

/// `BASICSUPPORT` ne demande aucun privilège : décrire une propriété n'est pas la modifier.
#[test]
fn basic_support_ne_demande_aucun_privilege() {
    let this = faux(true, false);
    let (status, _, _) = basic_support(this, &ITEM_ETAT, TAILLE_DESCRIPTION);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 0);
    common::release(this);
}

// ---------------------------------------------------------------------------------
// Les refus du thunk.
// ---------------------------------------------------------------------------------

/// Les refus que le thunk de `property.rs` porte, vérifiés sur nos deux entrées : requête
/// nulle, `MajorTarget` nul, verbe inconnu.
#[test]
fn le_thunk_refuse_les_requetes_mal_formees() {
    let this = faux(true, true);

    for item in [&ITEM_ETAT, &ITEM_VERSION] {
        let slot = item.0.Handler.unwrap();
        assert_eq!(
            unsafe { slot(ptr::null_mut()) },
            STATUS_INVALID_PARAMETER,
            "requête nulle"
        );

        // `MajorTarget` nul : la garde ne peut pas lire de vtable.
        let mut req = requete(this, item, KSPROPERTY_TYPE_GET);
        req.MajorTarget = ptr::null_mut();
        assert_eq!(appeler(item, &mut req), STATUS_INVALID_DEVICE_REQUEST);

        // Verbe hors GET/SET/BASICSUPPORT (`KSPROPERTY_TYPE_SETSUPPORT`, 256).
        let mut req = requete(this, item, 256);
        assert_eq!(appeler(item, &mut req), STATUS_INVALID_DEVICE_REQUEST);
        let mut req = requete(this, item, 0);
        assert_eq!(appeler(item, &mut req), STATUS_INVALID_DEVICE_REQUEST);
    }

    // Rien de tout cela n'a touché à l'état.
    assert!(etat(this).is_connected());
    assert_eq!(interieur(this).ecritures.load(Ordering::SeqCst), 0);
    common::release(this);
}

/// La trace est appelée une fois par requête servie, et porte le statut rendu.
#[test]
fn la_trace_suit_chaque_requete() {
    let this = faux(true, false);
    let compteur = || interieur(this).traces.load(Ordering::SeqCst);

    lire_etat(this, CABLE_STATE_BYTES);
    assert_eq!(compteur(), 1);
    assert_eq!(
        interieur(this).dernier_statut.load(Ordering::SeqCst),
        STATUS_SUCCESS as u32
    );

    // Un refus de privilège se trace lui aussi : c'est la seule trace qu'on aura d'un
    // client qui essaie sans droit.
    ecrire(this, &valeur(CABLE, false));
    assert_eq!(compteur(), 2);
    assert_eq!(
        interieur(this).dernier_statut.load(Ordering::SeqCst),
        STATUS_PRIVILEGE_NOT_HELD as u32
    );

    basic_support(this, &ITEM_ETAT, TAILLE_DESCRIPTION);
    assert_eq!(compteur(), 3);

    // Un verbe refusé par le thunk n'atteint pas le gestionnaire, donc pas la trace.
    let mut req = requete(this, &ITEM_ETAT, 256);
    appeler(&ITEM_ETAT, &mut req);
    assert_eq!(compteur(), 3);

    common::release(this);
}
