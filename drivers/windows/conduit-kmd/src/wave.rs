//! Miniports WaveRT d'un câble (driver-design.md §4, §5) : [`WaveRender`] et
//! [`WaveCapture`], un par sens, implémentent `portcls::MiniportWaveRT`.
//!
//! Ils exposent leur descripteur de filtre (`descriptors::WAVE_RENDER_FILTER`,
//! `WAVE_CAPTURE_FILTER`, M1a-06), ce qui publie les endpoints, et `NewStream` vérifie
//! la broche et le sens, traduit le `KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]` reçu en
//! `conduit_kmd_core::RequestedFormat` ([`requested_format`]) et le valide contre
//! `M1A_FORMATS`. Les deux créent ensuite le même flux ([`crate::stream::WaveStream`] :
//! tampon cyclique, horloge, position, notifications) par [`open_stream`], qui l'inscrit
//! dans l'emplacement du [`Cable`] correspondant à son sens (un seul flux par sens :
//! `STATUS_DEVICE_BUSY` sinon) et le rend à PortCls par `StreamObject`. C'est le tick du
//! câble qui, dès lors, copie le rendu vers la capture (M1a-08, §5.3).
//!
//! Depuis l'étape 1.4, l'objet rendu est un **flux composite** (`portcls::packet`) : il
//! répond aussi à l'interface du mode paquets qui correspond à son sens
//! (`IMiniportWaveRTOutputStream` pour le rendu, `IMiniportWaveRTInputStream` pour la
//! capture).
//! `DataRangeIntersection` et `GetDeviceDescription` restent aux défauts de `portcls`.
//!
//! # Lecture du format
//!
//! PortCls remet une `KSDATAFORMAT` suivie de `FormatSize` octets. Rien n'est lu au-delà
//! de ce que `FormatSize` couvre : l'en-tête d'abord, puis, si la taille le permet, la
//! `KSDATAFORMAT_WAVEFORMATEX` (82 octets), puis, si `wFormatTag` l'annonce et que la
//! taille le permet, la `KSDATAFORMAT_WAVEFORMATEXTENSIBLE` (104 octets). Les structures
//! sont `packed` : lues par `read_unaligned` depuis un pointeur typé, jamais par
//! indexation d'octets.

use core::mem::size_of;

use conduit_kmd_core::{M1A_FORMATS, RequestedFormat, SampleKind, SupportedFormat, validate};
use portcls::conduit_com::{
    ComRef, NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
};
use portcls::{
    MiniportWaveRT, PacketInterfaces, PortWaveRT, PortWaveRTStream, ResourceList, StreamObject,
    try_new_packet_stream_object,
};
use portcls_sys::{
    GUID, IUnknown, KSDATAFORMAT, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, KSDATAFORMAT_SUBTYPE_PCM, KSDATAFORMAT_TYPE_AUDIO,
    KSDATAFORMAT_WAVEFORMATEX, KSDATAFORMAT_WAVEFORMATEXTENSIBLE, PCFILTER_DESCRIPTOR,
    WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM,
};

use crate::cable::{Cable, Direction};
use crate::descriptors::{
    WAVE_CAPTURE_FILTER, WAVE_CAPTURE_PIN_SYSTEM, WAVE_RENDER_FILTER, WAVE_RENDER_PIN_SYSTEM,
};
use crate::stream::WaveStream;

/// Taille minimale de `cbSize` pour qu'un `WAVEFORMATEX` étendu contienne les champs
/// de `WAVEFORMATEXTENSIBLE` (`Samples`, `dwChannelMask`, `SubFormat` : 22 octets).
const WAVEFORMATEXTENSIBLE_CBSIZE: u16 = 22;

/// Égalité de deux `GUID` (le type généré n'implémente pas `PartialEq`).
fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.Data1 == b.Data1 && a.Data2 == b.Data2 && a.Data3 == b.Data3 && a.Data4 == b.Data4
}

/// Famille d'échantillons d'après le sous-format d'un `WAVEFORMATEXTENSIBLE`.
fn kind_of_subformat(subformat: &GUID) -> SampleKind {
    if guid_eq(subformat, &KSDATAFORMAT_SUBTYPE_PCM) {
        SampleKind::Pcm
    } else if guid_eq(subformat, &KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) {
        SampleKind::Float
    } else {
        SampleKind::Other
    }
}

/// Famille d'échantillons d'après le `wFormatTag` d'un `WAVEFORMATEX` simple.
fn kind_of_tag(tag: u16) -> SampleKind {
    match tag {
        WAVE_FORMAT_PCM => SampleKind::Pcm,
        WAVE_FORMAT_IEEE_FLOAT => SampleKind::Float,
        _ => SampleKind::Other,
    }
}

/// Traduit le format reçu par `NewStream` en [`RequestedFormat`] ; `None` si ce n'est
/// pas un `KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]` audio complet.
///
/// Acceptés : type majeur `KSDATAFORMAT_TYPE_AUDIO`, spécificateur
/// `KSDATAFORMAT_SPECIFIER_WAVEFORMATEX`, `FormatSize` couvrant la structure lue.
/// Un `WAVE_FORMAT_EXTENSIBLE` exige en plus `cbSize >= 22` et `FormatSize >=
/// sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE)` ; ses bits valides viennent de
/// `Samples.wValidBitsPerSample`, ceux d'un format simple valent `wBitsPerSample`.
///
/// # Safety
///
/// `format` est suivi en mémoire d'au moins `FormatSize` octets lisibles (contrat de
/// `IMiniportWaveRT::NewStream`, relayé par le thunk de `portcls`).
unsafe fn requested_format(format: &KSDATAFORMAT) -> Option<RequestedFormat> {
    // SAFETY: lecture du membre nommé de l'union `KSDATAFORMAT` : PortCls remplit toujours
    // la structure (l'autre membre, `Alignment`, ne sert qu'à l'alignement).
    let header = unsafe { format.__bindgen_anon_1 };
    let size = usize::try_from(header.FormatSize).ok()?;
    if !guid_eq(&header.MajorFormat, &KSDATAFORMAT_TYPE_AUDIO)
        || !guid_eq(&header.Specifier, &KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
        || size < size_of::<KSDATAFORMAT_WAVEFORMATEX>()
    {
        return None;
    }
    let base: *const KSDATAFORMAT = format;
    // SAFETY: `FormatSize >= sizeof(KSDATAFORMAT_WAVEFORMATEX)` (vérifié) et le contrat de
    // la fonction garantit `FormatSize` octets lisibles à partir de `format` ; la
    // structure est `packed`, d'où la lecture non alignée.
    let wave = unsafe { base.cast::<KSDATAFORMAT_WAVEFORMATEX>().read_unaligned() }.WaveFormatEx;
    let tag = wave.wFormatTag;
    let (kind, valid_bits) = if tag == WAVE_FORMAT_EXTENSIBLE {
        if wave.cbSize < WAVEFORMATEXTENSIBLE_CBSIZE
            || size < size_of::<KSDATAFORMAT_WAVEFORMATEXTENSIBLE>()
        {
            return None;
        }
        // SAFETY: `FormatSize >= sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE)` (vérifié) ;
        // même contrat et même lecture non alignée que ci-dessus.
        let ext = unsafe {
            base.cast::<KSDATAFORMAT_WAVEFORMATEXTENSIBLE>()
                .read_unaligned()
        }
        .WaveFormatExt;
        let samples = ext.Samples;
        let subformat = ext.SubFormat;
        // SAFETY: les trois membres de l'union `Samples` sont des `WORD` occupant les mêmes
        // deux octets, écrits par le moteur audio : la lecture est toujours initialisée.
        let valid = unsafe { samples.wValidBitsPerSample };
        (kind_of_subformat(&subformat), valid)
    } else {
        (kind_of_tag(tag), wave.wBitsPerSample)
    };
    Some(RequestedFormat {
        sample_rate: wave.nSamplesPerSec,
        channels: wave.nChannels,
        bits_per_sample: wave.wBitsPerSample,
        valid_bits,
        kind,
    })
}

/// Vérifications communes de `NewStream` : broche système attendue, sens attendu, format
/// lisible et supporté (§5.4). Renvoie le format retenu.
///
/// # Safety
///
/// Contrat de [`requested_format`] sur `format`.
unsafe fn check_stream_request(
    expected_pin: u32,
    expected_capture: bool,
    pin: u32,
    capture: bool,
    format: &KSDATAFORMAT,
) -> Result<SupportedFormat, NtStatus> {
    if pin != expected_pin || capture != expected_capture {
        return Err(STATUS_INVALID_PARAMETER);
    }
    // SAFETY: contrat relayé tel quel.
    let requested = unsafe { requested_format(format) }.ok_or(STATUS_INVALID_PARAMETER)?;
    validate(&requested, &M1A_FORMATS).map_err(|_| STATUS_INVALID_PARAMETER)
}

/// Crée le flux du sens `direction` sur le câble `cable` (numéro `n`), l'inscrit dans
/// l'emplacement correspondant (`STATUS_DEVICE_BUSY` s'il est pris) et le rend à PortCls.
/// Commun aux deux miniports : rendu et capture ne diffèrent que par `direction` (§5.3).
///
/// IRQL : `PASSIVE_LEVEL`.
fn open_stream(
    n: u32,
    direction: Direction,
    cable: &'static Cable,
    port_stream: PortWaveRTStream,
    pin: u32,
    supported: SupportedFormat,
) -> Result<StreamObject, NtStatus> {
    let name = direction.stream_name();
    // Mode paquets (étape 1.4) : un flux de rendu est une *sortie* du point de vue du
    // moteur, qui y écrit ses paquets (`IMiniportWaveRTOutputStream`) ; un flux de capture
    // est une *entrée*, dont il lit les paquets (`IMiniportWaveRTInputStream`).
    let interfaces = match direction {
        Direction::Render => PacketInterfaces::Output,
        Direction::Capture => PacketInterfaces::Input,
    };
    let stream = WaveStream::new(n, direction, cable, port_stream, supported)?;
    let object = try_new_packet_stream_object(stream, interfaces).ok_or_else(|| {
        kmd_log!("{name}{n}::NewStream : allocation du flux impossible");
        STATUS_INSUFFICIENT_RESOURCES
    })?;
    // SAFETY: `object` possède l'objet COM alloué : le `WaveStream` est à son adresse
    // définitive jusqu'au dernier `Release`, dont le `Drop` se retire de l'emplacement du
    // câble avant toute libération. En cas d'échec, `object` est lâché ici même : son
    // `Drop` ne trouve rien à retirer du câble.
    if let Err(status) = unsafe { object.attach() } {
        kmd_log!(
            "{name}{n}::NewStream broche {pin} refusé : {status:#010x} (flux déjà ouvert dans ce sens)"
        );
        return Err(status);
    }
    kmd_log!(
        "{name}{n}::NewStream broche {pin} format {supported:?} : flux {:p} ({interfaces:?})",
        object.as_raw()
    );
    Ok(StreamObject::from(object))
}

/// Miniport WaveRT du filtre `WaveRender<n>` : le lecteur écrit sur sa broche système.
#[derive(Debug)]
pub struct WaveRender {
    /// Numéro du câble.
    pub n: u32,
    /// État partagé du câble : le flux rendu courant y est inscrit par `NewStream`.
    pub cable: &'static Cable,
}

impl MiniportWaveRT for WaveRender {
    // IRQL: PASSIVE_LEVEL
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        _port: PortWaveRT,
    ) -> NtStatus {
        kmd_log!("WaveRender{}::Init (câble {})", self.n, self.cable.index);
        STATUS_SUCCESS
    }

    // IRQL: PASSIVE_LEVEL
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        WAVE_RENDER_FILTER.get()
    }

    // IRQL: PASSIVE_LEVEL
    fn new_stream(
        &self,
        port_stream: PortWaveRTStream,
        pin: u32,
        capture: bool,
        format: &KSDATAFORMAT,
    ) -> Result<StreamObject, NtStatus> {
        // SAFETY: `format` vient du thunk `NewStream` de `portcls`, qui garantit une
        // `KSDATAFORMAT` suivie de son extension `FormatSize`.
        let supported =
            unsafe { check_stream_request(WAVE_RENDER_PIN_SYSTEM, false, pin, capture, format) }?;
        open_stream(
            self.n,
            Direction::Render,
            self.cable,
            port_stream,
            pin,
            supported,
        )
    }
}

/// Miniport WaveRT du filtre `WaveCapture<n>` : l'enregistreur lit sur sa broche système.
#[derive(Debug)]
pub struct WaveCapture {
    /// Numéro du câble.
    pub n: u32,
    /// État partagé du câble (le flux capture courant y sera inscrit en M1a-08).
    pub cable: &'static Cable,
}

impl MiniportWaveRT for WaveCapture {
    // IRQL: PASSIVE_LEVEL
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        _port: PortWaveRT,
    ) -> NtStatus {
        kmd_log!("WaveCapture{}::Init (câble {})", self.n, self.cable.index);
        STATUS_SUCCESS
    }

    // IRQL: PASSIVE_LEVEL
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        WAVE_CAPTURE_FILTER.get()
    }

    // IRQL: PASSIVE_LEVEL
    fn new_stream(
        &self,
        port_stream: PortWaveRTStream,
        pin: u32,
        capture: bool,
        format: &KSDATAFORMAT,
    ) -> Result<StreamObject, NtStatus> {
        // SAFETY: voir `WaveRender::new_stream`.
        let supported =
            unsafe { check_stream_request(WAVE_CAPTURE_PIN_SYSTEM, true, pin, capture, format) }?;
        open_stream(
            self.n,
            Direction::Capture,
            self.cable,
            port_stream,
            pin,
            supported,
        )
    }
}
