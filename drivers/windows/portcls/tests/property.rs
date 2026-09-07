//! Le thunk de propriété KS vu par un faux PortCls : une `PCPROPERTY_REQUEST` bâtie à la
//! main est passée au `Handler` d'un `PCPROPERTY_ITEM` produit par `property::item`,
//! exactement comme PortCls le ferait après avoir trouvé l'entrée dans la
//! `PCAUTOMATION_TABLE` de la cible.
//!
//! Rien ici ne touche au noyau : le thunk ne fait que du calcul et de la mémoire
//! (`MajorTarget` est le `this` d'un `new_topology_object`, `Irp` est nul, les tampons
//! sont des tableaux de la pile).

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
use std::sync::atomic::{AtomicU32, Ordering};

use common::This;
use conduit_com::{ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls::portcls_sys::{
    GUID, IMiniportTopologyVtbl, IUnknown, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET,
    KSPROPERTY_TYPE_SET, KSPROPERTY_TYPE_SETSUPPORT, NTSTATUS, PCFILTER_DESCRIPTOR,
    PCPROPERTY_ITEM, PCPROPERTY_REQUEST, ULONG,
};
use portcls::{
    MiniportTopology, PortTopology, PropertyHandler, Request, ResourceList, STATUS_BUFFER_OVERFLOW,
    STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST, STATUS_NOT_SUPPORTED,
    new_topology_object, property,
};

// ---------------------------------------------------------------------------------
// Deux miniports topologie de test : `Cible`, celui que les gestionnaires attendent, et
// `Intrus`, un autre type portant la **même vtable au sens du type** mais une autre
// instanciation — donc une autre adresse (test 8).
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour la
/// `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

/// Le miniport visé par les gestionnaires de test : un compteur de `set` et une valeur.
struct Cible {
    valeur: AtomicU32,
    sets: AtomicU32,
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

/// Un autre miniport topologie : même vtable `IMiniportTopologyVtbl`, autre `T`, donc
/// **autre** `ComObject` et autre adresse de vtable.
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

// ---------------------------------------------------------------------------------
// Gestionnaires de test.
// ---------------------------------------------------------------------------------

/// Valeur rendue par `Complet::get` : 8 octets, écrits en `to_ne_bytes`.
const VALEUR_GET: u64 = 0x1122_3344_5566_7788;
/// Taille requise par `Complet::get`.
const TAILLE_GET: u32 = 8;
/// Statut rendu par `Complet::set` quand la valeur est trop courte.
const ERREUR_SET: NtStatus = STATUS_INVALID_PARAMETER;

/// Gestionnaire implémentant les trois verbes.
struct Complet;

impl PropertyHandler<Cible> for Complet {
    fn get(req: &Request<'_, Cible>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // `node` et `instance` sont bien ceux de la requête : contrôlés par le test.
        assert_eq!(req.node, NODE);
        if let Some(place) = value.get_mut(..8) {
            place.copy_from_slice(&VALEUR_GET.to_ne_bytes());
        }
        Ok(TAILLE_GET)
    }

    fn set(req: &Request<'_, Cible>, value: &[u8]) -> Result<(), NtStatus> {
        let Some(quatre) = value.get(..4) else {
            return Err(ERREUR_SET);
        };
        let mut mot = [0u8; 4];
        mot.copy_from_slice(quatre);
        req.target
            .valeur
            .store(u32::from_ne_bytes(mot), Ordering::SeqCst);
        req.target.sets.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn basic_support(_req: &Request<'_, Cible>, value: &mut [u8]) -> Result<u32, NtStatus> {
        // Comme un vrai `BASICSUPPORT` : écrire ce qui tient, dire combien a été écrit.
        // Le premier appel de KS n'a que `sizeof(ULONG)` d'`AccessFlags` : il doit réussir.
        let mut ecrits = 0u32;
        if let Some(place) = value.get_mut(..4) {
            place.copy_from_slice(&(KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET).to_ne_bytes());
            ecrits = 4;
        }
        if let Some(place) = value.get_mut(4..6) {
            place.copy_from_slice(&7u16.to_ne_bytes());
            ecrits = 6;
        }
        Ok(ecrits)
    }
}

/// Gestionnaire qui n'implémente rien : les trois défauts du trait.
struct Muet;

impl PropertyHandler<Cible> for Muet {}

/// Gestionnaire dont `get` sérialise **plusieurs champs de largeurs différentes**, pour
/// le test de tampon désaligné : c'est ce test qui interdit qu'on « optimise » un jour la
/// sérialisation en transtypant vers un `*mut` de structure.
struct Multi;

/// Taille sérialisée par `Multi::get`.
const TAILLE_MULTI: usize = 16;

impl PropertyHandler<Cible> for Multi {
    fn get(_req: &Request<'_, Cible>, value: &mut [u8]) -> Result<u32, NtStatus> {
        if let Some(place) = value.get_mut(..TAILLE_MULTI) {
            place[0..4].copy_from_slice(&0xDEAD_BEEF_u32.to_ne_bytes());
            place[4..6].copy_from_slice(&0xC0DE_u16.to_ne_bytes());
            place[6..8].copy_from_slice(&0x0102_u16.to_ne_bytes());
            place[8..16].copy_from_slice(&0x0F1E_2D3C_4B5A_6978_u64.to_ne_bytes());
        }
        Ok(TAILLE_MULTI as u32)
    }
}

// ---------------------------------------------------------------------------------
// Tables d'automatisation de test.
// ---------------------------------------------------------------------------------

/// `PCPROPERTY_ITEM` contient un `*const GUID` et un pointeur de fonction : `Sync`
/// déclaré à la main pour la `static`, comme le pilote le fera pour ses vraies tables.
struct SyncItem(PCPROPERTY_ITEM);
unsafe impl Sync for SyncItem {}

static SET_TEST: GUID = GUID {
    Data1: 0xDEAD_0001,
    Data2: 0x1234,
    Data3: 0x4000,
    Data4: [0x80, 0, 0, 0, 0, 0, 0, 1],
};

/// Nœud visé par les requêtes de test.
const NODE: ULONG = 3;
/// `Id` des propriétés de test.
const ID: ULONG = 42;

static ITEM_COMPLET: SyncItem = SyncItem(property::item::<IMiniportTopologyVtbl, Cible, Complet>(
    &SET_TEST,
    ID,
    KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
));

static ITEM_MUET: SyncItem = SyncItem(property::item::<IMiniportTopologyVtbl, Cible, Muet>(
    &SET_TEST,
    ID,
    KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
));

static ITEM_MULTI: SyncItem = SyncItem(property::item::<IMiniportTopologyVtbl, Cible, Multi>(
    &SET_TEST,
    ID,
    KSPROPERTY_TYPE_GET,
));

// ---------------------------------------------------------------------------------
// Outillage : construire une requête et appeler le slot comme PortCls.
// ---------------------------------------------------------------------------------

fn cible() -> This {
    new_topology_object(Cible {
        valeur: AtomicU32::new(0),
        sets: AtomicU32::new(0),
    })
    .into_raw()
}

fn intrus() -> This {
    new_topology_object(Intrus).into_raw()
}

/// Requête minimale : cible, verbe, ni instance ni valeur.
fn requete(item: &'static SyncItem, this: This, verb: ULONG) -> PCPROPERTY_REQUEST {
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

/// L'appel que PortCls fait : `item.Handler.unwrap()(&mut req)`.
fn appeler(item: &'static SyncItem, req: &mut PCPROPERTY_REQUEST) -> NTSTATUS {
    let slot = item
        .0
        .Handler
        .expect("Handler renseigné par property::item");
    unsafe { slot(req) }
}

/// Branche un tampon de valeur sur la requête.
fn avec_valeur(req: &mut PCPROPERTY_REQUEST, tampon: &mut [u8]) {
    req.Value = tampon.as_mut_ptr().cast();
    req.ValueSize = tampon.len() as ULONG;
}

// ---------------------------------------------------------------------------------
// 1 à 8 : la table des statuts.
// ---------------------------------------------------------------------------------

/// L'invariant dont dépend toute la garde : les trois chemins qui nomment la vtable de
/// `Cible` — celle stockée dans l'objet par `new_topology_object`, `topology::vtbl_of` et
/// `TargetVtbl::vtbl` (celui qu'emprunte le thunk) — donnent **la même adresse**.
///
/// C'est ce que `#[inline(never)]` sur `vtbl_of` protège : `&T::VTBL` est une constante
/// promue, donc `unnamed_addr`, donc duplicable par unité de génération de code. Sans
/// l'attribut, ce test et onze autres de ce fichier tombent sous
/// `cargo test -p portcls --release` (le profil du pilote : opt-level 3 + LTO), pendant
/// que le profil `dev`, faute d'inlining, ne voit rien. **Rejouer ce fichier en
/// `--release` après toute retouche à `vtbl_of` ou à `TargetVtbl`.**
#[test]
fn la_vtable_attendue_a_une_seule_adresse() {
    let this = cible();
    let dans_objet: *const IMiniportTopologyVtbl =
        unsafe { common::vtbl_de::<IMiniportTopologyVtbl>(this) };
    let par_vtbl_of: *const IMiniportTopologyVtbl = portcls::topology::vtbl_of::<Cible>();
    let par_trait: *const IMiniportTopologyVtbl =
        <IMiniportTopologyVtbl as portcls::TargetVtbl<Cible>>::vtbl();

    assert!(
        ptr::eq(dans_objet, par_vtbl_of),
        "l'objet porte l'adresse de vtbl_of ({dans_objet:p} vs {par_vtbl_of:p})"
    );
    assert!(
        ptr::eq(dans_objet, par_trait),
        "le chemin du thunk voit la même adresse ({dans_objet:p} vs {par_trait:p})"
    );
    assert_eq!(common::release(this), 0);
}

#[test]
fn requete_nulle_est_un_parametre_invalide() {
    let slot = ITEM_COMPLET.0.Handler.expect("Handler");
    let status = unsafe { slot(ptr::null_mut()) };
    assert_eq!(status, STATUS_INVALID_PARAMETER);
}

#[test]
fn major_target_nul_est_une_requete_invalide_et_laisse_valuesize_intact() {
    let mut req = requete(&ITEM_COMPLET, ptr::null_mut(), KSPROPERTY_TYPE_GET);
    req.ValueSize = 1234;
    assert_eq!(
        appeler(&ITEM_COMPLET, &mut req),
        STATUS_INVALID_DEVICE_REQUEST
    );
    assert_eq!(req.ValueSize, 1234, "ValueSize intact en cas d'erreur");
}

#[test]
fn verbe_inconnu_est_une_requete_invalide() {
    let this = cible();
    for verbe in [0, KSPROPERTY_TYPE_SETSUPPORT] {
        let mut req = requete(&ITEM_COMPLET, this, verbe);
        req.ValueSize = 99;
        assert_eq!(
            appeler(&ITEM_COMPLET, &mut req),
            STATUS_INVALID_DEVICE_REQUEST,
            "verbe {verbe:#x}"
        );
        assert_eq!(req.ValueSize, 99, "ValueSize intact (verbe {verbe:#x})");
    }
    assert_eq!(common::release(this), 0);
}

#[test]
fn verbe_non_implemente_est_non_supporte_et_laisse_valuesize_intact() {
    let this = cible();
    let mut tampon = [0u8; 16];
    for verbe in [
        KSPROPERTY_TYPE_GET,
        KSPROPERTY_TYPE_SET,
        KSPROPERTY_TYPE_BASICSUPPORT,
    ] {
        let mut req = requete(&ITEM_MUET, this, verbe);
        avec_valeur(&mut req, &mut tampon);
        assert_eq!(
            appeler(&ITEM_MUET, &mut req),
            STATUS_NOT_SUPPORTED,
            "verbe {verbe:#x}"
        );
        assert_eq!(req.ValueSize, 16, "ValueSize intact (verbe {verbe:#x})");
    }
    assert_eq!(tampon, [0u8; 16], "un défaut du trait n'écrit rien");
    assert_eq!(common::release(this), 0);
}

#[test]
fn get_sans_tampon_est_un_depassement_et_rend_la_taille_requise() {
    let this = cible();
    // `Value` nul et `ValueSize` nulle : l'interrogation de taille de KS.
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_GET);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_BUFFER_OVERFLOW);
    assert_eq!(req.ValueSize, TAILLE_GET);
    assert_eq!(common::release(this), 0);
}

#[test]
fn get_avec_tampon_trop_petit_ne_touche_pas_au_tampon() {
    let this = cible();
    let mut tampon = [0xAAu8; 4];
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_BUFFER_TOO_SMALL);
    assert_eq!(req.ValueSize, TAILLE_GET, "la taille requise est rendue");
    assert_eq!(tampon, [0xAAu8; 4], "tampon inchangé");
    assert_eq!(common::release(this), 0);
}

#[test]
fn valuesize_maximale_avec_valeur_nulle_ne_construit_aucune_tranche() {
    let this = cible();
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_GET);
    req.ValueSize = ULONG::MAX;
    req.Value = ptr::null_mut();
    // Aucun plantage : `Value` nul ⇒ tranche vide, donc interrogation de taille.
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_BUFFER_OVERFLOW);
    assert_eq!(req.ValueSize, TAILLE_GET);
    assert_eq!(common::release(this), 0);
}

#[test]
fn confusion_de_type_arretee_par_la_garde_de_vtable() {
    let attendu = cible();
    let autre = intrus();

    // La preuve que la garde a quelque chose à mordre : les deux objets ont bien des
    // vtables d'adresses différentes, alors qu'elles ont le même type Rust.
    let vtbl_attendue = unsafe { common::vtbl_de::<IMiniportTopologyVtbl>(attendu) };
    let vtbl_autre = unsafe { common::vtbl_de::<IMiniportTopologyVtbl>(autre) };
    assert!(
        !ptr::eq(vtbl_attendue, vtbl_autre),
        "deux `T` distincts, deux vtables distinctes"
    );

    // Le gestionnaire est monomorphisé pour `Cible` ; on lui passe un `ComObject` d'un
    // autre type, comme le ferait une table d'automatisation mal câblée. Sans la garde,
    // ce serait un `inner::<_, Cible>` sur un `Intrus` : un écran bleu, pas un statut.
    let mut req = requete(&ITEM_COMPLET, autre, KSPROPERTY_TYPE_GET);
    req.ValueSize = 4321;
    assert_eq!(
        appeler(&ITEM_COMPLET, &mut req),
        STATUS_INVALID_DEVICE_REQUEST
    );
    assert_eq!(req.ValueSize, 4321, "ValueSize intact");

    // Le même appel sur le bon objet passe : la garde ne mord que la confusion.
    let mut req = requete(&ITEM_COMPLET, attendu, KSPROPERTY_TYPE_GET);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_BUFFER_OVERFLOW);
    assert_eq!(req.ValueSize, TAILLE_GET);

    assert_eq!(common::release(attendu), 0);
    assert_eq!(common::release(autre), 0);
}

// ---------------------------------------------------------------------------------
// Chemins nominaux.
// ---------------------------------------------------------------------------------

#[test]
fn get_avec_place_suffisante_ecrit_et_rend_la_taille() {
    let this = cible();
    let mut tampon = [0u8; 16];
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, TAILLE_GET);
    assert_eq!(&tampon[..8], &VALEUR_GET.to_ne_bytes());
    assert_eq!(&tampon[8..], &[0u8; 8], "rien au-delà de la taille requise");
    assert_eq!(common::release(this), 0);
}

#[test]
fn set_atteint_le_miniport_et_laisse_valuesize_intact() {
    let this = cible();
    let mut tampon = 0x1234_5678_u32.to_ne_bytes();
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_SET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 4, "SET ne touche pas à ValueSize");

    let cible_ref = unsafe { conduit_com::ComObject::<IMiniportTopologyVtbl, Cible>::inner(this) };
    assert_eq!(cible_ref.valeur.load(Ordering::SeqCst), 0x1234_5678);
    assert_eq!(cible_ref.sets.load(Ordering::SeqCst), 1);
    assert_eq!(common::release(this), 0);
}

#[test]
fn set_en_erreur_propage_le_statut_et_laisse_valuesize_intact() {
    let this = cible();
    let mut tampon = [0u8; 2];
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_SET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), ERREUR_SET);
    assert_eq!(req.ValueSize, 2);

    let cible_ref = unsafe { conduit_com::ComObject::<IMiniportTopologyVtbl, Cible>::inner(this) };
    assert_eq!(cible_ref.sets.load(Ordering::SeqCst), 0);
    assert_eq!(common::release(this), 0);
}

#[test]
fn basicsupport_rend_les_octets_ecrits_pas_la_taille_requise() {
    let this = cible();

    // Premier appel de KS : `sizeof(ULONG)` pour les seuls `AccessFlags`. Répondre
    // `STATUS_BUFFER_TOO_SMALL` ici casserait la négociation — d'où l'asymétrie entre
    // `get` (taille requise) et `basic_support` (octets écrits).
    let mut petit = [0u8; 4];
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_BASICSUPPORT);
    avec_valeur(&mut req, &mut petit);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 4, "octets écrits, pas taille requise");
    assert_eq!(
        petit,
        (KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET).to_ne_bytes()
    );

    // Second appel : la description complète.
    let mut grand = [0u8; 16];
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_BASICSUPPORT);
    avec_valeur(&mut req, &mut grand);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 6);
    assert_eq!(&grand[4..6], &7u16.to_ne_bytes());

    // Sans tampon du tout : toujours `STATUS_SUCCESS`, zéro octet écrit.
    let mut req = requete(&ITEM_COMPLET, this, KSPROPERTY_TYPE_BASICSUPPORT);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 0);

    assert_eq!(common::release(this), 0);
}

#[test]
fn basicsupport_prime_sur_get_dans_un_verbe_combine() {
    let this = cible();
    let mut tampon = [0u8; 16];
    let mut req = requete(
        &ITEM_COMPLET,
        this,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
    );
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_COMPLET, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 6, "c'est basic_support qui a répondu");
    assert_eq!(common::release(this), 0);
}

#[test]
fn instance_est_rendue_telle_quelle_et_vide_si_absente() {
    /// Gestionnaire qui recopie son `instance` dans la valeur.
    struct Echo;

    impl PropertyHandler<Cible> for Echo {
        fn get(req: &Request<'_, Cible>, value: &mut [u8]) -> Result<u32, NtStatus> {
            let n = req.instance.len();
            if let Some(place) = value.get_mut(..n) {
                place.copy_from_slice(req.instance);
            }
            Ok(n as u32)
        }
    }

    static ITEM_ECHO: SyncItem = SyncItem(property::item::<IMiniportTopologyVtbl, Cible, Echo>(
        &SET_TEST,
        ID,
        KSPROPERTY_TYPE_GET,
    ));

    let this = cible();
    let mut instance = *b"abcde";
    let mut tampon = [0u8; 8];

    let mut req = requete(&ITEM_ECHO, this, KSPROPERTY_TYPE_GET);
    req.Instance = instance.as_mut_ptr().cast();
    req.InstanceSize = instance.len() as ULONG;
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_ECHO, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 5);
    assert_eq!(&tampon[..5], b"abcde");

    // Instance nulle mais taille non nulle : tranche vide, pas de déréférencement.
    let mut req = requete(&ITEM_ECHO, this, KSPROPERTY_TYPE_GET);
    req.Instance = ptr::null_mut();
    req.InstanceSize = ULONG::MAX;
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_ECHO, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, 0, "instance vide");

    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Tampon désaligné : la sérialisation champ par champ doit produire les mêmes octets
// qu'à l'adresse alignée. Ce test tombe le jour où quelqu'un transtype vers un `*mut`
// de structure et écrit à travers.
// ---------------------------------------------------------------------------------

/// Tableau d'octets dont l'adresse de base est alignée sur 8 : `[u8; N]` n'a qu'un
/// alignement de 1, donc « aligné » ne voudrait rien dire sans ce `repr`.
#[repr(align(8))]
struct Aligne([u8; 80]);

#[test]
fn tampon_desaligne_donne_les_memes_octets_qu_aligne() {
    let this = cible();

    let mut ecritures = Vec::new();
    for decalage in [0usize, 1] {
        let mut tampon = Aligne([0u8; 80]);
        assert_eq!(
            tampon.0.as_ptr() as usize % 8,
            0,
            "la base du tableau est bien alignée sur 8"
        );
        let tranche = &mut tampon.0[decalage..];
        assert_eq!(
            tranche.as_ptr() as usize % 8,
            decalage,
            "décalage {decalage} : alignement voulu"
        );

        let mut req = requete(&ITEM_MULTI, this, KSPROPERTY_TYPE_GET);
        avec_valeur(&mut req, tranche);
        assert_eq!(
            appeler(&ITEM_MULTI, &mut req),
            STATUS_SUCCESS,
            "décalage {decalage}"
        );
        assert_eq!(req.ValueSize, TAILLE_MULTI as ULONG);
        ecritures.push(tranche[..TAILLE_MULTI].to_vec());
    }

    assert_eq!(
        ecritures[0], ecritures[1],
        "les octets écrits ne dépendent pas de l'alignement du tampon"
    );
    assert_eq!(&ecritures[0][0..4], &0xDEAD_BEEF_u32.to_ne_bytes());
    assert_eq!(
        &ecritures[0][8..16],
        &0x0F1E_2D3C_4B5A_6978_u64.to_ne_bytes()
    );

    assert_eq!(common::release(this), 0);
}
