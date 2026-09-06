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
//! Minimum accepté par le générateur d'endpoints (§4.1) : deux broches par filtre, une
//! connexion directe broche 0 → broche 1 (`PCFILTER_NODE`), aucun nœud, table
//! d'automatisation vide (PortCls gère `KSPROPSETID_Pin` et `KSPROPSETID_Topology`),
//! catégories laissées à PortCls (`CategoryCount = 0` : `KSCATEGORY_AUDIO`,
//! `KSCATEGORY_RENDER`/`CAPTURE`, `KSCATEGORY_REALTIME` pour les ports WaveRT,
//! `KSCATEGORY_AUDIO`, `KSCATEGORY_TOPOLOGY` pour les ports topologie, comme SYSVAD).
//! Volume, mute et jack viendront en M1b-03.
//!
//! | Filtre | Broche | Flux | Communication | Rôle |
//! |---|---|---|---|---|
//! | `WaveRender` | [`WAVE_RENDER_PIN_SYSTEM`] = 0 | `IN` | `SINK` | le lecteur écrit ici (plages système) |
//! | | [`WAVE_RENDER_PIN_BRIDGE`] = 1 | `OUT` | `NONE` | bridge analogique vers `TopoRender` |
//! | `TopoRender` | [`TOPO_RENDER_PIN_BRIDGE`] = 0 | `IN` | `NONE` | bridge depuis `WaveRender` |
//! | | [`TOPO_RENDER_PIN_ENDPOINT`] = 1 | `OUT` | `NONE` | `KSNODETYPE_SPEAKER` : l'endpoint |
//! | `WaveCapture` | [`WAVE_CAPTURE_PIN_BRIDGE`] = 0 | `IN` | `NONE` | bridge depuis `TopoCapture` |
//! | | [`WAVE_CAPTURE_PIN_SYSTEM`] = 1 | `OUT` | `SINK` | l'enregistreur lit ici (plages système) |
//! | `TopoCapture` | [`TOPO_CAPTURE_PIN_ENDPOINT`] = 0 | `IN` | `NONE` | `KSNODETYPE_LINE_CONNECTOR` : l'endpoint |
//! | | [`TOPO_CAPTURE_PIN_BRIDGE`] = 1 | `OUT` | `NONE` | bridge vers `WaveCapture` |
//!
//! Les deux broches endpoint sont les seules à porter un `KsPinDescriptor.Name`
//! (`portcls::pin_name_guid(0)`, M1a-09) : c'est ce GUID que KS résout en « Conduit 1 »
//! par la clé `HKR\MediaCategories` que l'INF écrit, et donc le nom que l'utilisateur
//! voit (driver-design.md §4.2). Toutes les autres broches laissent `Name` nul.
//!
//! Les numéros de broche suivent la direction des données, comme dans §4.1 et dans
//! SYSVAD : la broche 0 est toujours l'entrée (`KSPIN_DATAFLOW_IN`), la broche 1 la
//! sortie ; c'est pourquoi les constantes sont nommées par filtre et non par rôle.
//!
//! Ce crate ne se teste pas en mode utilisateur : les invariants (nombre de broches,
//! orientation, tailles des structures, cohérence avec `conduit_kmd_core::M1A_FORMATS`)
//! sont des assertions `const`, vérifiées à la compilation.

use core::fmt;
use core::mem::size_of;
use core::ptr;

use conduit_kmd_core::{M1A_FORMATS, SampleFormat};
use portcls_sys::{
    GUID, KSCATEGORY_AUDIO, KSDATAFORMAT, KSDATAFORMAT__bindgen_ty_1, KSDATAFORMAT_SPECIFIER_NONE,
    KSDATAFORMAT_SPECIFIER_WAVEFORMATEX, KSDATAFORMAT_SUBTYPE_ANALOG,
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, KSDATAFORMAT_SUBTYPE_PCM, KSDATAFORMAT_TYPE_AUDIO,
    KSDATARANGE, KSDATARANGE_AUDIO, KSNODETYPE_LINE_CONNECTOR, KSNODETYPE_SPEAKER,
    KSPIN_COMMUNICATION, KSPIN_DATAFLOW, KSPIN_DESCRIPTOR, KSPIN_DESCRIPTOR__bindgen_ty_1,
    PCAUTOMATION_TABLE, PCCONNECTION_DESCRIPTOR, PCEVENT_ITEM, PCFILTER_DESCRIPTOR, PCFILTER_NODE,
    PCMETHOD_ITEM, PCNODE_DESCRIPTOR, PCPIN_DESCRIPTOR, PCPROPERTY_ITEM, PKSDATARANGE, ULONG,
};

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

// ---------------------------------------------------------------------------------
// Formats du spike (§5.4), tirés de `conduit_kmd_core::M1A_FORMATS`.
// ---------------------------------------------------------------------------------

/// Fréquence d'échantillonnage unique de M1a (Hz).
const SAMPLE_RATE: ULONG = 48_000;
/// Nombre de canaux unique de M1a.
const CHANNELS: ULONG = 2;
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
/// C'est **la seule broche nommée** : son GUID `Name` ([`PIN_NAME_CABLE_0`]) est ce que
/// KS résout en « Conduit 1 » (§4.2). Sans lui, KS retomberait sur la catégorie et
/// l'endpoint s'appellerait « Haut-parleurs » ou « Ligne ».
const fn endpoint_pin(flow: KSPIN_DATAFLOW::Type, category: &'static GUID) -> PCPIN_DESCRIPTOR {
    pin(
        flow,
        KSPIN_COMMUNICATION::KSPIN_COMMUNICATION_NONE,
        category,
        Some(&PIN_NAME_CABLE_0),
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

/// Descripteur de filtre : `pins` et `connections` logés dans des `static`, aucun nœud,
/// table d'automatisation `automation`, catégories laissées à PortCls
/// (`CategoryCount = 0`).
const fn filter<const P: usize, const C: usize>(
    automation: &'static Shared<PCAUTOMATION_TABLE>,
    pins: &'static Shared<[PCPIN_DESCRIPTOR; P]>,
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
        NodeCount: 0,
        Nodes: ptr::null(),
        ConnectionCount: C as ULONG,
        Connections: ptr::from_ref(connections).cast::<PCCONNECTION_DESCRIPTOR>(),
        CategoryCount: 0,
        Categories: ptr::null(),
    }
}

// ---------------------------------------------------------------------------------
// Les tables.
// ---------------------------------------------------------------------------------

/// `KSCATEGORY_AUDIO`, adressable (les broches pointent la catégorie).
static CATEGORY_AUDIO: GUID = KSCATEGORY_AUDIO;
/// `KSNODETYPE_SPEAKER`, catégorie de la broche endpoint rendu.
static CATEGORY_SPEAKER: GUID = KSNODETYPE_SPEAKER;
/// `KSNODETYPE_LINE_CONNECTOR`, catégorie de la broche endpoint capture.
static CATEGORY_LINE_CONNECTOR: GUID = KSNODETYPE_LINE_CONNECTOR;
/// GUID de nom des broches endpoint du câble 0, adressable : `KsPinDescriptor.Name` en
/// prend l'adresse et PortCls la conserve (§4.2). L'INF associe ce même GUID à
/// « Conduit 1 » (`conduit_kmd.inx`, `GUID.PinName.Cable0`).
static PIN_NAME_CABLE_0: GUID = portcls::PIN_NAME_CABLE_0;

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
static EMPTY_AUTOMATION: Shared<PCAUTOMATION_TABLE> = Shared(PCAUTOMATION_TABLE {
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
});

/// Connexion directe broche 0 → broche 1, commune aux quatre filtres (l'entrée est
/// toujours la broche 0, la sortie la broche 1).
static DIRECT_CONNECTION: Shared<[PCCONNECTION_DESCRIPTOR; 1]> =
    Shared([connection(PCFILTER_NODE, 0, PCFILTER_NODE, 1)]);

/// Broches de `WaveRender` : système (entrée) puis bridge (sortie).
const WAVE_RENDER_PINS: [PCPIN_DESCRIPTOR; PIN_COUNT] = [
    system_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
    bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
];
/// Broches de `TopoRender` : bridge (entrée) puis endpoint haut-parleur (sortie).
const TOPO_RENDER_PINS: [PCPIN_DESCRIPTOR; PIN_COUNT] = [
    bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
    endpoint_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT, &CATEGORY_SPEAKER),
];
/// Broches de `WaveCapture` : bridge (entrée) puis système (sortie).
const WAVE_CAPTURE_PINS: [PCPIN_DESCRIPTOR; PIN_COUNT] = [
    bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN),
    system_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
];
/// Broches de `TopoCapture` : endpoint connecteur de ligne (entrée) puis bridge (sortie).
const TOPO_CAPTURE_PINS: [PCPIN_DESCRIPTOR; PIN_COUNT] = [
    endpoint_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_IN, &CATEGORY_LINE_CONNECTOR),
    bridge_pin(KSPIN_DATAFLOW::KSPIN_DATAFLOW_OUT),
];

static WAVE_RENDER_PINS_TABLE: Shared<[PCPIN_DESCRIPTOR; PIN_COUNT]> = Shared(WAVE_RENDER_PINS);
static TOPO_RENDER_PINS_TABLE: Shared<[PCPIN_DESCRIPTOR; PIN_COUNT]> = Shared(TOPO_RENDER_PINS);
static WAVE_CAPTURE_PINS_TABLE: Shared<[PCPIN_DESCRIPTOR; PIN_COUNT]> = Shared(WAVE_CAPTURE_PINS);
static TOPO_CAPTURE_PINS_TABLE: Shared<[PCPIN_DESCRIPTOR; PIN_COUNT]> = Shared(TOPO_CAPTURE_PINS);

/// Descripteur du filtre `WaveRender<n>` (rendu par `wave::WaveRender::description`).
pub static WAVE_RENDER_FILTER: Shared<PCFILTER_DESCRIPTOR> = Shared(filter(
    &EMPTY_AUTOMATION,
    &WAVE_RENDER_PINS_TABLE,
    &DIRECT_CONNECTION,
));
/// Descripteur du filtre `TopoRender<n>` (rendu par `topo::TopoRender::description`).
pub static TOPO_RENDER_FILTER: Shared<PCFILTER_DESCRIPTOR> = Shared(filter(
    &EMPTY_AUTOMATION,
    &TOPO_RENDER_PINS_TABLE,
    &DIRECT_CONNECTION,
));
/// Descripteur du filtre `WaveCapture<n>` (rendu par `wave::WaveCapture::description`).
pub static WAVE_CAPTURE_FILTER: Shared<PCFILTER_DESCRIPTOR> = Shared(filter(
    &EMPTY_AUTOMATION,
    &WAVE_CAPTURE_PINS_TABLE,
    &DIRECT_CONNECTION,
));
/// Descripteur du filtre `TopoCapture<n>` (rendu par `topo::TopoCapture::description`).
pub static TOPO_CAPTURE_FILTER: Shared<PCFILTER_DESCRIPTOR> = Shared(filter(
    &EMPTY_AUTOMATION,
    &TOPO_CAPTURE_PINS_TABLE,
    &DIRECT_CONNECTION,
));

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

const _: () = {
    use KSPIN_COMMUNICATION::{KSPIN_COMMUNICATION_NONE as NONE, KSPIN_COMMUNICATION_SINK as SINK};
    use KSPIN_DATAFLOW::{KSPIN_DATAFLOW_IN as IN, KSPIN_DATAFLOW_OUT as OUT};

    // Deux broches par filtre, numérotées par la direction des données.
    assert!(WAVE_RENDER_PINS.len() == PIN_COUNT && TOPO_RENDER_PINS.len() == PIN_COUNT);
    assert!(WAVE_CAPTURE_PINS.len() == PIN_COUNT && TOPO_CAPTURE_PINS.len() == PIN_COUNT);
    assert!(WAVE_RENDER_PIN_SYSTEM == 0 && WAVE_RENDER_PIN_BRIDGE == 1);
    assert!(TOPO_RENDER_PIN_BRIDGE == 0 && TOPO_RENDER_PIN_ENDPOINT == 1);
    assert!(WAVE_CAPTURE_PIN_BRIDGE == 0 && WAVE_CAPTURE_PIN_SYSTEM == 1);
    assert!(TOPO_CAPTURE_PIN_ENDPOINT == 0 && TOPO_CAPTURE_PIN_BRIDGE == 1);

    // Orientation : broches système `SINK` à 1 instance, bridges et endpoints `NONE` à 0.
    assert!(pin_is(&WAVE_RENDER_PINS[0], IN, SINK, 1));
    assert!(pin_is(&WAVE_RENDER_PINS[1], OUT, NONE, 0));
    assert!(pin_is(&TOPO_RENDER_PINS[0], IN, NONE, 0));
    assert!(pin_is(&TOPO_RENDER_PINS[1], OUT, NONE, 0));
    assert!(pin_is(&WAVE_CAPTURE_PINS[0], IN, NONE, 0));
    assert!(pin_is(&WAVE_CAPTURE_PINS[1], OUT, SINK, 1));
    assert!(pin_is(&TOPO_CAPTURE_PINS[0], IN, NONE, 0));
    assert!(pin_is(&TOPO_CAPTURE_PINS[1], OUT, NONE, 0));

    // Plages : deux plages système, une plage analogique.
    assert!(WAVE_RENDER_PINS[0].KsPinDescriptor.DataRangesCount == 2);
    assert!(WAVE_RENDER_PINS[1].KsPinDescriptor.DataRangesCount == 1);
    assert!(WAVE_CAPTURE_PINS[1].KsPinDescriptor.DataRangesCount == 2);
    assert!(TOPO_RENDER_PINS[1].KsPinDescriptor.DataRangesCount == 1);

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
    // portent un (`endpoint_pin` n'a qu'un GUID à donner, celui du câble 0) ; comparer
    // les adresses ici est hors de portée de l'évaluation `const`, l'absence ou la
    // présence suffit.
    assert!(!TOPO_RENDER_PINS[1].KsPinDescriptor.Name.is_null());
    assert!(!TOPO_CAPTURE_PINS[0].KsPinDescriptor.Name.is_null());
    assert!(TOPO_RENDER_PINS[0].KsPinDescriptor.Name.is_null());
    assert!(TOPO_CAPTURE_PINS[1].KsPinDescriptor.Name.is_null());
    assert!(WAVE_RENDER_PINS[0].KsPinDescriptor.Name.is_null());
    assert!(WAVE_RENDER_PINS[1].KsPinDescriptor.Name.is_null());
    assert!(WAVE_CAPTURE_PINS[0].KsPinDescriptor.Name.is_null());
    assert!(WAVE_CAPTURE_PINS[1].KsPinDescriptor.Name.is_null());

    // Connexion directe 0 → 1 via `PCFILTER_NODE`.
    let direct = connection(PCFILTER_NODE, 0, PCFILTER_NODE, 1);
    assert!(direct.FromNode == ULONG::MAX && direct.ToNode == ULONG::MAX);
    assert!(direct.FromNodePin == 0 && direct.ToNodePin == 1);
};

/// `PinCount == Pins.len()`, `ConnectionCount == 1`, aucun nœud, tailles d'élément
/// exactes, pour chacun des quatre descripteurs.
const fn filter_is_well_formed(filter: &PCFILTER_DESCRIPTOR) -> bool {
    filter.Version == 0
        && filter.PinSize as usize == size_of::<PCPIN_DESCRIPTOR>()
        && filter.PinCount as usize == PIN_COUNT
        && !filter.Pins.is_null()
        && filter.NodeSize as usize == size_of::<PCNODE_DESCRIPTOR>()
        && filter.NodeCount == 0
        && filter.ConnectionCount == 1
        && !filter.Connections.is_null()
        && !filter.AutomationTable.is_null()
        && filter.CategoryCount == 0
}

const _: () = {
    assert!(filter_is_well_formed(&filter::<PIN_COUNT, 1>(
        &EMPTY_AUTOMATION,
        &WAVE_RENDER_PINS_TABLE,
        &DIRECT_CONNECTION,
    )));
    assert!(filter_is_well_formed(&filter::<PIN_COUNT, 1>(
        &EMPTY_AUTOMATION,
        &TOPO_RENDER_PINS_TABLE,
        &DIRECT_CONNECTION,
    )));
    assert!(filter_is_well_formed(&filter::<PIN_COUNT, 1>(
        &EMPTY_AUTOMATION,
        &WAVE_CAPTURE_PINS_TABLE,
        &DIRECT_CONNECTION,
    )));
    assert!(filter_is_well_formed(&filter::<PIN_COUNT, 1>(
        &EMPTY_AUTOMATION,
        &TOPO_CAPTURE_PINS_TABLE,
        &DIRECT_CONNECTION,
    )));
};
