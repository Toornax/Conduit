//! Les gestionnaires audio (`Volume`, `Mute`) vus par un faux PortCls, sur le modèle de
//! `tests/property.rs` : une `PCPROPERTY_REQUEST` bâtie à la main est passée au `Handler`
//! du `PCPROPERTY_ITEM` que `audio::volume_item` / `audio::mute_item` produisent.
//!
//! Le faux miniport (`Faux`) porte l'état dans des atomiques, comme le fera le câble côté
//! `conduit-kmd` : `audio.rs` lui-même ne stocke rien.
//!
//! Deux points que ces tests verrouillent en particulier :
//!
//! - **le décalage du canal** : l'`Instance` de test met un `Reserved` non nul
//!   (`0xA5A5_A5A5`) derrière le `Channel`. Lire le mauvais `LONG` ne rendrait donc pas
//!   « canal 0 » par accident mais un canal absurde, et les tests tombent au lieu de
//!   passer par chance ;
//! - **la sérialisation champ par champ** : le test de tampon désaligné exige les mêmes
//!   octets à l'adresse `+1` qu'à l'adresse alignée, ce qu'un transtypage vers un `*mut`
//!   de structure ne saurait garantir.

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
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::thread;

use common::This;
use conduit_com::{ComRef, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls::portcls_sys::{
    GUID, IMiniportTopologyVtbl, IUnknown, KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_UNIFORM,
    KSPROPERTY_MEMBER_STEPPEDRANGES, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET,
    KSPROPERTY_TYPE_SET, KSPROPTYPESETID_General, NTSTATUS, PCFILTER_DESCRIPTOR, PCPROPERTY_ITEM,
    PCPROPERTY_REQUEST, ULONG, VARENUM,
};
use portcls::{
    ACCESS_FLAGS, AudioNodes, Channel, MiniportTopology, PortTopology, ResourceList,
    STATUS_BUFFER_TOO_SMALL, Trace, VOLUME_DELTA, VOLUME_MAX, VOLUME_MIN, mute_item,
    new_topology_object, volume_item,
};

// ---------------------------------------------------------------------------------
// Le faux miniport.
// ---------------------------------------------------------------------------------

/// `PCFILTER_DESCRIPTOR` contient des pointeurs bruts : `Sync` déclaré à la main pour la
/// `static` (tout est nul, rien n'est jamais déréférencé).
struct SyncDesc(PCFILTER_DESCRIPTOR);
unsafe impl Sync for SyncDesc {}
static DESCRIPTION: SyncDesc = SyncDesc(unsafe { core::mem::zeroed() });

/// Canaux **déclarés** par le faux miniport (un câble stéréo).
const CANAUX: u32 = 2;
/// Emplacements de niveau réellement présents : deux de plus que les canaux déclarés,
/// pour vérifier qu'une écriture « tous canaux » n'en déborde pas.
const EMPLACEMENTS: usize = 4;
/// Niveau de départ de tous les emplacements : −24 dB, un multiple exact du pas.
const NIVEAU_INITIAL: i32 = -24 * 0x1_0000;
/// Sentinelle écrite au-delà des canaux déclarés : doit rester intacte.
const SENTINELLE: i32 = 0x0BAD_0BAD;
/// `Reserved` de `KSNODEPROPERTY_AUDIO_CHANNEL`, non nul à dessein : lire le mauvais
/// `LONG` de l'instance donnerait un canal absurde, pas « 0 » par accident.
const RESERVED: u32 = 0xA5A5_A5A5;

/// Canal noté par la dernière trace quand le décodage a échoué.
const CANAL_ERREUR: i32 = -99;

struct Faux {
    niveaux: [AtomicI32; EMPLACEMENTS],
    mute: AtomicBool,
    /// Nombre d'appels à [`AudioNodes::trace`].
    traces: AtomicU32,
    /// Canal de la dernière trace ([`CANAL_ERREUR`] si le décodage a échoué, `-1` pour
    /// « tous »).
    dernier_canal: AtomicI32,
    /// Valeur de la dernière trace (`i32::MIN` si aucune).
    derniere_valeur: AtomicI32,
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

impl AudioNodes for Faux {
    fn channels(&self) -> u32 {
        CANAUX
    }

    fn volume(&self, channel: u32) -> i32 {
        self.niveaux[channel as usize].load(Ordering::SeqCst)
    }

    fn set_volume(&self, channel: u32, level: i32) {
        self.niveaux[channel as usize].store(level, Ordering::SeqCst);
    }

    fn muted(&self) -> bool {
        self.mute.load(Ordering::SeqCst)
    }

    fn set_muted(&self, muted: bool) {
        self.mute.store(muted, Ordering::SeqCst);
    }

    fn trace(&self, trace: &Trace<'_>) {
        self.traces.fetch_add(1, Ordering::SeqCst);
        let canal = match trace.channel {
            Ok(Channel::All) => -1,
            Ok(Channel::One(indice)) => indice as i32,
            Err(_) => CANAL_ERREUR,
        };
        self.dernier_canal.store(canal, Ordering::SeqCst);
        self.derniere_valeur
            .store(trace.value.unwrap_or(i32::MIN), Ordering::SeqCst);
    }
}

fn faux() -> This {
    new_topology_object(Faux {
        niveaux: [
            AtomicI32::new(NIVEAU_INITIAL),
            AtomicI32::new(NIVEAU_INITIAL),
            AtomicI32::new(SENTINELLE),
            AtomicI32::new(SENTINELLE),
        ],
        mute: AtomicBool::new(false),
        traces: AtomicU32::new(0),
        dernier_canal: AtomicI32::new(CANAL_ERREUR),
        derniere_valeur: AtomicI32::new(i32::MIN),
    })
    .into_raw()
}

fn interieur(this: This) -> &'static Faux {
    unsafe { conduit_com::ComObject::<IMiniportTopologyVtbl, Faux>::inner(this) }
}

// ---------------------------------------------------------------------------------
// Tables d'automatisation de test.
// ---------------------------------------------------------------------------------

/// `PCPROPERTY_ITEM` contient un `*const GUID` et un pointeur de fonction : `Sync`
/// déclaré à la main pour la `static`, comme le pilote le fera pour ses vraies tables.
struct SyncItem(PCPROPERTY_ITEM);
unsafe impl Sync for SyncItem {}

static ITEM_VOLUME: SyncItem = SyncItem(volume_item::<IMiniportTopologyVtbl, Faux>());
static ITEM_MUTE: SyncItem = SyncItem(mute_item::<IMiniportTopologyVtbl, Faux>());

/// Nœud visé : sans importance ici, les gestionnaires ne lisent que `Instance`.
const NODE: ULONG = 1;

// ---------------------------------------------------------------------------------
// Outillage.
// ---------------------------------------------------------------------------------

/// La queue de `KSNODEPROPERTY_AUDIO_CHANNEL` telle que PortCls la laisse dans
/// `Instance` : `Channel: LONG` **en premier**, puis `Reserved: ULONG`.
fn instance(canal: i32) -> [u8; 8] {
    let mut octets = [0u8; 8];
    octets[0..4].copy_from_slice(&canal.to_ne_bytes());
    octets[4..8].copy_from_slice(&RESERVED.to_ne_bytes());
    octets
}

/// Requête sans instance ni valeur.
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

fn appeler(item: &'static SyncItem, req: &mut PCPROPERTY_REQUEST) -> NTSTATUS {
    let slot = item.0.Handler.expect("Handler renseigné par audio::*_item");
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

/// `GET` du volume sur un canal : `(statut, valeur lue)`.
fn lire_volume(this: This, canal: i32) -> (NTSTATUS, i32) {
    let mut inst = instance(canal);
    let mut tampon = [0u8; 4];
    let mut req = requete(&ITEM_VOLUME, this, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    let status = appeler(&ITEM_VOLUME, &mut req);
    (status, i32::from_ne_bytes(tampon))
}

/// `SET` du volume sur un canal.
fn ecrire_volume(this: This, canal: i32, valeur: i32) -> NTSTATUS {
    let mut inst = instance(canal);
    let mut tampon = valeur.to_ne_bytes();
    let mut req = requete(&ITEM_VOLUME, this, KSPROPERTY_TYPE_SET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    appeler(&ITEM_VOLUME, &mut req)
}

/// `BASICSUPPORT` avec un tampon de `place` octets : `(statut, ValueSize, octets)`.
fn basic_support(item: &'static SyncItem, this: This, place: usize) -> (NTSTATUS, ULONG, Vec<u8>) {
    let mut tampon = vec![0u8; place];
    let mut req = requete(item, this, KSPROPERTY_TYPE_BASICSUPPORT);
    if place > 0 {
        avec_valeur(&mut req, &mut tampon);
    }
    let status = appeler(item, &mut req);
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

// ---------------------------------------------------------------------------------
// Lecture.
// ---------------------------------------------------------------------------------

#[test]
fn lecture_d_un_canal_precharge() {
    let this = faux();
    interieur(this).niveaux[1].store(-3 * 0x1_0000, Ordering::SeqCst);

    let (status, valeur) = lire_volume(this, 0);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(valeur, NIVEAU_INITIAL);

    let (status, valeur) = lire_volume(this, 1);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(valeur, -3 * 0x1_0000, "c'est bien le canal 1 qui est lu");

    assert_eq!(common::release(this), 0);
}

/// Le décalage du canal, verrouillé : `Channel` est le **premier** `LONG` de l'instance.
/// Le `Reserved` qui le suit vaut `0xA5A5_A5A5` ; lire le mauvais mot rendrait un canal
/// invalide, pas « 0 » par accident. C'est aussi ce que la trace expose au premier essai
/// en machine.
#[test]
fn le_canal_est_le_premier_long_de_l_instance() {
    let this = faux();
    let me = interieur(this);
    me.niveaux[1].store(-7 * 0x1_0000, Ordering::SeqCst);

    let (status, valeur) = lire_volume(this, 1);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(valeur, -7 * 0x1_0000);
    assert_eq!(
        me.dernier_canal.load(Ordering::SeqCst),
        1,
        "la trace annonce le canal 1 : c'est ce qu'on relit en machine"
    );
    assert_eq!(me.derniere_valeur.load(Ordering::SeqCst), -7 * 0x1_0000);
    assert_eq!(me.traces.load(Ordering::SeqCst), 1);

    // Le mot suivant, s'il était pris pour le canal, serait `0xA5A5_A5A5` : négatif et
    // différent de −1, donc rejeté. La preuve que le test ne passe pas par hasard.
    assert_eq!(
        Channel::decode(&(RESERVED as i32).to_ne_bytes(), CANAUX),
        Err(STATUS_INVALID_PARAMETER)
    );

    assert_eq!(common::release(this), 0);
}

#[test]
fn instance_absente_est_un_parametre_invalide() {
    let this = faux();
    let mut tampon = [0u8; 4];
    // `InstanceSize = 0` : la requête n'a pas de canal.
    let mut req = requete(&ITEM_VOLUME, this, KSPROPERTY_TYPE_GET);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_VOLUME, &mut req), STATUS_INVALID_PARAMETER);
    assert_eq!(req.ValueSize, 4, "ValueSize intact en cas d'erreur");
    assert_eq!(
        interieur(this).dernier_canal.load(Ordering::SeqCst),
        CANAL_ERREUR,
        "la trace signale l'échec de décodage"
    );

    // Trois octets : toujours trop court pour un `LONG`.
    let mut inst = [0u8; 3];
    let mut req = requete(&ITEM_VOLUME, this, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_VOLUME, &mut req), STATUS_INVALID_PARAMETER);

    assert_eq!(common::release(this), 0);
}

#[test]
fn canal_hors_bornes_est_un_parametre_invalide() {
    let this = faux();
    for canal in [CANAUX as i32, 7, -2, i32::MIN] {
        let (status, _) = lire_volume(this, canal);
        assert_eq!(status, STATUS_INVALID_PARAMETER, "canal {canal}");
        assert_eq!(ecrire_volume(this, canal, 0), STATUS_INVALID_PARAMETER);
    }
    // Rien n'a bougé.
    let me = interieur(this);
    for slot in 0..CANAUX as usize {
        assert_eq!(me.niveaux[slot].load(Ordering::SeqCst), NIVEAU_INITIAL);
    }
    assert_eq!(common::release(this), 0);
}

#[test]
fn canal_moins_un_en_lecture_rend_le_canal_zero() {
    let this = faux();
    let me = interieur(this);
    me.niveaux[0].store(-2 * 0x1_0000, Ordering::SeqCst);
    me.niveaux[1].store(-11 * 0x1_0000, Ordering::SeqCst);

    let (status, valeur) = lire_volume(this, -1);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(valeur, -2 * 0x1_0000, "« tous canaux » lit le canal 0");
    assert_eq!(me.dernier_canal.load(Ordering::SeqCst), -1);

    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Écriture : borner et arrondir.
// ---------------------------------------------------------------------------------

#[test]
fn ecriture_au_dela_du_maximum_est_bornee_a_zero() {
    let this = faux();
    assert_eq!(ecrire_volume(this, 0, i32::MAX), STATUS_SUCCESS);
    assert_eq!(
        interieur(this).niveaux[0].load(Ordering::SeqCst),
        VOLUME_MAX
    );
    let (_, relu) = lire_volume(this, 0);
    assert_eq!(relu, VOLUME_MAX);
    assert_eq!(common::release(this), 0);
}

#[test]
fn ecriture_au_dela_du_minimum_est_bornee_au_minimum() {
    let this = faux();
    assert_eq!(ecrire_volume(this, 0, i32::MIN), STATUS_SUCCESS);
    assert_eq!(
        interieur(this).niveaux[0].load(Ordering::SeqCst),
        VOLUME_MIN
    );
    let (_, relu) = lire_volume(this, 0);
    assert_eq!(relu, VOLUME_MIN);
    assert_eq!(common::release(this), 0);
}

/// Une valeur qui ne tombe pas sur le pas est **arrondie**, pas seulement bornée : sinon
/// une lecture rendrait un niveau absent de la plage annoncée par `BASICSUPPORT`.
#[test]
fn ecriture_non_alignee_est_arrondie_au_pas_le_plus_proche() {
    let this = faux();
    let pas = VOLUME_DELTA as i32;
    // (valeur demandée, valeur attendue après bornage puis arrondi)
    let cas = [
        (-0x0001, 0),                 // à peine sous 0 → 0
        (-0x3FFF, 0),                 // sous le demi-pas → 0
        (-0x4000, 0),                 // le demi-pas exact → vers le haut
        (-0x4001, -pas),              // juste au-delà → un pas
        (-0x8000, -pas),              // déjà sur le pas, inchangé
        (-0xC000, -pas),              // un pas et demi exact → vers le haut
        (-0xC001, -2 * pas),          // juste au-delà → deux pas
        (-1_000_000, -31 * pas),      // valeur quelconque : −1 015 808
        (i32::MAX, VOLUME_MAX),       // borné d'abord
        (i32::MIN, VOLUME_MIN),       // borné d'abord
        (VOLUME_MIN + 1, VOLUME_MIN), // dans la plage, arrondi vers le minimum
    ];
    for (demande, attendu) in cas {
        assert_eq!(ecrire_volume(this, 0, demande), STATUS_SUCCESS);
        let (_, relu) = lire_volume(this, 0);
        assert_eq!(relu, attendu, "demande {demande}");
        assert_eq!(relu % pas, 0, "demande {demande} : multiple du pas");
        assert!(
            (VOLUME_MIN..=VOLUME_MAX).contains(&relu),
            "demande {demande} : dans la plage annoncée par BASICSUPPORT"
        );
        assert!(
            (relu - demande.clamp(VOLUME_MIN, VOLUME_MAX)).abs() <= pas / 2,
            "demande {demande} : c'est bien le pas le PLUS PROCHE, obtenu {relu}"
        );
    }
    assert_eq!(common::release(this), 0);
}

#[test]
fn ecriture_sans_place_est_refusee_sur_la_place_pas_sur_la_valeur() {
    let this = faux();
    let mut inst = instance(0);
    let mut tampon = [0u8; 3];
    let mut req = requete(&ITEM_VOLUME, this, KSPROPERTY_TYPE_SET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    assert_eq!(appeler(&ITEM_VOLUME, &mut req), STATUS_BUFFER_TOO_SMALL);
    assert_eq!(
        interieur(this).niveaux[0].load(Ordering::SeqCst),
        NIVEAU_INITIAL,
        "rien écrit"
    );
    assert_eq!(common::release(this), 0);
}

#[test]
fn ecriture_canal_moins_un_touche_tous_les_canaux_declares_et_eux_seuls() {
    let this = faux();
    let cible = -6 * 0x1_0000;
    assert_eq!(ecrire_volume(this, -1, cible), STATUS_SUCCESS);

    let me = interieur(this);
    for slot in 0..CANAUX as usize {
        assert_eq!(
            me.niveaux[slot].load(Ordering::SeqCst),
            cible,
            "canal déclaré {slot}"
        );
    }
    for slot in CANAUX as usize..EMPLACEMENTS {
        assert_eq!(
            me.niveaux[slot].load(Ordering::SeqCst),
            SENTINELLE,
            "emplacement non déclaré {slot} intact"
        );
    }
    assert_eq!(me.dernier_canal.load(Ordering::SeqCst), -1);
    assert_eq!(common::release(this), 0);
}

/// L'aller-retour que vérifient les tests de conformité audio : ce qu'on écrit sur un
/// canal est ce qu'on relit sur ce canal, et sur lui seul.
#[test]
fn aller_retour_ecriture_puis_lecture_sur_chaque_canal() {
    let this = faux();
    let niveaux = [-0x8000, -48 * 0x1_0000];
    for (canal, niveau) in niveaux.iter().copied().enumerate() {
        assert_eq!(ecrire_volume(this, canal as i32, niveau), STATUS_SUCCESS);
    }
    for (canal, niveau) in niveaux.iter().copied().enumerate() {
        let (status, relu) = lire_volume(this, canal as i32);
        assert_eq!(status, STATUS_SUCCESS);
        assert_eq!(relu, niveau, "canal {canal}");
    }
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// BASICSUPPORT du volume.
// ---------------------------------------------------------------------------------

#[test]
fn basicsupport_du_volume_aux_quatre_paliers() {
    let this = faux();

    // Palier « rien ne tient » : moins de quatre octets.
    let (status, taille, _) = basic_support(&ITEM_VOLUME, this, 3);
    assert_eq!(status, STATUS_BUFFER_TOO_SMALL);
    assert_eq!(taille, 3, "ValueSize intact sur erreur");

    // Palier `sizeof(ULONG)` : le premier appel de KS. Doit réussir.
    let (status, taille, octets) = basic_support(&ITEM_VOLUME, this, 4);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 4, "octets écrits, pas taille requise");
    assert_eq!(u32_a(&octets, 0), ACCESS_FLAGS);
    assert_eq!(ACCESS_FLAGS, 515, "GET | SET | BASICSUPPORT");

    // Palier « la description seule ».
    let (status, taille, octets) = basic_support(&ITEM_VOLUME, this, 40);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 40);
    verifier_description(&octets, VARENUM::VT_I4 as u32, 72, 1);

    // Le palier intermédiaire, celui qu'on oublie : 50 octets, la place d'une description
    // mais pas des membres. Doit écrire 40 et rendre 40.
    let (status, taille, octets) = basic_support(&ITEM_VOLUME, this, 50);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 40, "50 octets de place : on n'en écrit que 40");
    verifier_description(&octets, VARENUM::VT_I4 as u32, 72, 1);
    assert_eq!(
        &octets[40..50],
        &[0u8; 10],
        "rien au-delà des 40 octets écrits"
    );

    // Palier complet.
    let (status, taille, octets) = basic_support(&ITEM_VOLUME, this, 72);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 72);
    verifier_description(&octets, VARENUM::VT_I4 as u32, 72, 1);
    verifier_plage(&octets);

    assert_eq!(common::release(this), 0);
}

/// `KSPROPERTY_DESCRIPTION`, octet à octet, aux décalages du golden.
fn verifier_description(octets: &[u8], variante: u32, taille_totale: u32, membres: u32) {
    assert_eq!(u32_a(octets, 0), ACCESS_FLAGS, "AccessFlags @0");
    assert_eq!(
        u32_a(octets, 4),
        taille_totale,
        "DescriptionSize @4 : la taille COMPLÈTE, même si on n'écrit que 40 octets"
    );
    assert_eq!(
        &octets[8..24],
        guid_octets(&KSPROPTYPESETID_General).as_slice(),
        "PropTypeSet.Set @8 : KSPROPTYPESETID_General"
    );
    assert_eq!(u32_a(octets, 24), variante, "PropTypeSet.Id @24");
    assert_eq!(u32_a(octets, 28), 0, "PropTypeSet.Flags @28");
    assert_eq!(
        u32_a(octets, 32),
        membres,
        "MembersListCount @32 — PAS @24 ni @16 : PropTypeSet est un KSIDENTIFIER de 24 octets"
    );
    assert_eq!(u32_a(octets, 36), 0, "Reserved @36");
}

/// `KSPROPERTY_MEMBERSHEADER` + `KSPROPERTY_STEPPING_LONG` du volume.
fn verifier_plage(octets: &[u8]) {
    assert_eq!(
        u32_a(octets, 40),
        KSPROPERTY_MEMBER_STEPPEDRANGES,
        "MembersFlags @40"
    );
    assert_eq!(u32_a(octets, 44), 16, "MembersSize @44");
    assert_eq!(u32_a(octets, 48), 1, "MembersCount @48");
    assert_eq!(
        u32_a(octets, 52),
        KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_UNIFORM,
        "Flags @52"
    );
    assert_eq!(
        u32_a(octets, 56),
        VOLUME_DELTA,
        "SteppingDelta @56 : 0,5 dB"
    );
    assert_eq!(u32_a(octets, 60), 0, "Reserved @60");
    assert_eq!(
        i32_a(octets, 64),
        VOLUME_MIN,
        "Bounds.SignedMinimum @64 : −96 dB"
    );
    assert_eq!(i32_a(octets, 68), VOLUME_MAX, "Bounds.SignedMaximum @68");
    // Ce que `IAudioEndpointVolume::GetVolumeRange` doit rendre en machine.
    assert_eq!(i32_a(octets, 64) / 0x1_0000, -96);
    assert_eq!(u32_a(octets, 56) * 2, 0x1_0000, "un pas de 0,5 dB");
}

/// La sérialisation ne doit rien devoir à l'alignement du tampon : c'est ce test qui
/// interdit qu'on « optimise » un jour en transtypant vers un `*mut KSPROPERTY_DESCRIPTION`
/// et en écrivant à travers — comportement indéfini en Rust, même sur x64.
#[repr(align(8))]
struct Aligne([u8; 80]);

#[test]
fn basicsupport_desaligne_donne_les_memes_octets_qu_aligne() {
    let this = faux();

    for palier in [3usize, 4, 40, 50, 72] {
        let mut sorties = Vec::new();
        for decalage in [0usize, 1] {
            let mut tampon = Aligne([0u8; 80]);
            assert_eq!(tampon.0.as_ptr() as usize % 8, 0, "base alignée sur 8");
            let tranche = &mut tampon.0[decalage..decalage + palier];
            assert_eq!(
                tranche.as_ptr() as usize % 8,
                decalage,
                "palier {palier}, décalage {decalage}"
            );

            let mut req = requete(&ITEM_VOLUME, this, KSPROPERTY_TYPE_BASICSUPPORT);
            avec_valeur(&mut req, tranche);
            let status = appeler(&ITEM_VOLUME, &mut req);
            sorties.push((status, req.ValueSize, tranche.to_vec()));
        }
        assert_eq!(
            sorties[0], sorties[1],
            "palier {palier} : mêmes octets à +0 et à +1"
        );
    }

    assert_eq!(common::release(this), 0);
}

/// La séquence réelle du moteur audio à l'ouverture d'un endpoint.
#[test]
fn sequence_du_moteur_audio() {
    let this = faux();

    // 1. `BASICSUPPORT` avec `sizeof(ULONG)` : les `AccessFlags` seuls.
    let (status, taille, octets) = basic_support(&ITEM_VOLUME, this, 4);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 4);
    assert_eq!(u32_a(&octets, 0), ACCESS_FLAGS);

    // 2. `BASICSUPPORT` complet : la plage, celle que `GetVolumeRange` publie.
    let (status, taille, octets) = basic_support(&ITEM_VOLUME, this, 72);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 72);
    verifier_plage(&octets);

    // 3. Lecture du niveau courant, canal 0.
    let (status, avant) = lire_volume(this, 0);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(avant, NIVEAU_INITIAL);

    // 4. Écriture (le curseur bouge), tous canaux.
    let cible = -18 * 0x1_0000;
    assert_eq!(ecrire_volume(this, -1, cible), STATUS_SUCCESS);

    // 5. Relecture : la nouvelle valeur, sur les deux canaux.
    for canal in 0..CANAUX as i32 {
        let (status, apres) = lire_volume(this, canal);
        assert_eq!(status, STATUS_SUCCESS);
        assert_eq!(apres, cible, "canal {canal}");
    }

    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Concurrence.
// ---------------------------------------------------------------------------------

/// `This` est un `*mut c_void` : non `Send` par défaut, alors que l'objet, lui, l'est
/// (`ComObject<_, T: Send + Sync>`). PortCls appelle bien depuis plusieurs fils.
#[derive(Clone, Copy)]
struct EnvoiThis(This);
unsafe impl Send for EnvoiThis {}

impl EnvoiThis {
    /// Passe par une méthode, et non par `envoi.0` : la capture disjointe de l'édition
    /// 2021 ne prendrait sinon que le champ `*mut c_void`, qui n'est pas `Send`.
    fn this(self) -> This {
        self.0
    }
}

#[test]
fn ecritures_et_lectures_concurrentes_restent_coherentes() {
    let this = faux();
    let envoi = EnvoiThis(this);

    // Quatre valeurs, toutes alignées sur le pas : ce sont les seules qu'une lecture peut
    // rendre, avec le niveau de départ.
    let ecrites: [i32; 4] = [0, -0x8000, -32 * 0x1_0000, VOLUME_MIN];
    let admissibles: Vec<i32> = ecrites.iter().copied().chain([NIVEAU_INITIAL]).collect();

    let mut fils = Vec::new();
    for (i, valeur) in ecrites.iter().copied().enumerate() {
        fils.push(thread::spawn(move || {
            for tour in 0..200 {
                let canal = ((i + tour) % CANAUX as usize) as i32;
                assert_eq!(ecrire_volume(envoi.this(), canal, valeur), STATUS_SUCCESS);
            }
        }));
    }
    for _ in 0..4 {
        let admissibles = admissibles.clone();
        fils.push(thread::spawn(move || {
            for tour in 0..200 {
                let canal = (tour % CANAUX as usize) as i32;
                let (status, lu) = lire_volume(envoi.this(), canal);
                assert_eq!(status, STATUS_SUCCESS);
                assert!(
                    (VOLUME_MIN..=VOLUME_MAX).contains(&lu),
                    "valeur lue hors plage : {lu}"
                );
                assert_eq!(lu % VOLUME_DELTA as i32, 0, "valeur lue hors pas : {lu}");
                assert!(
                    admissibles.contains(&lu),
                    "valeur lue jamais écrite : {lu} (admissibles {admissibles:?})"
                );
            }
        }));
    }
    for f in fils {
        f.join().expect("aucun fil ne panique");
    }

    // L'état final est l'une des valeurs écrites.
    for canal in 0..CANAUX as i32 {
        let (_, lu) = lire_volume(this, canal);
        assert!(admissibles.contains(&lu));
    }
    assert_eq!(common::release(this), 0);
}

// ---------------------------------------------------------------------------------
// Sourdine.
// ---------------------------------------------------------------------------------

/// `GET`/`SET` de la sourdine sur un canal donné : `(statut, valeur)`.
fn lire_mute(this: This, canal: i32) -> (NTSTATUS, i32) {
    let mut inst = instance(canal);
    let mut tampon = [0u8; 4];
    let mut req = requete(&ITEM_MUTE, this, KSPROPERTY_TYPE_GET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    let status = appeler(&ITEM_MUTE, &mut req);
    (status, i32::from_ne_bytes(tampon))
}

fn ecrire_mute(this: This, canal: i32, valeur: i32) -> NTSTATUS {
    let mut inst = instance(canal);
    let mut tampon = valeur.to_ne_bytes();
    let mut req = requete(&ITEM_MUTE, this, KSPROPERTY_TYPE_SET);
    avec_instance(&mut req, &mut inst);
    avec_valeur(&mut req, &mut tampon);
    appeler(&ITEM_MUTE, &mut req)
}

#[test]
fn sourdine_lecture_et_ecriture() {
    let this = faux();
    let me = interieur(this);

    let (status, valeur) = lire_mute(this, 0);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(valeur, 0, "sourdine éteinte au départ");

    // Windows écrit la sourdine avec `Channel == -1`.
    assert_eq!(ecrire_mute(this, -1, 1), STATUS_SUCCESS);
    assert!(me.muted());
    let (_, valeur) = lire_mute(this, 1);
    assert_eq!(valeur, 1);

    // Tout non-nul vaut vrai, zéro éteint.
    assert_eq!(ecrire_mute(this, 0, 0x1234), STATUS_SUCCESS);
    assert!(me.muted());
    assert_eq!(ecrire_mute(this, 0, 0), STATUS_SUCCESS);
    assert!(!me.muted());
    let (_, valeur) = lire_mute(this, 0);
    assert_eq!(valeur, 0);

    // Mêmes règles de canal que le volume.
    let (status, _) = lire_mute(this, CANAUX as i32);
    assert_eq!(status, STATUS_INVALID_PARAMETER);
    assert_eq!(ecrire_mute(this, -2, 1), STATUS_INVALID_PARAMETER);
    assert!(!me.muted(), "une requête refusée ne change rien");

    assert_eq!(common::release(this), 0);
}

#[test]
fn basicsupport_de_la_sourdine_est_un_booleen_sans_membres() {
    let this = faux();

    let (status, taille, _) = basic_support(&ITEM_MUTE, this, 3);
    assert_eq!(status, STATUS_BUFFER_TOO_SMALL);
    assert_eq!(taille, 3);

    let (status, taille, octets) = basic_support(&ITEM_MUTE, this, 4);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 4);
    assert_eq!(u32_a(&octets, 0), ACCESS_FLAGS);

    // Description complète : 40 octets, `VT_BOOL`, aucun membre.
    let (status, taille, octets) = basic_support(&ITEM_MUTE, this, 40);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 40);
    verifier_description(&octets, VARENUM::VT_BOOL as u32, 40, 0);
    assert_eq!(u32_a(&octets, 24), 11, "VT_BOOL vaut 11");
    assert_eq!(u32_a(&octets, 32), 0, "MembersListCount = 0 @32");

    // Plus de place ne change rien : la sourdine n'a pas de plage.
    let (status, taille, plus) = basic_support(&ITEM_MUTE, this, 72);
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(taille, 40, "la sourdine s'arrête à 40 octets");
    assert_eq!(&plus[..40], &octets[..40]);
    assert_eq!(&plus[40..], &[0u8; 32], "rien au-delà");

    assert_eq!(common::release(this), 0);
}

#[test]
fn basicsupport_de_la_sourdine_desaligne_donne_les_memes_octets() {
    let this = faux();
    for palier in [4usize, 40, 72] {
        let mut sorties = Vec::new();
        for decalage in [0usize, 1] {
            let mut tampon = Aligne([0u8; 80]);
            let tranche = &mut tampon.0[decalage..decalage + palier];
            let mut req = requete(&ITEM_MUTE, this, KSPROPERTY_TYPE_BASICSUPPORT);
            avec_valeur(&mut req, tranche);
            let status = appeler(&ITEM_MUTE, &mut req);
            sorties.push((status, req.ValueSize, tranche.to_vec()));
        }
        assert_eq!(sorties[0], sorties[1], "palier {palier}");
    }
    assert_eq!(common::release(this), 0);
}
