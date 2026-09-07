//! Le gestionnaire `KSPROPERTY_JACK_DESCRIPTION` vu par un faux PortCls, sur le modèle de
//! `tests/audio.rs` : une `PCPROPERTY_REQUEST` bâtie à la main est passée au `Handler` du
//! `PCPROPERTY_ITEM` que `jack::jack_description_item` produit.
//!
//! Trois points que ces tests verrouillent en particulier :
//!
//! - **le décalage du `PinId`** : l'`Instance` de test met un `Reserved` non nul
//!   (`0xA5A5_A5A5`) derrière le `PinId`. Lire le mauvais `ULONG` ne rendrait donc pas
//!   « broche 0 » par accident mais une broche absurde, refusée ;
//! - **chaque champ à son décalage du golden** : la réponse est relue octet par octet,
//!   `KSMULTIPLE_ITEM` compris, aux neuf décalages que `layout.golden` mesure avec
//!   `cl.exe` ;
//! - **la cohérence entre le compte annoncé et les octets écrits** : `KSMULTIPLE_ITEM.Size`
//!   doit valoir `8 + Count * 28` **et** la taille que le thunk reporte dans `ValueSize`.

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
use portcls::portcls_sys::{
    EPcxConnectionType, EPcxGenLocation, EPcxGeoLocation, EPxcPortConnection, GUID, GUID_NULL,
    IMiniportTopologyVtbl, IUnknown, KSAUDIO_SPEAKER_STEREO, KSPROPERTY_TYPE_BASICSUPPORT,
    KSPROPERTY_TYPE_GET, KSPROPERTY_TYPE_SET, NTSTATUS, PCFILTER_DESCRIPTOR, PCPROPERTY_ITEM,
    PCPROPERTY_REQUEST, ULONG,
};
use portcls::{
    JACK_ACCESS_FLAGS, JackInfo, JackTrace, MiniportTopology, Pin, PortTopology, ResourceList,
    STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NOT_SUPPORTED, jack_description_item, new_topology_object,
};

// ---------------------------------------------------------------------------------
// Décalages et tailles, recopiés de portcls-sys/tests/layout.golden.
// ---------------------------------------------------------------------------------

/// `KSMULTIPLE_ITEM::Size`.
const MI_SIZE: usize = 0;
/// `KSMULTIPLE_ITEM::Count`.
const MI_COUNT: usize = 4;
/// `sizeof(KSMULTIPLE_ITEM)`.
const TAILLE_MULTIPLE_ITEM: usize = 8;
/// `KSJACK_DESCRIPTION::ChannelMapping`, derrière le `KSMULTIPLE_ITEM`.
const J_CHANNELMAPPING: usize = 8;
/// `KSJACK_DESCRIPTION::Color`.
const J_COLOR: usize = 12;
/// `KSJACK_DESCRIPTION::ConnectionType`.
const J_CONNECTIONTYPE: usize = 16;
/// `KSJACK_DESCRIPTION::GeoLocation`.
const J_GEOLOCATION: usize = 20;
/// `KSJACK_DESCRIPTION::GenLocation`.
const J_GENLOCATION: usize = 24;
/// `KSJACK_DESCRIPTION::PortConnection`.
const J_PORTCONNECTION: usize = 28;
/// `KSJACK_DESCRIPTION::IsConnected`.
const J_ISCONNECTED: usize = 32;
/// `sizeof(KSJACK_DESCRIPTION)`.
const TAILLE_JACK: usize = 28;
/// Réponse complète : `KSMULTIPLE_ITEM` + une prise.
const TAILLE_REPONSE: usize = TAILLE_MULTIPLE_ITEM + TAILLE_JACK;

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

// ---------------------------------------------------------------------------------
// Le faux miniport.
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour la
/// `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

/// Broches du faux filtre, comme un filtre de topologie Conduit.
const BROCHES: u32 = 2;
/// Broche à prise du sens rendu : l'endpoint est la **sortie**.
const BROCHE_JACK_RENDU: u32 = 1;
/// Broche à prise du sens capture : l'endpoint est l'**entrée**.
const BROCHE_JACK_CAPTURE: u32 = 0;
/// `Reserved` de `KSP_PIN`, non nul à dessein : lire le mauvais `ULONG` de l'instance
/// donnerait une broche inexistante, pas « 0 » par accident.
const RESERVED: u32 = 0xA5A5_A5A5;
/// Broche notée par la dernière trace quand le décodage a échoué.
const BROCHE_ERREUR: u32 = 0xDEAD;
/// Broche notée par la dernière trace pour une broche existante sans prise.
const BROCHE_SANS: u32 = 0xBEEF;

struct Faux {
    jack_pin: AtomicU32,
    mapping: AtomicU32,
    connecte: AtomicBool,
    /// Nombre d'appels à [`JackInfo::trace`].
    traces: AtomicU32,
    /// Broche de la dernière trace ([`BROCHE_ERREUR`] / [`BROCHE_SANS`] selon le cas).
    derniere_broche: AtomicU32,
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

impl JackInfo for Faux {
    fn pin_count(&self) -> u32 {
        BROCHES
    }

    fn jack_pin(&self) -> u32 {
        self.jack_pin.load(Ordering::SeqCst)
    }

    fn channel_mapping(&self) -> u32 {
        self.mapping.load(Ordering::SeqCst)
    }

    fn is_connected(&self) -> bool {
        self.connecte.load(Ordering::SeqCst)
    }

    fn trace(&self, trace: &JackTrace<'_>) {
        self.traces.fetch_add(1, Ordering::SeqCst);
        let broche = match trace.pin {
            Ok(Pin::Jack) => self.jack_pin(),
            Ok(Pin::Sans) => BROCHE_SANS,
            Err(_) => BROCHE_ERREUR,
        };
        self.derniere_broche.store(broche, Ordering::SeqCst);
    }
}

/// Faux miniport de rendu : prise sur la broche 1, cartographie stéréo, connecté.
fn faux_rendu() -> This {
    faux(BROCHE_JACK_RENDU, KSAUDIO_SPEAKER_STEREO, true)
}

fn faux(jack_pin: u32, mapping: u32, connecte: bool) -> This {
    new_topology_object(Faux {
        jack_pin: AtomicU32::new(jack_pin),
        mapping: AtomicU32::new(mapping),
        connecte: AtomicBool::new(connecte),
        traces: AtomicU32::new(0),
        derniere_broche: AtomicU32::new(BROCHE_ERREUR),
    })
    .into_raw()
}

fn interieur(this: This) -> &'static Faux {
    unsafe { conduit_com::ComObject::<IMiniportTopologyVtbl, Faux>::inner(this) }
}

// ---------------------------------------------------------------------------------
// Table d'automatisation de test.
// ---------------------------------------------------------------------------------

/// `PCPROPERTY_ITEM` contient un `*const GUID` et un pointeur de fonction : `Sync` déclaré
/// à la main pour la `static`, comme le pilote le fera pour sa vraie table de filtre.
struct SyncItem(PCPROPERTY_ITEM);
unsafe impl Sync for SyncItem {}

static ITEM_JACK: SyncItem = SyncItem(jack_description_item::<IMiniportTopologyVtbl, Faux>());

/// `Node` d'une propriété de filtre : `ULONG(-1)`. Le gestionnaire ne le lit pas, mais
/// c'est ce que PortCls met, et le mettre ici évite un test qui passerait par hasard.
const NODE: ULONG = ULONG::MAX;

// ---------------------------------------------------------------------------------
// Outillage.
// ---------------------------------------------------------------------------------

/// La queue de `KSP_PIN` telle que PortCls la laisse dans `Instance` : `PinId: ULONG`
/// **en premier**, puis `Reserved: ULONG`.
fn instance(pin: u32) -> [u8; 8] {
    let mut octets = [0u8; 8];
    octets[0..4].copy_from_slice(&pin.to_ne_bytes());
    octets[4..8].copy_from_slice(&RESERVED.to_ne_bytes());
    octets
}

fn requete(this: This, verb: ULONG) -> PCPROPERTY_REQUEST {
    PCPROPERTY_REQUEST {
        MajorTarget: this.cast(),
        MinorTarget: ptr::null_mut(),
        Node: NODE,
        PropertyItem: &ITEM_JACK.0,
        Verb: verb,
        InstanceSize: 0,
        Instance: ptr::null_mut(),
        ValueSize: 0,
        Value: ptr::null_mut(),
        Irp: ptr::null_mut(),
    }
}

fn appeler(req: &mut PCPROPERTY_REQUEST) -> NTSTATUS {
    let slot = ITEM_JACK
        .0
        .Handler
        .expect("Handler renseigné par jack_description_item");
    unsafe { slot(req) }
}

fn avec_instance(req: &mut PCPROPERTY_REQUEST, tampon: &mut [u8]) {
    req.Instance = tampon.as_mut_ptr().cast();
    req.InstanceSize = tampon.len() as ULONG;
}

fn avec_valeur(req: &mut PCPROPERTY_REQUEST, tampon: &mut [u8]) {
    req.Value = tampon.as_mut_ptr().cast();
    req.ValueSize = tampon.len() as ULONG;
}

/// `GET` sur une broche avec `place` octets de tampon : `(statut, ValueSize, octets)`.
fn lire(this: This, pin: u32, place: usize) -> (NTSTATUS, ULONG, Vec<u8>) {
    lire_dans(this, pin, vec![0u8; place])
}

/// Idem, avec un tampon fourni (pour les essais désalignés).
fn lire_dans(this: This, pin: u32, mut tampon: Vec<u8>) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut inst = instance(pin);
    let mut req = requete(this, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    if !tampon.is_empty() {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(&mut req);
    (status, req.ValueSize, tampon)
}

/// `BASICSUPPORT` avec un tampon de `place` octets : `(statut, ValueSize, octets)`.
fn basic_support(this: This, pin: u32, place: usize) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut inst = instance(pin);
    let mut tampon = vec![0u8; place];
    let mut req = requete(this, KSPROPERTY_TYPE_BASICSUPPORT);
    avec_instance(&mut req, &mut inst);
    if place > 0 {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(&mut req);
    (status, req.ValueSize, tampon)
}

fn u32_a(octets: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(octets[offset..offset + 4].try_into().unwrap())
}

fn i32_a(octets: &[u8], offset: usize) -> i32 {
    i32::from_ne_bytes(octets[offset..offset + 4].try_into().unwrap())
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

/// Relit une réponse de prise champ par champ, à chaque décalage du golden.
fn verifier_prise(octets: &[u8], mapping: u32, connecte: bool) {
    // KSMULTIPLE_ITEM : la taille annoncée est celle de l'en-tête plus une prise, et le
    // compte vaut exactement une prise — pas un canal, pas deux.
    assert_eq!(u32_a(octets, MI_SIZE), TAILLE_REPONSE as u32, "Size");
    assert_eq!(
        u32_a(octets, MI_COUNT),
        1,
        "Count : une prise, pas un canal"
    );
    assert_eq!(
        u32_a(octets, MI_SIZE) as usize,
        TAILLE_MULTIPLE_ITEM + u32_a(octets, MI_COUNT) as usize * TAILLE_JACK,
        "Size == sizeof(KSMULTIPLE_ITEM) + Count * sizeof(KSJACK_DESCRIPTION)"
    );

    // KSJACK_DESCRIPTION, champ par champ.
    assert_eq!(u32_a(octets, J_CHANNELMAPPING), mapping, "ChannelMapping");
    assert_eq!(u32_a(octets, J_COLOR), 0, "Color : noir, pas de connecteur");
    assert_eq!(
        u32_a(octets, J_CONNECTIONTYPE),
        EPcxConnectionType::eConnTypeUnknown as u32,
        "ConnectionType"
    );
    assert_eq!(
        u32_a(octets, J_GEOLOCATION),
        EPcxGeoLocation::eGeoLocNotApplicable as u32,
        "GeoLocation : la valeur prévue pour l'absence de prise physique"
    );
    assert_eq!(
        u32_a(octets, J_GENLOCATION),
        EPcxGenLocation::eGenLocOther as u32,
        "GenLocation"
    );
    assert_eq!(
        u32_a(octets, J_PORTCONNECTION),
        EPxcPortConnection::ePortConnUnknown as u32,
        "PortConnection"
    );
    assert_eq!(
        i32_a(octets, J_ISCONNECTED),
        i32::from(connecte),
        "IsConnected"
    );
}

// ---------------------------------------------------------------------------------
// Lecture.
// ---------------------------------------------------------------------------------

#[test]
fn reponse_d_une_broche_connectee() {
    let this = faux_rendu();
    let (status, taille, octets) = lire(this, BROCHE_JACK_RENDU, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    verifier_prise(&octets, KSAUDIO_SPEAKER_STEREO, true);
    assert_eq!(common::release(this), 0);
}

#[test]
fn reponse_d_une_broche_deconnectee() {
    let this = faux(BROCHE_JACK_RENDU, KSAUDIO_SPEAKER_STEREO, false);
    let (status, taille, octets) = lire(this, BROCHE_JACK_RENDU, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    verifier_prise(&octets, KSAUDIO_SPEAKER_STEREO, false);
    // La seule différence avec le câble connecté tient dans ce mot-là : c'est lui qui fait
    // apparaître l'endpoint sous « Périphériques déconnectés ».
    assert_eq!(i32_a(&octets, J_ISCONNECTED), 0);
    assert_eq!(common::release(this), 0);
}

/// Le sens capture : la prise est sur la broche 0, et `ChannelMapping` doit être **nul**
/// (« *For capture pins or for digital rendering pins, set this member to 0* »).
#[test]
fn le_sens_capture_a_sa_prise_sur_l_autre_broche_et_aucune_cartographie() {
    let this = faux(BROCHE_JACK_CAPTURE, 0, true);
    let (status, taille, octets) = lire(this, BROCHE_JACK_CAPTURE, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    verifier_prise(&octets, 0, true);

    // Et la broche 1, qui portait la prise au rendu, n'en porte pas ici.
    let (status, taille, octets) = lire(this, 1, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_MULTIPLE_ITEM as ULONG);
    assert_eq!(u32_a(&octets, MI_COUNT), 0);
    assert_eq!(common::release(this), 0);
}

/// Une broche existante sans prise : `STATUS_SUCCESS` et un `KSMULTIPLE_ITEM` **seul**,
/// comme le prescrit la documentation (et non une erreur).
#[test]
fn broche_sans_prise_rend_un_multiple_item_vide() {
    let this = faux_rendu();
    let (status, taille, octets) = lire(this, 0, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_MULTIPLE_ITEM as ULONG);
    assert_eq!(u32_a(&octets, MI_SIZE), TAILLE_MULTIPLE_ITEM as u32);
    assert_eq!(u32_a(&octets, MI_COUNT), 0);
    // Rien au-delà de l'en-tête : le tampon reste tel quel.
    assert!(
        octets[TAILLE_MULTIPLE_ITEM..].iter().all(|o| *o == 0),
        "aucune prise n'est écrite derrière l'en-tête"
    );
    assert_eq!(
        interieur(this).derniere_broche.load(Ordering::SeqCst),
        BROCHE_SANS
    );
    assert_eq!(common::release(this), 0);
}

/// Le décalage du `PinId`, verrouillé : c'est le **premier** `ULONG` de l'instance. Le
/// `Reserved` qui le suit vaut `0xA5A5_A5A5` ; lire le mauvais mot donnerait une broche
/// inexistante, pas « 0 » par accident.
#[test]
fn le_pin_id_est_le_premier_ulong_de_l_instance() {
    let this = faux_rendu();
    let me = interieur(this);

    let (status, _, _) = lire(this, BROCHE_JACK_RENDU, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        me.derniere_broche.load(Ordering::SeqCst),
        BROCHE_JACK_RENDU,
        "la trace annonce la broche à prise : c'est ce qu'on relit en machine"
    );
    assert_eq!(me.traces.load(Ordering::SeqCst), 1);

    // Le mot suivant, s'il était pris pour le `PinId`, désignerait la broche
    // `0xA5A5_A5A5` : bien au-delà des deux broches du filtre, donc rejetée. La preuve que
    // le test ne passe pas par hasard.
    assert_eq!(
        Pin::decode(&RESERVED.to_ne_bytes(), BROCHES, BROCHE_JACK_RENDU),
        Err(STATUS_INVALID_PARAMETER)
    );

    assert_eq!(common::release(this), 0);
}

#[test]
fn broche_inexistante_est_un_parametre_invalide() {
    let this = faux_rendu();
    let (status, taille, _) = lire(this, BROCHES, TAILLE_REPONSE);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert_eq!(
        taille, TAILLE_REPONSE as ULONG,
        "ValueSize intact en cas d'erreur"
    );
    assert_eq!(
        interieur(this).derniere_broche.load(Ordering::SeqCst),
        BROCHE_ERREUR
    );
    assert_eq!(common::release(this), 0);
}

/// Instance absente ou tronquée : requête mal formée, statut **différent** de celui d'une
/// broche inexistante — c'est la distinction que fait l'exemple MSVAD.
#[test]
fn instance_absente_ou_tronquee_est_une_requete_invalide() {
    let this = faux_rendu();

    let mut tampon = [0u8; TAILLE_REPONSE];
    let mut req = requete(this, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&mut req), STATUS_INVALID_DEVICE_REQUEST);
    assert_eq!(req.ValueSize, TAILLE_REPONSE as ULONG);

    let mut inst = [0u8; 3];
    let mut req = requete(this, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&mut req), STATUS_INVALID_DEVICE_REQUEST);

    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Négociation de taille.
// ---------------------------------------------------------------------------------

/// Les trois issues de la négociation : interrogation de taille, tampon trop court,
/// tampon suffisant. Ce sont celles que l'exemple MSVAD écrit à la main et que le thunk de
/// `property.rs` déduit de la taille requise.
#[test]
fn negociation_de_taille_de_la_lecture() {
    let this = faux_rendu();

    // `ValueSize == 0` : interrogation de taille.
    let (status, taille, _) = lire(this, BROCHE_JACK_RENDU, 0);
    assert_eq!(status, STATUS_BUFFER_OVERFLOW);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);

    // Un octet de trop peu : refus, mais la taille requise est rendue.
    let (status, taille, octets) = lire(this, BROCHE_JACK_RENDU, TAILLE_REPONSE - 1);
    assert_eq!(status, STATUS_BUFFER_TOO_SMALL);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    assert!(
        octets.iter().all(|o| *o == 0),
        "rien n'est écrit dans un tampon trop court"
    );

    // La place exacte, puis davantage : les deux réussissent.
    for place in [TAILLE_REPONSE, TAILLE_REPONSE + 16] {
        let (status, taille, octets) = lire(this, BROCHE_JACK_RENDU, place);
        assert_eq!(status, STATUS_SUCCESS, "place {place}");
        assert_eq!(taille, TAILLE_REPONSE as ULONG, "place {place}");
        verifier_prise(&octets, KSAUDIO_SPEAKER_STEREO, true);
        assert!(
            octets[TAILLE_REPONSE..].iter().all(|o| *o == 0),
            "rien n'est écrit au-delà de la réponse"
        );
    }

    // Même chose pour la réponse vide : huit octets suffisent, sept non.
    let (status, taille, _) = lire(this, 0, TAILLE_MULTIPLE_ITEM - 1);
    assert_eq!(status, STATUS_BUFFER_TOO_SMALL);
    assert_eq!(taille, TAILLE_MULTIPLE_ITEM as ULONG);

    assert_eq!(common::release(this), 0);
}

/// Le compte annoncé par `KSMULTIPLE_ITEM` et les octets réellement écrits s'accordent,
/// dans les deux cas et quelle que soit la place offerte.
#[test]
fn le_compte_annonce_et_les_octets_ecrits_s_accordent() {
    let this = faux_rendu();
    for (broche, prises) in [(BROCHE_JACK_RENDU, 1u32), (0, 0)] {
        let attendu = TAILLE_MULTIPLE_ITEM + prises as usize * TAILLE_JACK;
        for place in [attendu, attendu + 64] {
            let (status, taille, octets) = lire(this, broche, place);
            assert_eq!(status, STATUS_SUCCESS, "broche {broche}, place {place}");
            assert_eq!(taille as usize, attendu, "ValueSize rapporté");
            assert_eq!(u32_a(&octets, MI_COUNT), prises, "Count");
            assert_eq!(u32_a(&octets, MI_SIZE) as usize, attendu, "Size");
            // Le contrat de la documentation, écrit tel quel.
            assert_eq!(
                u32_a(&octets, MI_SIZE) as usize,
                TAILLE_MULTIPLE_ITEM + u32_a(&octets, MI_COUNT) as usize * TAILLE_JACK
            );
            // Et les octets au-delà de ce que `Size` annonce n'ont pas été touchés.
            assert!(
                octets[attendu..].iter().all(|o| *o == 0),
                "rien au-delà de Size"
            );
        }
    }
    assert_eq!(common::release(this), 0);
}

/// Sérialisation champ par champ : les mêmes octets à l'adresse `+1` qu'à l'adresse
/// alignée. Un transtypage vers un `*mut KSJACK_DESCRIPTION` ne saurait le garantir.
#[test]
fn reponse_desalignee_donne_les_memes_octets_qu_alignee() {
    let this = faux_rendu();
    let (_, _, aligne) = lire(this, BROCHE_JACK_RENDU, TAILLE_REPONSE);

    let mut brut = vec![0u8; TAILLE_REPONSE + 1];
    let mut inst = instance(BROCHE_JACK_RENDU);
    let mut req = requete(this, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    req.Value = unsafe { brut.as_mut_ptr().add(1) }.cast();
    req.ValueSize = TAILLE_REPONSE as ULONG;
    assert_eq!(appeler(&mut req), STATUS_SUCCESS);

    assert_eq!(&brut[1..=TAILLE_REPONSE], &aligne[..]);
    assert_eq!(brut[0], 0, "rien n'est écrit avant le tampon");
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// BASICSUPPORT.
// ---------------------------------------------------------------------------------

/// Les trois paliers : moins de 4 octets, les seuls `AccessFlags`, la description
/// complète. Aucun membre : la valeur est de taille variable, un `PropTypeSet` nul le dit.
#[test]
fn basicsupport_aux_trois_paliers() {
    let this = faux_rendu();

    // Palier 0 : pas même un `ULONG`.
    for place in [0usize, 1, 3] {
        let (status, _, _) = basic_support(this, BROCHE_JACK_RENDU, place);
        assert_eq!(status, STATUS_BUFFER_TOO_SMALL, "place {place}");
    }

    // Palier 1 : `sizeof(ULONG)`, le premier appel de KS.
    for place in [4usize, TAILLE_DESCRIPTION - 1] {
        let (status, taille, octets) = basic_support(this, BROCHE_JACK_RENDU, place);
        assert_eq!(status, STATUS_SUCCESS, "place {place}");
        assert_eq!(taille, 4, "place {place} : quatre octets écrits");
        assert_eq!(u32_a(&octets, D_ACCESSFLAGS), JACK_ACCESS_FLAGS);
        assert!(
            octets[4..].iter().all(|o| *o == 0),
            "place {place} : rien au-delà des AccessFlags"
        );
    }

    // Palier 2 : la description complète, et rien de plus (aucun membre).
    for place in [TAILLE_DESCRIPTION, TAILLE_DESCRIPTION + 32] {
        let (status, taille, octets) = basic_support(this, BROCHE_JACK_RENDU, place);
        assert_eq!(status, STATUS_SUCCESS, "place {place}");
        assert_eq!(taille, TAILLE_DESCRIPTION as ULONG, "place {place}");
        assert_eq!(u32_a(&octets, D_ACCESSFLAGS), JACK_ACCESS_FLAGS);
        assert_eq!(
            u32_a(&octets, D_DESCRIPTIONSIZE),
            TAILLE_DESCRIPTION as u32,
            "DescriptionSize : il n'y a pas de palier au-delà"
        );
        // `PropTypeSet` nul : l'équivalent du `VT_ILLEGAL` de SYSVAD, pour une valeur de
        // taille variable qu'aucune `VARENUM` ne décrit.
        assert_eq!(
            &octets[D_PROPTYPESET_SET..D_PROPTYPESET_SET + 16],
            &guid_octets(&GUID_NULL)[..],
            "PropTypeSet.Set == GUID_NULL"
        );
        assert_eq!(u32_a(&octets, D_PROPTYPESET_ID), 0, "PropTypeSet.Id");
        assert_eq!(u32_a(&octets, D_PROPTYPESET_FLAGS), 0, "PropTypeSet.Flags");
        assert_eq!(
            u32_a(&octets, D_MEMBERSLISTCOUNT),
            0,
            "MembersListCount : aucun membre"
        );
        assert_eq!(u32_a(&octets, D_RESERVED), 0, "Reserved");
        assert!(
            octets[TAILLE_DESCRIPTION..].iter().all(|o| *o == 0),
            "place {place} : rien au-delà de la description"
        );
    }

    assert_eq!(common::release(this), 0);
}

/// `BASICSUPPORT` valide la broche **avant** le verbe, comme l'exemple MSVAD : décrire la
/// propriété d'une broche inexistante n'a pas de sens.
#[test]
fn basicsupport_valide_la_broche_avant_de_decrire() {
    let this = faux_rendu();
    let (status, _, octets) = basic_support(this, BROCHES, TAILLE_DESCRIPTION);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert!(octets.iter().all(|o| *o == 0), "rien n'est écrit");

    // Une broche existante sans prise, elle, est décrite : la description ne parle pas de
    // la prise, seulement de la propriété.
    let (status, taille, _) = basic_support(this, 0, TAILLE_DESCRIPTION);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_DESCRIPTION as ULONG);

    assert_eq!(common::release(this), 0);
}

#[test]
fn basicsupport_desaligne_donne_les_memes_octets_qu_aligne() {
    let this = faux_rendu();
    let (_, _, aligne) = basic_support(this, BROCHE_JACK_RENDU, TAILLE_DESCRIPTION);

    let mut brut = vec![0u8; TAILLE_DESCRIPTION + 1];
    let mut inst = instance(BROCHE_JACK_RENDU);
    let mut req = requete(this, KSPROPERTY_TYPE_BASICSUPPORT);
    avec_instance(&mut req, &mut inst);
    req.Value = unsafe { brut.as_mut_ptr().add(1) }.cast();
    req.ValueSize = TAILLE_DESCRIPTION as ULONG;
    assert_eq!(appeler(&mut req), STATUS_SUCCESS);

    assert_eq!(&brut[1..=TAILLE_DESCRIPTION], &aligne[..]);
    assert_eq!(brut[0], 0);
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Lecture seule, et la table.
// ---------------------------------------------------------------------------------

/// Le jack ne s'écrit pas : `SET` est refusé, et le bit n'est pas dans les `Flags` de
/// l'entrée de table (PortCls filtre déjà dessus ; le refus est la seconde ligne).
#[test]
fn le_jack_est_en_lecture_seule() {
    let this = faux_rendu();
    let mut inst = instance(BROCHE_JACK_RENDU);
    let mut tampon = [0u8; TAILLE_REPONSE];
    let mut req = requete(this, KSPROPERTY_TYPE_SET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&mut req), STATUS_NOT_SUPPORTED);

    assert_eq!(ITEM_JACK.0.Flags, JACK_ACCESS_FLAGS);
    assert_eq!(ITEM_JACK.0.Flags & KSPROPERTY_TYPE_SET, 0);
    assert_eq!(ITEM_JACK.0.Flags & KSPROPERTY_TYPE_GET, KSPROPERTY_TYPE_GET);
    assert_eq!(
        ITEM_JACK.0.Flags & KSPROPERTY_TYPE_BASICSUPPORT,
        KSPROPERTY_TYPE_BASICSUPPORT
    );
    assert_eq!(common::release(this), 0);
}

/// L'entrée de table désigne bien `KSPROPSETID_Jack` / `KSPROPERTY_JACK_DESCRIPTION`.
#[test]
fn l_entree_de_table_designe_le_jeu_et_l_identifiant_attendus() {
    let set = unsafe { *ITEM_JACK.0.Set };
    assert_eq!(
        guid_octets(&set),
        guid_octets(&portcls::portcls_sys::KSPROPSETID_Jack)
    );
    assert_eq!(
        ITEM_JACK.0.Id,
        portcls::portcls_sys::KSPROPERTY_JACK::KSPROPERTY_JACK_DESCRIPTION as u32
    );
    assert!(ITEM_JACK.0.Handler.is_some());
}

/// La garde de vtable de `property.rs` protège aussi ce gestionnaire : un `MajorTarget`
/// qui n'est pas un `ComObject<IMiniportTopologyVtbl, Faux>` est refusé.
#[test]
fn un_major_target_etranger_est_refuse() {
    let mut req = requete(ptr::null_mut(), KSPROPERTY_TYPE_GET);
    assert_eq!(appeler(&mut req), STATUS_INVALID_DEVICE_REQUEST);

    // Un objet quelconque dont le premier mot n'est pas notre vtable.
    let mut faux_vtbl: *const u8 = ptr::null();
    let mut req = requete(ptr::from_mut(&mut faux_vtbl).cast(), KSPROPERTY_TYPE_GET);
    assert_eq!(appeler(&mut req), STATUS_INVALID_DEVICE_REQUEST);
}
