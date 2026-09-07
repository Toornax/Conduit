//! Sémantique audio KS des nœuds volume et sourdine : trait [`AudioNodes`] que le
//! miniport topologie implémentera, et deux [`PropertyHandler`] (`Volume`, `Mute`) posés
//! sur la brique de [`crate::property`].
//!
//! **Ce module ne stocke aucun état.** Il ne fait que deux choses : traduire une requête
//! KS (canal, valeur, verbe) en appels de trait, et sérialiser des structures KS champ par
//! champ. L'état — le niveau par canal, la sourdine — vit dans le câble, côté
//! `conduit-kmd`, dans des atomiques ; c'est lui qui implémentera [`AudioNodes`] pour
//! `TopoRender` et `TopoCapture`.
//!
//! # L'échelle de volume : 1/65536 dB, documentée et pas encore mesurée
//!
//! `KSPROPERTY_AUDIO_VOLUMELEVEL` transporte un `LONG` en unités de **1/65536 dB**
//! (virgule fixe 16.16), **et non en centièmes de décibel** — c'est l'erreur classique,
//! et elle est silencieuse : l'endpoint fonctionne, mais le curseur de Windows parcourt
//! une plage absurde. D'où [`VOLUME_MIN`], [`VOLUME_MAX`] et [`VOLUME_DELTA`], appuyés
//! sur **deux sources concordantes**, et une **vérification qui reste à faire** :
//!
//! - la documentation Microsoft de `KSPROPERTY_AUDIO_VOLUMELEVEL` (« *in units of
//!   1/65536 decibel* ») ;
//! - les constantes de SYSVAD (`VOLUME_STEPPING_DELTA`, `VOLUME_SIGNED_MAXIMUM`,
//!   `VOLUME_SIGNED_MINIMUM` dans son `minwavert.h`), qui décrivent la même plage ;
//! - **à faire, une fois le nœud câblé** : `conduit-looptest --list --show-volume` sur
//!   l'endpoint « Conduit 1 » doit rendre `−96,0 / 0,0 / 0,5` dB. Tant que ce relevé
//!   n'existe pas, l'échelle est une lecture de documentation et rien de plus.
//!
//! Le relevé du 2026-09-07 sur les cartes de la machine hôte donne bien `−96,0 / 0,0`
//! comme bornes, mais un pas de **1,5 dB** — et les trois cartes, matériels sans rapport,
//! annoncent une plage rigoureusement identique. C'est donc le volume **logiciel** de
//! Windows qui répond, faute de nœud dans leur topologie : la mesure conforte les bornes
//! et ne dit rien du pas d'un vrai nœud. C'est le test de `BASICSUPPORT` complet
//! (palier 72) qui fige ce que nous annonçons, et la plage qu'il décrit est
//! *littéralement* ce que le moteur audio affichera.
//!
//! # `KSPROPERTY_TYPE_BASICSUPPORT` n'est pas optionnel
//!
//! Nos `PCPROPERTY_ITEM` déclarent `Flags = GET | SET | BASICSUPPORT`
//! (`1 | 2 | 512 = 515`). À partir du moment où le bit `BASICSUPPORT` est là, **PortCls ne
//! répond plus lui-même** : le gestionnaire doit décrire la propriété en entier, sans quoi
//! `IAudioEndpointVolume::GetVolumeRange` échoue et le moteur audio traite l'endpoint
//! comme dépourvu de contrôle matériel. C'est par ce verbe, et par lui seul, que la plage
//! du nœud arrive jusqu'à l'interface utilisateur.
//!
//! Réponse en **paliers**, parce que KS interroge deux fois : d'abord `sizeof(ULONG)` pour
//! les seuls `AccessFlags`, puis la taille complète. Et [`PropertyHandler::basic_support`]
//! rend les octets **écrits**, jamais une taille requise (voir l'asymétrie documentée sur
//! le trait) :
//!
//! | Place dans `Value` | Écrit | rendu |
//! |---|---|---|
//! | < 4 | rien | `Err(STATUS_BUFFER_TOO_SMALL)` |
//! | 4 à 39 | `AccessFlags: ULONG` | 4 |
//! | 40 à 71 | `KSPROPERTY_DESCRIPTION` seule | 40 |
//! | ≥ 72 (volume) | description + `KSPROPERTY_MEMBERSHEADER` + `KSPROPERTY_STEPPING_LONG` | 72 |
//!
//! `DescriptionSize` vaut **toujours** la taille complète (72 pour le volume), même au
//! palier où l'on n'écrit que 40 octets : c'est ainsi que le client sait quoi redemander.
//! La sourdine s'arrête à 40 (`VT_BOOL`, aucun membre), son `DescriptionSize` vaut donc 40.
//!
//! # Les décalages viennent du golden, pas de l'intuition
//!
//! `drivers/windows/portcls-sys/tests/layout.golden` les tient de `cl.exe` sur les
//! en-têtes du WDK ; les constantes `D_*`, `M_*` et `S_*` de ce module les recopient une à
//! une, et chaque champ est écrit **à son décalage nommé**, pour que le code se relise en
//! regard du golden.
//!
//! | Structure | Champ → décalage | Taille |
//! |---|---|---|
//! | `KSPROPERTY_DESCRIPTION` | AccessFlags 0, DescriptionSize 4, PropTypeSet 8, MembersListCount **32**, Reserved 36 | 40 |
//! | `KSPROPERTY_MEMBERSHEADER` | MembersFlags 0, MembersSize 4, MembersCount 8, Flags 12 | 16 |
//! | `KSPROPERTY_STEPPING_LONG` | SteppingDelta 0, Reserved 4, Bounds 8 | 16 |
//!
//! **Le piège** : `KSPROPERTY_DESCRIPTION::PropTypeSet` est un `KSIDENTIFIER` de **24**
//! octets aligné sur 8 (`Set: GUID` 16, `Id: ULONG`, `Flags: ULONG`), pas un `GUID` de 16.
//! `MembersListCount` est donc à **32**, ni 24 ni 16. Une sérialisation écrite au jugé se
//! trompe ici, et la réponse serait corrompue sans que rien ne le signale.
//!
//! # Le canal, et comment le vérifier en une minute
//!
//! PortCls retire l'en-tête `KSNODEPROPERTY` et ne laisse dans `Instance` que la **queue**
//! de `KSNODEPROPERTY_AUDIO_CHANNEL` : `InstanceSize == 8`, un `Channel: LONG` **en
//! premier** puis un `Reserved: ULONG`. C'est le seul décalage de tout le plan dont
//! l'erreur serait silencieuse — lire le `Reserved` au lieu du `Channel` rend simplement
//! « canal 0 » à chaque fois, et un endpoint stéréo qui bouge ses deux canaux ensemble
//! ressemble à un endpoint qui marche.
//!
//! D'où [`AudioNodes::trace`], appelée à chaque `GET`/`SET` avec les octets **bruts** de
//! `Instance` et le canal qu'on en a décodé (voir [`Trace`]). Le pilote la câble sur
//! `kmd_log!` en une ligne, et le premier essai en machine tranche tout de suite :
//!
//! - **attendu** sur un endpoint stéréo — Windows interroge canal `0` puis canal `1`, et
//!   écrit avec canal `-1` ;
//! - **le décalage est faux** si la trace ne montre que des « canal 0 » : c'est le
//!   `Reserved`, toujours nul, qu'on est en train de lire.
//!
//! Aucune capture, aucun débogueur pas à pas : deux lignes de journal suffisent à conclure.
//!
//! # Écrire : borner **et** arrondir
//!
//! Un `SET` refusé fait apparaître l'endpoint cassé dans l'interface, donc une valeur hors
//! plage est **bornée**, jamais rejetée. Mais elle est aussi **arrondie au multiple de
//! [`VOLUME_DELTA`] le plus proche** : sans cela, une lecture pourrait rendre une valeur
//! qui n'appartient pas à la plage annoncée par `BASICSUPPORT` — exactement le genre
//! d'incohérence que relèvent les tests de conformité audio. Une écriture ne rend donc
//! jamais d'erreur sur la *valeur* ; seulement sur la place (moins de 4 octets) ou sur le
//! canal.
//!
//! # Pourquoi la sourdine part avec le volume
//!
//! Elle ne coûte que quarante octets de `BASICSUPPORT` et une paire d'accesseurs, la
//! brique de propriété est écrite de toute façon, et surtout **on ne sait pas** si Windows
//! renonce à son traitement logiciel de sourdine quand il ne trouve qu'un nœud de volume.
//! Livrer les deux supprime la question au lieu de la reporter à la première écoute en VM.

use conduit_com::{NtStatus, STATUS_INVALID_PARAMETER};
use portcls_sys::{
    GUID, KSPROPERTY_AUDIO, KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_UNIFORM,
    KSPROPERTY_MEMBER_STEPPEDRANGES, KSPROPERTY_TYPE_BASICSUPPORT, KSPROPERTY_TYPE_GET,
    KSPROPERTY_TYPE_SET, KSPROPSETID_Audio, KSPROPTYPESETID_General, PCPROPERTY_ITEM, VARENUM,
};

use crate::property::{self, PropertyHandler, Request, TargetVtbl};
use crate::status::STATUS_BUFFER_TOO_SMALL;

// ---------------------------------------------------------------------------------
// L'échelle (voir la documentation du module).
// ---------------------------------------------------------------------------------

/// Niveau minimal du nœud de volume : **−96 dB**, en unités de 1/65536 dB.
///
/// `-96 * 0x1_0000 = -6_291_456`. C'est ce que `IAudioEndpointVolume::GetVolumeRange`
/// doit rendre comme minimum, en `−96,0` dB.
pub const VOLUME_MIN: i32 = -96 * 0x1_0000;

/// Niveau maximal du nœud de volume : **0 dB** (pas d'amplification).
pub const VOLUME_MAX: i32 = 0;

/// Pas du nœud de volume : **0,5 dB**, en unités de 1/65536 dB (`0x8000`).
///
/// Toute valeur écrite est arrondie à un multiple de ce pas (voir la documentation du
/// module) : la plage annoncée par `BASICSUPPORT` et les valeurs relues coïncident.
pub const VOLUME_DELTA: u32 = 0x8000;

/// `Flags` des `PCPROPERTY_ITEM` de ce module : `GET | SET | BASICSUPPORT` (515).
///
/// Le bit `BASICSUPPORT` fait de nous le seul répondant à ce verbe : PortCls ne le traite
/// plus lui-même (voir la documentation du module).
pub const ACCESS_FLAGS: u32 =
    KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT;

/// Demi-pas, ajouté avant troncature pour obtenir l'arrondi au plus proche.
const DEMI_PAS: i32 = 0x4000;

/// Masque effaçant les bits bas d'un multiple de [`VOLUME_DELTA`] : `!(0x8000 - 1)`.
///
/// L'arrondi ne marche que parce que le pas est une puissance de deux (assertion
/// ci-dessous) : en complément à deux, `& MASQUE_PAS` tronque vers −∞, y compris pour les
/// négatifs, qui sont ici tout le domaine utile.
const MASQUE_PAS: i32 = !0x7FFF;

const _: () = assert!(VOLUME_DELTA.is_power_of_two());
const _: () = assert!(VOLUME_DELTA == 0x8000 && DEMI_PAS == 0x4000 && MASQUE_PAS == !0x7FFF);
const _: () = assert!(VOLUME_MIN == -6_291_456 && VOLUME_MAX == 0);
const _: () = assert!(ACCESS_FLAGS == 515);

/// `Channel == -1` dans `KSNODEPROPERTY_AUDIO_CHANNEL` : « tous les canaux ».
const CANAL_TOUS: i32 = -1;

/// Taille d'une valeur de volume (`LONG`) ou de sourdine (`BOOL`, soit un `LONG`).
const TAILLE_VALEUR: usize = 4;

// ---------------------------------------------------------------------------------
// Décalages recopiés de portcls-sys/tests/layout.golden (oracle cl.exe). Chaque champ
// est écrit à son décalage nommé : le code se relit en regard du golden.
// ---------------------------------------------------------------------------------

/// `KSPROPERTY_DESCRIPTION::AccessFlags` (`ULONG`).
const D_ACCESSFLAGS: usize = 0;
/// `KSPROPERTY_DESCRIPTION::DescriptionSize` (`ULONG`).
const D_DESCRIPTIONSIZE: usize = 4;
/// `KSPROPERTY_DESCRIPTION::PropTypeSet.Set` : le `GUID` du `KSIDENTIFIER`.
const D_PROPTYPESET_SET: usize = 8;
/// `KSPROPERTY_DESCRIPTION::PropTypeSet.Id` : la variante (`VT_I4`, `VT_BOOL`).
const D_PROPTYPESET_ID: usize = 24;
/// `KSPROPERTY_DESCRIPTION::PropTypeSet.Flags`.
const D_PROPTYPESET_FLAGS: usize = 28;
/// `KSPROPERTY_DESCRIPTION::MembersListCount` — **32**, pas 24 ni 16 : `PropTypeSet` est
/// un `KSIDENTIFIER` de 24 octets, pas un `GUID` de 16.
const D_MEMBERSLISTCOUNT: usize = 32;
/// `KSPROPERTY_DESCRIPTION::Reserved`.
const D_RESERVED: usize = 36;

/// `KSPROPERTY_MEMBERSHEADER::MembersFlags`, à la suite de la description (40 + 0).
const M_MEMBERSFLAGS: usize = 40;
/// `KSPROPERTY_MEMBERSHEADER::MembersSize` (40 + 4).
const M_MEMBERSSIZE: usize = 44;
/// `KSPROPERTY_MEMBERSHEADER::MembersCount` (40 + 8).
const M_MEMBERSCOUNT: usize = 48;
/// `KSPROPERTY_MEMBERSHEADER::Flags` (40 + 12).
const M_FLAGS: usize = 52;

/// `KSPROPERTY_STEPPING_LONG::SteppingDelta`, à la suite de l'en-tête (56 + 0).
const S_STEPPINGDELTA: usize = 56;
/// `KSPROPERTY_STEPPING_LONG::Reserved` (56 + 4).
const S_RESERVED: usize = 60;
/// `KSPROPERTY_STEPPING_LONG::Bounds.SignedMinimum` (56 + 8).
const S_MINIMUM: usize = 64;
/// `KSPROPERTY_STEPPING_LONG::Bounds.SignedMaximum` (56 + 12).
const S_MAXIMUM: usize = 68;

/// `sizeof(ULONG)` : le premier palier de `BASICSUPPORT`, les seuls `AccessFlags`.
const TAILLE_ACCESSFLAGS: usize = 4;
/// `sizeof(KSPROPERTY_DESCRIPTION)` (golden).
const TAILLE_DESCRIPTION: usize = 40;
/// `sizeof(KSPROPERTY_MEMBERSHEADER)` (golden).
const TAILLE_MEMBERSHEADER: usize = 16;
/// `sizeof(KSPROPERTY_STEPPING_LONG)` (golden).
const TAILLE_STEPPING_LONG: usize = 16;
/// Description complète du volume : 40 + 16 + 16 = 72.
const TAILLE_BASICSUPPORT_VOLUME: usize =
    TAILLE_DESCRIPTION + TAILLE_MEMBERSHEADER + TAILLE_STEPPING_LONG;

const _: () = assert!(D_MEMBERSLISTCOUNT == 32);
const _: () = assert!(TAILLE_DESCRIPTION == 40 && TAILLE_BASICSUPPORT_VOLUME == 72);

// ---------------------------------------------------------------------------------
// Canal.
// ---------------------------------------------------------------------------------

/// Le canal visé par une requête de nœud audio, décodé de `Instance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// `Channel == -1` : **tous** les canaux déclarés en écriture, le canal 0 en lecture.
    All,
    /// Un canal précis, déjà validé contre [`AudioNodes::channels`] : strictement
    /// inférieur au nombre de canaux déclarés.
    One(u32),
}

impl Channel {
    /// Décode le `Channel: LONG` en tête de `instance`, la queue de
    /// `KSNODEPROPERTY_AUDIO_CHANNEL` que PortCls nous laisse (`InstanceSize == 8` :
    /// `Channel` puis `Reserved`).
    ///
    /// `instance` est hostile et **d'alignement quelconque** : les quatre premiers octets
    /// sont recopiés puis interprétés, jamais transtypés.
    ///
    /// - moins de 4 octets (`InstanceSize < 4`) → `STATUS_INVALID_PARAMETER` ;
    /// - `channels == 0` (miniport sans canal) → `STATUS_INVALID_PARAMETER` ;
    /// - `-1` → [`Channel::All`] ;
    /// - `0 <= canal < channels` → [`Channel::One`] ;
    /// - tout le reste (négatif autre que `-1`, `>= channels`) →
    ///   `STATUS_INVALID_PARAMETER`.
    pub fn decode(instance: &[u8], channels: u32) -> Result<Self, NtStatus> {
        let Some(quatre) = instance.get(..TAILLE_VALEUR) else {
            return Err(STATUS_INVALID_PARAMETER);
        };
        let mut mot = [0u8; TAILLE_VALEUR];
        mot.copy_from_slice(quatre);
        let canal = i32::from_ne_bytes(mot);

        if channels == 0 {
            return Err(STATUS_INVALID_PARAMETER);
        }
        if canal == CANAL_TOUS {
            return Ok(Self::All);
        }
        match u32::try_from(canal) {
            Ok(indice) if indice < channels => Ok(Self::One(indice)),
            _ => Err(STATUS_INVALID_PARAMETER),
        }
    }

    /// Le canal à **lire** : `All` lit le canal 0 (convention KS pour une interrogation
    /// « tous canaux »).
    fn a_lire(self) -> u32 {
        match self {
            Self::All => 0,
            Self::One(indice) => indice,
        }
    }
}

// ---------------------------------------------------------------------------------
// Trace.
// ---------------------------------------------------------------------------------

/// Ce qu'un gestionnaire vient de faire d'une requête `GET`/`SET`, passé à
/// [`AudioNodes::trace`].
///
/// Existe pour une raison précise : rendre le décalage du `Channel` vérifiable en une
/// minute au premier essai en machine (voir la documentation du module). `instance` porte
/// les octets **bruts**, `channel` ce qu'on en a tiré ; les comparer suffit.
///
/// `BASICSUPPORT` n'est pas tracé : ce verbe n'a pas de canal, et son contenu est fixe.
#[derive(Debug)]
pub struct Trace<'a> {
    /// La propriété : `"volume"` ou `"sourdine"`.
    pub property: &'static str,
    /// Le verbe : `"GET"` ou `"SET"`.
    pub verb: &'static str,
    /// Les octets d'`Instance` tels que PortCls les a laissés : la queue de
    /// `KSNODEPROPERTY_AUDIO_CHANNEL`, normalement 8 octets, `Channel` d'abord.
    pub instance: &'a [u8],
    /// Le canal décodé de ces octets, ou le statut d'erreur rendu à l'appelant.
    pub channel: Result<Channel, NtStatus>,
    /// La valeur lue ou écrite — pour le volume, en unités de 1/65536 dB, **après**
    /// bornage et arrondi ; pour la sourdine, 0 ou 1. `None` si la requête a échoué.
    pub value: Option<i32>,
}

/// Nom de propriété des traces du volume.
const PROP_VOLUME: &str = "volume";
/// Nom de propriété des traces de la sourdine.
const PROP_MUTE: &str = "sourdine";
/// Nom de verbe des traces de lecture.
const VERBE_GET: &str = "GET";
/// Nom de verbe des traces d'écriture.
const VERBE_SET: &str = "SET";

// ---------------------------------------------------------------------------------
// Le trait métier.
// ---------------------------------------------------------------------------------

/// Les nœuds audio d'un miniport topologie, vus par les gestionnaires de propriété.
///
/// `TopoRender` et `TopoCapture` l'implémenteront sur l'état du câble (des atomiques,
/// côté `conduit-kmd`) : **ce module ne stocke rien**.
///
/// Toutes les méthodes reçoivent des arguments **déjà validés** par le gestionnaire — un
/// `channel` strictement inférieur à [`channels`](Self::channels), un `level` déjà borné
/// à `[VOLUME_MIN, VOLUME_MAX]` et arrondi à un multiple de [`VOLUME_DELTA`] — de sorte
/// qu'aucune implémentation n'ait à refaire la validation, ni à pouvoir échouer.
///
/// IRQL : `PASSIVE_LEVEL` (les propriétés KS sont traitées en ligne dans le contexte du
/// fil appelant). Appels concurrents possibles depuis plusieurs fils : d'où les
/// atomiques, et non un verrou.
pub trait AudioNodes: Send + Sync + 'static {
    /// Nombre de canaux déclarés par le nœud de volume (2 pour un câble stéréo).
    ///
    /// Sert à valider le canal des requêtes : au-delà, `STATUS_INVALID_PARAMETER`. Doit
    /// être constant pour la vie du miniport et s'accorder avec le format de la broche.
    fn channels(&self) -> u32;

    /// Niveau du canal `channel`, en unités de 1/65536 dB.
    ///
    /// `channel < self.channels()` est garanti par l'appelant.
    fn volume(&self, channel: u32) -> i32;

    /// Fixe le niveau du canal `channel`.
    ///
    /// `level` est déjà borné et arrondi ; `channel < self.channels()` est garanti. Un
    /// `SET` « tous canaux » se traduit par un appel par canal déclaré.
    fn set_volume(&self, channel: u32, level: i32);

    /// État de la sourdine (un seul état pour le nœud, tous canaux confondus).
    fn muted(&self) -> bool;

    /// Fixe l'état de la sourdine.
    fn set_muted(&self, muted: bool);

    /// Point de trace, appelé une fois par requête `GET`/`SET`, après coup.
    ///
    /// Défaut : ne fait rien. Le pilote la câble sur `kmd_log!` en une ligne
    /// (`kmd_log!("{trace:?}")`) — c'est ce qui rend le décalage du `Channel` vérifiable
    /// au premier essai en machine (voir la documentation du module et [`Trace`]).
    ///
    /// Ne doit ni allouer ni bloquer : même IRQL que le reste du trait.
    fn trace(&self, trace: &Trace<'_>) {
        let _ = trace;
    }
}

// ---------------------------------------------------------------------------------
// Bornage et arrondi.
// ---------------------------------------------------------------------------------

/// Normalise une valeur de volume demandée par un client : bornée à
/// `[VOLUME_MIN, VOLUME_MAX]`, puis arrondie au multiple de [`VOLUME_DELTA`] le plus
/// proche (les demi-pas exacts vont vers le haut).
///
/// Les deux opérations sont nécessaires : borner évite le `SET` refusé qui ferait
/// apparaître l'endpoint cassé, arrondir évite qu'une lecture rende une valeur absente de
/// la plage annoncée par `BASICSUPPORT`.
fn normaliser(demande: i32) -> i32 {
    let borne = demande.clamp(VOLUME_MIN, VOLUME_MAX);
    // `borne + DEMI_PAS` ne peut pas déborder (`borne <= 0`), mais le lint
    // `arithmetic_side_effects` interdit le `+` nu et un débordement silencieux n'aurait
    // ici aucune raison d'exister : `checked_add`, et le maximum en repli.
    let arrondi = match borne.checked_add(DEMI_PAS) {
        Some(decale) => decale & MASQUE_PAS,
        None => VOLUME_MAX,
    };
    // Défensif : `MIN` et `MAX` sont tous deux des multiples exacts du pas, l'arrondi ne
    // peut donc pas sortir de la plage — mais rien dans le type ne le dit.
    arrondi.clamp(VOLUME_MIN, VOLUME_MAX)
}

// ---------------------------------------------------------------------------------
// Sérialisation champ par champ.
// ---------------------------------------------------------------------------------

/// Écrivain de champs à décalage **absolu** dans le tampon `Value`.
///
/// Jamais de transtypage vers un `*mut` de structure : `Value` n'est aligné sur rien
/// (frontière de confiance de [`crate::property`]), et une écriture désalignée par un
/// pointeur de structure est un comportement indéfini en Rust même là où x64 la tolère.
/// Chaque champ est copié par `get_mut(a..b)` + `copy_from_slice(&x.to_ne_bytes())`, à un
/// décalage nommé recopié du golden.
struct Champs<'a> {
    dest: &'a mut [u8],
}

impl Champs<'_> {
    /// Copie `src` au décalage `offset`. Ne fait rien si la place manque : les appelants
    /// choisissent leur palier avant d'écrire, cette garde n'est là que pour qu'aucun
    /// chemin ne puisse déborder ni paniquer.
    fn octets(&mut self, offset: usize, src: &[u8]) {
        let Some(fin) = offset.checked_add(src.len()) else {
            return;
        };
        if let Some(place) = self.dest.get_mut(offset..fin) {
            place.copy_from_slice(src);
        }
    }

    /// Un `ULONG` au décalage `offset`.
    fn u32(&mut self, offset: usize, valeur: u32) {
        self.octets(offset, &valeur.to_ne_bytes());
    }

    /// Un `LONG` au décalage `offset`.
    fn i32(&mut self, offset: usize, valeur: i32) {
        self.octets(offset, &valeur.to_ne_bytes());
    }

    /// Un `GUID` au décalage `offset`, champ par champ dans l'ordre de `guiddef.h`
    /// (`Data1: ULONG`, `Data2: USHORT`, `Data3: USHORT`, `Data4: [UCHAR; 8]`).
    fn guid(&mut self, offset: usize, valeur: &GUID) {
        self.u32(offset, valeur.Data1);
        // Décalages internes du GUID : 4, 6, 8. Écrits en dur plutôt que calculés, comme
        // partout ici, pour rester lisibles en regard de `guiddef.h`.
        let Some(base) = offset.checked_add(4) else {
            return;
        };
        self.octets(base, &valeur.Data2.to_ne_bytes());
        let Some(base) = offset.checked_add(6) else {
            return;
        };
        self.octets(base, &valeur.Data3.to_ne_bytes());
        let Some(base) = offset.checked_add(8) else {
            return;
        };
        self.octets(base, &valeur.Data4);
    }
}

/// Écrit la `KSPROPERTY_DESCRIPTION` (40 octets) commune aux deux propriétés.
///
/// `taille_totale` est la taille **complète** de la description (72 pour le volume, 40
/// pour la sourdine), écrite même quand on n'a la place que pour les 40 premiers octets :
/// c'est ce qui dit au client quoi redemander.
fn ecrire_description(champs: &mut Champs<'_>, taille_totale: u32, variante: u32, membres: u32) {
    champs.u32(D_ACCESSFLAGS, ACCESS_FLAGS);
    champs.u32(D_DESCRIPTIONSIZE, taille_totale);
    // PropTypeSet : KSIDENTIFIER de 24 octets — GUID, puis Id, puis Flags. C'est de là
    // que vient le décalage 32 de MembersListCount.
    champs.guid(D_PROPTYPESET_SET, &KSPROPTYPESETID_General);
    champs.u32(D_PROPTYPESET_ID, variante);
    champs.u32(D_PROPTYPESET_FLAGS, 0);
    champs.u32(D_MEMBERSLISTCOUNT, membres);
    champs.u32(D_RESERVED, 0);
}

/// Réponse à `KSPROPERTY_TYPE_BASICSUPPORT`, en paliers (voir la documentation du module).
///
/// `avec_plage` : `true` pour le volume (72 octets, un `KSPROPERTY_MEMBERSHEADER` et un
/// `KSPROPERTY_STEPPING_LONG`), `false` pour la sourdine (40 octets, aucun membre).
///
/// Renvoie les octets **écrits**, jamais une taille requise.
fn basic_support_ks(value: &mut [u8], variante: u32, avec_plage: bool) -> Result<u32, NtStatus> {
    let (taille, membres) = if avec_plage {
        (TAILLE_BASICSUPPORT_VOLUME, 1)
    } else {
        (TAILLE_DESCRIPTION, 0)
    };

    if value.len() < TAILLE_ACCESSFLAGS {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }

    let mut champs = Champs { dest: value };

    if champs.dest.len() < TAILLE_DESCRIPTION {
        // Premier appel de KS : `sizeof(ULONG)`, les seuls `AccessFlags`.
        champs.u32(D_ACCESSFLAGS, ACCESS_FLAGS);
        return Ok(TAILLE_ACCESSFLAGS as u32);
    }

    // Le palier intermédiaire, celui qu'on oublie : la place d'une description mais pas
    // des membres. On écrit 40 octets et on rend 40 — avec `DescriptionSize` à sa valeur
    // complète, pour que le client sache redemander 72.
    ecrire_description(&mut champs, taille as u32, variante, membres);
    if !avec_plage || champs.dest.len() < TAILLE_BASICSUPPORT_VOLUME {
        return Ok(TAILLE_DESCRIPTION as u32);
    }

    // KSPROPERTY_MEMBERSHEADER : une liste de plages échelonnées, uniforme sur tous les
    // canaux (`BASICSUPPORT_UNIFORM` : une seule plage vaut pour l'ensemble).
    champs.u32(M_MEMBERSFLAGS, KSPROPERTY_MEMBER_STEPPEDRANGES);
    champs.u32(M_MEMBERSSIZE, TAILLE_STEPPING_LONG as u32);
    champs.u32(M_MEMBERSCOUNT, 1);
    champs.u32(M_FLAGS, KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_UNIFORM);

    // KSPROPERTY_STEPPING_LONG : le pas, puis les bornes signées. C'est *littéralement*
    // ce que rendra `IAudioEndpointVolume::GetVolumeRange` : −96,0 / 0,0 / 0,5 dB.
    champs.u32(S_STEPPINGDELTA, VOLUME_DELTA);
    champs.u32(S_RESERVED, 0);
    champs.i32(S_MINIMUM, VOLUME_MIN);
    champs.i32(S_MAXIMUM, VOLUME_MAX);

    Ok(TAILLE_BASICSUPPORT_VOLUME as u32)
}

/// Lit un `LONG` en tête de `value` (tampon hostile et d'alignement quelconque).
fn lire_long(value: &[u8]) -> Result<i32, NtStatus> {
    let Some(quatre) = value.get(..TAILLE_VALEUR) else {
        return Err(STATUS_BUFFER_TOO_SMALL);
    };
    let mut mot = [0u8; TAILLE_VALEUR];
    mot.copy_from_slice(quatre);
    Ok(i32::from_ne_bytes(mot))
}

/// Écrit un `LONG` en tête de `value` s'il y tient (sinon rien : le thunk rendra la
/// taille requise et `STATUS_BUFFER_TOO_SMALL`).
fn ecrire_long(value: &mut [u8], valeur: i32) {
    if let Some(place) = value.get_mut(..TAILLE_VALEUR) {
        place.copy_from_slice(&valeur.to_ne_bytes());
    }
}

// ---------------------------------------------------------------------------------
// Les deux gestionnaires.
// ---------------------------------------------------------------------------------

/// `KSPROPERTY_AUDIO_VOLUMELEVEL` sur un nœud `KSNODETYPE_VOLUME` : le niveau par canal.
#[derive(Debug)]
pub struct Volume;

impl<T: AudioNodes> PropertyHandler<T> for Volume {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        let canal = Channel::decode(req.instance, req.target.channels());
        let niveau = canal.map(|c| {
            let lu = req.target.volume(c.a_lire());
            ecrire_long(value, lu);
            lu
        });
        req.target.trace(&Trace {
            property: PROP_VOLUME,
            verb: VERBE_GET,
            instance: req.instance,
            channel: canal,
            value: niveau.ok(),
        });
        // Taille **requise**, écrite ou non : c'est le contrat de `get` (le thunk en
        // déduit SUCCESS / BUFFER_TOO_SMALL / BUFFER_OVERFLOW).
        niveau.map(|_| TAILLE_VALEUR as u32)
    }

    fn set(req: &Request<'_, T>, value: &[u8]) -> Result<(), NtStatus> {
        let canal = Channel::decode(req.instance, req.target.channels());
        let retenu = canal.and_then(|c| {
            // Borner **et** arrondir : jamais d'erreur sur la valeur, seulement sur la
            // place ou sur le canal (voir la documentation du module).
            let retenu = normaliser(lire_long(value)?);
            match c {
                Channel::One(indice) => req.target.set_volume(indice, retenu),
                // `Channel == -1` : tous les canaux **déclarés**, et eux seuls.
                Channel::All => {
                    for indice in 0..req.target.channels() {
                        req.target.set_volume(indice, retenu);
                    }
                }
            }
            Ok(retenu)
        });
        req.target.trace(&Trace {
            property: PROP_VOLUME,
            verb: VERBE_SET,
            instance: req.instance,
            channel: canal,
            value: retenu.ok(),
        });
        retenu.map(|_| ())
    }

    fn basic_support(_req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        basic_support_ks(value, VARENUM::VT_I4 as u32, true)
    }
}

/// `KSPROPERTY_AUDIO_MUTE` sur un nœud `KSNODETYPE_MUTE` : la sourdine.
///
/// Un seul état pour le nœud, tous canaux confondus ; le canal de la requête est tout de
/// même décodé et validé, aux mêmes règles que le volume, pour que `Channel == -1` et un
/// canal hors bornes se comportent partout pareil.
#[derive(Debug)]
pub struct Mute;

impl<T: AudioNodes> PropertyHandler<T> for Mute {
    fn get(req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        let canal = Channel::decode(req.instance, req.target.channels());
        let etat = canal.map(|_| {
            let lu = i32::from(req.target.muted());
            ecrire_long(value, lu);
            lu
        });
        req.target.trace(&Trace {
            property: PROP_MUTE,
            verb: VERBE_GET,
            instance: req.instance,
            channel: canal,
            value: etat.ok(),
        });
        etat.map(|_| TAILLE_VALEUR as u32)
    }

    fn set(req: &Request<'_, T>, value: &[u8]) -> Result<(), NtStatus> {
        let canal = Channel::decode(req.instance, req.target.channels());
        let etat = canal.and_then(|_| {
            // `BOOL` de KS : tout non-nul vaut vrai.
            let brut = lire_long(value)?;
            let mute = brut != 0;
            req.target.set_muted(mute);
            Ok(i32::from(mute))
        });
        req.target.trace(&Trace {
            property: PROP_MUTE,
            verb: VERBE_SET,
            instance: req.instance,
            channel: canal,
            value: etat.ok(),
        });
        etat.map(|_| ())
    }

    fn basic_support(_req: &Request<'_, T>, value: &mut [u8]) -> Result<u32, NtStatus> {
        basic_support_ks(value, VARENUM::VT_BOOL as u32, false)
    }
}

// ---------------------------------------------------------------------------------
// Entrées de table prêtes à poser.
// ---------------------------------------------------------------------------------

/// `KSPROPSETID_Audio` en `static` : `PCPROPERTY_ITEM::Set` veut une adresse `'static`,
/// et les GUID de `portcls-sys` sont des `const`.
static SET_AUDIO: GUID = KSPROPSETID_Audio;

/// Entrée de `PCAUTOMATION_TABLE` du nœud de volume : `KSPROPSETID_Audio`,
/// `KSPROPERTY_AUDIO_VOLUMELEVEL`, `GET | SET | BASICSUPPORT`.
///
/// `const fn` : les tables d'automatisation du pilote sont des `static`. `V` est la vtable
/// du miniport qui porte la table (`IMiniportTopologyVtbl`), `T` son type — c'est ce
/// couple que la garde de vtable de [`crate::property::handler`] vérifie.
pub const fn volume_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: AudioNodes,
{
    property::item::<V, T, Volume>(
        &SET_AUDIO,
        KSPROPERTY_AUDIO::KSPROPERTY_AUDIO_VOLUMELEVEL as u32,
        ACCESS_FLAGS,
    )
}

/// Entrée de `PCAUTOMATION_TABLE` du nœud de sourdine : `KSPROPSETID_Audio`,
/// `KSPROPERTY_AUDIO_MUTE`, `GET | SET | BASICSUPPORT`.
pub const fn mute_item<V, T>() -> PCPROPERTY_ITEM
where
    V: TargetVtbl<T>,
    T: AudioNodes,
{
    property::item::<V, T, Mute>(
        &SET_AUDIO,
        KSPROPERTY_AUDIO::KSPROPERTY_AUDIO_MUTE as u32,
        ACCESS_FLAGS,
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

    /// Les tailles des paliers sont bien la somme des structures du golden.
    #[test]
    fn les_paliers_sont_la_somme_des_structures() {
        assert_eq!(
            TAILLE_BASICSUPPORT_VOLUME,
            TAILLE_DESCRIPTION + TAILLE_MEMBERSHEADER + TAILLE_STEPPING_LONG
        );
        assert_eq!(TAILLE_DESCRIPTION, 40);
        assert_eq!(TAILLE_MEMBERSHEADER, 16);
        assert_eq!(TAILLE_STEPPING_LONG, 16);
    }

    /// L'échelle : −96 dB en unités de 1/65536 dB, pas de 0,5 dB, plage alignée sur le pas.
    #[test]
    fn l_echelle_est_en_soixante_cinq_mille_cinq_cent_trente_sixiemes_de_decibel() {
        assert_eq!(VOLUME_MIN, -6_291_456);
        assert_eq!(VOLUME_MIN / 0x1_0000, -96);
        assert_eq!(VOLUME_MAX, 0);
        assert_eq!(VOLUME_DELTA, 0x8000);
        // 0x8000 / 0x10000 = 0,5 dB.
        assert_eq!(VOLUME_DELTA * 2, 0x1_0000);
        // Les deux bornes sont des multiples exacts du pas : l'arrondi ne peut pas sortir
        // de la plage.
        assert_eq!(VOLUME_MIN % (VOLUME_DELTA as i32), 0);
        assert_eq!(VOLUME_MAX % (VOLUME_DELTA as i32), 0);
    }

    /// `MASQUE_PAS` et `DEMI_PAS` sont bien dérivés de `VOLUME_DELTA`.
    #[test]
    fn le_masque_d_arrondi_derive_du_pas() {
        assert_eq!(DEMI_PAS, (VOLUME_DELTA / 2) as i32);
        assert_eq!(MASQUE_PAS, !((VOLUME_DELTA as i32) - 1));
    }

    /// Bornage puis arrondi au multiple le plus proche, demis vers le haut.
    #[test]
    fn normaliser_borne_puis_arrondit() {
        assert_eq!(normaliser(0), 0);
        assert_eq!(normaliser(i32::MAX), VOLUME_MAX);
        assert_eq!(normaliser(i32::MIN), VOLUME_MIN);
        assert_eq!(normaliser(VOLUME_MIN - 1), VOLUME_MIN);
        assert_eq!(normaliser(-0x8000), -0x8000);
        // Juste sous le demi-pas : arrondi vers le haut (0).
        assert_eq!(normaliser(-0x3FFF), 0);
        // Le demi-pas exact : vers le haut.
        assert_eq!(normaliser(-0x4000), 0);
        // Juste au-delà : vers le bas.
        assert_eq!(normaliser(-0x4001), -0x8000);
        // Tout résultat est un multiple du pas et reste dans la plage.
        for brut in [-1, -12_345, -1_000_000, -6_000_000, -6_291_455, 12_345] {
            let n = normaliser(brut);
            assert_eq!(n % (VOLUME_DELTA as i32), 0, "brut {brut}");
            assert!((VOLUME_MIN..=VOLUME_MAX).contains(&n), "brut {brut}");
        }
    }

    /// Décodage du canal : les cinq cas.
    #[test]
    fn decodage_du_canal() {
        let instance = |canal: i32| canal.to_ne_bytes();
        assert_eq!(Channel::decode(&instance(0), 2), Ok(Channel::One(0)));
        assert_eq!(Channel::decode(&instance(1), 2), Ok(Channel::One(1)));
        assert_eq!(Channel::decode(&instance(-1), 2), Ok(Channel::All));
        assert_eq!(
            Channel::decode(&instance(2), 2),
            Err(STATUS_INVALID_PARAMETER)
        );
        assert_eq!(
            Channel::decode(&instance(-2), 2),
            Err(STATUS_INVALID_PARAMETER)
        );
        assert_eq!(Channel::decode(&[], 2), Err(STATUS_INVALID_PARAMETER));
        assert_eq!(
            Channel::decode(&[0, 0, 0], 2),
            Err(STATUS_INVALID_PARAMETER)
        );
        assert_eq!(
            Channel::decode(&instance(0), 0),
            Err(STATUS_INVALID_PARAMETER)
        );
    }
}
