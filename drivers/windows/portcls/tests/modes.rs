//! Le gestionnaire `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` vu par un faux PortCls, sur le
//! modèle de `tests/jack.rs` : une `PCPROPERTY_REQUEST` bâtie à la main est passée au
//! `Handler` du `PCPROPERTY_ITEM` que `modes::signal_processing_modes_item` produit.
//!
//! Quatre points que ces tests verrouillent en particulier :
//!
//! - **le décalage du `PinId`** : l'`Instance` de test met un `Reserved` non nul
//!   (`0xA5A5_A5A5`) derrière le `PinId`. Lire le mauvais `ULONG` ne rendrait donc pas
//!   « broche 0 » par accident mais une broche absurde, refusée ;
//! - **le GUID rendu est `AUDIO_SIGNALPROCESSINGMODE_DEFAULT`**, relu octet par octet.
//!   `DEFAULT` et `RAW` se ressemblent à la relecture, et déclarer le mauvais serait une
//!   panne parfaitement muette : la propriété répondrait, le moteur audio lirait un mode
//!   que le pilote ne sert pas ;
//! - **la broche bridge répond `Count = 0`** et non une erreur, comme le prescrit la
//!   documentation (« *For loopback or bridge pins the audio driver should still support
//!   the property, but return a KSMULTIPLE_ITEM structure with its Count parameter set to
//!   zero* ») ;
//! - **la garde de vtable sépare les deux sens** : l'entrée monomorphisée sur le miniport
//!   de rendu, appelée sur un miniport de capture, est refusée. C'est ce qui rendrait le
//!   pilote muet d'un côté si les deux sens partageaient une table.

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
    AUDIO_SIGNALPROCESSINGMODE_DEFAULT, AUDIO_SIGNALPROCESSINGMODE_RAW, GUID, GUID_NULL,
    IMiniportWaveRTVtbl, IUnknown, KSDATAFORMAT, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET,
    KSPROPERTY_TYPE_SET, NTSTATUS, PCFILTER_DESCRIPTOR, PCPROPERTY_ITEM, PCPROPERTY_REQUEST, ULONG,
};
use portcls::{
    MODES_ACCESS_FLAGS, MiniportWaveRT, ModePin, ModesTrace, PortWaveRT, PortWaveRTStream,
    ResourceList, STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NOT_SUPPORTED, SignalModes, StreamObject, new_wavert_object,
    signal_processing_modes_item,
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
/// `sizeof(GUID)`.
const TAILLE_GUID: usize = 16;
/// Le GUID de mode, derrière le `KSMULTIPLE_ITEM`.
const M_MODE: usize = TAILLE_MULTIPLE_ITEM;
/// Réponse complète : `KSMULTIPLE_ITEM` + un mode.
const TAILLE_REPONSE: usize = TAILLE_MULTIPLE_ITEM + TAILLE_GUID;

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
// Les deux faux miniports wave : même code, deux **types** distincts, comme
// `conduit_kmd::wave::WaveRender` et `WaveCapture`. C'est cette dualité qui permet
// d'exercer la garde de vtable.
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour la
/// `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

/// Broches d'un filtre wave Conduit.
const BROCHES: u32 = 2;
/// Broche de flux du sens rendu : le lecteur écrit sur l'**entrée**.
const BROCHE_FLUX_RENDU: u32 = 0;
/// Broche de flux du sens capture : l'enregistreur lit sur la **sortie**.
const BROCHE_FLUX_CAPTURE: u32 = 1;
/// `Reserved` de `KSP_PIN`, non nul à dessein.
const RESERVED: u32 = 0xA5A5_A5A5;
/// Broche notée par la dernière trace quand le décodage a échoué.
const BROCHE_ERREUR: u32 = 0xDEAD;
/// Broche notée par la dernière trace pour une broche existante sans mode.
const BROCHE_SANS: u32 = 0xBEEF;

/// L'état commun aux deux faux miniports : la broche de flux et le compteur de traces.
struct Etat {
    broche_flux: u32,
    traces: AtomicU32,
    derniere_broche: AtomicU32,
}

impl Etat {
    fn new(broche_flux: u32) -> Self {
        Self {
            broche_flux,
            traces: AtomicU32::new(0),
            derniere_broche: AtomicU32::new(BROCHE_ERREUR),
        }
    }

    fn noter(&self, trace: &ModesTrace<'_>) {
        self.traces.fetch_add(1, Ordering::SeqCst);
        let broche = match trace.pin {
            Ok(ModePin::Flux) => self.broche_flux,
            Ok(ModePin::Sans) => BROCHE_SANS,
            Err(_) => BROCHE_ERREUR,
        };
        self.derniere_broche.store(broche, Ordering::SeqCst);
    }
}

/// Deux faux miniports pour le prix d'un : la macro engendre le type, son
/// `MiniportWaveRT` (des méthodes minimales, jamais appelées ici) et son `SignalModes`.
macro_rules! faux_miniport {
    ($nom:ident) => {
        struct $nom(Etat);

        impl MiniportWaveRT for $nom {
            fn init(
                &self,
                _adapter: Option<ComRef<IUnknown>>,
                _resources: ResourceList,
                _port: PortWaveRT,
            ) -> NtStatus {
                STATUS_SUCCESS
            }

            fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
                &DESCRIPTION.0
            }

            fn new_stream(
                &self,
                _port_stream: PortWaveRTStream,
                _pin: u32,
                _capture: bool,
                _format: &KSDATAFORMAT,
            ) -> Result<StreamObject, NtStatus> {
                Err(STATUS_INVALID_PARAMETER)
            }
        }

        impl SignalModes for $nom {
            fn pin_count(&self) -> u32 {
                BROCHES
            }

            fn streaming_pin(&self) -> u32 {
                self.0.broche_flux
            }

            fn trace(&self, trace: &ModesTrace<'_>) {
                self.0.noter(trace);
            }
        }
    };
}

faux_miniport!(FauxRendu);
faux_miniport!(FauxCapture);

fn faux_rendu() -> This {
    new_wavert_object(FauxRendu(Etat::new(BROCHE_FLUX_RENDU))).into_raw()
}

fn faux_capture() -> This {
    new_wavert_object(FauxCapture(Etat::new(BROCHE_FLUX_CAPTURE))).into_raw()
}

fn interieur_rendu(this: This) -> &'static FauxRendu {
    unsafe { conduit_com::ComObject::<IMiniportWaveRTVtbl, FauxRendu>::inner(this) }
}

// ---------------------------------------------------------------------------------
// Tables d'automatisation de test : une par sens, comme dans le pilote.
// ---------------------------------------------------------------------------------

/// `PCPROPERTY_ITEM` contient un `*const GUID` et un pointeur de fonction : `Sync` déclaré
/// à la main pour la `static`, comme le pilote le fait pour ses tables de filtre.
struct SyncItem(PCPROPERTY_ITEM);
unsafe impl Sync for SyncItem {}

static ITEM_RENDU: SyncItem =
    SyncItem(signal_processing_modes_item::<IMiniportWaveRTVtbl, FauxRendu>());
static ITEM_CAPTURE: SyncItem = SyncItem(signal_processing_modes_item::<
    IMiniportWaveRTVtbl,
    FauxCapture,
>());

/// `Node` d'une propriété de filtre : `ULONG(-1)`.
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
    let slot = item
        .0
        .Handler
        .expect("Handler renseigné par signal_processing_modes_item");
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
fn lire(this: This, item: &'static SyncItem, pin: u32, place: usize) -> (NTSTATUS, ULONG, Vec<u8>) {
    lire_dans(this, item, pin, vec![0u8; place])
}

/// Idem, avec un tampon fourni (pour les essais désalignés).
fn lire_dans(
    this: This,
    item: &'static SyncItem,
    pin: u32,
    mut tampon: Vec<u8>,
) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut inst = instance(pin);
    let mut req = requete(this, item, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    if !tampon.is_empty() {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(item, &mut req);
    (status, req.ValueSize, tampon)
}

/// `BASICSUPPORT` avec un tampon de `place` octets : `(statut, ValueSize, octets)`.
fn basic_support(
    this: This,
    item: &'static SyncItem,
    pin: u32,
    place: usize,
) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut inst = instance(pin);
    let mut tampon = vec![0u8; place];
    let mut req = requete(this, item, KSPROPERTY_TYPE_BASICSUPPORT);
    avec_instance(&mut req, &mut inst);
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
    let mut v = Vec::with_capacity(TAILLE_GUID);
    v.extend_from_slice(&guid.Data1.to_ne_bytes());
    v.extend_from_slice(&guid.Data2.to_ne_bytes());
    v.extend_from_slice(&guid.Data3.to_ne_bytes());
    v.extend_from_slice(&guid.Data4);
    v
}

// ---------------------------------------------------------------------------------
// Lecture.
// ---------------------------------------------------------------------------------

/// La broche de flux du rendu rend un mode, et c'est `DEFAULT`.
#[test]
fn la_broche_de_flux_rend_le_mode_par_defaut() {
    let this = faux_rendu();
    let (status, taille, octets) = lire(this, &ITEM_RENDU, BROCHE_FLUX_RENDU, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    assert_eq!(u32_a(&octets, MI_SIZE), TAILLE_REPONSE as u32, "Size");
    assert_eq!(u32_a(&octets, MI_COUNT), 1, "Count : un mode");
    assert_eq!(
        u32_a(&octets, MI_SIZE) as usize,
        TAILLE_MULTIPLE_ITEM + u32_a(&octets, MI_COUNT) as usize * TAILLE_GUID,
        "Size == sizeof(KSMULTIPLE_ITEM) + Count * sizeof(GUID)"
    );
    assert_eq!(
        &octets[M_MODE..M_MODE + TAILLE_GUID],
        guid_octets(&AUDIO_SIGNALPROCESSINGMODE_DEFAULT).as_slice(),
        "le mode déclaré est DEFAULT"
    );
    assert_ne!(
        &octets[M_MODE..M_MODE + TAILLE_GUID],
        guid_octets(&AUDIO_SIGNALPROCESSINGMODE_RAW).as_slice(),
        "et surtout pas RAW, que le pilote ne sert pas"
    );
    assert_eq!(common::release(this), 0);
}

/// Le sens capture a sa broche de flux sur l'autre broche, et la broche de rendu n'a
/// aucun mode chez lui.
#[test]
fn le_sens_capture_a_sa_broche_de_flux_sur_l_autre_broche() {
    let this = faux_capture();
    let (status, taille, octets) = lire(this, &ITEM_CAPTURE, BROCHE_FLUX_CAPTURE, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    assert_eq!(u32_a(&octets, MI_COUNT), 1);

    let (status, taille, octets) = lire(this, &ITEM_CAPTURE, 0, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_MULTIPLE_ITEM as ULONG);
    assert_eq!(u32_a(&octets, MI_COUNT), 0);
    assert_eq!(common::release(this), 0);
}

/// Une broche existante sans mode — chez nous la broche bridge : `STATUS_SUCCESS` et un
/// `KSMULTIPLE_ITEM` **seul**, `Count = 0`, rien au-delà.
#[test]
fn broche_bridge_rend_un_multiple_item_vide() {
    let this = faux_rendu();
    let (status, taille, octets) = lire(this, &ITEM_RENDU, 1, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_MULTIPLE_ITEM as ULONG);
    assert_eq!(u32_a(&octets, MI_SIZE), TAILLE_MULTIPLE_ITEM as u32);
    assert_eq!(u32_a(&octets, MI_COUNT), 0);
    assert!(
        octets[TAILLE_MULTIPLE_ITEM..].iter().all(|o| *o == 0),
        "rien n'est écrit au-delà de l'en-tête"
    );
    // La trace a bien vu une broche existante sans mode, et non une erreur.
    let etat = &interieur_rendu(this).0;
    assert_eq!(etat.traces.load(Ordering::SeqCst), 1);
    assert_eq!(etat.derniere_broche.load(Ordering::SeqCst), BROCHE_SANS);
    assert_eq!(common::release(this), 0);
}

/// Une broche inexistante est un paramètre faux ; une instance tronquée, une requête mal
/// formée. Les deux statuts se distinguent, comme pour le jack.
#[test]
fn broche_inexistante_et_instance_tronquee_ont_deux_statuts_differents() {
    let this = faux_rendu();
    let (status, _, _) = lire(this, &ITEM_RENDU, BROCHES, TAILLE_REPONSE);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    let (status, _, _) = lire(this, &ITEM_RENDU, u32::MAX, TAILLE_REPONSE);
    assert_eq!(status, STATUS_INVALID_PARAMETER);

    // Instance de trois octets : moins que le `PinId`.
    let mut inst = [0u8; 3];
    let mut tampon = vec![0u8; TAILLE_REPONSE];
    let mut req = requete(this, &ITEM_RENDU, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(
        appeler(&ITEM_RENDU, &mut req),
        STATUS_INVALID_DEVICE_REQUEST
    );

    // Aucune instance du tout : idem.
    let mut req = requete(this, &ITEM_RENDU, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(
        appeler(&ITEM_RENDU, &mut req),
        STATUS_INVALID_DEVICE_REQUEST
    );
    assert_eq!(common::release(this), 0);
}

/// Interrogation de taille (`Value` nul) : `STATUS_BUFFER_OVERFLOW` et la taille requise
/// dans `ValueSize`. C'est le premier appel que fait KS.
#[test]
fn interrogation_de_taille_rend_buffer_overflow_et_la_taille() {
    let this = faux_rendu();
    let (status, taille, _) = lire(this, &ITEM_RENDU, BROCHE_FLUX_RENDU, 0);
    assert_eq!(status, STATUS_BUFFER_OVERFLOW);
    assert_eq!(taille, TAILLE_REPONSE as ULONG);
    // Même chose sur la broche sans mode : 8 octets.
    let (status, taille, _) = lire(this, &ITEM_RENDU, 1, 0);
    assert_eq!(status, STATUS_BUFFER_OVERFLOW);
    assert_eq!(taille, TAILLE_MULTIPLE_ITEM as ULONG);
    assert_eq!(common::release(this), 0);
}

/// Tampon trop court : `STATUS_BUFFER_TOO_SMALL`, la taille requise, et **rien d'écrit**.
/// Une réponse à demi écrite annoncerait un mode absent du tampon.
#[test]
fn tampon_trop_court_n_ecrit_rien() {
    let this = faux_rendu();
    for place in [1, TAILLE_MULTIPLE_ITEM, TAILLE_REPONSE - 1] {
        let (status, taille, octets) = lire(this, &ITEM_RENDU, BROCHE_FLUX_RENDU, place);
        assert_eq!(status, STATUS_BUFFER_TOO_SMALL, "place {place}");
        assert_eq!(taille, TAILLE_REPONSE as ULONG, "place {place}");
        assert!(
            octets.iter().all(|o| *o == 0),
            "place {place} : rien ne doit être écrit"
        );
    }
    assert_eq!(common::release(this), 0);
}

/// Un tampon désaligné se remplit comme un autre : la sérialisation se fait champ par
/// champ, jamais par transtypage vers un pointeur de structure.
#[test]
fn tampon_desaligne() {
    let this = faux_rendu();
    let mut brut = [0u8; TAILLE_REPONSE + 8];
    // Décale d'un octet : l'adresse de départ est impaire.
    let decale = &mut brut[1..1 + TAILLE_REPONSE];
    let mut inst = instance(BROCHE_FLUX_RENDU);
    let mut req = requete(this, &ITEM_RENDU, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, decale);
    assert_eq!(appeler(&ITEM_RENDU, &mut req), STATUS_SUCCESS);
    assert_eq!(req.ValueSize, TAILLE_REPONSE as ULONG);
    assert_eq!(u32_a(&brut[1..], MI_COUNT), 1);
    assert_eq!(
        &brut[1 + M_MODE..1 + M_MODE + TAILLE_GUID],
        guid_octets(&AUDIO_SIGNALPROCESSINGMODE_DEFAULT).as_slice()
    );
    assert_eq!(common::release(this), 0);
}

/// `SET` n'est pas implémenté : la propriété est en lecture seule.
#[test]
fn set_est_refuse() {
    let this = faux_rendu();
    let mut inst = instance(BROCHE_FLUX_RENDU);
    let mut tampon = vec![0u8; TAILLE_REPONSE];
    let mut req = requete(this, &ITEM_RENDU, KSPROPERTY_TYPE_SET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_RENDU, &mut req), STATUS_NOT_SUPPORTED);
    // Et les `Flags` de l'entrée ne déclarent pas `SET` : PortCls ne nous appellerait même
    // pas.
    assert_eq!(ITEM_RENDU.0.Flags, MODES_ACCESS_FLAGS);
    assert_eq!(ITEM_RENDU.0.Flags & KSPROPERTY_TYPE_SET, 0);
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Support de base.
// ---------------------------------------------------------------------------------

/// Les deux paliers de `BASICSUPPORT` : `sizeof(ULONG)` puis la description complète.
#[test]
fn basic_support_en_paliers() {
    let this = faux_rendu();

    // Premier appel de KS : quatre octets, les seuls `AccessFlags`.
    let (status, taille, octets) = basic_support(this, &ITEM_RENDU, BROCHE_FLUX_RENDU, 4);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 4);
    assert_eq!(u32_a(&octets, D_ACCESSFLAGS), MODES_ACCESS_FLAGS);

    // Second appel : les 40 octets de la `KSPROPERTY_DESCRIPTION`.
    let (status, taille, octets) =
        basic_support(this, &ITEM_RENDU, BROCHE_FLUX_RENDU, TAILLE_DESCRIPTION);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, TAILLE_DESCRIPTION as ULONG);
    assert_eq!(u32_a(&octets, D_ACCESSFLAGS), MODES_ACCESS_FLAGS);
    assert_eq!(
        u32_a(&octets, D_DESCRIPTIONSIZE),
        TAILLE_DESCRIPTION as u32,
        "il n'y a pas de palier au-delà"
    );
    // Valeur de taille variable : `PropTypeSet` nul et **aucun** membre — une plage de
    // `KSPROPERTY_MEMBERSHEADER` ne saurait décrire « zéro ou un GUID ».
    assert_eq!(
        &octets[D_PROPTYPESET_SET..D_PROPTYPESET_SET + TAILLE_GUID],
        guid_octets(&GUID_NULL).as_slice()
    );
    assert_eq!(u32_a(&octets, D_PROPTYPESET_ID), 0);
    assert_eq!(u32_a(&octets, D_PROPTYPESET_FLAGS), 0);
    assert_eq!(u32_a(&octets, D_MEMBERSLISTCOUNT), 0);
    assert_eq!(u32_a(&octets, D_RESERVED), 0);
    assert_eq!(common::release(this), 0);
}

/// Moins de quatre octets : `STATUS_BUFFER_TOO_SMALL`. Et une broche inexistante est
/// refusée **avant** le verbe, comme dans l'exemple MSVAD.
#[test]
fn basic_support_refuse_un_tampon_minuscule_et_une_broche_absente() {
    let this = faux_rendu();
    let (status, _, _) = basic_support(this, &ITEM_RENDU, BROCHE_FLUX_RENDU, 3);
    assert_eq!(status, STATUS_BUFFER_TOO_SMALL);
    let (status, _, _) = basic_support(this, &ITEM_RENDU, BROCHES, TAILLE_DESCRIPTION);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Garde de vtable : les deux sens ne se mélangent pas.
// ---------------------------------------------------------------------------------

/// L'entrée monomorphisée sur le miniport de **rendu**, appelée sur un miniport de
/// **capture**, est refusée sans toucher à l'objet.
///
/// C'est la raison d'être des deux tables du pilote : une table commune aux deux sens
/// ferait un `inner` sur un objet d'un autre type — et la garde, qui compare l'adresse de
/// la vtable, transforme cet écran bleu en `STATUS_INVALID_DEVICE_REQUEST`.
#[test]
fn la_garde_de_vtable_separe_les_deux_sens() {
    let rendu = faux_rendu();
    let capture = faux_capture();

    let (status, _, _) = lire(capture, &ITEM_RENDU, BROCHE_FLUX_CAPTURE, TAILLE_REPONSE);
    assert_eq!(status, STATUS_INVALID_DEVICE_REQUEST);
    let (status, _, _) = lire(rendu, &ITEM_CAPTURE, BROCHE_FLUX_RENDU, TAILLE_REPONSE);
    assert_eq!(status, STATUS_INVALID_DEVICE_REQUEST);

    // Chacun avec sa table, en revanche, répond.
    let (status, _, _) = lire(rendu, &ITEM_RENDU, BROCHE_FLUX_RENDU, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);
    let (status, _, _) = lire(capture, &ITEM_CAPTURE, BROCHE_FLUX_CAPTURE, TAILLE_REPONSE);
    assert_eq!(status, STATUS_SUCCESS);

    assert_eq!(common::release(rendu), 0);
    assert_eq!(common::release(capture), 0);
}

/// L'entrée pointe le bon jeu et le bon identifiant : `KSPROPSETID_AudioSignalProcessing`
/// et `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` (0). Un identifiant faux rendrait la
/// propriété inatteignable sans le moindre message.
#[test]
fn l_entree_designe_le_bon_jeu_et_le_bon_identifiant() {
    let attendu = portcls::portcls_sys::KSPROPSETID_AudioSignalProcessing;
    let set = unsafe { *ITEM_RENDU.0.Set };
    assert_eq!(guid_octets(&set), guid_octets(&attendu));
    assert_eq!(
        ITEM_RENDU.0.Id,
        portcls::portcls_sys::KSPROPERTY_AUDIOSIGNALPROCESSING::KSPROPERTY_AUDIOSIGNALPROCESSING_MODES
            as u32
    );
    assert_eq!(ITEM_CAPTURE.0.Id, ITEM_RENDU.0.Id);
}
