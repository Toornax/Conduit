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
//!   connexion directe broche 0 → broche 1 (`PCFILTER_NODE`, [`DIRECT_CONNECTION`]), table
//!   d'automatisation vide (PortCls gère `KSPROPSETID_Pin` et `KSPROPSETID_Topology`) ;
//! - **forme topologie** ([`topo_filter`]), pour `TopoRender` et `TopoCapture` : deux nœuds
//!   — index [`NODE_VOLUME`] `KSNODETYPE_VOLUME`, index [`NODE_MUTE`] `KSNODETYPE_MUTE` —
//!   chacun avec sa table d'automatisation d'une propriété, et trois connexions
//!   ([`TOPO_CONNECTIONS`]) qui les mettent en série entre les deux broches. La table
//!   d'automatisation du *filtre* porte, depuis M1b-03, la **seule** propriété qui ne soit
//!   ni de nœud ni de broche : `KSPROPERTY_JACK_DESCRIPTION`.
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
//! Le seul contenu réellement par câble d'un descripteur est le GUID
//! `KsPinDescriptor.Name` des broches endpoint ([`PIN_NAMES`], un par câble depuis
//! M1b-02) : les seize filtres de topologie d'un même sens ne diffèrent que par lui, d'où
//! les tables [`TOPO_RENDER_PINS`] et [`TOPO_CAPTURE_PINS`], une rangée par câble, et les
//! deux tableaux de [`PCFILTER_DESCRIPTOR`] qui les pointent. Les **filtres wave**, eux,
//! restent uniques : rien n'y est par câble.
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
//! orientation, tailles des structures, cohérence avec `conduit_kmd_core::M1A_FORMATS`,
//! bonne formation du graphe de topologie et des tables d'automatisation) sont des
//! assertions `const`, vérifiées à la compilation. Ce sont les **seules** vérifications
//! disponibles ici, et deux d'entre elles attrapent des pannes muettes : une table de
//! connexions incohérente ne donne « aucun endpoint, aucun message d'erreur », et une
//! table d'automatisation mal remplie fait taire la propriété sans rien signaler.

use core::fmt;
use core::mem::size_of;
use core::ptr;

use conduit_kmd_core::{M1A_FORMATS, SampleFormat};
use portcls::{
    CABLE_COUNT, JACK_ACCESS_FLAGS, PIN_NAME_GUIDS, jack_description_item, mute_item, volume_item,
};
use portcls_sys::{
    GUID, IMiniportTopologyVtbl, KSCATEGORY_AUDIO, KSDATAFORMAT, KSDATAFORMAT__bindgen_ty_1,
    KSDATAFORMAT_SPECIFIER_NONE, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX, KSDATAFORMAT_SUBTYPE_ANALOG,
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, KSDATAFORMAT_SUBTYPE_PCM, KSDATAFORMAT_TYPE_AUDIO,
    KSDATARANGE, KSDATARANGE_AUDIO, KSNODEPIN_STANDARD_IN, KSNODEPIN_STANDARD_OUT,
    KSNODETYPE_LINE_CONNECTOR, KSNODETYPE_MUTE, KSNODETYPE_SPEAKER, KSNODETYPE_VOLUME,
    KSPIN_COMMUNICATION, KSPIN_DATAFLOW, KSPIN_DESCRIPTOR, KSPIN_DESCRIPTOR__bindgen_ty_1,
    PCAUTOMATION_TABLE, PCCONNECTION_DESCRIPTOR, PCEVENT_ITEM, PCFILTER_DESCRIPTOR, PCFILTER_NODE,
    PCMETHOD_ITEM, PCNODE_DESCRIPTOR, PCPIN_DESCRIPTOR, PCPROPERTY_ITEM, PKSDATARANGE, ULONG,
};

use crate::topo::{TopoCapture, TopoRender};

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

/// Nombre de canaux d'un câble, et donc du nœud de volume (`AudioNodes::channels`).
///
/// C'est la valeur unique de M1a, celle des plages de formats ci-dessous ; `topo` vérifie
/// en `const` qu'elle tient dans `cable::MAX_CHANNELS`.
pub const CHANNELS: ULONG = 2;

// ---------------------------------------------------------------------------------
// Formats du spike (§5.4), tirés de `conduit_kmd_core::M1A_FORMATS`.
// ---------------------------------------------------------------------------------

/// Fréquence d'échantillonnage unique de M1a (Hz).
const SAMPLE_RATE: ULONG = 48_000;
/// Taille du conteneur des échantillons flottants (bits).
const BITS_F32: ULONG = 32;
/// Taille du conteneur des échantillons PCM entiers (bits).
const BITS_I16: ULONG = 16;

const _: () = {
    assert!(M1A_FORMATS.len() == 2, "deux plages système : F32 et I16");
    let fmt_f32 = M1A_FORMATS[0];
    let fmt_i16 = M1A_FORMATS[1];
    assert!(fmt_f32.sample_rate == SAMPLE_RATE && fmt_i16.sample_rate == SAMPLE_RATE);
    assert!(fmt_f32.channels as ULONG == CHANNELS && fmt_i16.channels as ULONG == CHANNELS);
    assert!(matches!(fmt_f32.format, SampleFormat::F32));
    assert!(matches!(fmt_i16.format, SampleFormat::I16));
    // 4 octets = 32 bits, 2 octets = 16 bits.
    assert!(SampleFormat::F32.bytes_per_sample() == 4 && BITS_F32 == 32);
    assert!(SampleFormat::I16.bytes_per_sample() == 2 && BITS_I16 == 16);
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

/// En-tête `KSDATAFORMAT` d'une plage audio : `FormatSize = size`, aucune option, type
/// majeur `KSDATAFORMAT_TYPE_AUDIO`.
const fn data_format(size: ULONG, subtype: GUID, specifier: GUID) -> KSDATAFORMAT {
    KSDATAFORMAT {
        __bindgen_anon_1: KSDATAFORMAT__bindgen_ty_1 {
            FormatSize: size,
            Flags: 0,
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
/// exactement, 2 canaux, 48 000 Hz exactement (§4.1).
pub const fn audio_range(subtype: GUID, bits: ULONG) -> KSDATARANGE_AUDIO {
    KSDATARANGE_AUDIO {
        DataRange: data_format(
            KSDATARANGE_AUDIO_SIZE,
            subtype,
            KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
        ),
        MaximumChannels: CHANNELS,
        MinimumBitsPerSample: bits,
        MaximumBitsPerSample: bits,
        MinimumSampleFrequency: SAMPLE_RATE,
        MaximumSampleFrequency: SAMPLE_RATE,
    }
}

/// Plage « analogique » des broches bridge : `KSDATARANGE` simple, sous-type
/// `KSDATAFORMAT_SUBTYPE_ANALOG`, spécificateur `KSDATAFORMAT_SPECIFIER_NONE`.
pub const fn analog_range() -> KSDATARANGE {
    data_format(
        KSDATARANGE_SIZE,
        KSDATAFORMAT_SUBTYPE_ANALOG,
        KSDATAFORMAT_SPECIFIER_NONE,
    )
}

/// Pointeur `PKSDATARANGE` sur une plage logée dans une `static` (`KSDATARANGE_AUDIO`
/// commence par sa `KSDATARANGE`). Le `cast_mut` satisfait le prototype ; PortCls ne
/// modifie pas les plages.
const fn range_ptr<T>(range: &'static Shared<T>) -> PKSDATARANGE {
    ptr::from_ref(range).cast::<KSDATARANGE>().cast_mut()
}

/// Broche de filtre : `flow`/`comm` (§4.1), catégorie `category`, nom `name` (§4.2 :
/// GUID que KS résout en chaîne dans `HKR\MediaCategories` pour répondre à
/// `KSPROPERTY_PIN_NAME` — `None` laisse KS retomber sur la catégorie), plages `ranges`
/// (tableau de pointeurs logé dans une `static`), `instances` instances possibles
/// (globales et par filtre : 1 pour une broche système, 0 pour une broche bridge, qui ne
/// s'instancie pas). Ni interface ni médium déclarés (défauts KS), pas de table
/// d'automatisation propre.
pub const fn pin<const N: usize>(
    flow: KSPIN_DATAFLOW::Type,
    comm: KSPIN_COMMUNICATION::Type,
    category: &'static GUID,
    name: Option<&'static GUID>,
    ranges: &'static Shared<[PKSDATARANGE; N]>,
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
        &BRIDGE_RANGES,
        0,
    )
}

/// Broche système d'un filtre WaveRT : `KSPIN_COMMUNICATION_SINK`, catégorie
/// `KSCATEGORY_AUDIO`, plages système, une instance.
const fn system_pin(flow: KSPIN_DATAFLOW::Type) -> PCPIN_DESCRIPTOR {
    pin(
        flow,
        KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_SINK,
        &CATEGORY_AUDIO,
        None,
        &SYSTEM_RANGES,
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
        &BRIDGE_RANGES,
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

/// **Forme wave** : filtre WaveRT sans nœud, table d'automatisation de filtre vide.
///
/// Le signal traverse le filtre WaveRT sans traitement ; les nœuds audio sont sur le filtre
/// de topologie du même sens ([`topo_filter`]).
const fn wave_filter<const P: usize, const C: usize>(
    pins: &'static [PCPIN_DESCRIPTOR; P],
    connections: &'static Shared<[PCCONNECTION_DESCRIPTOR; C]>,
) -> PCFILTER_DESCRIPTOR {
    filter::<P, 0, C>(&EMPTY_AUTOMATION, pins, &NO_NODES, connections)
}

/// **Forme topologie** : filtre de topologie avec ses nœuds.
///
/// `automation` est la table du **filtre** : elle ne porte que
/// `KSPROPERTY_JACK_DESCRIPTION` (voir l'en-tête de module), et se dédouble par sens
/// comme celles des nœuds. Les propriétés audio, elles, restent sur les nœuds
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

/// Table d'automatisation à une propriété : celle des nœuds audio, et celle des filtres de
/// topologie depuis M1b-03. Aucune méthode, aucun événement.
///
/// Les `*ItemSize` sont renseignés même à compte nul, comme dans [`EMPTY_AUTOMATION`] :
/// PortCls s'en sert pour avancer dans les tableaux, et une taille nulle avec un compte nul
/// est un piège inutile à laisser.
///
/// `EventCount = 0` est ce que **M1b-04 devra changer** : dès que l'état de connexion d'un
/// câble deviendra modifiable, il faudra y déclarer un `PCEVENT_ITEM`
/// `KSEVENT_PINCAPS_JACKINFOCHANGE` sur les filtres de topologie, faute de quoi Windows
/// n'ira jamais relire le jack et l'interface restera figée sur l'état du démarrage (voir
/// `portcls::jack`).
const fn one_property_automation(
    properties: &'static Shared<[PCPROPERTY_ITEM; 1]>,
) -> PCAUTOMATION_TABLE {
    PCAUTOMATION_TABLE {
        PropertyItemSize: size_of::<PCPROPERTY_ITEM>() as ULONG,
        PropertyCount: 1,
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

/// Plage système flottante 32 bits.
static RANGE_F32: Shared<KSDATARANGE_AUDIO> =
    Shared(audio_range(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, BITS_F32));
/// Plage système PCM 16 bits.
static RANGE_I16: Shared<KSDATARANGE_AUDIO> =
    Shared(audio_range(KSDATAFORMAT_SUBTYPE_PCM, BITS_I16));
/// Plage analogique des broches bridge et endpoint.
static RANGE_ANALOG: Shared<KSDATARANGE> = Shared(analog_range());

/// Plages des broches système (`DataRanges` : tableau de pointeurs).
static SYSTEM_RANGES: Shared<[PKSDATARANGE; 2]> =
    Shared([range_ptr(&RANGE_F32), range_ptr(&RANGE_I16)]);
/// Plage des broches bridge et endpoint.
static BRIDGE_RANGES: Shared<[PKSDATARANGE; 1]> = Shared([range_ptr(&RANGE_ANALOG)]);

/// Table d'automatisation vide : tailles d'élément renseignées, aucun élément.
///
/// C'est celle des **quatre** filtres : les filtres WaveRT n'ont aucune propriété propre,
/// et sur les filtres topologie les propriétés sont portées par les nœuds. `const` séparée
/// de la `static` pour que les assertions ci-dessous puissent la lire (l'évaluation `const`
/// ne lit pas les `static`).
const EMPTY_AUTOMATION_TABLE: PCAUTOMATION_TABLE = PCAUTOMATION_TABLE {
    PropertyItemSize: size_of::<PCPROPERTY_ITEM>() as ULONG,
    PropertyCount: 0,
    Properties: ptr::null(),
    MethodItemSize: size_of::<PCMETHOD_ITEM>() as ULONG,
    MethodCount: 0,
    Methods: ptr::null(),
    EventItemSize: size_of::<PCEVENT_ITEM>() as ULONG,
    EventCount: 0,
    Events: ptr::null(),
    Reserved: 0,
};
static EMPTY_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(EMPTY_AUTOMATION_TABLE);

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

/// `KSPROPERTY_JACK_DESCRIPTION` du **filtre** `TopoRender` (M1b-03).
const RENDER_JACK_ITEMS: [PCPROPERTY_ITEM; 1] =
    [jack_description_item::<IMiniportTopologyVtbl, TopoRender>()];
/// `KSPROPERTY_JACK_DESCRIPTION` du **filtre** `TopoCapture`.
const CAPTURE_JACK_ITEMS: [PCPROPERTY_ITEM; 1] =
    [jack_description_item::<IMiniportTopologyVtbl, TopoCapture>()];

static RENDER_JACK_PROPERTIES: Shared<[PCPROPERTY_ITEM; 1]> = Shared(RENDER_JACK_ITEMS);
static CAPTURE_JACK_PROPERTIES: Shared<[PCPROPERTY_ITEM; 1]> = Shared(CAPTURE_JACK_ITEMS);

/// Table d'automatisation du filtre `TopoRender<n>` : le jack, et rien d'autre.
static TOPO_RENDER_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(one_property_automation(&RENDER_JACK_PROPERTIES));
/// Table d'automatisation du filtre `TopoCapture<n>`.
static TOPO_CAPTURE_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(one_property_automation(&CAPTURE_JACK_PROPERTIES));

static RENDER_VOLUME_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(one_property_automation(&RENDER_VOLUME_PROPERTIES));
static RENDER_MUTE_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(one_property_automation(&RENDER_MUTE_PROPERTIES));
static CAPTURE_VOLUME_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(one_property_automation(&CAPTURE_VOLUME_PROPERTIES));
static CAPTURE_MUTE_AUTOMATION: Shared<PCAUTOMATION_TABLE> =
    Shared(one_property_automation(&CAPTURE_MUTE_PROPERTIES));

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

/// Broches de `WaveRender` : système (entrée) puis bridge (sortie).
const WAVE_RENDER_PINS: [PCPIN_DESCRIPTOR; PIN_COUNT] = [
    system_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
    bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
];
/// Broches de `WaveCapture` : bridge (entrée) puis système (sortie).
const WAVE_CAPTURE_PINS: [PCPIN_DESCRIPTOR; PIN_COUNT] = [
    bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
    system_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
];

static WAVE_RENDER_PINS_TABLE: Shared<[PCPIN_DESCRIPTOR; PIN_COUNT]> = Shared(WAVE_RENDER_PINS);
static WAVE_CAPTURE_PINS_TABLE: Shared<[PCPIN_DESCRIPTOR; PIN_COUNT]> = Shared(WAVE_CAPTURE_PINS);

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

/// La rangée du câble `cable` dans une table de broches logée dans une `static` : c'est
/// son adresse que `PCFILTER_DESCRIPTOR::Pins` conserve.
#[allow(clippy::indexing_slicing)] // évalué à la compilation, `cable < CABLE_COUNT`
const fn pins_row(
    table: &'static Shared<[[PCPIN_DESCRIPTOR; PIN_COUNT]; CABLE_COUNT]>,
    cable: usize,
) -> &'static [PCPIN_DESCRIPTOR; PIN_COUNT] {
    &table.get()[cable]
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

/// Descripteur du filtre `WaveRender<n>` (rendu par `wave::WaveRender::description`).
///
/// **Un seul pour les seize câbles** : rien n'y est par câble (le nom de broche est sur
/// les filtres de topologie).
pub static WAVE_RENDER_FILTER: Shared<PCFILTER_DESCRIPTOR> = Shared(wave_filter(
    WAVE_RENDER_PINS_TABLE.get(),
    &DIRECT_CONNECTION_TABLE,
));
/// Descripteur du filtre `WaveCapture<n>` (rendu par `wave::WaveCapture::description`),
/// unique lui aussi.
pub static WAVE_CAPTURE_FILTER: Shared<PCFILTER_DESCRIPTOR> = Shared(wave_filter(
    WAVE_CAPTURE_PINS_TABLE.get(),
    &DIRECT_CONNECTION_TABLE,
));

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
    use KSPIN_COMMUNICATION::{KSPIN_COMMUNICATION_NONE as NONE, KSPIN_COMMUNICATION_SINK as SINK};
    use KSPIN_DATAFLOW::{KSPIN_DATAFLOW_IN as IN, KSPIN_DATAFLOW_OUT as OUT};

    // Deux broches par filtre, numérotées par la direction des données ; un jeu de
    // broches de topologie par câble.
    assert!(WAVE_RENDER_PINS.len() == PIN_COUNT && WAVE_CAPTURE_PINS.len() == PIN_COUNT);
    assert!(TOPO_RENDER_PINS.len() == CABLE_COUNT && TOPO_CAPTURE_PINS.len() == CABLE_COUNT);
    assert!(WAVE_RENDER_PIN_SYSTEM == 0 && WAVE_RENDER_PIN_BRIDGE == 1);
    assert!(TOPO_RENDER_PIN_BRIDGE == 0 && TOPO_RENDER_PIN_ENDPOINT == 1);
    assert!(WAVE_CAPTURE_PIN_BRIDGE == 0 && WAVE_CAPTURE_PIN_SYSTEM == 1);
    assert!(TOPO_CAPTURE_PIN_ENDPOINT == 0 && TOPO_CAPTURE_PIN_BRIDGE == 1);

    // Orientation : broches système `SINK` à 1 instance, bridges et endpoints `NONE` à 0.
    assert!(pin_is(&WAVE_RENDER_PINS[0], IN, SINK, 1));
    assert!(pin_is(&WAVE_RENDER_PINS[1], OUT, NONE, 0));
    assert!(pin_is(&WAVE_CAPTURE_PINS[0], IN, NONE, 0));
    assert!(pin_is(&WAVE_CAPTURE_PINS[1], OUT, SINK, 1));
    // Les seize jeux de broches de topologie, orientation et nommage compris.
    assert!(topo_pins_are_well_formed(&TOPO_RENDER_PINS, true));
    assert!(topo_pins_are_well_formed(&TOPO_CAPTURE_PINS, false));

    // Plages : deux plages système, une plage analogique.
    assert!(WAVE_RENDER_PINS[0].KsPinDescriptor.DataRangesCount == 2);
    assert!(WAVE_RENDER_PINS[1].KsPinDescriptor.DataRangesCount == 1);
    assert!(WAVE_CAPTURE_PINS[1].KsPinDescriptor.DataRangesCount == 2);

    // Plages système : 2 canaux, 48 kHz, 32 bits flottants et 16 bits PCM, `FormatSize`
    // exact.
    let range_f32 = audio_range(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, BITS_F32);
    let range_i16 = audio_range(KSDATAFORMAT_SUBTYPE_PCM, BITS_I16);
    assert!(range_f32.MaximumChannels == 2 && range_i16.MaximumChannels == 2);
    assert!(range_f32.MinimumSampleFrequency == 48_000);
    assert!(range_f32.MaximumSampleFrequency == 48_000);
    assert!(range_i16.MinimumSampleFrequency == 48_000);
    assert!(range_i16.MaximumSampleFrequency == 48_000);
    assert!(range_f32.MinimumBitsPerSample == 32 && range_f32.MaximumBitsPerSample == 32);
    assert!(range_i16.MinimumBitsPerSample == 16 && range_i16.MaximumBitsPerSample == 16);
    // SAFETY: lecture du membre nommé de l'union, celui que `data_format` a écrit.
    let header = unsafe { range_f32.DataRange.__bindgen_anon_1 };
    assert!(header.FormatSize == 88 && header.Flags == 0);
    // SAFETY: idem.
    let analog = unsafe { analog_range().__bindgen_anon_1 };
    assert!(analog.FormatSize == 64);

    // Nom de broche (§4.2) : seules les deux broches endpoint des filtres topologie en
    // portent un (vérifié rangée par rangée ci-dessus) ; comparer les adresses ici est
    // hors de portée de l'évaluation `const`, mais le **numéro** porté par chaque GUID
    // l'est, et c'est lui qui distingue les seize câbles.
    assert!(PIN_NAME_GUIDS.len() == CABLE_COUNT);
    assert!(pin_names_are_numbered(&PIN_NAME_GUIDS, 0));
    assert!(WAVE_RENDER_PINS[0].KsPinDescriptor.Name.is_null());
    assert!(WAVE_RENDER_PINS[1].KsPinDescriptor.Name.is_null());
    assert!(WAVE_CAPTURE_PINS[0].KsPinDescriptor.Name.is_null());
    assert!(WAVE_CAPTURE_PINS[1].KsPinDescriptor.Name.is_null());

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

const _: () = {
    assert!(wave_filter_is_well_formed(&wave_filter::<
        PIN_COUNT,
        WAVE_CONNECTION_COUNT,
    >(
        WAVE_RENDER_PINS_TABLE.get(),
        &DIRECT_CONNECTION_TABLE
    )));
    assert!(wave_filter_is_well_formed(&wave_filter::<
        PIN_COUNT,
        WAVE_CONNECTION_COUNT,
    >(
        WAVE_CAPTURE_PINS_TABLE.get(),
        &DIRECT_CONNECTION_TABLE
    )));
    // Les seize filtres de chaque sens, et pas seulement le premier.
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

/// Une `PCAUTOMATION_TABLE` : tailles d'élément exactes, `PropertyCount` égal à la
/// longueur du tableau qu'elle pointe, `Properties` nul si et seulement si le compte l'est,
/// et aucune méthode ni événement (le pilote n'en expose pas).
const fn automation_is_well_formed(table: &PCAUTOMATION_TABLE, properties: usize) -> bool {
    table.PropertyItemSize as usize == size_of::<PCPROPERTY_ITEM>()
        && table.PropertyCount as usize == properties
        && table.Properties.is_null() == (properties == 0)
        && table.MethodItemSize as usize == size_of::<PCMETHOD_ITEM>()
        && table.MethodCount == 0
        && table.Methods.is_null()
        && table.EventItemSize as usize == size_of::<PCEVENT_ITEM>()
        && table.EventCount == 0
        && table.Events.is_null()
        && table.Reserved == 0
}

const _: () = {
    // La table des filtres **wave**, vide : c'est PortCls qui répond à `KSPROPSETID_Pin` et
    // `KSPROPSETID_Topology`.
    assert!(automation_is_well_formed(&EMPTY_AUTOMATION_TABLE, 0));

    // Les deux tables de filtre **topologie** : le jack, et rien d'autre.
    assert!(automation_is_well_formed(
        &one_property_automation(&RENDER_JACK_PROPERTIES),
        1
    ));
    assert!(automation_is_well_formed(
        &one_property_automation(&CAPTURE_JACK_PROPERTIES),
        1
    ));
    assert!(property_items_are_well_formed(&RENDER_JACK_ITEMS));
    assert!(property_items_are_well_formed(&CAPTURE_JACK_ITEMS));

    // `GET | BASICSUPPORT`, et surtout **pas** `SET` : `KSPROPERTY_JACK_DESCRIPTION` est en
    // lecture seule (« Get: Yes, Set: No »). Un `SET` déclaré par erreur ferait croire au
    // client qu'il peut brancher le câble par cette propriété-là, alors que c'est M1b-04 et
    // une propriété privée qui s'en chargeront.
    assert!(RENDER_JACK_ITEMS[0].Flags == JACK_ACCESS_FLAGS);
    assert!(CAPTURE_JACK_ITEMS[0].Flags == JACK_ACCESS_FLAGS);
    assert!(JACK_ACCESS_FLAGS & portcls_sys::KSPROPERTY_TYPE_SET == 0);

    // Les quatre tables de nœud : une propriété chacune, réellement pointée.
    assert!(automation_is_well_formed(
        &one_property_automation(&RENDER_VOLUME_PROPERTIES),
        1
    ));
    assert!(automation_is_well_formed(
        &one_property_automation(&RENDER_MUTE_PROPERTIES),
        1
    ));
    assert!(automation_is_well_formed(
        &one_property_automation(&CAPTURE_VOLUME_PROPERTIES),
        1
    ));
    assert!(automation_is_well_formed(
        &one_property_automation(&CAPTURE_MUTE_PROPERTIES),
        1
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
