//! Descripteurs de filtres KS des quatre sous-périphériques d'un câble
//! (driver-design.md §4.1) : `WaveRender`, `TopoRender`, `WaveCapture`, `TopoCapture`.
//!
//! PortCls **conserve les pointeurs** qu'il reçoit de `IMiniport::GetDescription` pour
//! toute la vie du filtre : chaque table est donc une `static` construite en `const`, et
//! les pointeurs qu'elle contient (`Pins`, `DataRanges`, `Connections`,
//! `AutomationTable`, `Category`) visent d'autres `static` de ce module. Les types
//! générés contiennent des pointeurs bruts et ne sont pas `Sync` ; l'enveloppe
//! [`Shared`] le déclare, ce qui est justifié : ces données sont immuables et ne sont
//! jamais écrites, ni par le pilote ni par PortCls (les prototypes `PPCFILTER_DESCRIPTOR`
//! non `const` sont une facilité du header, SYSVAD aussi les déclare `static`).
//!
//! Deux broches par filtre et catégories laissées à PortCls (`CategoryCount = 0` :
//! `KSCATEGORY_AUDIO`, `KSCATEGORY_RENDER`/`CAPTURE`, `KSCATEGORY_REALTIME` pour les ports
//! WaveRT, `KSCATEGORY_AUDIO`, `KSCATEGORY_TOPOLOGY` pour les ports topologie, comme
//! SYSVAD). Au-delà, les deux formes de filtre diffèrent :
//!
//! - **forme wave** ([`wave_filter`]), pour `WaveRender` et `WaveCapture` : aucun nœud, une
//!   connexion directe broche 0 → broche 1 (`PCFILTER_NODE`, [`DIRECT_CONNECTION`]), et une
//!   table d'automatisation qui ne porte, depuis M1b-22, qu'une seule propriété —
//!   `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` ([`WAVE_RENDER_FILTER_ITEMS`],
//!   [`WAVE_CAPTURE_FILTER_ITEMS`]) ; tout le reste (`KSPROPSETID_Pin`,
//!   `KSPROPSETID_Topology`) demeure servi par PortCls ;
//! - **forme topologie** ([`topo_filter`]), pour `TopoRender` et `TopoCapture` : deux nœuds
//!   — index [`NODE_VOLUME`] `KSNODETYPE_VOLUME`, index [`NODE_MUTE`] `KSNODETYPE_MUTE` —
//!   chacun avec sa table d'automatisation d'une propriété, et trois connexions
//!   ([`TOPO_CONNECTIONS`]) qui les mettent en série entre les deux broches. La table
//!   d'automatisation du *filtre* porte, depuis M1b-03, les propriétés qui ne sont ni de
//!   nœud ni de broche : `KSPROPERTY_JACK_DESCRIPTION`, puis les deux propriétés du jeu
//!   privé `KSPROPSETID_Conduit` de M1b-04 (état du câble, version du contrat) — et,
//!   depuis M1b-04 également, l'**événement** `KSEVENT_PINCAPS_JACKINFOCHANGE`, sans lequel
//!   Windows ne relirait jamais le jack après un changement d'état.
//!
//! # Le jack est une propriété du **filtre** (M1b-03)
//!
//! Contre-intuitif, et la documentation Microsoft y insiste : la valeur décrit une broche,
//! mais « *the property is a property of the filter, not of the pin* ». L'entrée va donc
//! dans `PCFILTER_DESCRIPTOR::AutomationTable` ([`TOPO_RENDER_AUTOMATION`],
//! [`TOPO_CAPTURE_AUTOMATION`]) et le `PCPIN_DESCRIPTOR::AutomationTable` de toutes nos
//! broches reste **nul** — ce qui n'est pas qu'une question de conformité : une broche
//! bridge ne s'instancie pas (`MaxInstanceCount = 0`), une table posée là ne serait jamais
//! atteinte. Le client interroge le filtre et désigne la broche par un `KSP_PIN` dont
//! `PortCls` laisse le `PinId` dans `Instance`.
//!
//! La broche décrite est la broche **endpoint** ([`TOPO_RENDER_PIN_ENDPOINT`],
//! [`TOPO_CAPTURE_PIN_ENDPOINT`]) : c'est elle qui fait face au périphérique d'extrémité,
//! c'est d'elle que Windows tire l'endpoint, et c'est elle que la documentation appelle
//! « bridge pin ». Notre [`bridge_pin`] à nous désigne autre chose — la broche interne vers
//! le filtre WaveRT —, et cette collision de vocabulaire est le piège de M1b-03 : la
//! propriété répondrait quand même, mais toujours par une description vide.
//!
//! # Pourquoi ces nœuds existent (M1b-03b, driver-design.md §5.5)
//!
//! Faute de nœud de volume, Windows insère son **APO logiciel** et applique aux trames le
//! volume par défaut qu'il donne à tout endpoint neuf — mesuré à 64 %, soit une amplitude
//! de 0,229 pour 0,500 demandée. Le mécanisme qui corrige cela a **deux moitiés** :
//! *exposer* le nœud, ce que fait ce module, et *ne pas appliquer* la valeur que Windows y
//! pousse aussitôt, ce que garantit `cable::NodeState`. Le signal traverse `copy_frames`
//! inchangé, octet pour octet ; le curseur de volume d'un endpoint Conduit est décoratif,
//! et c'est volontaire.
//!
//! # Ce qui est *par câble*, et ce qui ne l'est pas
//!
//! Deux contenus varient, et pas selon le même axe.
//!
//! - Le GUID `KsPinDescriptor.Name` des broches endpoint ([`PIN_NAMES`], un par câble
//!   depuis M1b-02) varie **par câble** : les seize filtres de topologie d'un même sens ne
//!   diffèrent que par lui, d'où les tables [`TOPO_RENDER_PINS`] et [`TOPO_CAPTURE_PINS`],
//!   une rangée par câble, et les deux tableaux de [`PCFILTER_DESCRIPTOR`] qui les
//!   pointent.
//! - Les **plages système** des filtres wave varient, depuis M1b-05, par **variante de
//!   format** : la fréquence et le nombre de canaux du câble, soit
//!   `3 × 8 = `[`VARIANT_COUNT`] combinaisons (`conduit_kmd_core::variant_of`, qui **n'est
//!   plus ici** : l'arithmétique des variantes est dans le crate portable depuis la
//!   correction de M1b-05, pour être testée en mode utilisateur). Les filtres wave sont donc
//!   eux aussi des tables — [`WAVE_RENDER_FILTERS`], [`WAVE_CAPTURE_FILTERS`] — mais
//!   indexées par variante et non par câble : deux câbles réglés pareil partagent la même
//!   rangée, et c'est ce qui garde la table à 24 entrées au lieu de 16.
//!
//! # Le mode de traitement du signal (M1b-22, driver-design.md §4.3)
//!
//! Les broches de **flux** wave — celles-là seules, jamais les ponts ni les topologies —
//! déclarent le mode `AUDIO_SIGNALPROCESSINGMODE_DEFAULT`, et lui seul, en **deux**
//! endroits que la documentation Microsoft sépare nettement :
//!
//! - chacune de leurs plages porte le drapeau `KSDATARANGE_ATTRIBUTES` ([`audio_range`]) et
//!   est **suivie**, dans le tableau `DataRanges`, d'une entrée qui n'est pas une plage
//!   mais une `KSATTRIBUTE_LIST` d'un seul élément ([`MODE_ATTRIBUTE_ENTRY`]). Cet attribut
//!   ne nomme aucun mode : il dit « cette broche sait qu'un mode existe ». D'où
//!   [`ENTRIES_PER_SYSTEM_PIN`] = 2 × [`RANGES_PER_SYSTEM_PIN`] ;
//! - la propriété `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES`, sur la table d'automatisation
//!   du **filtre** wave, énumère les modes servis (`portcls::modes`).
//!
//! C'est la dernière condition, hors mode paquets, qui distinguait Conduit de SYSVAD ;
//! l'expérience qu'elle sert est décrite dans driver-design.md §4.3.
//!
//! Un câble ne change **jamais** de variante en cours de route : `Shared<T>` est `Sync`
//! *parce que* son contenu ne change jamais, et PortCls conserve le pointeur rendu par
//! `GetDescription` pour toute la vie du filtre. Le format se lit une fois, au démarrage
//! ([`apply_cable_format`], depuis `registry::read_params`), et en changer demande de
//! redémarrer le devnode — ce que fait le service d'assistance, en espace utilisateur.
//!
//! Les nœuds, leurs tables d'automatisation et leurs `PCPROPERTY_ITEM` sont des `static`
//! **partagées par tous les câbles** : le gestionnaire retrouve le
//! miniport — donc le câble, donc le `NodeState` — par le `MajorTarget` de la requête. Ce
//! qui se dédouble ici se dédouble par **sens**, pas par câble : `volume_item::<V, T>` est
//! monomorphisé sur le type du miniport (`TopoRender` ou `TopoCapture`), et la garde de
//! vtable de `portcls::property::handler` compare l'adresse de `T::VTBL` — une table
//! commune aux deux sens rendrait le pilote muet d'un côté.
//!
//! | Filtre | Broche | Flux | Communication | Rôle |
//! |---|---|---|---|---|
//! | `WaveRender` | [`WAVE_RENDER_PIN_SYSTEM`] = 0 | `IN` | `SINK` | le lecteur écrit ici (plages système) |
//! | | [`WAVE_RENDER_PIN_BRIDGE`] = 1 | `OUT` | `NONE` | bridge analogique vers `TopoRender` |
//! | `TopoRender` | [`TOPO_RENDER_PIN_BRIDGE`] = 0 | `IN` | `NONE` | bridge depuis `WaveRender` |
//! | | [`TOPO_RENDER_PIN_ENDPOINT`] = 1 | `OUT` | `NONE` | `KSNODETYPE_LINE_CONNECTOR` : l'endpoint (voir [`CATEGORY_LINE_CONNECTOR`]) |
//! | `WaveCapture` | [`WAVE_CAPTURE_PIN_BRIDGE`] = 0 | `IN` | `NONE` | bridge depuis `TopoCapture` |
//! | | [`WAVE_CAPTURE_PIN_SYSTEM`] = 1 | `OUT` | `SINK` | l'enregistreur lit ici (plages système) |
//! | `TopoCapture` | [`TOPO_CAPTURE_PIN_ENDPOINT`] = 0 | `IN` | `NONE` | `KSNODETYPE_LINE_CONNECTOR` : l'endpoint |
//! | | [`TOPO_CAPTURE_PIN_BRIDGE`] = 1 | `OUT` | `NONE` | bridge vers `WaveCapture` |
//!
//! Les deux broches endpoint sont les seules à porter un `KsPinDescriptor.Name`
//! (`portcls::pin_name_guid(n)`, M1a-09) : c'est ce GUID que KS résout en
//! « Conduit *n+1* » par la clé `HKR\MediaCategories` que l'INF écrit, et donc le nom que
//! l'utilisateur voit (driver-design.md §4.2). Toutes les autres broches laissent `Name`
//! nul.
//!
//! Les numéros de broche suivent la direction des données, comme dans §4.1 et dans
//! SYSVAD : la broche 0 est toujours l'entrée (`KSPIN_DATAFLOW_IN`), la broche 1 la
//! sortie ; c'est pourquoi les constantes sont nommées par filtre et non par rôle.
//!
//! Ce crate ne se teste pas en mode utilisateur : les invariants (nombre de broches,
//! orientation, tailles des structures, cohérence avec la matrice de
//! `conduit_kmd_core::format`, bonne formation du graphe de topologie et des tables
//! d'automatisation) sont des assertions `const`, vérifiées à la compilation. Deux d'entre
//! elles attrapent des pannes muettes : une table de connexions incohérente ne donne
//! « aucun endpoint, aucun message d'erreur », et une table d'automatisation mal remplie
//! fait taire la propriété sans rien signaler.
//!
//! # Ce que les assertions `const` ne voient pas, et qui le voit (correction de M1b-05)
//!
//! Elles vérifient les **tables**, c'est-à-dire ce qui est écrit à la compilation. Elles ne
//! disent rien de ce qui se décide au **démarrage** : quel format le registre a posé dans
//! [`CABLE_FORMATS`], quelle rangée `wave::description` en tirera, ni ce que PortCls lira au
//! bout des pointeurs `DataRanges`. C'est là que se logeait le seul repli muet du module —
//! les `unwrap_or_else` de `wave.rs`, qui rendent la variante du défaut sans que rien ne le
//! dise. [`check_cable_pins`] referme cet intervalle : `adapter::start_device` l'appelle
//! pour chaque câble de la réserve, **avant** le premier `GetDescription`, et une divergence
//! part au journal d'événements — donc visible en release, sans débogueur, là où
//! `kmd_log!` est vide.

use core::fmt;
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use conduit_kmd_core::config::{CABLE_FORMAT_DEFAULT, CableFormat};
use conduit_kmd_core::{
    FORMATS_PER_CABLE, FrameLayout, SAMPLE_DEPTHS, SAMPLE_RATES, SampleFormat, VARIANT_COUNT,
};
use portcls::{
    CABLE_COUNT, CABLE_STATE_ACCESS_FLAGS, COUNTERS_ACCESS_FLAGS, JACK_ACCESS_FLAGS,
    JACK_EVENT_FLAGS, JACK_INFO_CHANGE_ID, MODES_ACCESS_FLAGS, PIN_NAME_GUIDS,
    VERSION_ACCESS_FLAGS, cable_state_item, counters_item, jack_description_item,
    jack_info_change_item, mute_item, signal_processing_modes_item, version_item, volume_item,
    with_events,
};
use portcls_sys::{
    GUID, IMiniportTopologyVtbl, IMiniportWaveRTVtbl, KSATTRIBUTE, KSATTRIBUTE_LIST,
    KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE, KSCATEGORY_AUDIO, KSDATAFORMAT,
    KSDATAFORMAT__bindgen_ty_1, KSDATAFORMAT_SPECIFIER_NONE, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
    KSDATAFORMAT_SUBTYPE_ANALOG, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, KSDATAFORMAT_SUBTYPE_PCM,
    KSDATAFORMAT_TYPE_AUDIO, KSDATARANGE, KSDATARANGE_ATTRIBUTES, KSDATARANGE_AUDIO,
    KSNODEPIN_STANDARD_IN, KSNODEPIN_STANDARD_OUT, KSNODETYPE_LINE_CONNECTOR, KSNODETYPE_MUTE,
    KSNODETYPE_SPEAKER, KSNODETYPE_VOLUME, KSPIN_COMMUNICATION, KSPIN_DATAFLOW, KSPIN_DESCRIPTOR,
    KSPIN_DESCRIPTOR__bindgen_ty_1, PCAUTOMATION_TABLE, PCCONNECTION_DESCRIPTOR, PCEVENT_ITEM,
    PCFILTER_DESCRIPTOR, PCFILTER_NODE, PCMETHOD_ITEM, PCNODE_DESCRIPTOR, PCPIN_DESCRIPTOR,
    PCPROPERTY_ITEM, PKSATTRIBUTE, PKSDATARANGE, ULONG,
};

use crate::topo::{TopoCapture, TopoRender};
use crate::wave::{WaveCapture, WaveRender};

// ---------------------------------------------------------------------------------
// Numéros de broche (§4.1), du type que `PcRegisterPhysicalConnection` et les
// `PCCONNECTION_DESCRIPTOR` attendent.
// ---------------------------------------------------------------------------------

/// `WaveRender` : broche système (`KSPIN_DATAFLOW_IN`, `SINK`) où le lecteur écrit.
pub const WAVE_RENDER_PIN_SYSTEM: ULONG = 0;
/// `WaveRender` : broche bridge (`KSPIN_DATAFLOW_OUT`) vers `TopoRender`.
pub const WAVE_RENDER_PIN_BRIDGE: ULONG = 1;
/// `TopoRender` : broche bridge (`KSPIN_DATAFLOW_IN`) depuis `WaveRender`.
pub const TOPO_RENDER_PIN_BRIDGE: ULONG = 0;
/// `TopoRender` : broche endpoint (`KSPIN_DATAFLOW_OUT`, `KSNODETYPE_SPEAKER`).
pub const TOPO_RENDER_PIN_ENDPOINT: ULONG = 1;
/// `WaveCapture` : broche bridge (`KSPIN_DATAFLOW_IN`) depuis `TopoCapture`.
pub const WAVE_CAPTURE_PIN_BRIDGE: ULONG = 0;
/// `WaveCapture` : broche système (`KSPIN_DATAFLOW_OUT`, `SINK`) où l'enregistreur lit.
pub const WAVE_CAPTURE_PIN_SYSTEM: ULONG = 1;
/// `TopoCapture` : broche endpoint (`KSPIN_DATAFLOW_IN`, `KSNODETYPE_LINE_CONNECTOR`).
pub const TOPO_CAPTURE_PIN_ENDPOINT: ULONG = 0;
/// `TopoCapture` : broche bridge (`KSPIN_DATAFLOW_OUT`) vers `WaveCapture`.
pub const TOPO_CAPTURE_PIN_BRIDGE: ULONG = 1;

/// Nombre de broches de chaque filtre du spike.
pub const PIN_COUNT: usize = 2;

/// Broche **d'entrée** d'un filtre topologie, quel que soit le sens.
///
/// La numérotation des broches suit déjà le flux des deux côtés (0 = entrée, 1 = sortie) :
/// c'est ce qui permet aux trois connexions de [`TOPO_CONNECTIONS`] d'être **identiques**
/// pour le rendu et pour la capture, alors que la broche 0 y est un bridge d'un côté et un
/// endpoint de l'autre. L'assertion `const` ci-dessous relie ces deux constantes aux
/// quatre numéros nommés par filtre : le jour où l'un d'eux bougerait, la compilation
/// s'arrête ici plutôt que de laisser une topologie inversée.
const TOPO_PIN_IN: ULONG = 0;
/// Broche **de sortie** d'un filtre topologie, quel que soit le sens (voir
/// [`TOPO_PIN_IN`]).
const TOPO_PIN_OUT: ULONG = 1;

const _: () = {
    assert!(TOPO_PIN_IN == TOPO_RENDER_PIN_BRIDGE && TOPO_PIN_IN == TOPO_CAPTURE_PIN_ENDPOINT);
    assert!(TOPO_PIN_OUT == TOPO_RENDER_PIN_ENDPOINT && TOPO_PIN_OUT == TOPO_CAPTURE_PIN_BRIDGE);
};

/// Index du nœud `KSNODETYPE_VOLUME` dans les deux filtres topologie.
pub const NODE_VOLUME: ULONG = 0;
/// Index du nœud `KSNODETYPE_MUTE` dans les deux filtres topologie.
pub const NODE_MUTE: ULONG = 1;
/// Nombre de nœuds d'un filtre topologie.
pub const NODE_COUNT: usize = 2;
/// Nombre de connexions d'un filtre topologie : broche 0 → volume → sourdine → broche 1.
pub const TOPO_CONNECTION_COUNT: usize = 3;
/// Nombre de connexions d'un filtre WaveRT : la connexion directe broche 0 → broche 1.
pub const WAVE_CONNECTION_COUNT: usize = 1;

// ---------------------------------------------------------------------------------
// La matrice des formats (M1b-05, §5.4) : 3 fréquences × 8 canaux = 24 variantes, chacune
// déclarant les 3 profondeurs de `conduit_kmd_core::SAMPLE_DEPTHS`.
// ---------------------------------------------------------------------------------

/// Nombre maximal de canaux par câble (SPEC F-03), du type des tables d'ici.
pub const MAX_CHANNELS: usize = FrameLayout::MAX_CHANNELS as usize;

/// Nombre de fréquences d'échantillonnage servables (`conduit_kmd_core::SAMPLE_RATES`).
pub const RATE_COUNT: usize = SAMPLE_RATES.len();

/// Nombre de plages système par broche : une par profondeur.
pub const RANGES_PER_SYSTEM_PIN: usize = FORMATS_PER_CABLE;

/// Nombre d'**entrées** du tableau `DataRanges` d'une broche système : deux par plage
/// depuis M1b-22 — la plage, puis la `KSATTRIBUTE_LIST` qu'elle annonce.
///
/// `DataRangesCount` compte les deux : le tableau est un tableau de `PKSDATARANGE`, et une
/// entrée d'attributs y occupe une case comme une plage (voir [`MODE_ATTRIBUTE_ENTRY`] et
/// la documentation de `KSDATARANGE::Flags`). Confondre les deux comptes est exactement la
/// faute que [`wave_pins_are_well_formed`] attrape.
pub const ENTRIES_PER_SYSTEM_PIN: usize = RANGES_PER_SYSTEM_PIN * 2;

/// L'index de variante du format par défaut (48 kHz, 2 canaux) : le repli de toutes les
/// fonctions de sélection, et le seul index dont on puisse prouver l'existence en `const`.
pub const DEFAULT_VARIANT: usize = match CABLE_FORMAT_DEFAULT.variant() {
    Some(index) => index,
    // Inatteignable : le défaut est dans le domaine, ce que l'assertion `const` de
    // `conduit_kmd_core::config` établit. Un `0` plutôt qu'un `panic!`, interdit ici.
    None => 0,
};

/// Fréquence de la variante `variant`, en Hz ; celle du défaut hors domaine.
///
/// Total, parce que les constructeurs `const` des tables ne savent ni paniquer ni traiter
/// une `Option` ; le repli est **inatteignable** pour `variant < VARIANT_COUNT` — c'est
/// `conduit_kmd_core::variant_rate` qui le garantit, et
/// [`variant_ranges_are_well_formed`] le recoupe sur les valeurs réellement bâties.
const fn variant_rate(variant: usize) -> ULONG {
    match conduit_kmd_core::variant_rate(variant) {
        Some(hz) => hz,
        None => CABLE_FORMAT_DEFAULT.sample_rate,
    }
}

/// Nombre de canaux de la variante `variant` ; celui du défaut hors domaine (voir
/// [`variant_rate`]).
const fn variant_channels(variant: usize) -> ULONG {
    match conduit_kmd_core::variant_channels(variant) {
        Some(canaux) => canaux as ULONG,
        None => CABLE_FORMAT_DEFAULT.channels as ULONG,
    }
}

const _: () = {
    assert!(RATE_COUNT == 3 && MAX_CHANNELS == 8);
    assert!(VARIANT_COUNT == 24, "3 fréquences × 8 canaux, pas 72");
    assert!(SAMPLE_DEPTHS.len() == RANGES_PER_SYSTEM_PIN && RANGES_PER_SYSTEM_PIN == 3);
    // La réciprocité de l'indexation est vérifiée — et **testée en mode utilisateur** —
    // dans `conduit_kmd_core::format` depuis la correction de M1b-05 ; ici on ne garde que
    // le lien avec les tables de ce module : le défaut a bien une variante, et c'est celle
    // qu'on croit (48 kHz, rang 1, sur 2 canaux : 1 × 8 + 1 = 9).
    assert!(DEFAULT_VARIANT == 9 && DEFAULT_VARIANT < VARIANT_COUNT);
};

// ---------------------------------------------------------------------------------
// Enveloppe `Sync` des tables.
// ---------------------------------------------------------------------------------

/// Table immuable partagée avec PortCls par pointeur, logée dans une `static`.
///
/// `#[repr(transparent)]` : l'adresse de l'enveloppe est celle de la table, ce que les
/// constructeurs exploitent (`ptr::from_ref(&STATIC).cast()`).
#[repr(transparent)]
pub struct Shared<T>(T);

// SAFETY: les tables sont écrites une fois, à la compilation, et ne sont plus jamais
// modifiées : ni par le pilote (aucun accès `&mut`, aucune mutabilité intérieure), ni
// par PortCls (lecture seule, prototypes non `const` par facilité du header). Des
// lectures concurrentes de données immuables sont sûres.
unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    /// La table.
    pub const fn get(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Shared(<table KS>)")
    }
}

// ---------------------------------------------------------------------------------
// Constructeurs `const`.
// ---------------------------------------------------------------------------------

/// Taille de `KSDATARANGE_AUDIO` telle que `FormatSize` l'annonce (88 octets).
const KSDATARANGE_AUDIO_SIZE: ULONG = size_of::<KSDATARANGE_AUDIO>() as ULONG;
/// Taille de `KSDATARANGE` (64 octets).
const KSDATARANGE_SIZE: ULONG = size_of::<KSDATARANGE>() as ULONG;

const _: () = {
    assert!(
        KSDATARANGE_AUDIO_SIZE == 88,
        "KSDATARANGE_AUDIO fait 88 octets sur x64"
    );
    assert!(KSDATARANGE_SIZE == 64, "KSDATARANGE fait 64 octets");
    assert!(size_of::<PCPIN_DESCRIPTOR>() == 112);
    assert!(size_of::<PCFILTER_DESCRIPTOR>() == 80);
    // Au golden depuis M1b-03b (`portcls-sys/tests/layout.golden`) : ces deux tailles sont
    // annoncées à PortCls dans `NodeSize` et `PropertyItemSize`, qui s'en sert pour
    // avancer dans les tableaux. Une erreur ici ne se voit pas, elle se lit.
    assert!(size_of::<PCNODE_DESCRIPTOR>() == 32);
    assert!(size_of::<PCPROPERTY_ITEM>() == 24);
};

/// En-tête `KSDATAFORMAT` d'une plage audio : `FormatSize = size`, `Flags = flags`, type
/// majeur `KSDATAFORMAT_TYPE_AUDIO`.
///
/// `flags` ne prend que deux valeurs dans ce module : 0 pour la plage analogique des
/// broches bridge, [`KSDATARANGE_ATTRIBUTES`] pour les plages système, qui annoncent ainsi
/// la liste d'attributs logée juste derrière elles dans le tableau `DataRanges` (voir
/// [`MODE_ATTRIBUTE_ENTRY`]).
const fn data_format(size: ULONG, flags: ULONG, subtype: GUID, specifier: GUID) -> KSDATAFORMAT {
    KSDATAFORMAT {
        __bindgen_anon_1: KSDATAFORMAT__bindgen_ty_1 {
            FormatSize: size,
            Flags: flags,
            SampleSize: 0,
            Reserved: 0,
            MajorFormat: KSDATAFORMAT_TYPE_AUDIO,
            SubFormat: subtype,
            Specifier: specifier,
        },
    }
}

/// Plage système (`KSDATARANGE_AUDIO`) : spécificateur `WAVEFORMATEX`, sous-type
/// `subtype` (`KSDATAFORMAT_SUBTYPE_PCM` ou `IEEE_FLOAT`), `bits` par échantillon
/// exactement, `channels` canaux, `rate` Hz exactement (§4.1).
///
/// **Ponctuelle des deux côtés** : minimum et maximum confondus pour la profondeur comme
/// pour la fréquence, et `MaximumChannels` unique. Une plage ponctuelle ne laisse rien à
/// choisir à qui l'intersecte, ce qui est exactement le but : le format proposé au moteur
/// audio est toujours un de ceux que `NewStream` acceptera.
///
/// Depuis M1b-21, c'est **notre** gestionnaire qui intersecte ([`crate::intersect`]) et non
/// plus celui de PortCls, dont la documentation limite le défaut au PCM, au mono et au
/// stéréo, et exclut tout `WAVEFORMATEXTENSIBLE`. Attention au piège de `MaximumChannels` :
/// une `KSDATARANGE_AUDIO` n'a pas de minimum, donc cette plage dit littéralement « un à
/// `channels` canaux » alors que le câble n'en sert qu'un nombre. C'est le gestionnaire qui
/// referme l'écart, en n'acceptant que le compte exact.
///
/// # `KSDATARANGE_ATTRIBUTES` (M1b-22)
///
/// Le `Flags` de l'en-tête ne vaut plus 0 : il porte `KSDATARANGE_ATTRIBUTES`, qui annonce
/// que l'**entrée suivante** du tableau `DataRanges` n'est pas une plage mais une
/// `KSATTRIBUTE_LIST` — celle de [`MODE_ATTRIBUTE_ENTRY`], qui déclare la broche « mode
/// aware » (voir l'en-tête de `portcls::modes`). C'est littéralement ce que dit la
/// documentation de `KSDATARANGE::Flags` : « *Set Flags to KSDATARANGE_ATTRIBUTES (0x2) to
/// indicate that the following KSDATARANGE is to be interpreted as an attribute list* ».
pub const fn audio_range(
    subtype: GUID,
    bits: ULONG,
    channels: ULONG,
    rate: ULONG,
) -> KSDATARANGE_AUDIO {
    KSDATARANGE_AUDIO {
        DataRange: data_format(
            KSDATARANGE_AUDIO_SIZE,
            KSDATARANGE_ATTRIBUTES,
            subtype,
            KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
        ),
        MaximumChannels: channels,
        MinimumBitsPerSample: bits,
        MaximumBitsPerSample: bits,
        MinimumSampleFrequency: rate,
        MaximumSampleFrequency: rate,
    }
}

/// Le sous-type KS d'une profondeur : `IEEE_FLOAT` pour le flottant, `PCM` pour les deux
/// entiers.
///
/// Le bras `_` existe parce que `SampleFormat` est `#[non_exhaustive]` et vient d'un autre
/// crate : il est **inatteignable** pour les trois profondeurs de `SAMPLE_DEPTHS`, ce que
/// [`variant_ranges_are_well_formed`] vérifie en comparant le sous-type de chaque plage
/// bâtie. Sans cette vérification, une quatrième profondeur ajoutée un jour se déclarerait
/// en silence comme du PCM — une plage flottante annoncée en entier est exactement le genre
/// de faute qui ne se voit qu'à l'écoute.
const fn subtype_of(depth: SampleFormat) -> GUID {
    match depth {
        SampleFormat::F32 => KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        SampleFormat::Pcm24 | SampleFormat::I16 => KSDATAFORMAT_SUBTYPE_PCM,
        _ => KSDATAFORMAT_SUBTYPE_PCM,
    }
}

/// Les trois plages système de la variante `variant` : une par profondeur de
/// `SAMPLE_DEPTHS`, à la fréquence et sur le nombre de canaux de la variante.
const fn variant_ranges(variant: usize) -> [KSDATARANGE_AUDIO; RANGES_PER_SYSTEM_PIN] {
    let channels = variant_channels(variant);
    let rate = variant_rate(variant);
    // Motif de tranche plutôt qu'indexation : `SAMPLE_DEPTHS` a exactement trois entrées,
    // et les déstructurer ici fait échouer la compilation si elle en gagnait une — plutôt
    // que de déclarer en silence une profondeur de moins que la matrice n'en annonce.
    let [d0, d1, d2] = SAMPLE_DEPTHS;
    [
        audio_range(subtype_of(d0), d0.bits_per_sample(), channels, rate),
        audio_range(subtype_of(d1), d1.bits_per_sample(), channels, rate),
        audio_range(subtype_of(d2), d2.bits_per_sample(), channels, rate),
    ]
}

/// Les [`VARIANT_COUNT`] jeux de plages système, dans l'ordre des variantes.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `i < VARIANT_COUNT`
const fn all_variant_ranges() -> [[KSDATARANGE_AUDIO; RANGES_PER_SYSTEM_PIN]; VARIANT_COUNT] {
    let mut out = [const { variant_ranges(0) }; VARIANT_COUNT];
    let mut i = 0;
    while i < VARIANT_COUNT {
        out[i] = variant_ranges(i);
        i = i.wrapping_add(1);
    }
    out
}

/// Plage « analogique » des broches bridge : `KSDATARANGE` simple, sous-type
/// `KSDATAFORMAT_SUBTYPE_ANALOG`, spécificateur `KSDATAFORMAT_SPECIFIER_NONE`.
///
/// `Flags` à **0** : une broche bridge n'a pas de format, donc pas de mode de traitement du
/// signal, donc aucune liste d'attributs derrière elle. C'est aussi ce que fait SYSVAD
/// (`SpeakerPinDataRangesBridge`), et la différence est voulue : le mode se déclare sur les
/// broches de flux, et sur elles seules.
pub const fn analog_range() -> KSDATARANGE {
    data_format(
        KSDATARANGE_SIZE,
        0,
        KSDATAFORMAT_SUBTYPE_ANALOG,
        KSDATAFORMAT_SPECIFIER_NONE,
    )
}

/// Pointeur `PKSDATARANGE` sur une plage logée dans une `static` (`KSDATARANGE_AUDIO`
/// commence par sa `KSDATARANGE`, et [`Shared`] est `repr(transparent)` : les trois
/// adresses sont la même). Le `cast_mut` satisfait le prototype ; PortCls ne modifie pas
/// les plages.
const fn range_ptr<T>(range: &'static T) -> PKSDATARANGE {
    ptr::from_ref(range).cast::<KSDATARANGE>().cast_mut()
}

// ---------------------------------------------------------------------------------
// La liste d'attributs des broches de flux (M1b-22, driver-design.md §4.3).
//
// SYSVAD attache à chaque plage de ses broches de flux un `KSATTRIBUTE_LIST` d'un seul
// élément, dont l'`Attribute` est `KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE`. L'attribut ne
// nomme **pas** de mode : il dit « cette broche sait qu'un mode existe », et c'est la
// propriété `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` (`portcls::modes`) qui énumère les
// modes servis. La documentation « Audio Signal Processing Modes » le dit mot pour mot :
// « *This list has a single element in it, which is a KSATTRIBUTE. The Attribute member of
// the KSATTRIBUTE structure is set to KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE.* »
// ---------------------------------------------------------------------------------

/// L'unique attribut : en-tête `KSATTRIBUTE` de 24 octets, aucun drapeau, l'identifiant du
/// mode de traitement du signal.
///
/// `Flags` à 0 et non `KSATTRIBUTE_REQUIRED` : un attribut « requis » obligerait tout
/// client à en fournir la valeur dans sa requête d'intersection. SYSVAD le laisse à 0, et
/// notre gestionnaire d'intersection ([`crate::intersect`]) ne lit aucun attribut — exiger
/// ce que personne ne lit ne ferait que refuser des clients.
/// Doublet `const` puis `static` : les assertions de fin de fichier lisent la `const`
/// (l'évaluation `const` ne lit pas les `static`), et c'est elle que la `static` enveloppe.
const MODE_ATTRIBUTE_VALUE: KSATTRIBUTE = KSATTRIBUTE {
    Size: size_of::<KSATTRIBUTE>() as ULONG,
    Flags: 0,
    Attribute: KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE,
};
static MODE_ATTRIBUTE: Shared<KSATTRIBUTE> = Shared(MODE_ATTRIBUTE_VALUE);

/// Le tableau de pointeurs que `KSATTRIBUTE_LIST::Attributes` conserve : un seul élément.
static MODE_ATTRIBUTE_POINTERS: Shared<[PKSATTRIBUTE; 1]> =
    Shared([ptr::from_ref(&MODE_ATTRIBUTE)
        .cast::<KSATTRIBUTE>()
        .cast_mut()]);

/// Entrée de `DataRanges` portant la `KSATTRIBUTE_LIST` : la liste **et un remplissage**
/// qui la porte à la taille d'une `KSDATARANGE_AUDIO`.
///
/// # Pourquoi ce remplissage existe
///
/// Une entrée d'attributs occupe une case du tableau `DataRanges`, typée `PKSDATARANGE`.
/// KS et PortCls savent la reconnaître au drapeau `KSDATARANGE_ATTRIBUTES` de la plage qui
/// la précède, et ne la présentent jamais à `DataRangeIntersection` — SYSVAD, qui a
/// exactement cette construction *et* un gestionnaire d'intersection maison, le démontre.
///
/// Mais « ne devrait jamais » n'est pas « ne peut pas ». Si un jour PortCls nous tendait
/// cette entrée comme une plage, `intersect::read_our_range` lirait un en-tête de 64 octets
/// au bout d'un objet qui n'en fait que 16 : une lecture **hors objet**, comportement
/// indéfini en Rust, là où x64 ne bronche pas. Les 72 octets de remplissage transforment
/// cette lecture en lecture parfaitement définie — elle rendrait `FormatSize == 1` (le
/// `Count` de la liste), que `read_our_range` refuse déjà par `STATUS_NO_MATCH`.
///
/// 72 octets dans la section de données, une fois pour tout le pilote (la liste est
/// **partagée** par les 24 variantes et les 144 entrées), contre un comportement indéfini
/// dépendant d'une garantie non écrite : le compte est vite fait.
#[repr(C, align(8))]
pub struct AttributeListEntry {
    /// La liste elle-même, à l'offset 0 : c'est cette adresse que le tableau conserve.
    list: KSATTRIBUTE_LIST,
    /// Remplissage jusqu'à `sizeof(KSDATARANGE_AUDIO)`. **Jamais lu par KS**, qui s'arrête
    /// à `Count` et `Attributes` ; il n'est là que pour rendre définie une lecture d'en-tête
    /// de plage faite par erreur (voir la documentation du type).
    _reserve: [u8; KSDATARANGE_AUDIO_SIZE as usize - size_of::<KSATTRIBUTE_LIST>()],
}

/// L'entrée d'attributs, unique et partagée par toutes les plages système de toutes les
/// variantes.
static MODE_ATTRIBUTE_ENTRY: Shared<AttributeListEntry> = Shared(AttributeListEntry {
    list: KSATTRIBUTE_LIST {
        Count: 1,
        Attributes: ptr::from_ref(&MODE_ATTRIBUTE_POINTERS)
            .cast::<PKSATTRIBUTE>()
            .cast_mut(),
    },
    _reserve: [0; KSDATARANGE_AUDIO_SIZE as usize - size_of::<KSATTRIBUTE_LIST>()],
});

/// Le pointeur `PKSDATARANGE` de l'entrée d'attributs, tel que le tableau `DataRanges` le
/// porte (voir [`AttributeListEntry`] : la liste est à l'offset 0, et [`Shared`] est
/// `repr(transparent)`).
const fn mode_attribute_ptr() -> PKSDATARANGE {
    range_ptr(&MODE_ATTRIBUTE_ENTRY)
}

/// Broche de filtre : `flow`/`comm` (§4.1), catégorie `category`, nom `name` (§4.2 :
/// GUID que KS résout en chaîne dans `HKR\MediaCategories` pour répondre à
/// `KSPROPERTY_PIN_NAME` — `None` laisse KS retomber sur la catégorie), plages `ranges`
/// (**rangée** d'un tableau de pointeurs logé dans une `static` : une par variante de
/// format pour les broches système, l'unique plage analogique pour les autres),
/// `instances` instances possibles (globales et par filtre : 1 pour une broche système,
/// 0 pour une broche bridge, qui ne s'instancie pas). Ni interface ni médium déclarés
/// (défauts KS), pas de table d'automatisation propre.
pub const fn pin<const N: usize>(
    flow: KSPIN_DATAFLOW::Type,
    comm: KSPIN_COMMUNICATION::Type,
    category: &'static GUID,
    name: Option<&'static GUID>,
    ranges: &'static [PKSDATARANGE; N],
    instances: ULONG,
) -> PCPIN_DESCRIPTOR {
    let name = match name {
        Some(guid) => ptr::from_ref(guid),
        None => ptr::null(),
    };
    PCPIN_DESCRIPTOR {
        MaxGlobalInstanceCount: instances,
        MaxFilterInstanceCount: instances,
        MinFilterInstanceCount: 0,
        AutomationTable: ptr::null(),
        KsPinDescriptor: KSPIN_DESCRIPTOR {
            InterfacesCount: 0,
            Interfaces: ptr::null(),
            MediumsCount: 0,
            Mediums: ptr::null(),
            DataRangesCount: N as ULONG,
            DataRanges: ptr::from_ref(ranges).cast::<PKSDATARANGE>(),
            DataFlow: flow,
            Communication: comm,
            Category: category,
            Name: name,
            __bindgen_anon_1: KSPIN_DESCRIPTOR__bindgen_ty_1 { Reserved: 0 },
        },
    }
}

/// Broche bridge : `KSPIN_COMMUNICATION_NONE`, catégorie `KSCATEGORY_AUDIO`, plage
/// analogique, aucune instance.
pub const fn bridge_pin(flow: KSPIN_DATAFLOW::Type) -> PCPIN_DESCRIPTOR {
    pin(
        flow,
        KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE,
        &CATEGORY_AUDIO,
        None,
        BRIDGE_RANGES.get(),
        0,
    )
}

/// Broche système d'un filtre WaveRT : `KSPIN_COMMUNICATION_SINK`, catégorie
/// `KSCATEGORY_AUDIO`, les trois plages système de la variante `variant`, une instance.
const fn system_pin(flow: KSPIN_DATAFLOW::Type, variant: usize) -> PCPIN_DESCRIPTOR {
    pin(
        flow,
        KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_SINK,
        &CATEGORY_AUDIO,
        None,
        ranges_row(&SYSTEM_RANGES_TABLE, variant),
        1,
    )
}

/// Broche endpoint d'un filtre topologie : `KSPIN_COMMUNICATION_NONE`, catégorie
/// `KSNODETYPE_SPEAKER` ou `KSNODETYPE_LINE_CONNECTOR` (c'est elle que le générateur
/// d'endpoints expose), plage analogique, aucune instance.
///
/// C'est **la seule broche nommée** : son GUID `Name` ([`PIN_NAMES`], celui du câble) est
/// ce que KS résout en « Conduit *n+1* » (§4.2). Sans lui, KS retomberait sur la
/// catégorie et l'endpoint s'appellerait « Haut-parleurs » ou « Ligne ». C'est aussi le
/// **seul** contenu qui distingue les seize filtres de topologie d'un même sens.
const fn endpoint_pin(
    flow: KSPIN_DATAFLOW::Type,
    category: &'static GUID,
    name: &'static GUID,
) -> PCPIN_DESCRIPTOR {
    pin(
        flow,
        KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE,
        category,
        Some(name),
        BRIDGE_RANGES.get(),
        0,
    )
}

/// Connexion de topologie : `from_node`/`from_pin` → `to_node`/`to_pin`, `PCFILTER_NODE`
/// désignant une broche du filtre lui-même.
pub const fn connection(
    from_node: ULONG,
    from_pin: ULONG,
    to_node: ULONG,
    to_pin: ULONG,
) -> PCCONNECTION_DESCRIPTOR {
    PCCONNECTION_DESCRIPTOR {
        FromNode: from_node,
        FromNodePin: from_pin,
        ToNode: to_node,
        ToNodePin: to_pin,
    }
}

/// Descripteur de filtre, forme générale : `pins`, `nodes` et `connections` logés dans des
/// `static`, table d'automatisation de filtre `automation`, catégories laissées à PortCls
/// (`CategoryCount = 0`).
///
/// Privé : les filtres du pilote passent par [`wave_filter`] ou [`topo_filter`], qui
/// nomment les deux formes existantes et sont chacune vérifiée par ses propres assertions
/// `const`.
///
/// Pas d'`Option` pour les nœuds : en contexte `const`, le paramètre générique `N` doit
/// être fourni de toute façon, et `N == 0` dit déjà « aucun nœud ». C'est alors `Nodes` qui
/// est mis à **nul** — un tableau vide a une adresse valide, mais PortCls attend le nul
/// quand `NodeCount` est nul, comme SYSVAD l'écrit.
///
/// `pins` est une **rangée** d'une table logée dans une `static` : les filtres topologie
/// en ont une par câble ([`pins_row`]), les filtres wave une seule
/// (`Shared::get`). Les autres tables restent enveloppées, elles ne se dédoublent pas.
const fn filter<const P: usize, const N: usize, const C: usize>(
    automation: &'static Shared<PCAUTOMATION_TABLE>,
    pins: &'static [PCPIN_DESCRIPTOR; P],
    nodes: &'static Shared<[PCNODE_DESCRIPTOR; N]>,
    connections: &'static Shared<[PCCONNECTION_DESCRIPTOR; C]>,
) -> PCFILTER_DESCRIPTOR {
    PCFILTER_DESCRIPTOR {
        // `PCFILTER_DESCRIPTOR_VERSION` n'existe pas dans `portcls.h` 26100 ; SYSVAD
        // écrit 0.
        Version: 0,
        AutomationTable: ptr::from_ref(automation).cast::<PCAUTOMATION_TABLE>(),
        PinSize: size_of::<PCPIN_DESCRIPTOR>() as ULONG,
        PinCount: P as ULONG,
        Pins: ptr::from_ref(pins).cast::<PCPIN_DESCRIPTOR>(),
        NodeSize: size_of::<PCNODE_DESCRIPTOR>() as ULONG,
        NodeCount: N as ULONG,
        Nodes: if N == 0 {
            ptr::null()
        } else {
            ptr::from_ref(nodes).cast::<PCNODE_DESCRIPTOR>()
        },
        ConnectionCount: C as ULONG,
        Connections: ptr::from_ref(connections).cast::<PCCONNECTION_DESCRIPTOR>(),
        CategoryCount: 0,
        Categories: ptr::null(),
    }
}

/// **Forme wave** : filtre WaveRT sans nœud.
///
/// Le signal traverse le filtre WaveRT sans traitement ; les nœuds audio sont sur le filtre
/// de topologie du même sens ([`topo_filter`]).
///
/// `automation` n'est plus vide depuis M1b-22 : elle porte
/// `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES`, propriété du **filtre** qui décrit une broche
/// (voir l'en-tête de `portcls::modes`), et se dédouble par **sens** — le gestionnaire est
/// monomorphisé sur `WaveRender` ou `WaveCapture`.
const fn wave_filter<const P: usize, const C: usize>(
    automation: &'static Shared<PCAUTOMATION_TABLE>,
    pins: &'static [PCPIN_DESCRIPTOR; P],
    connections: &'static Shared<[PCCONNECTION_DESCRIPTOR; C]>,
) -> PCFILTER_DESCRIPTOR {
    filter::<P, 0, C>(automation, pins, &NO_NODES, connections)
}

/// **Forme topologie** : filtre de topologie avec ses nœuds.
///
/// `automation` est la table du **filtre** : elle porte les propriétés qui ne sont ni de
/// nœud ni de broche et l'événement `JACKINFOCHANGE` (voir l'en-tête de module), et se
/// dédouble par sens comme celles des nœuds. Les propriétés audio, elles, restent sur les
/// nœuds
/// (`PCNODE_DESCRIPTOR::AutomationTable`) : PortCls route `KSPROPERTY_AUDIO_*` vers le nœud
/// désigné par `PCPROPERTY_REQUEST::Node`, et les propriétés de filtre vers cette table-ci.
const fn topo_filter<const P: usize, const N: usize, const C: usize>(
    automation: &'static Shared<PCAUTOMATION_TABLE>,
    pins: &'static [PCPIN_DESCRIPTOR; P],
    nodes: &'static Shared<[PCNODE_DESCRIPTOR; N]>,
    connections: &'static Shared<[PCCONNECTION_DESCRIPTOR; C]>,
) -> PCFILTER_DESCRIPTOR {
    filter::<P, N, C>(automation, pins, nodes, connections)
}

/// Table d'automatisation de `P` propriétés, sans méthode ni événement.
///
/// Deux usages : les nœuds audio (une propriété chacun) et les filtres de topologie (le
/// jack de M1b-03, plus les deux propriétés privées de M1b-04). Le nombre est **déduit du
/// tableau** par `const` générique plutôt que passé à part : une `PropertyCount` qui ne
/// vaudrait pas la longueur du tableau ferait lire à PortCls des `PCPROPERTY_ITEM` qui
/// n'existent pas, ou rendrait muettes les propriétés du bout — et rien ne le signalerait.
///
/// Les `*ItemSize` sont renseignés même à compte nul, comme dans [`EMPTY_AUTOMATION`] :
/// PortCls s'en sert pour avancer dans les tableaux, et une taille nulle avec un compte nul
/// est un piège inutile à laisser.
///
/// `EventCount = 0` : cette fonction ne bâtit que le triplet des propriétés. Les deux
/// tables de **filtre** de topologie y ajoutent leur `PCEVENT_ITEM`
/// `KSEVENT_PINCAPS_JACKINFOCHANGE` en enveloppant le résultat dans `portcls::with_events`
/// ([`TOPO_RENDER_AUTOMATION`], [`TOPO_CAPTURE_AUTOMATION`]) ; les quatre tables de nœud,
/// elles, n'ont pas d'événement et gardent le triplet nul. C'est exactement ce que fait la
/// macro `DEFINE_PCAUTOMATION_TABLE_PROP_EVENT` de `portcls.h`, en deux temps.
const fn property_automation<const P: usize>(
    properties: &'static Shared<[PCPROPERTY_ITEM; P]>,
) -> PCAUTOMATION_TABLE {
    PCAUTOMATION_TABLE {
        PropertyItemSize: size_of::<PCPROPERTY_ITEM>() as ULONG,
        PropertyCount: P as ULONG,
        Properties: ptr::from_ref(properties).cast::<PCPROPERTY_ITEM>(),
        MethodItemSize: size_of::<PCMETHOD_ITEM>() as ULONG,
        MethodCount: 0,
        Methods: ptr::null(),
        EventItemSize: size_of::<PCEVENT_ITEM>() as ULONG,
        EventCount: 0,
        Events: ptr::null(),
        Reserved: 0,
    }
}

/// Nœud de topologie : type `node_type`, table d'automatisation `automation`, sans nom.
///
/// `Name` **nul** : KS retombe alors sur `Type` pour le nom affiché, ce qui donne les noms
/// standard — et traduits — du volume et de la sourdine. C'est le repli documenté de
/// `KSPROPERTY_PIN_NAME` (§4.2), appliqué aux nœuds, et c'est exactement ce qu'on veut
/// ici : un nom de nœud propre au câble n'aurait aucun sens, et introduirait un contenu
/// par câble dans une `static` qui doit rester partagée par les seize.
///
/// `Flags` à 0 : aucun `PCNODE_DESCRIPTOR_FLAG_*` ne s'applique (ils concernent les nœuds
/// de mixage et de démultiplexage).
const fn audio_node(
    node_type: &'static GUID,
    automation: &'static Shared<PCAUTOMATION_TABLE>,
) -> PCNODE_DESCRIPTOR {
    PCNODE_DESCRIPTOR {
        Flags: 0,
        AutomationTable: ptr::from_ref(automation).cast::<PCAUTOMATION_TABLE>(),
        Type: ptr::from_ref(node_type),
        Name: ptr::null(),
    }
}

// ---------------------------------------------------------------------------------
// Les tables.
// ---------------------------------------------------------------------------------

/// `KSCATEGORY_AUDIO`, adressable (les broches pointent la catégorie).
static CATEGORY_AUDIO: GUID = KSCATEGORY_AUDIO;
/// `KSNODETYPE_SPEAKER`. **Plus utilisé** : conservé pour mémoire et pour le jour où
/// Windows cessera d'imposer le nom des endpoints haut-parleur (voir
/// [`CATEGORY_LINE_CONNECTOR`] et driver-design.md §4.2).
#[allow(dead_code)]
static CATEGORY_SPEAKER: GUID = KSNODETYPE_SPEAKER;
/// `KSNODETYPE_LINE_CONNECTOR`, catégorie des **deux** broches endpoint.
///
/// Mesuré dans la VM le 2026-09-06, avec la même construction des deux côtés à la seule
/// catégorie près : la broche capture (`LINE_CONNECTOR`) donne l'endpoint « Conduit 1 »,
/// la broche rendu (`SPEAKER`) donne « Haut-parleurs ». Le générateur d'endpoints impose
/// donc bien le nom des sorties haut-parleur, comme le laissait craindre la
/// documentation. Avec seize câbles, seize « Haut-parleurs » indistinguables : le rendu
/// passe lui aussi en connecteur de ligne, au prix de l'icône et du rang de sélection par
/// défaut. C'est le repli prévu par driver-design.md §4.2.
static CATEGORY_LINE_CONNECTOR: GUID = KSNODETYPE_LINE_CONNECTOR;
/// GUID de nom des broches endpoint, **un par câble** et adressables :
/// `KsPinDescriptor.Name` en prend l'adresse et PortCls la conserve (§4.2). L'INF associe
/// ces mêmes GUID à « Conduit *n+1* » (`conduit_kmd.inx`, `GUID.PinName.Cable<n>`).
///
/// Doublet `const` puis `static` : les assertions ci-dessous lisent
/// [`portcls::PIN_NAME_GUIDS`], l'évaluation `const` ne lisant pas les `static`.
static PIN_NAMES: [GUID; CABLE_COUNT] = portcls::PIN_NAME_GUIDS;

/// Le GUID de nom de broche du câble `cable`, adressable.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `cable < CABLE_COUNT`
const fn pin_name(cable: usize) -> &'static GUID {
    &PIN_NAMES[cable]
}

/// `KSNODETYPE_VOLUME`, adressable (`PCNODE_DESCRIPTOR::Type` en prend l'adresse).
static NODE_TYPE_VOLUME: GUID = KSNODETYPE_VOLUME;
/// `KSNODETYPE_MUTE`, adressable.
static NODE_TYPE_MUTE: GUID = KSNODETYPE_MUTE;

/// Plage analogique des broches bridge et endpoint.
static RANGE_ANALOG: Shared<KSDATARANGE> = Shared(analog_range());

/// Plage des broches bridge et endpoint : la même pour tous les câbles et toutes les
/// variantes — une broche analogique n'a pas de format.
static BRIDGE_RANGES: Shared<[PKSDATARANGE; 1]> = Shared([range_ptr(&RANGE_ANALOG)]);

/// Les [`VARIANT_COUNT`] × 3 plages système, **les valeurs** : c'est ici qu'elles vivent,
/// et c'est leur adresse que les tableaux de pointeurs ci-dessous conservent.
///
/// 24 × 3 × 88 octets = 6 336 octets dans la section de données du pilote. Les avoir
/// toutes à la compilation plutôt que d'en bâtir une au démarrage est ce qui permet aux
/// assertions `const` de les vérifier — et le coût est celui d'une page et demie.
static SYSTEM_RANGE_VALUES: Shared<[[KSDATARANGE_AUDIO; RANGES_PER_SYSTEM_PIN]; VARIANT_COUNT]> =
    Shared(all_variant_ranges());

/// Les entrées `DataRanges` de la variante `variant` : chacune des trois plages de la
/// rangée correspondante de `table`, **suivie** de l'entrée d'attributs partagée.
///
/// L'alternance est le contrat de `KSDATARANGE_ATTRIBUTES` : la liste d'attributs vaut pour
/// la plage qui la précède immédiatement, donc chacune des trois profondeurs porte la
/// sienne. Une seule entrée d'attributs en fin de tableau ne qualifierait que la dernière
/// plage — panne muette de plus : le pilote resterait « mode aware » sur une profondeur
/// et pas sur les deux autres.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `variant < VARIANT_COUNT`
const fn system_range_ptrs(
    table: &'static Shared<[[KSDATARANGE_AUDIO; RANGES_PER_SYSTEM_PIN]; VARIANT_COUNT]>,
    variant: usize,
) -> [PKSDATARANGE; ENTRIES_PER_SYSTEM_PIN] {
    let [f32_, pcm24, i16_] = &table.get()[variant];
    let attributs = mode_attribute_ptr();
    [
        range_ptr(f32_),
        attributs,
        range_ptr(pcm24),
        attributs,
        range_ptr(i16_),
        attributs,
    ]
}

/// Les [`VARIANT_COUNT`] tableaux de pointeurs `DataRanges`, un par variante.
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn all_system_range_ptrs() -> [[PKSDATARANGE; ENTRIES_PER_SYSTEM_PIN]; VARIANT_COUNT] {
    let mut out = [const { system_range_ptrs(&SYSTEM_RANGE_VALUES, 0) }; VARIANT_COUNT];
    let mut i = 0;
    while i < VARIANT_COUNT {
        out[i] = system_range_ptrs(&SYSTEM_RANGE_VALUES, i);
        i = i.wrapping_add(1);
    }
    out
}

/// Plages des broches système (`DataRanges` : tableau de pointeurs), **une rangée par
/// variante de format**.
static SYSTEM_RANGES_TABLE: Shared<[[PKSDATARANGE; ENTRIES_PER_SYSTEM_PIN]; VARIANT_COUNT]> =
    Shared(all_system_range_ptrs());

/// La rangée de plages de la variante `variant` : c'est son adresse que
/// `KSPIN_DESCRIPTOR::DataRanges` conserve.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `variant < VARIANT_COUNT`
const fn ranges_row(
    table: &'static Shared<[[PKSDATARANGE; ENTRIES_PER_SYSTEM_PIN]; VARIANT_COUNT]>,
    variant: usize,
) -> &'static [PKSDATARANGE; ENTRIES_PER_SYSTEM_PIN] {
    &table.get()[variant]
}

/// Nombre de propriétés portées par un filtre **wave** : les modes de traitement du signal,
/// et rien d'autre.
///
/// Le reste de ce que PortCls sert sur un filtre WaveRT (`KSPROPSETID_Pin`,
/// `KSPROPSETID_Topology`) reste à PortCls : la table ne porte que ce que lui seul ne sait
/// pas répondre.
const WAVE_FILTER_PROPERTY_COUNT: usize = 1;

/// `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` du filtre `WaveRender<n>` (M1b-22).
///
/// Monomorphisé sur `WaveRender` comme les propriétés de topologie le sont sur `TopoRender`
/// et pour la même raison : la garde de vtable du thunk compare l'adresse de `T::VTBL`,
/// et une table commune aux deux sens rendrait le pilote muet d'un côté.
const WAVE_RENDER_FILTER_ITEMS: [PCPROPERTY_ITEM; WAVE_FILTER_PROPERTY_COUNT] =
    [signal_processing_modes_item::<
        IMiniportWaveRTVtbl,
        WaveRender,
    >()];
/// La même propriété sur le filtre `WaveCapture<n>`, monomorphisée sur son type.
const WAVE_CAPTURE_FILTER_ITEMS: [PCPROPERTY_ITEM; WAVE_FILTER_PROPERTY_COUNT] =
    [signal_processing_modes_item::<
        IMiniportWaveRTVtbl,
        WaveCapture,
    >()];

static WAVE_RENDER_FILTER_PROPERTIES: Shared<[PCPROPERTY_ITEM; WAVE_FILTER_PROPERTY_COUNT]> =
    Shared(WAVE_RENDER_FILTER_ITEMS);
static WAVE_CAPTURE_FILTER_PROPERTIES: Shared<[PCPROPERTY_ITEM; WAVE_FILTER_PROPERTY_COUNT]> =
    Shared(WAVE_CAPTURE_FILTER_ITEMS);

/// Table d'automatisation du filtre `WaveRender<n>`.
///
/// Doublet `const` puis `static`, comme pour les filtres de topologie : c'est **cette**
/// valeur que les assertions de fin de fichier lisent (l'évaluation `const` ne lit pas les
/// `static`), et c'est elle que la `static` enveloppe. Une table de filtre wave qui
/// perdrait sa propriété passerait autrement la compilation sans un mot, et le symptôme
/// serait un pilote qui n'a jamais l'air « mode aware » — exactement la panne que M1b-22
/// cherche à écarter.
const WAVE_RENDER_AUTOMATION_TABLE: PCAUTOMATION_TABLE =
    property_automation(&WAVE_RENDER_FILTER_PROPERTIES);
/// Le même doublet pour le sens capture.
const WAVE_CAPTURE_AUTOMATION_TABLE: PCAUTOMATION_TABLE =
    property_automation(&WAVE_CAPTURE_FILTER_PROPERTIES);

static WAVE_RENDER_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(WAVE_RENDER_AUTOMATION_TABLE);
static WAVE_CAPTURE_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(WAVE_CAPTURE_AUTOMATION_TABLE);

/// Aucun nœud : le tableau vide que la forme wave passe à [`filter`], qui met alors
/// `Nodes` à nul. Jamais déréférencé.
static NO_NODES: Shared<[PCNODE_DESCRIPTOR; 0]> = Shared([]);

// ---------------------------------------------------------------------------------
// Nœuds audio des filtres topologie (M1b-03b).
//
// Ce qui suit se dédouble par **sens**, pas par câble : `volume_item::<V, T>` et
// `mute_item::<V, T>` sont monomorphisés sur le type du miniport, et la garde de vtable de
// `portcls::property::handler` compare l'adresse de `T::VTBL`. Les seize câbles de M1b-02
// partagent ces mêmes `static` ; c'est le `MajorTarget` de la requête qui ramène le
// gestionnaire au bon miniport, donc au bon `cable::NodeState`.
// ---------------------------------------------------------------------------------

/// `KSPROPERTY_AUDIO_VOLUMELEVEL` du nœud de volume de `TopoRender`.
const RENDER_VOLUME_ITEMS: [PCPROPERTY_ITEM; 1] =
    [volume_item::<IMiniportTopologyVtbl, TopoRender>()];
/// `KSPROPERTY_AUDIO_MUTE` du nœud de sourdine de `TopoRender`.
const RENDER_MUTE_ITEMS: [PCPROPERTY_ITEM; 1] = [mute_item::<IMiniportTopologyVtbl, TopoRender>()];
/// `KSPROPERTY_AUDIO_VOLUMELEVEL` du nœud de volume de `TopoCapture`.
const CAPTURE_VOLUME_ITEMS: [PCPROPERTY_ITEM; 1] =
    [volume_item::<IMiniportTopologyVtbl, TopoCapture>()];
/// `KSPROPERTY_AUDIO_MUTE` du nœud de sourdine de `TopoCapture`.
const CAPTURE_MUTE_ITEMS: [PCPROPERTY_ITEM; 1] =
    [mute_item::<IMiniportTopologyVtbl, TopoCapture>()];

static RENDER_VOLUME_PROPERTIES: Shared<[PCPROPERTY_ITEM; 1]> = Shared(RENDER_VOLUME_ITEMS);
static RENDER_MUTE_PROPERTIES: Shared<[PCPROPERTY_ITEM; 1]> = Shared(RENDER_MUTE_ITEMS);
static CAPTURE_VOLUME_PROPERTIES: Shared<[PCPROPERTY_ITEM; 1]> = Shared(CAPTURE_VOLUME_ITEMS);
static CAPTURE_MUTE_PROPERTIES: Shared<[PCPROPERTY_ITEM; 1]> = Shared(CAPTURE_MUTE_ITEMS);

/// Les quatre propriétés du **filtre** `TopoRender` : le jack (M1b-03), puis l'état et la
/// version du jeu privé `KSPROPSETID_Conduit` (M1b-04), puis les compteurs de la boucle
/// locale (M1b-21).
///
/// L'ordre n'a pas d'importance pour PortCls, qui cherche par `Set`/`Id`, mais celui-ci se
/// lit dans l'ordre d'apparition des tâches.
const RENDER_FILTER_ITEMS: [PCPROPERTY_ITEM; FILTER_PROPERTY_COUNT] = [
    jack_description_item::<IMiniportTopologyVtbl, TopoRender>(),
    cable_state_item::<IMiniportTopologyVtbl, TopoRender>(),
    version_item::<IMiniportTopologyVtbl, TopoRender>(),
    counters_item::<IMiniportTopologyVtbl, TopoRender>(),
];
/// Les quatre mêmes propriétés du **filtre** `TopoCapture`, monomorphisées sur son type.
const CAPTURE_FILTER_ITEMS: [PCPROPERTY_ITEM; FILTER_PROPERTY_COUNT] = [
    jack_description_item::<IMiniportTopologyVtbl, TopoCapture>(),
    cable_state_item::<IMiniportTopologyVtbl, TopoCapture>(),
    version_item::<IMiniportTopologyVtbl, TopoCapture>(),
    counters_item::<IMiniportTopologyVtbl, TopoCapture>(),
];

/// Nombre de propriétés portées par un filtre de topologie : jack, état, version,
/// compteurs.
///
/// Nommée plutôt qu'écrite quatre fois : c'est elle que
/// `PCAUTOMATION_TABLE::PropertyCount` reçoit, par déduction du tableau dans
/// [`property_automation`].
const FILTER_PROPERTY_COUNT: usize = 4;

static RENDER_FILTER_PROPERTIES: Shared<[PCPROPERTY_ITEM; FILTER_PROPERTY_COUNT]> =
    Shared(RENDER_FILTER_ITEMS);
static CAPTURE_FILTER_PROPERTIES: Shared<[PCPROPERTY_ITEM; FILTER_PROPERTY_COUNT]> =
    Shared(CAPTURE_FILTER_ITEMS);

/// Nombre d'événements portés par un filtre de topologie : `JACKINFOCHANGE`, et lui seul.
const FILTER_EVENT_COUNT: usize = 1;

/// `KSEVENT_PINCAPS_JACKINFOCHANGE` du **filtre** `TopoRender<n>` (M1b-04).
///
/// Sur le **filtre**, pas sur la broche endpoint : l'item est déclaré au niveau filtre et
/// c'est le *signalement* qui désigne la broche (`GenerateEventList(…, PinEvent = TRUE,
/// PinId = …)`). Cette géométrie est celle de SYSVAD, **attestée et non spécifiée** — voir
/// l'en-tête de `portcls::event`, qui dit aussi quelle expérience tenter si l'événement ne
/// partait pas en machine.
///
/// Monomorphisé sur `TopoRender` comme les propriétés, et pour la même raison : la garde de
/// vtable du thunk compare l'adresse de `T::VTBL`.
const RENDER_FILTER_EVENT_ITEMS: [PCEVENT_ITEM; FILTER_EVENT_COUNT] =
    [jack_info_change_item::<IMiniportTopologyVtbl, TopoRender>()];
/// Le même événement sur le filtre `TopoCapture<n>`, monomorphisé sur son type.
const CAPTURE_FILTER_EVENT_ITEMS: [PCEVENT_ITEM; FILTER_EVENT_COUNT] =
    [jack_info_change_item::<IMiniportTopologyVtbl, TopoCapture>()];

static RENDER_FILTER_EVENTS: Shared<[PCEVENT_ITEM; FILTER_EVENT_COUNT]> =
    Shared(RENDER_FILTER_EVENT_ITEMS);
static CAPTURE_FILTER_EVENTS: Shared<[PCEVENT_ITEM; FILTER_EVENT_COUNT]> =
    Shared(CAPTURE_FILTER_EVENT_ITEMS);

/// Table d'automatisation du filtre `TopoRender<n>` : les trois propriétés, **plus**
/// l'événement qui fait relire le jack.
///
/// `property_automation` bâtit les deux premiers triplets, `with_events` le troisième :
/// c'est ce que fait la macro `DEFINE_PCAUTOMATION_TABLE_PROP_EVENT` de `portcls.h`.
const fn topo_render_automation() -> PCAUTOMATION_TABLE {
    with_events(
        property_automation(&RENDER_FILTER_PROPERTIES),
        RENDER_FILTER_EVENTS.get(),
    )
}

/// Table d'automatisation du filtre `TopoCapture<n>`, même forme, l'autre sens.
const fn topo_capture_automation() -> PCAUTOMATION_TABLE {
    with_events(
        property_automation(&CAPTURE_FILTER_PROPERTIES),
        CAPTURE_FILTER_EVENTS.get(),
    )
}

/// Doublet `const` puis `static`, comme [`EMPTY_AUTOMATION_TABLE`] : c'est **cette**
/// valeur que les assertions de fin de fichier lisent, et c'est elle que la `static`
/// enveloppe. Sans le doublet, les assertions devraient reconstruire une table de leur
/// côté et ne diraient plus rien de celle que le pilote livre — une table de filtre qui
/// perdrait son `with_events` passerait alors la compilation **sans un mot**, et le seul
/// symptôme serait un endpoint figé sur son état de démarrage. C'est le pire mode de panne
/// de ce module (voir l'en-tête), et le doublet est ce qui l'empêche.
const TOPO_RENDER_AUTOMATION_TABLE: PCAUTOMATION_TABLE = topo_render_automation();
/// Le même doublet pour le sens capture.
const TOPO_CAPTURE_AUTOMATION_TABLE: PCAUTOMATION_TABLE = topo_capture_automation();

static TOPO_RENDER_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(TOPO_RENDER_AUTOMATION_TABLE);
static TOPO_CAPTURE_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(TOPO_CAPTURE_AUTOMATION_TABLE);

static RENDER_VOLUME_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(property_automation(&RENDER_VOLUME_PROPERTIES));
static RENDER_MUTE_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(property_automation(&RENDER_MUTE_PROPERTIES));
static CAPTURE_VOLUME_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(property_automation(&CAPTURE_VOLUME_PROPERTIES));
static CAPTURE_MUTE_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(property_automation(&CAPTURE_MUTE_PROPERTIES));

/// Nœuds de `TopoRender` : volume à l'index [`NODE_VOLUME`], sourdine à [`NODE_MUTE`].
const TOPO_RENDER_NODES: [PCNODE_DESCRIPTOR; NODE_COUNT] = [
    audio_node(&NODE_TYPE_VOLUME, &RENDER_VOLUME_AUTOMATION),
    audio_node(&NODE_TYPE_MUTE, &RENDER_MUTE_AUTOMATION),
];
/// Nœuds de `TopoCapture` : mêmes types aux mêmes index, gestionnaires du sens capture.
const TOPO_CAPTURE_NODES: [PCNODE_DESCRIPTOR; NODE_COUNT] = [
    audio_node(&NODE_TYPE_VOLUME, &CAPTURE_VOLUME_AUTOMATION),
    audio_node(&NODE_TYPE_MUTE, &CAPTURE_MUTE_AUTOMATION),
];

static TOPO_RENDER_NODES_TABLE: Shared<[PCNODE_DESCRIPTOR; NODE_COUNT]> = Shared(TOPO_RENDER_NODES);
static TOPO_CAPTURE_NODES_TABLE: Shared<[PCNODE_DESCRIPTOR; NODE_COUNT]> =
    Shared(TOPO_CAPTURE_NODES);

/// Connexion directe broche 0 → broche 1, commune aux **deux filtres WaveRT** (l'entrée est
/// toujours la broche 0, la sortie la broche 1). Les filtres topologie, eux, font passer le
/// signal par leurs deux nœuds ([`TOPO_CONNECTIONS`]).
const DIRECT_CONNECTION: [PCCONNECTION_DESCRIPTOR; WAVE_CONNECTION_COUNT] =
    [connection(PCFILTER_NODE, 0, PCFILTER_NODE, 1)];
static DIRECT_CONNECTION_TABLE: Shared<[PCCONNECTION_DESCRIPTOR; WAVE_CONNECTION_COUNT]> =
    Shared(DIRECT_CONNECTION);

/// Les trois connexions d'un filtre topologie : broche d'entrée → volume → sourdine →
/// broche de sortie.
///
/// **Identiques pour les deux sens**, parce que la numérotation des broches y suit déjà le
/// flux des deux côtés ([`TOPO_PIN_IN`], [`TOPO_PIN_OUT`]). Les broches des *nœuds*, elles,
/// sont numérotées à l'envers des broches de filtre : `KSNODEPIN_STANDARD_IN` vaut **1** et
/// `KSNODEPIN_STANDARD_OUT` vaut **0** (`ks.h`) — d'où les constantes nommées plutôt que
/// des littéraux, qui se liraient à contresens.
const TOPO_CONNECTIONS: [PCCONNECTION_DESCRIPTOR; TOPO_CONNECTION_COUNT] = [
    connection(
        PCFILTER_NODE,
        TOPO_PIN_IN,
        NODE_VOLUME,
        KSNODEPIN_STANDARD_IN,
    ),
    connection(
        NODE_VOLUME,
        KSNODEPIN_STANDARD_OUT,
        NODE_MUTE,
        KSNODEPIN_STANDARD_IN,
    ),
    connection(
        NODE_MUTE,
        KSNODEPIN_STANDARD_OUT,
        PCFILTER_NODE,
        TOPO_PIN_OUT,
    ),
];
static TOPO_CONNECTIONS_TABLE: Shared<[PCCONNECTION_DESCRIPTOR; TOPO_CONNECTION_COUNT]> =
    Shared(TOPO_CONNECTIONS);

/// Broches de `WaveRender` pour la variante `variant` : système (entrée, plages de la
/// variante) puis bridge (sortie).
const fn wave_render_pins(variant: usize) -> [PCPIN_DESCRIPTOR; PIN_COUNT] {
    [
        system_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN, variant),
        bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
    ]
}
/// Broches de `WaveCapture` pour la variante `variant` : bridge (entrée) puis système
/// (sortie, plages de la variante).
const fn wave_capture_pins(variant: usize) -> [PCPIN_DESCRIPTOR; PIN_COUNT] {
    [
        bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
        system_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT, variant),
    ]
}

/// Les [`VARIANT_COUNT`] jeux de broches d'un sens wave (`rendu` comme dans
/// [`topo_pins_table`] : un `bool` plutôt qu'un pointeur de fonction, que l'évaluation
/// `const` n'appelle pas).
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn wave_pins_table(rendu: bool) -> [[PCPIN_DESCRIPTOR; PIN_COUNT]; VARIANT_COUNT] {
    let mut out = [const { wave_render_pins(0) }; VARIANT_COUNT];
    let mut i = 0;
    while i < VARIANT_COUNT {
        out[i] = if rendu {
            wave_render_pins(i)
        } else {
            wave_capture_pins(i)
        };
        i = i.wrapping_add(1);
    }
    out
}

/// Broches des 24 filtres `WaveRender`, une rangée par variante de format.
const WAVE_RENDER_PINS: [[PCPIN_DESCRIPTOR; PIN_COUNT]; VARIANT_COUNT] = wave_pins_table(true);
/// Broches des 24 filtres `WaveCapture`.
const WAVE_CAPTURE_PINS: [[PCPIN_DESCRIPTOR; PIN_COUNT]; VARIANT_COUNT] = wave_pins_table(false);

static WAVE_RENDER_PINS_TABLE: Shared<[[PCPIN_DESCRIPTOR; PIN_COUNT]; VARIANT_COUNT]> =
    Shared(WAVE_RENDER_PINS);
static WAVE_CAPTURE_PINS_TABLE: Shared<[[PCPIN_DESCRIPTOR; PIN_COUNT]; VARIANT_COUNT]> =
    Shared(WAVE_CAPTURE_PINS);

/// Broches de `TopoRender<n>` : bridge (entrée) puis endpoint (sortie), nommé par le GUID
/// du câble `cable`.
///
/// L'endpoint est un **connecteur de ligne** et non un haut-parleur : voir
/// [`CATEGORY_LINE_CONNECTOR`], c'est la seule catégorie qui laisse le pilote nommer son
/// endpoint.
const fn topo_render_pins(cable: usize) -> [PCPIN_DESCRIPTOR; PIN_COUNT] {
    [
        bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
        endpoint_pin(
            KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT,
            &CATEGORY_LINE_CONNECTOR,
            pin_name(cable),
        ),
    ]
}

/// Broches de `TopoCapture<n>` : endpoint connecteur de ligne (entrée) puis bridge
/// (sortie).
const fn topo_capture_pins(cable: usize) -> [PCPIN_DESCRIPTOR; PIN_COUNT] {
    [
        endpoint_pin(
            KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN,
            &CATEGORY_LINE_CONNECTOR,
            pin_name(cable),
        ),
        bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
    ]
}

/// Les [`CABLE_COUNT`] jeux de broches d'un sens de topologie, dans l'ordre des câbles.
///
/// `rendu` : vrai pour `TopoRender`, faux pour `TopoCapture` — un `bool` plutôt qu'un
/// pointeur de fonction, que l'évaluation `const` n'appelle pas.
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn topo_pins_table(rendu: bool) -> [[PCPIN_DESCRIPTOR; PIN_COUNT]; CABLE_COUNT] {
    let mut out = [const { topo_render_pins(0) }; CABLE_COUNT];
    let mut i = 0;
    while i < CABLE_COUNT {
        out[i] = if rendu {
            topo_render_pins(i)
        } else {
            topo_capture_pins(i)
        };
        i = i.wrapping_add(1);
    }
    out
}

/// Broches des seize filtres `TopoRender<n>`.
const TOPO_RENDER_PINS: [[PCPIN_DESCRIPTOR; PIN_COUNT]; CABLE_COUNT] = topo_pins_table(true);
/// Broches des seize filtres `TopoCapture<n>`.
const TOPO_CAPTURE_PINS: [[PCPIN_DESCRIPTOR; PIN_COUNT]; CABLE_COUNT] = topo_pins_table(false);

static TOPO_RENDER_PINS_TABLE: Shared<[[PCPIN_DESCRIPTOR; PIN_COUNT]; CABLE_COUNT]> =
    Shared(TOPO_RENDER_PINS);
static TOPO_CAPTURE_PINS_TABLE: Shared<[[PCPIN_DESCRIPTOR; PIN_COUNT]; CABLE_COUNT]> =
    Shared(TOPO_CAPTURE_PINS);

/// La rangée `index` d'une table de broches logée dans une `static` : c'est son adresse
/// que `PCFILTER_DESCRIPTOR::Pins` conserve.
///
/// Générique sur la longueur de la table depuis M1b-05 : les filtres de topologie en ont
/// une par **câble** ([`CABLE_COUNT`] rangées, le GUID de nom variant), les filtres wave
/// une par **variante de format** ([`VARIANT_COUNT`] rangées, les plages variant). Deux
/// axes, une seule fonction — et le `const` générique interdit de confondre les deux
/// tables, ce qu'un paramètre `usize` nu aurait laissé passer.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `index < N`
const fn pins_row<const N: usize>(
    table: &'static Shared<[[PCPIN_DESCRIPTOR; PIN_COUNT]; N]>,
    index: usize,
) -> &'static [PCPIN_DESCRIPTOR; PIN_COUNT] {
    &table.get()[index]
}

/// Descripteur du filtre `TopoRender<n>` du câble `cable`.
const fn topo_render_filter_of(cable: usize) -> PCFILTER_DESCRIPTOR {
    topo_filter(
        &TOPO_RENDER_AUTOMATION,
        pins_row(&TOPO_RENDER_PINS_TABLE, cable),
        &TOPO_RENDER_NODES_TABLE,
        &TOPO_CONNECTIONS_TABLE,
    )
}

/// Descripteur du filtre `TopoCapture<n>` du câble `cable`.
const fn topo_capture_filter_of(cable: usize) -> PCFILTER_DESCRIPTOR {
    topo_filter(
        &TOPO_CAPTURE_AUTOMATION,
        pins_row(&TOPO_CAPTURE_PINS_TABLE, cable),
        &TOPO_CAPTURE_NODES_TABLE,
        &TOPO_CONNECTIONS_TABLE,
    )
}

/// Les [`CABLE_COUNT`] descripteurs de filtre topologie d'un sens (voir
/// [`topo_pins_table`] pour `rendu`).
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn topo_filters(rendu: bool) -> [PCFILTER_DESCRIPTOR; CABLE_COUNT] {
    let mut out = [const { topo_render_filter_of(0) }; CABLE_COUNT];
    let mut i = 0;
    while i < CABLE_COUNT {
        out[i] = if rendu {
            topo_render_filter_of(i)
        } else {
            topo_capture_filter_of(i)
        };
        i = i.wrapping_add(1);
    }
    out
}

/// Descripteur du filtre `WaveRender` de la variante `variant`.
const fn wave_render_filter_of(variant: usize) -> PCFILTER_DESCRIPTOR {
    wave_filter(
        &WAVE_RENDER_AUTOMATION,
        pins_row(&WAVE_RENDER_PINS_TABLE, variant),
        &DIRECT_CONNECTION_TABLE,
    )
}

/// Descripteur du filtre `WaveCapture` de la variante `variant`.
const fn wave_capture_filter_of(variant: usize) -> PCFILTER_DESCRIPTOR {
    wave_filter(
        &WAVE_CAPTURE_AUTOMATION,
        pins_row(&WAVE_CAPTURE_PINS_TABLE, variant),
        &DIRECT_CONNECTION_TABLE,
    )
}

/// Les [`VARIANT_COUNT`] descripteurs de filtre wave d'un sens (voir [`wave_pins_table`]
/// pour `rendu`).
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn wave_filters(rendu: bool) -> [PCFILTER_DESCRIPTOR; VARIANT_COUNT] {
    let mut out = [const { wave_render_filter_of(0) }; VARIANT_COUNT];
    let mut i = 0;
    while i < VARIANT_COUNT {
        out[i] = if rendu {
            wave_render_filter_of(i)
        } else {
            wave_capture_filter_of(i)
        };
        i = i.wrapping_add(1);
    }
    out
}

/// Descripteurs des 24 filtres `WaveRender` (const : lu par les assertions).
const WAVE_RENDER_FILTERS: [PCFILTER_DESCRIPTOR; VARIANT_COUNT] = wave_filters(true);
/// Descripteurs des 24 filtres `WaveCapture`.
const WAVE_CAPTURE_FILTERS: [PCFILTER_DESCRIPTOR; VARIANT_COUNT] = wave_filters(false);

static WAVE_RENDER_FILTER_TABLE: Shared<[PCFILTER_DESCRIPTOR; VARIANT_COUNT]> =
    Shared(WAVE_RENDER_FILTERS);
static WAVE_CAPTURE_FILTER_TABLE: Shared<[PCFILTER_DESCRIPTOR; VARIANT_COUNT]> =
    Shared(WAVE_CAPTURE_FILTERS);

/// Descripteurs des seize filtres `TopoRender<n>` (const : lu par les assertions).
const TOPO_RENDER_FILTERS: [PCFILTER_DESCRIPTOR; CABLE_COUNT] = topo_filters(true);
/// Descripteurs des seize filtres `TopoCapture<n>`.
const TOPO_CAPTURE_FILTERS: [PCFILTER_DESCRIPTOR; CABLE_COUNT] = topo_filters(false);

static TOPO_RENDER_FILTER_TABLE: Shared<[PCFILTER_DESCRIPTOR; CABLE_COUNT]> =
    Shared(TOPO_RENDER_FILTERS);
static TOPO_CAPTURE_FILTER_TABLE: Shared<[PCFILTER_DESCRIPTOR; CABLE_COUNT]> =
    Shared(TOPO_CAPTURE_FILTERS);

/// Descripteur du filtre `TopoRender<n>` du câble `cable`, `None` au-delà du dernier.
///
/// L'appelant (`topo::TopoRender::description`) a un `n < CABLE_COUNT` garanti mais doit
/// rendre une référence : c'est à lui de dire ce qu'il fait du `None`, comme
/// `cable::NodeState::volume` avec son canal.
#[must_use]
pub fn topo_render_filter(cable: u32) -> Option<&'static PCFILTER_DESCRIPTOR> {
    usize::try_from(cable)
        .ok()
        .and_then(|index| TOPO_RENDER_FILTER_TABLE.get().get(index))
}

/// Descripteur du filtre `TopoCapture<n>` du câble `cable`, `None` au-delà du dernier.
#[must_use]
pub fn topo_capture_filter(cable: u32) -> Option<&'static PCFILTER_DESCRIPTOR> {
    usize::try_from(cable)
        .ok()
        .and_then(|index| TOPO_CAPTURE_FILTER_TABLE.get().get(index))
}

// ---------------------------------------------------------------------------------
// Le format de chaque câble, lu une fois au démarrage (M1b-05).
// ---------------------------------------------------------------------------------

/// Le format de chaque câble, **encodé** (`CableFormat::encode`), tel que le registre l'a
/// fixé au dernier `StartDevice`.
///
/// # Pourquoi ici, et pourquoi encodé
///
/// *Ici* parce que c'est ce module qui traduit un format en variante de descripteurs, et
/// que le seul usage de cette valeur est de choisir une rangée de table. La loger dans
/// `cable::Cable` aurait mêlé de la **configuration** (lue au démarrage, immuable ensuite)
/// à de l'**état** (le flux courant, l'état de connexion), et le câble n'est pas le seul
/// lecteur : `wave` choisit son descripteur avant même qu'un flux existe.
///
/// *Encodé* parce qu'un `AtomicU32` se lit sans verrou depuis n'importe quel IRQL, et que
/// l'encodage est déjà la représentation canonique — en garder une seconde, décodée,
/// obligerait à les tenir cohérentes. `Relaxed` partout : la valeur est écrite une fois par
/// `StartDevice`, avant l'enregistrement du moindre sous-périphérique, donc avant qu'aucun
/// lecteur n'existe ; il n'y a aucune relation d'ordre à établir avec un autre champ. C'est
/// le raisonnement de `cable::Cable::connected`, à l'identique.
static CABLE_FORMATS: [AtomicU32; CABLE_COUNT] =
    [const { AtomicU32::new(CABLE_FORMAT_DEFAULT.encode()) }; CABLE_COUNT];

/// Fixe le format du câble `cable`. Sans effet au-delà du dernier câble.
///
/// Appelée par `registry::read_params` à chaque `StartDevice`, **avant** que le moindre
/// sous-périphérique ne soit enregistré : les descripteurs sont demandés plus tard, par
/// `wave::WaveRender::description` et consorts.
///
/// IRQL : `PASSIVE_LEVEL` (contexte de `IRP_MN_START_DEVICE`).
pub fn apply_cable_format(cable: u32, format: CableFormat) {
    if let Some(slot) = usize::try_from(cable)
        .ok()
        .and_then(|i| CABLE_FORMATS.get(i))
    {
        slot.store(format.encode(), Ordering::Relaxed);
    }
}

/// Le format du câble `cable`, ou [`CABLE_FORMAT_DEFAULT`] au-delà du dernier câble.
///
/// Le repli couvre aussi l'impossible — une valeur stockée que `decode` refuserait — parce
/// que la seule écriture passe par [`apply_cable_format`], qui encode un format déjà
/// validé. Rendre le défaut vaut mieux qu'un `Option` que chaque appelant traiterait à sa
/// façon : c'est l'idiome de `cable::NodeState::volume` et de `topo_render_filter_0`.
///
/// IRQL : quelconque.
#[must_use]
pub fn cable_format(cable: u32) -> CableFormat {
    usize::try_from(cable)
        .ok()
        .and_then(|i| CABLE_FORMATS.get(i))
        .map(|slot| slot.load(Ordering::Relaxed))
        .and_then(|brut| CableFormat::decode(brut).ok())
        .unwrap_or(CABLE_FORMAT_DEFAULT)
}

/// Descripteur du filtre `WaveRender` servant `format`, `None` si sa variante n'existe
/// pas.
#[must_use]
pub fn wave_render_filter(format: CableFormat) -> Option<&'static PCFILTER_DESCRIPTOR> {
    format
        .variant()
        .and_then(|variant| WAVE_RENDER_FILTER_TABLE.get().get(variant))
}

/// Descripteur du filtre `WaveCapture` servant `format`, `None` si sa variante n'existe
/// pas.
#[must_use]
pub fn wave_capture_filter(format: CableFormat) -> Option<&'static PCFILTER_DESCRIPTOR> {
    format
        .variant()
        .and_then(|variant| WAVE_CAPTURE_FILTER_TABLE.get().get(variant))
}

/// Le filtre `WaveRender` du format par défaut, repli de [`wave_render_filter`].
///
/// [`DEFAULT_VARIANT`] existe (assertion `const` plus haut), mais le trait
/// `MiniportWaveRT::description` rend une **référence** : il faut donc un chemin sans
/// `Option` ni panique, exactement comme [`topo_render_filter_0`]. Le motif de tranche
/// `split_at` évite l'indexation.
#[must_use]
pub fn wave_render_filter_default() -> &'static PCFILTER_DESCRIPTOR {
    variant_or_first(WAVE_RENDER_FILTER_TABLE.get())
}

/// Le filtre `WaveCapture` du format par défaut, repli de [`wave_capture_filter`].
#[must_use]
pub fn wave_capture_filter_default() -> &'static PCFILTER_DESCRIPTOR {
    variant_or_first(WAVE_CAPTURE_FILTER_TABLE.get())
}

/// La variante par défaut de `table`, ou sa première entrée si l'index n'y était pas —
/// ce qui ne peut arriver que si [`DEFAULT_VARIANT`] et [`VARIANT_COUNT`] divergeaient,
/// cas que les assertions `const` interdisent déjà.
fn variant_or_first(
    table: &'static [PCFILTER_DESCRIPTOR; VARIANT_COUNT],
) -> &'static PCFILTER_DESCRIPTOR {
    match table.get(DEFAULT_VARIANT) {
        Some(filtre) => filtre,
        None => {
            let [premier, ..] = table;
            premier
        }
    }
}

/// Le filtre `TopoRender0`, repli de [`topo_render_filter`] : le câble 0 existe toujours
/// ([`CABLE_COUNT`] ≥ 1, assertion `const` de `portcls::adapter`). Motif de tranche — ni
/// indexation ni arithmétique.
#[must_use]
pub fn topo_render_filter_0() -> &'static PCFILTER_DESCRIPTOR {
    let [premier, ..] = TOPO_RENDER_FILTER_TABLE.get();
    premier
}

/// Le filtre `TopoCapture0`, repli de [`topo_capture_filter`].
#[must_use]
pub fn topo_capture_filter_0() -> &'static PCFILTER_DESCRIPTOR {
    let [premier, ..] = TOPO_CAPTURE_FILTER_TABLE.get();
    premier
}

// ---------------------------------------------------------------------------------
// Le garde-fou de bout en bout (correction de M1b-05).
//
// Les assertions `const` de ce module vérifient les **tables**. Elles ne disent rien du
// chemin qui va du magasin `CABLE_FORMATS` — écrit au démarrage, depuis le registre — à la
// rangée que `wave::description` finira par rendre, ni de ce que PortCls lira au bout des
// pointeurs `DataRanges`. C'est précisément l'intervalle où un défaut serait **muet** : les
// deux `unwrap_or_else` de `wave.rs` avalent une variante introuvable, et `kmd_log!` est
// vide en release. [`check_cable_pins`] referme cet intervalle, à `StartDevice`, câble par
// câble, et son échec part au **journal d'événements** — visible sans débogueur.
// ---------------------------------------------------------------------------------

/// Ce qui sépare le format retenu pour un câble de ce que sa broche système déclare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMismatch {
    /// Le format du câble n'a aucune variante : `wave::description` se replierait sur
    /// celle du défaut, et l'endpoint servirait un autre format que la clé n'annonce.
    SansVariante,
    /// La variante existe mais la table de filtres du sens ne la contient pas.
    SansFiltre(usize),
    /// Le descripteur est là, mais sa broche système ne porte aucune plage audio lisible.
    SansPlage(usize),
    /// Les plages de la broche système ne portent pas la liste d'attributs de mode de
    /// traitement du signal, ou pas là où KS la cherche (M1b-22).
    SansAttribut(usize),
    /// La broche déclare `(fréquence, canaux)` là où la clé annonce autre chose.
    Plage {
        /// Ce que la broche déclare : fréquence en Hz, nombre de canaux.
        declares: (ULONG, ULONG),
        /// Ce que le magasin des formats annonce.
        attendus: (ULONG, ULONG),
    },
}

impl fmt::Display for PinMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SansVariante => f.write_str(
                "le format retenu n'a aucune variante de descripteurs (repli silencieux \
                 sur celle du format par défaut)",
            ),
            Self::SansFiltre(variant) => write!(
                f,
                "la variante {variant} n'a pas de descripteur de filtre wave \
                 (table de {VARIANT_COUNT} entrées)"
            ),
            Self::SansPlage(variant) => write!(
                f,
                "la broche système de la variante {variant} ne porte aucune plage audio"
            ),
            Self::SansAttribut(variant) => write!(
                f,
                "la broche système de la variante {variant} ne déclare pas le mode de \
                 traitement du signal (KSDATARANGE_ATTRIBUTES et sa KSATTRIBUTE_LIST) : \
                 le pilote n'apparaîtra pas « mode aware »"
            ),
            Self::Plage {
                declares: (hz, canaux),
                attendus: (hz_attendu, canaux_attendus),
            } => write!(
                f,
                "la broche système déclare {hz} Hz sur {canaux} canaux, \
                 le registre annonce {hz_attendu} Hz sur {canaux_attendus}"
            ),
        }
    }
}

/// La fréquence et le nombre de canaux que la broche `pin` de `filter` déclare
/// **réellement**, lus au bout des pointeurs que PortCls suivra ; `None` si cette broche
/// n'existe pas ou ne porte pas de plage audio (une broche bridge, par exemple).
fn declared_by_pin(filter: &'static PCFILTER_DESCRIPTOR, pin: ULONG) -> Option<(ULONG, ULONG)> {
    // La **première** entrée du tableau : depuis M1b-22 une entrée sur deux est une liste
    // d'attributs, mais la première reste toujours une plage.
    let first = range_entry(filter, pin, 0)?;
    // SAFETY: lecture du membre nommé de l'union `KSDATAFORMAT` — le préfixe commun aux
    // deux formes de plage de ce module. Lire l'en-tête est légitime avant de savoir
    // laquelle des deux on tient.
    let header = unsafe { first.__bindgen_anon_1 };
    if header.FormatSize != KSDATARANGE_AUDIO_SIZE {
        // Plage analogique (64 octets) : ce n'est pas une broche système. C'est ce
        // contrôle qui rend l'élargissement ci-dessous sûr, plutôt qu'une convention.
        return None;
    }
    // SAFETY: `FormatSize` annonce les 88 octets d'une `KSDATARANGE_AUDIO`, et toutes les
    // plages de cette taille de ce module sont bâties par `audio_range` puis logées dans
    // `SYSTEM_RANGE_VALUES` : l'élargissement du type ne dépasse pas l'objet.
    let audio = unsafe { &*ptr::from_ref(first).cast::<KSDATARANGE_AUDIO>() };
    Some((audio.MinimumSampleFrequency, audio.MaximumChannels))
}

/// Le nombre d'entrées d'un tableau `DataRanges`, et un accès borné à la `n`-ième.
///
/// Facteur commun de [`declared_by_pin`] et [`mode_attributes_declared`] : les deux
/// traversent le **même** tableau, celui que PortCls suivra, et une seconde copie de cette
/// arithmétique de pointeurs serait exactement le genre de divergence qu'on cherche à
/// éviter.
///
/// `None` si la broche n'existe pas, si le tableau est nul, ou si `n` est au-delà du
/// compte déclaré.
fn range_entry(
    filter: &'static PCFILTER_DESCRIPTOR,
    pin: ULONG,
    n: usize,
) -> Option<&'static KSDATARANGE> {
    let index = usize::try_from(pin).ok()?;
    let count = usize::try_from(filter.PinCount).ok()?;
    if index >= count || filter.Pins.is_null() {
        return None;
    }
    // SAFETY: `filter` est une rangée d'une des tables `static` de ce module ; son champ
    // `Pins` vise un tableau de `PinCount` `PCPIN_DESCRIPTOR` bâti en `const` dans ce même
    // module, immuable et jamais libéré. `index < count`, donc le décalage reste dedans.
    let descriptor = unsafe { &*filter.Pins.add(index) };
    let ranges = descriptor.KsPinDescriptor.DataRanges;
    let entries = usize::try_from(descriptor.KsPinDescriptor.DataRangesCount).ok()?;
    if ranges.is_null() || n >= entries {
        return None;
    }
    // SAFETY: `DataRanges` vise un tableau de `DataRangesCount` pointeurs logé dans
    // `SYSTEM_RANGES_TABLE` ou dans `BRIDGE_RANGES` ; `n < entries`, le décalage reste
    // dedans.
    let entry = unsafe { *ranges.add(n) };
    if entry.is_null() {
        return None;
    }
    // SAFETY: toute entrée de ces tableaux vise une `static` immuable de ce module d'au
    // moins `sizeof(KSDATARANGE)` octets — une `KSDATARANGE_AUDIO` (88), la plage
    // analogique (64, la taille exacte du type rendu) ou un [`AttributeListEntry`], dont le
    // remplissage existe précisément pour que cette lecture reste dans l'objet.
    Some(unsafe { &*entry })
}

/// Vrai si la broche `pin` de `filter` déclare bien le mode de traitement du signal :
/// chaque plage porte `KSDATARANGE_ATTRIBUTES` et l'entrée qui la suit **est** la liste
/// d'attributs à un élément (M1b-22).
///
/// C'est le pendant, du côté des attributs, de ce que [`declared_by_pin`] fait du côté du
/// format : les assertions `const` vérifient que les tables sont bien bâties, celle-ci
/// vérifie ce que PortCls lira **au bout des pointeurs**. La panne qu'elle attrape est
/// muette entre toutes : une entrée d'attributs manquante ou mal placée, et le pilote
/// reste exactement aussi peu « mode aware » qu'avant M1b-22 — mêmes endpoints, même son,
/// même allocation par scrutation, et rien nulle part pour le dire.
fn mode_attributes_declared(filter: &'static PCFILTER_DESCRIPTOR, pin: ULONG) -> bool {
    let mut n = 0;
    while n < ENTRIES_PER_SYSTEM_PIN {
        let Some(plage) = range_entry(filter, pin, n) else {
            return false;
        };
        // SAFETY: lecture du membre nommé de l'union `KSDATAFORMAT` : c'est celui que
        // `data_format` écrit, et la `KSATTRIBUTE_LIST` d'une entrée impaire est traitée
        // plus bas, sans passer par cet en-tête.
        let header = unsafe { plage.__bindgen_anon_1 };
        if n % 2 == 0 {
            // Entrée paire : une plage audio, drapeau compris.
            if header.FormatSize != KSDATARANGE_AUDIO_SIZE || header.Flags != KSDATARANGE_ATTRIBUTES
            {
                return false;
            }
        } else {
            // Entrée impaire : la liste d'attributs. Comparer les **adresses** est ce qui
            // dit qu'on a bien l'entrée partagée et pas une plage prise pour elle.
            let attendu: *const KSDATARANGE = mode_attribute_ptr().cast_const();
            if !ptr::eq(ptr::from_ref(plage), attendu) {
                return false;
            }
        }
        n = n.wrapping_add(1);
    }
    // Et la liste elle-même : un attribut, celui du mode de traitement du signal.
    let liste = MODE_ATTRIBUTE_ENTRY.get();
    if liste.list.Count != 1 || liste.list.Attributes.is_null() {
        return false;
    }
    // SAFETY: `Attributes` vise `MODE_ATTRIBUTE_POINTERS`, un tableau `static` d'un seul
    // pointeur, immuable et jamais libéré ; `Count` vaut 1 (vérifié).
    let premier = unsafe { *liste.list.Attributes };
    if premier.is_null() {
        return false;
    }
    // SAFETY: `premier` vise `MODE_ATTRIBUTE`, une `static` immuable de ce module.
    let attribut = unsafe { &*premier };
    attribut.Size == size_of::<KSATTRIBUTE>() as ULONG
        && crate::intersect::guid_eq(
            &attribut.Attribute,
            &KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE,
        )
}

/// Vérifie que les **deux** broches système du câble `cable` déclareront bien la fréquence
/// et le nombre de canaux que son format annonce.
///
/// C'est le seul contrôle du dépôt qui traverse toute la chaîne — magasin `CABLE_FORMATS`
/// → variante → rangée de `WAVE_*_FILTER_TABLE` → tableau `DataRanges` → `KSDATARANGE_AUDIO`
/// — et il la traverse **par les pointeurs que PortCls suivra**, pas par les valeurs qui ont
/// servi à les bâtir. Un décalage d'index, une variante introuvable, une rangée qui ne
/// correspondrait plus : tout cela se dit ici, au démarrage, dans le journal d'événements.
///
/// Les deux sens sont contrôlés ensemble et **doivent** s'accorder : c'est l'accord des deux
/// bouts d'un câble, celui sans lequel `copy_frames` refuserait de copier
/// (`RingError::ChannelMismatch`).
///
/// IRQL : quelconque ; ne lit que des `static` immuables et un atomique.
///
/// # Erreurs
///
/// [`PinMismatch`], qui nomme le maillon rompu.
pub fn check_cable_pins(cable: u32) -> Result<(), PinMismatch> {
    let format = cable_format(cable);
    let variant = format.variant().ok_or(PinMismatch::SansVariante)?;
    let attendus = (format.sample_rate, ULONG::from(format.channels));
    let render = WAVE_RENDER_FILTER_TABLE
        .get()
        .get(variant)
        .ok_or(PinMismatch::SansFiltre(variant))?;
    let capture = WAVE_CAPTURE_FILTER_TABLE
        .get()
        .get(variant)
        .ok_or(PinMismatch::SansFiltre(variant))?;
    for (filtre, broche) in [
        (render, WAVE_RENDER_PIN_SYSTEM),
        (capture, WAVE_CAPTURE_PIN_SYSTEM),
    ] {
        let declares = declared_by_pin(filtre, broche).ok_or(PinMismatch::SansPlage(variant))?;
        if declares != attendus {
            return Err(PinMismatch::Plage { declares, attendus });
        }
        // La condition SYSVAD de M1b-22, vérifiée là où PortCls la lira : chaque plage
        // porte son drapeau, et l'entrée qui la suit **est** la liste d'attributs.
        if !mode_attributes_declared(filtre, broche) {
            return Err(PinMismatch::SansAttribut(variant));
        }
    }
    Ok(())
}

/// Ce que la broche système du filtre `WaveRender` du câble `cable` déclarera
/// **réellement** à PortCls : fréquence en Hz et nombre de canaux, lus au bout des mêmes
/// pointeurs que [`check_cable_pins`] traverse. `None` si le format du câble n'a pas de
/// variante, si la table n'a pas cette rangée, ou si la broche ne porte pas de plage audio.
///
/// Existe pour que [`crate::topo::check_cable_topology`] confronte ce que la **topologie**
/// déclare — le nombre de canaux des nœuds, la cartographie de haut-parleurs du jack — à ce
/// que la broche wave déclare, plutôt qu'à la valeur qui a servi à bâtir les deux. Un
/// endpoint naît de la connexion des deux filtres : les faire diverger est exactement le
/// genre de faute que ni les assertions `const` (qui ne voient que les tables) ni
/// `check_cable_pins` (qui ne regarde que le côté wave) n'attraperaient.
///
/// IRQL : quelconque ; ne lit que des `static` immuables et un atomique.
#[must_use]
pub fn declared_by_render_system_pin(cable: u32) -> Option<(ULONG, ULONG)> {
    let variant = cable_format(cable).variant()?;
    let render = WAVE_RENDER_FILTER_TABLE.get().get(variant)?;
    declared_by_pin(render, WAVE_RENDER_PIN_SYSTEM)
}

// ---------------------------------------------------------------------------------
// Invariants (§4.1), vérifiés à la compilation.
// ---------------------------------------------------------------------------------

/// Flux et communication d'une broche.
const fn pin_is(
    pin: &PCPIN_DESCRIPTOR,
    flow: KSPIN_DATAFLOW::Type,
    comm: KSPIN_COMMUNICATION::Type,
    instances: ULONG,
) -> bool {
    pin.KsPinDescriptor.DataFlow == flow
        && pin.KsPinDescriptor.Communication == comm
        && pin.MaxGlobalInstanceCount == instances
        && pin.MaxFilterInstanceCount == instances
}

/// Les deux broches d'un filtre topologie, pour **toutes** les rangées d'une table (une
/// par câble) : orientation et communication, et la broche endpoint — la sortie au rendu,
/// l'entrée à la capture — est la seule nommée, avec sa plage analogique unique.
///
/// Motif de tranche : ni indexation ni arithmétique.
const fn topo_pins_are_well_formed(
    mut rangees: &[[PCPIN_DESCRIPTOR; PIN_COUNT]],
    endpoint_en_sortie: bool,
) -> bool {
    while let [rangee, reste @ ..] = rangees {
        let [entree, sortie] = rangee;
        if !pin_is(
            entree,
            KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE,
            0,
        ) || !pin_is(
            sortie,
            KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE,
            0,
        ) {
            return false;
        }
        let (endpoint, bridge) = if endpoint_en_sortie {
            (sortie, entree)
        } else {
            (entree, sortie)
        };
        if endpoint.KsPinDescriptor.Name.is_null() || !bridge.KsPinDescriptor.Name.is_null() {
            return false;
        }
        if endpoint.KsPinDescriptor.DataRangesCount != 1
            || bridge.KsPinDescriptor.DataRangesCount != 1
        {
            return false;
        }
        rangees = reste;
    }
    true
}

/// Les deux broches d'un filtre wave, pour **toutes** les rangées d'une table (une par
/// variante de format) : orientation, communication, comptes de plages, et aucune broche
/// nommée (le nom du câble est sur les filtres de topologie).
///
/// `systeme_en_entree` : vrai pour `WaveRender` (le lecteur écrit sur la broche 0), faux
/// pour `WaveCapture` (l'enregistreur lit sur la broche 1).
///
/// Motif de tranche : ni indexation ni arithmétique.
const fn wave_pins_are_well_formed(
    mut rangees: &[[PCPIN_DESCRIPTOR; PIN_COUNT]],
    systeme_en_entree: bool,
) -> bool {
    while let [rangee, reste @ ..] = rangees {
        let [entree, sortie] = rangee;
        let (systeme, bridge) = if systeme_en_entree {
            (entree, sortie)
        } else {
            (sortie, entree)
        };
        if !pin_is(
            entree,
            KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN,
            if systeme_en_entree {
                KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_SINK
            } else {
                KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE
            },
            if systeme_en_entree { 1 } else { 0 },
        ) || !pin_is(
            sortie,
            KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT,
            if systeme_en_entree {
                KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE
            } else {
                KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_SINK
            },
            if systeme_en_entree { 0 } else { 1 },
        ) {
            return false;
        }
        // Trois plages système (une par profondeur) **et leurs trois entrées
        // d'attributs**, une plage analogique sur le bridge. Compter `RANGES_*` plutôt que
        // `ENTRIES_*` ferait lire à KS la moitié du tableau : deux profondeurs perdues, et
        // la troisième sans son attribut.
        if systeme.KsPinDescriptor.DataRangesCount as usize != ENTRIES_PER_SYSTEM_PIN
            || bridge.KsPinDescriptor.DataRangesCount != 1
        {
            return false;
        }
        // Aucune broche wave ne porte de nom : KS retombe sur la catégorie, et le nom du
        // câble est ailleurs.
        if !systeme.KsPinDescriptor.Name.is_null() || !bridge.KsPinDescriptor.Name.is_null() {
            return false;
        }
        rangees = reste;
    }
    true
}

/// Chaque variante déclare bien la fréquence et le nombre de canaux qu'elle annonce, dans
/// les trois profondeurs, et toutes ses plages sont **ponctuelles**.
///
/// C'est l'assertion la plus utile de M1b-05, parce que la panne qu'elle attrape est
/// muette : vingt-quatre variantes toutes bâties sur les mêmes valeurs — un `variant` qui
/// ne serait pas propagé, un `wrapping_rem` pris pour un `wrapping_div` — donneraient un
/// pilote qui charge, seize endpoints qui apparaissent, et pas un câble au format demandé.
/// Rien dans le journal, rien dans `infverif`, rien dans la VM avant l'écoute.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `variant < VARIANT_COUNT`
const fn variant_ranges_are_well_formed() -> bool {
    let mut variant = 0;
    while variant < VARIANT_COUNT {
        let canaux = variant_channels(variant);
        let frequence = variant_rate(variant);
        // La variante doit se retrouver depuis ses propres valeurs : c'est le contrôle
        // qui attrape un rangement décalé d'un cran.
        let index = conduit_kmd_core::variant_of(frequence, canaux as u8);
        if !matches!(index, Some(i) if i == variant) {
            return false;
        }
        let [f32_, pcm24, i16_] = &SYSTEM_RANGE_VALUES.get()[variant];
        if !range_is(
            f32_,
            &KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            32,
            canaux,
            frequence,
        ) || !range_is(pcm24, &KSDATAFORMAT_SUBTYPE_PCM, 24, canaux, frequence)
            || !range_is(i16_, &KSDATAFORMAT_SUBTYPE_PCM, 16, canaux, frequence)
        {
            return false;
        }
        variant = variant.wrapping_add(1);
    }
    true
}

/// Égalité de deux `GUID` en contexte `const` (le type généré n'implémente pas
/// `PartialEq`). Motif de tranche sur `Data4` : ni indexation ni arithmétique.
const fn guid_eq(a: &GUID, b: &GUID) -> bool {
    let [a0, a1, a2, a3, a4, a5, a6, a7] = a.Data4;
    let [b0, b1, b2, b3, b4, b5, b6, b7] = b.Data4;
    a.Data1 == b.Data1
        && a.Data2 == b.Data2
        && a.Data3 == b.Data3
        && a0 == b0
        && a1 == b1
        && a2 == b2
        && a3 == b3
        && a4 == b4
        && a5 == b5
        && a6 == b6
        && a7 == b7
}

/// Une plage système : sous-type, profondeur, canaux et fréquence attendus, bornes
/// **ponctuelles** des deux côtés, spécificateur `WAVEFORMATEX` et `FormatSize` exact.
///
/// Le sous-type est vérifié parce que rien d'autre ne le ferait : une plage flottante
/// annoncée en `KSDATAFORMAT_SUBTYPE_PCM` a la bonne taille, les bonnes bornes, le bon
/// `FormatSize`, et ne se manifeste qu'à l'écoute — le moteur audio y écrirait des entiers
/// que le câble relirait comme des flottants.
const fn range_is(
    range: &KSDATARANGE_AUDIO,
    subtype: &GUID,
    bits: ULONG,
    channels: ULONG,
    rate: ULONG,
) -> bool {
    // SAFETY: lecture du membre nommé de l'union, le seul que `data_format` écrive.
    let header = unsafe { range.DataRange.__bindgen_anon_1 };
    range.MaximumChannels == channels
        && range.MinimumBitsPerSample == bits
        && range.MaximumBitsPerSample == bits
        && range.MinimumSampleFrequency == rate
        && range.MaximumSampleFrequency == rate
        && header.FormatSize == KSDATARANGE_AUDIO_SIZE
        // `KSDATARANGE_ATTRIBUTES` et lui seul (M1b-22) : sans ce drapeau, KS ignorerait
        // purement et simplement l'entrée d'attributs qui suit la plage, et la broche ne
        // serait pas « mode aware ». Avec `KSDATARANGE_REQUIRED_ATTRIBUTES` en plus, elle
        // exigerait de chaque client qu'il fournisse le mode — ce que le moteur ne fait pas
        // toujours.
        && header.Flags == KSDATARANGE_ATTRIBUTES
        && guid_eq(&header.MajorFormat, &KSDATAFORMAT_TYPE_AUDIO)
        && guid_eq(&header.SubFormat, subtype)
        && guid_eq(&header.Specifier, &KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
}

/// Les GUID de nom de broche portent le numéro du câble, dans l'ordre : `names[i]` a
/// `Data4[7] == depart + i`.
///
/// C'est l'assertion qui distingue seize câbles d'un seul. Les autres ne vérifient que la
/// **présence** d'un nom ; avec seize endpoints, les pannes à attraper sont « deux câbles
/// portent le même nom » (endpoints indistinguables dans le panneau de son) et « le
/// câble 5 porte le GUID du 6 » (endpoints permutés). `portcls::pin_name_guid` ne faisant
/// varier que ce dernier octet, celle-ci les couvre toutes les deux.
#[allow(clippy::indexing_slicing)] // index constant dans un tableau de 8 octets
const fn pin_names_are_numbered(mut names: &[GUID], mut attendu: u8) -> bool {
    while let [premier, reste @ ..] = names {
        if premier.Data4[7] != attendu {
            return false;
        }
        names = reste;
        attendu = attendu.wrapping_add(1);
    }
    true
}

const _: () = {
    // Deux broches par filtre, numérotées par la direction des données ; un jeu de
    // broches de topologie par câble, un jeu de broches wave par **variante de format**.
    assert!(WAVE_RENDER_PINS.len() == VARIANT_COUNT && WAVE_CAPTURE_PINS.len() == VARIANT_COUNT);
    assert!(TOPO_RENDER_PINS.len() == CABLE_COUNT && TOPO_CAPTURE_PINS.len() == CABLE_COUNT);
    assert!(WAVE_RENDER_PIN_SYSTEM == 0 && WAVE_RENDER_PIN_BRIDGE == 1);
    assert!(TOPO_RENDER_PIN_BRIDGE == 0 && TOPO_RENDER_PIN_ENDPOINT == 1);
    assert!(WAVE_CAPTURE_PIN_BRIDGE == 0 && WAVE_CAPTURE_PIN_SYSTEM == 1);
    assert!(TOPO_CAPTURE_PIN_ENDPOINT == 0 && TOPO_CAPTURE_PIN_BRIDGE == 1);

    // Orientation, nommage et comptes de plages, sur les **24** rangées de chaque sens et
    // pas seulement la première : une variante mal bâtie ne se manifesterait que sur les
    // câbles réglés dessus, c'est-à-dire chez l'utilisateur et nulle part ailleurs.
    assert!(wave_pins_are_well_formed(&WAVE_RENDER_PINS, true));
    assert!(wave_pins_are_well_formed(&WAVE_CAPTURE_PINS, false));
    // Les seize jeux de broches de topologie, orientation et nommage compris.
    assert!(topo_pins_are_well_formed(&TOPO_RENDER_PINS, true));
    assert!(topo_pins_are_well_formed(&TOPO_CAPTURE_PINS, false));

    // Chaque variante déclare bien SA fréquence et SES canaux, dans les trois profondeurs.
    // C'est l'assertion qui sépare une matrice correcte d'une matrice dont toutes les
    // entrées seraient celle du défaut — panne parfaitement muette : le pilote chargerait,
    // les endpoints apparaîtraient, et tous seraient stéréo à 48 kHz.
    assert!(variant_ranges_are_well_formed());

    // `FormatSize` exact, sur une plage prise au hasard de la matrice et sur l'analogique.
    // SAFETY: lecture du membre nommé de l'union, celui que `data_format` a écrit.
    let header = unsafe {
        audio_range(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 32, 2, 48_000)
            .DataRange
            .__bindgen_anon_1
    };
    assert!(header.FormatSize == 88 && header.Flags == KSDATARANGE_ATTRIBUTES);
    assert!(KSDATARANGE_ATTRIBUTES == 2, "(1 << 1), ks.h");
    // SAFETY: idem.
    let analog = unsafe { analog_range().__bindgen_anon_1 };
    // La plage analogique reste à `Flags = 0` : une broche bridge n'a pas de mode. C'est
    // la seule différence de forme entre les deux plages, et la confondre déclarerait une
    // liste d'attributs qui n'existe pas — KS lirait l'entrée suivante du tableau, qui est
    // une plage, comme une `KSATTRIBUTE_LIST`.
    assert!(analog.FormatSize == 64 && analog.Flags == 0);

    // La liste d'attributs : un seul élément, l'identifiant du mode de traitement du
    // signal, et un en-tête à la taille exacte d'un `KSATTRIBUTE`. `Size` est le seul champ
    // que KS relit pour avancer dans la liste : une valeur fausse ferait dérailler son
    // parcours, en silence.
    assert!(size_of::<KSATTRIBUTE>() == 24 && size_of::<KSATTRIBUTE_LIST>() == 16);
    assert!(MODE_ATTRIBUTE_VALUE.Size == size_of::<KSATTRIBUTE>() as ULONG);
    assert!(MODE_ATTRIBUTE_VALUE.Flags == 0);
    assert!(guid_eq(
        &MODE_ATTRIBUTE_VALUE.Attribute,
        &KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE
    ));
    // L'entrée d'attributs occupe au moins autant qu'une `KSDATARANGE_AUDIO` : c'est ce qui
    // rend définie la lecture d'en-tête que ferait `intersect::read_our_range` si PortCls
    // nous la tendait un jour comme une plage (voir `AttributeListEntry`).
    assert!(size_of::<AttributeListEntry>() == KSDATARANGE_AUDIO_SIZE as usize);
    // Et les entrées d'attributs sont bien comptées dans `DataRanges` : deux entrées par
    // profondeur, ni une de plus ni une de moins.
    assert!(ENTRIES_PER_SYSTEM_PIN == 6 && ENTRIES_PER_SYSTEM_PIN == RANGES_PER_SYSTEM_PIN * 2);

    // Nom de broche (§4.2) : seules les deux broches endpoint des filtres topologie en
    // portent un (vérifié rangée par rangée ci-dessus) ; comparer les adresses ici est
    // hors de portée de l'évaluation `const`, mais le **numéro** porté par chaque GUID
    // l'est, et c'est lui qui distingue les seize câbles.
    assert!(PIN_NAME_GUIDS.len() == CABLE_COUNT);
    assert!(pin_names_are_numbered(&PIN_NAME_GUIDS, 0));

    // Connexion directe 0 → 1 via `PCFILTER_NODE`.
    let direct = connection(PCFILTER_NODE, 0, PCFILTER_NODE, 1);
    assert!(direct.FromNode == ULONG::MAX && direct.ToNode == ULONG::MAX);
    assert!(direct.FromNodePin == 0 && direct.ToNodePin == 1);
};

/// Le tronc commun aux deux formes : version, tailles d'élément, broches, connexions et
/// catégories.
const fn filter_shape_is_common(filter: &PCFILTER_DESCRIPTOR) -> bool {
    filter.Version == 0
        && filter.PinSize as usize == size_of::<PCPIN_DESCRIPTOR>()
        && filter.PinCount as usize == PIN_COUNT
        && !filter.Pins.is_null()
        && filter.NodeSize as usize == size_of::<PCNODE_DESCRIPTOR>()
        && !filter.Connections.is_null()
        && !filter.AutomationTable.is_null()
        && filter.CategoryCount == 0
}

/// **Forme wave** : tronc commun, aucun nœud (`Nodes` nul, pas seulement `NodeCount` nul)
/// et l'unique connexion directe.
const fn wave_filter_is_well_formed(filter: &PCFILTER_DESCRIPTOR) -> bool {
    filter_shape_is_common(filter)
        && filter.NodeCount == 0
        && filter.Nodes.is_null()
        && filter.ConnectionCount as usize == WAVE_CONNECTION_COUNT
}

/// **Forme topologie** : tronc commun, [`NODE_COUNT`] nœuds réellement pointés et les
/// [`TOPO_CONNECTION_COUNT`] connexions qui les mettent en série.
const fn topo_filter_is_well_formed(filter: &PCFILTER_DESCRIPTOR) -> bool {
    filter_shape_is_common(filter)
        && filter.NodeCount as usize == NODE_COUNT
        && !filter.Nodes.is_null()
        && filter.ConnectionCount as usize == TOPO_CONNECTION_COUNT
}

/// Tous les filtres d'une table de topologie (même motif de tranche que
/// [`connections_are_well_formed`]).
const fn topo_filters_are_well_formed(mut filters: &[PCFILTER_DESCRIPTOR]) -> bool {
    while let [premier, reste @ ..] = filters {
        if !topo_filter_is_well_formed(premier) {
            return false;
        }
        filters = reste;
    }
    true
}

/// Tous les filtres d'une table wave (même motif de tranche).
const fn wave_filters_are_well_formed(mut filters: &[PCFILTER_DESCRIPTOR]) -> bool {
    while let [premier, reste @ ..] = filters {
        if !wave_filter_is_well_formed(premier) {
            return false;
        }
        filters = reste;
    }
    true
}

const _: () = {
    // Les 24 filtres wave de chaque sens, et pas seulement le premier.
    assert!(WAVE_RENDER_FILTERS.len() == VARIANT_COUNT);
    assert!(WAVE_CAPTURE_FILTERS.len() == VARIANT_COUNT);
    assert!(wave_filters_are_well_formed(&WAVE_RENDER_FILTERS));
    assert!(wave_filters_are_well_formed(&WAVE_CAPTURE_FILTERS));
    // Les seize filtres de topologie de chaque sens, idem.
    assert!(TOPO_RENDER_FILTERS.len() == CABLE_COUNT);
    assert!(TOPO_CAPTURE_FILTERS.len() == CABLE_COUNT);
    assert!(topo_filters_are_well_formed(&TOPO_RENDER_FILTERS));
    assert!(topo_filters_are_well_formed(&TOPO_CAPTURE_FILTERS));
};

// ---------------------------------------------------------------------------------
// Graphe de topologie bien formé.
//
// Le pire mode de panne de ce pilote : une table de connexions incohérente donne « aucun
// endpoint n'apparaît, aucun message d'erreur ». Ni le chargement, ni `StartDevice`, ni
// `infverif` ne bronchent — un index de nœud d'un de trop, une broche de nœud prise à
// l'endroit (`KSNODEPIN_STANDARD_IN` vaut 1, pas 0) et la topologie ne se construit
// simplement pas. Aucune relecture ne voit ça de façon fiable ; ces assertions, si.
// ---------------------------------------------------------------------------------

/// Une extrémité de connexion : soit une broche du filtre (`PCFILTER_NODE`, et le numéro
/// doit alors être une broche existante), soit un index de nœud valide.
const fn endpoint_is_well_formed(
    node: ULONG,
    pin: ULONG,
    node_count: ULONG,
    pin_count: ULONG,
) -> bool {
    if node == PCFILTER_NODE {
        pin < pin_count
    } else {
        // Les broches d'un nœud ne sont pas énumérables depuis le descripteur (elles
        // dépendent du type de nœud) : seul l'index est vérifiable ici.
        node < node_count
    }
}

/// Les deux extrémités d'une connexion sont bien formées.
const fn connection_is_well_formed(
    c: &PCCONNECTION_DESCRIPTOR,
    node_count: ULONG,
    pin_count: ULONG,
) -> bool {
    endpoint_is_well_formed(c.FromNode, c.FromNodePin, node_count, pin_count)
        && endpoint_is_well_formed(c.ToNode, c.ToNodePin, node_count, pin_count)
}

/// Toutes les connexions d'une table, sans indexation ni arithmétique (motif de tranche :
/// la tête et le reste, jusqu'à épuisement).
const fn connections_are_well_formed(
    mut connections: &[PCCONNECTION_DESCRIPTOR],
    node_count: ULONG,
    pin_count: ULONG,
) -> bool {
    while let [premiere, reste @ ..] = connections {
        if !connection_is_well_formed(premiere, node_count, pin_count) {
            return false;
        }
        connections = reste;
    }
    true
}

const _: () = {
    // Filtres WaveRT : aucun nœud, donc toute connexion doit passer par `PCFILTER_NODE`
    // et ne désigner que des broches existantes.
    assert!(connections_are_well_formed(
        &DIRECT_CONNECTION,
        0,
        PIN_COUNT as ULONG
    ));
    // Filtres topologie : deux nœuds et deux broches.
    assert!(connections_are_well_formed(
        &TOPO_CONNECTIONS,
        NODE_COUNT as ULONG,
        PIN_COUNT as ULONG
    ));

    // Et la série est bien celle qu'on croit : broche 0 → volume → sourdine → broche 1.
    // Sans cela, trois connexions individuellement valides pourraient former un graphe
    // qui ne relie pas les deux broches.
    assert!(TOPO_CONNECTIONS[0].FromNode == PCFILTER_NODE);
    assert!(TOPO_CONNECTIONS[0].FromNodePin == TOPO_PIN_IN);
    assert!(TOPO_CONNECTIONS[0].ToNode == NODE_VOLUME);
    assert!(TOPO_CONNECTIONS[0].ToNodePin == KSNODEPIN_STANDARD_IN);
    assert!(TOPO_CONNECTIONS[1].FromNode == NODE_VOLUME);
    assert!(TOPO_CONNECTIONS[1].FromNodePin == KSNODEPIN_STANDARD_OUT);
    assert!(TOPO_CONNECTIONS[1].ToNode == NODE_MUTE);
    assert!(TOPO_CONNECTIONS[1].ToNodePin == KSNODEPIN_STANDARD_IN);
    assert!(TOPO_CONNECTIONS[2].FromNode == NODE_MUTE);
    assert!(TOPO_CONNECTIONS[2].FromNodePin == KSNODEPIN_STANDARD_OUT);
    assert!(TOPO_CONNECTIONS[2].ToNode == PCFILTER_NODE);
    assert!(TOPO_CONNECTIONS[2].ToNodePin == TOPO_PIN_OUT);

    // Les broches de nœud sont numérotées à l'envers des broches de filtre : c'est le
    // genre de constante qu'on recopie de travers une fois pour toutes.
    assert!(KSNODEPIN_STANDARD_IN == 1 && KSNODEPIN_STANDARD_OUT == 0);

    // Les index de nœud sont ceux que les connexions ci-dessus désignent : les assertions
    // qui suivent indexent par littéral (`NODE_VOLUME as usize` n'est pas reconnu comme
    // index constant par `clippy::indexing_slicing`), celle-ci fait le lien.
    assert!(NODE_VOLUME == 0 && NODE_MUTE == 1);
    assert!(TOPO_RENDER_NODES.len() == NODE_COUNT && TOPO_CAPTURE_NODES.len() == NODE_COUNT);

    // Les nœuds : type non nul, table d'automatisation présente, nom **nul** — KS retombe
    // alors sur `Type` pour le nom affiché, ce qui donne les noms standard traduits et
    // aucun contenu par câble.
    assert!(!TOPO_RENDER_NODES[0].Type.is_null() && !TOPO_RENDER_NODES[1].Type.is_null());
    assert!(!TOPO_CAPTURE_NODES[0].Type.is_null() && !TOPO_CAPTURE_NODES[1].Type.is_null());
    assert!(!TOPO_RENDER_NODES[0].AutomationTable.is_null());
    assert!(!TOPO_RENDER_NODES[1].AutomationTable.is_null());
    assert!(!TOPO_CAPTURE_NODES[0].AutomationTable.is_null());
    assert!(!TOPO_CAPTURE_NODES[1].AutomationTable.is_null());
    assert!(TOPO_RENDER_NODES[0].Name.is_null() && TOPO_RENDER_NODES[1].Name.is_null());
    assert!(TOPO_CAPTURE_NODES[0].Name.is_null() && TOPO_CAPTURE_NODES[1].Name.is_null());
    assert!(TOPO_RENDER_NODES[0].Flags == 0 && TOPO_RENDER_NODES[1].Flags == 0);
    assert!(TOPO_CAPTURE_NODES[0].Flags == 0 && TOPO_CAPTURE_NODES[1].Flags == 0);
};

// ---------------------------------------------------------------------------------
// Tables d'automatisation bien formées.
//
// Seconde panne muette : PortCls avance dans le tableau de propriétés par pas de
// `PropertyItemSize`, et n'appelle que si `Flags` porte le verbe demandé. Une taille
// erronée, un compte qui ne correspond pas au tableau, un `Handler` absent ou des `Flags`
// nuls, et la propriété ne répond simplement jamais — sans erreur, sans trace.
// ---------------------------------------------------------------------------------

/// Une entrée de table : jeu de propriétés désigné, gestionnaire présent, et au moins un
/// verbe déclaré (`GET`, `SET` ou `BASICSUPPORT`, cf. `portcls::ACCESS_FLAGS`).
const fn property_item_is_well_formed(item: &PCPROPERTY_ITEM) -> bool {
    !item.Set.is_null() && item.Handler.is_some() && item.Flags & portcls::ACCESS_FLAGS != 0
}

/// Toutes les entrées d'un tableau de propriétés (même motif de tranche que
/// [`connections_are_well_formed`]).
const fn property_items_are_well_formed(mut items: &[PCPROPERTY_ITEM]) -> bool {
    while let [premier, reste @ ..] = items {
        if !property_item_is_well_formed(premier) {
            return false;
        }
        items = reste;
    }
    true
}

/// Une entrée d'événement : jeu désigné, gestionnaire présent, et les flags de SYSVAD —
/// `ENABLE` (sans quoi PortCls refuserait les verbes `ADD`/`REMOVE`, donc tout abonnement)
/// et **pas** `ONESHOT` (l'abonnement doit survivre à la première notification).
const fn event_item_is_well_formed(item: &PCEVENT_ITEM) -> bool {
    !item.Set.is_null()
        && item.Handler.is_some()
        && item.Id == JACK_INFO_CHANGE_ID
        && item.Flags == JACK_EVENT_FLAGS
}

/// Toutes les entrées d'un tableau d'événements (même motif de tranche que
/// [`connections_are_well_formed`]).
const fn event_items_are_well_formed(mut items: &[PCEVENT_ITEM]) -> bool {
    while let [premier, reste @ ..] = items {
        if !event_item_is_well_formed(premier) {
            return false;
        }
        items = reste;
    }
    true
}

/// Une `PCAUTOMATION_TABLE` : tailles d'élément exactes, `PropertyCount` et `EventCount`
/// égaux à la longueur des tableaux qu'ils pointent, pointeurs nuls si et seulement si les
/// comptes le sont, et aucune méthode (le pilote n'en expose pas).
///
/// `events` **n'est pas toujours nul** depuis M1b-04 : les deux tables de filtre de
/// topologie en déclarent un, les quatre tables de nœud aucun. C'est ce paramètre qui
/// attrape une table de filtre qui aurait perdu son `with_events` — cas parfaitement muet
/// autrement : la propriété continuerait de répondre, Windows ne s'abonnerait jamais, et
/// l'interface resterait figée.
const fn automation_is_well_formed(
    table: &PCAUTOMATION_TABLE,
    properties: usize,
    events: usize,
) -> bool {
    table.PropertyItemSize as usize == size_of::<PCPROPERTY_ITEM>()
        && table.PropertyCount as usize == properties
        && table.Properties.is_null() == (properties == 0)
        && table.MethodItemSize as usize == size_of::<PCMETHOD_ITEM>()
        && table.MethodCount == 0
        && table.Methods.is_null()
        && table.EventItemSize as usize == size_of::<PCEVENT_ITEM>()
        && table.EventCount as usize == events
        && table.Events.is_null() == (events == 0)
        && table.Reserved == 0
}

const _: () = {
    // Les deux tables de filtre **wave** (M1b-22) : une propriété — les modes de traitement
    // du signal —, aucun événement. Le reste (`KSPROPSETID_Pin`, `KSPROPSETID_Topology`)
    // reste servi par PortCls.
    assert!(automation_is_well_formed(
        &WAVE_RENDER_AUTOMATION_TABLE,
        WAVE_FILTER_PROPERTY_COUNT,
        0
    ));
    assert!(automation_is_well_formed(
        &WAVE_CAPTURE_AUTOMATION_TABLE,
        WAVE_FILTER_PROPERTY_COUNT,
        0
    ));
    assert!(property_items_are_well_formed(&WAVE_RENDER_FILTER_ITEMS));
    assert!(property_items_are_well_formed(&WAVE_CAPTURE_FILTER_ITEMS));
    // `GET | BASICSUPPORT`, et surtout **pas** `SET` : les modes servis sont un fait du
    // pilote, pas un réglage (« Get: Yes, Set: No »).
    assert!(WAVE_RENDER_FILTER_ITEMS[0].Flags == MODES_ACCESS_FLAGS);
    assert!(WAVE_CAPTURE_FILTER_ITEMS[0].Flags == MODES_ACCESS_FLAGS);
    assert!(MODES_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET == 0);

    // Les deux tables de filtre **topologie**, celles-là mêmes que les `static` livrent
    // (doublet `const` puis `static`) : le jack, l'état, la version et les compteurs,
    // **plus** l'événement `JACKINFOCHANGE`. C'est ce dernier qui sépare un endpoint qui
    // suit l'état du câble d'un endpoint figé au démarrage.
    assert!(automation_is_well_formed(
        &TOPO_RENDER_AUTOMATION_TABLE,
        FILTER_PROPERTY_COUNT,
        FILTER_EVENT_COUNT
    ));
    assert!(automation_is_well_formed(
        &TOPO_CAPTURE_AUTOMATION_TABLE,
        FILTER_PROPERTY_COUNT,
        FILTER_EVENT_COUNT
    ));
    assert!(property_items_are_well_formed(&RENDER_FILTER_ITEMS));
    assert!(property_items_are_well_formed(&CAPTURE_FILTER_ITEMS));

    // Les deux entrées d'événement : le bon jeu, le bon identifiant, les flags de SYSVAD.
    // `JACKINFOCHANGE` (1) et `FORMATCHANGE` (0) ne diffèrent que par l'identifiant, et
    // signaler le second à la place du premier serait indétectable.
    assert!(event_items_are_well_formed(&RENDER_FILTER_EVENT_ITEMS));
    assert!(event_items_are_well_formed(&CAPTURE_FILTER_EVENT_ITEMS));
    assert!(JACK_INFO_CHANGE_ID == 1);
    assert!(JACK_EVENT_FLAGS & portcls_sys::KSEVENT_TYPE_ONESHOT == 0);

    // `GET | BASICSUPPORT`, et surtout **pas** `SET` : `KSPROPERTY_JACK_DESCRIPTION` est en
    // lecture seule (« Get: Yes, Set: No »). C'est le jeu privé de M1b-04, deux entrées
    // plus loin dans la même table, qui porte l'écriture.
    assert!(RENDER_FILTER_ITEMS[0].Flags == JACK_ACCESS_FLAGS);
    assert!(CAPTURE_FILTER_ITEMS[0].Flags == JACK_ACCESS_FLAGS);
    assert!(JACK_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET == 0);

    // Le jeu privé `KSPROPSETID_Conduit` : l'état s'écrit (M1b-04), la version et les
    // compteurs non. Une version déclarée en écriture laisserait croire au service
    // d'assistance qu'il peut faire changer d'avis le pilote sur sa propre version ; un
    // compteur déclaré en écriture laisserait croire qu'on peut le remettre à zéro, alors
    // que seul un nouveau `StartDevice` le fait.
    assert!(RENDER_FILTER_ITEMS[1].Flags == CABLE_STATE_ACCESS_FLAGS);
    assert!(CAPTURE_FILTER_ITEMS[1].Flags == CABLE_STATE_ACCESS_FLAGS);
    assert!(CABLE_STATE_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET != 0);
    assert!(RENDER_FILTER_ITEMS[2].Flags == VERSION_ACCESS_FLAGS);
    assert!(CAPTURE_FILTER_ITEMS[2].Flags == VERSION_ACCESS_FLAGS);
    assert!(VERSION_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET == 0);
    assert!(RENDER_FILTER_ITEMS[3].Flags == COUNTERS_ACCESS_FLAGS);
    assert!(CAPTURE_FILTER_ITEMS[3].Flags == COUNTERS_ACCESS_FLAGS);
    assert!(COUNTERS_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET == 0);
    // Les trois entrées du jeu privé portent bien trois `Id` distincts : PortCls sert la
    // **première** entrée de même `Set`/`Id`, et deux entrées confondues rendraient une
    // propriété inatteignable sans le moindre message.
    assert!(RENDER_FILTER_ITEMS[1].Id != RENDER_FILTER_ITEMS[2].Id);
    assert!(RENDER_FILTER_ITEMS[1].Id != RENDER_FILTER_ITEMS[3].Id);
    assert!(RENDER_FILTER_ITEMS[2].Id != RENDER_FILTER_ITEMS[3].Id);
    assert!(CAPTURE_FILTER_ITEMS[1].Id != CAPTURE_FILTER_ITEMS[2].Id);
    assert!(CAPTURE_FILTER_ITEMS[1].Id != CAPTURE_FILTER_ITEMS[3].Id);
    assert!(CAPTURE_FILTER_ITEMS[2].Id != CAPTURE_FILTER_ITEMS[3].Id);

    // Les quatre tables de nœud : une propriété chacune, réellement pointée, et **aucun**
    // événement — les nœuds de volume et de sourdine n'en déclarent pas.
    assert!(automation_is_well_formed(
        &property_automation(&RENDER_VOLUME_PROPERTIES),
        1,
        0
    ));
    assert!(automation_is_well_formed(
        &property_automation(&RENDER_MUTE_PROPERTIES),
        1,
        0
    ));
    assert!(automation_is_well_formed(
        &property_automation(&CAPTURE_VOLUME_PROPERTIES),
        1,
        0
    ));
    assert!(automation_is_well_formed(
        &property_automation(&CAPTURE_MUTE_PROPERTIES),
        1,
        0
    ));

    // Et leurs entrées : gestionnaire présent, verbes déclarés.
    assert!(property_items_are_well_formed(&RENDER_VOLUME_ITEMS));
    assert!(property_items_are_well_formed(&RENDER_MUTE_ITEMS));
    assert!(property_items_are_well_formed(&CAPTURE_VOLUME_ITEMS));
    assert!(property_items_are_well_formed(&CAPTURE_MUTE_ITEMS));

    // `GET | SET | BASICSUPPORT` : le bit `BASICSUPPORT` fait de nous le seul répondant à
    // ce verbe (PortCls ne le traite plus lui-même) — c'est par lui que la plage du nœud
    // arrive jusqu'à l'interface utilisateur.
    assert!(RENDER_VOLUME_ITEMS[0].Flags == portcls::ACCESS_FLAGS);
    assert!(RENDER_MUTE_ITEMS[0].Flags == portcls::ACCESS_FLAGS);
    assert!(CAPTURE_VOLUME_ITEMS[0].Flags == portcls::ACCESS_FLAGS);
    assert!(CAPTURE_MUTE_ITEMS[0].Flags == portcls::ACCESS_FLAGS);
};
