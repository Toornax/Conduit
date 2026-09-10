//! Miniports WaveRT d'un câble (driver-design.md §4, §5) : [`WaveRender`] et
//! [`WaveCapture`], un par sens, implémentent `portcls::MiniportWaveRT`.
//!
//! Ils exposent le descripteur de filtre **du format de leur câble**
//! (`descriptors::wave_render_filter`, `wave_capture_filter`, M1a-06 puis M1b-05), ce qui
//! publie les endpoints, et `NewStream` vérifie la broche et le sens, traduit le
//! `KSDATAFORMAT_WAVEFORMATEX[TENSIBLE]` reçu en `conduit_kmd_core::RequestedFormat`
//! ([`requested_format`]) et le valide contre les formats de **ce** câble
//! (`conduit_kmd_core::cable_formats`). Les deux créent ensuite le même flux
//! ([`crate::stream::WaveStream`] :
//! tampon cyclique, horloge, position, notifications) par [`open_stream`], qui l'inscrit
//! dans l'emplacement du [`Cable`] correspondant à son sens (un seul flux par sens :
//! `STATUS_DEVICE_BUSY` sinon) et le rend à PortCls par `StreamObject`. C'est le tick du
//! câble qui, dès lors, copie le rendu vers la capture (M1a-08, §5.3).
//! `GetDeviceDescription` reste au défaut de `portcls`.
//!
//! Les deux implémentent aussi `portcls::SignalModes` depuis M1b-15 : c'est ce qui fait
//! répondre `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` sur leur filtre, en désignant leur
//! broche de flux (`WAVE_RENDER_PIN_SYSTEM` d'un côté, `WAVE_CAPTURE_PIN_SYSTEM` de
//! l'autre). Le trait ne demande **pas** la liste des modes : Conduit n'en sert qu'un,
//! `AUDIO_SIGNALPROCESSINGMODE_DEFAULT`, et le gestionnaire l'écrit lui-même — seule la
//! géométrie des broches est propre au miniport.
//!
//! # Deux portes, une seule liste de formats
//!
//! Windows passe par les deux, et elles doivent dire la même chose :
//!
//! - `DataRangeIntersection` ([`crate::intersect`], M1b-21) répond « quel format ce câble
//!   peut-il servir ? » — c'est de là que l'endpoint tire le sien ;
//! - `NewStream` répond « acceptes-tu celui-ci ? » au moment d'ouvrir le flux.
//!
//! Les deux confrontent la demande à `conduit_kmd_core::cable_formats` du **même** câble.
//! C'est ce qui garantit que le gestionnaire d'intersection ne propose jamais un format que
//! `NewStream` refuserait ensuite — la panne que la documentation du gestionnaire par
//! défaut décrit, et qui laisse le moteur audio parcourir une liste jusqu'à épuisement.
//!
//! L'objet rendu par `NewStream` est un **flux composite** (`portcls::packet`) : une seule
//! allocation, un seul compteur de références, plusieurs têtes de vtable. Le paramètre de
//! registre `PacketMode` décide de ce qu'il expose, et rien d'autre ne le décide.
//!
//! À 1, il expose l'interface de paquets du sens du flux, que `stream::WaveStream`
//! **sert** depuis le lot 3. À 0 — le défaut, le temps de la campagne Driver Verifier — il
//! est construit avec `PacketInterfaces::None`, donc sans exposer une interface de plus
//! qu'un flux ordinaire. Voir `open_stream` et
//! `conduit_kmd_core::params::DEFAULT_PACKET_MODE`.
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
use core::sync::atomic::AtomicBool;

use conduit_kmd_core::wavefmt::WAVEFORMATEXTENSIBLE_CB_SIZE;
use conduit_kmd_core::{
    RequestedFormat, SampleKind, SupportedFormat, cable_formats,
    config::{CableFormat, PacketExposure},
    validate,
};
use portcls::conduit_com::{
    ComRef, NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
};
use portcls::{
    MiniportWaveRT, ModesTrace, PacketInterfaces, PortWaveRT, PortWaveRTStream, ResourceList,
    SignalModes, StreamObject, try_new_packet_stream_object,
};
use portcls_sys::{
    GUID, IUnknown, KSDATAFORMAT, KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, KSDATAFORMAT_SUBTYPE_PCM, KSDATAFORMAT_TYPE_AUDIO,
    KSDATAFORMAT_WAVEFORMATEX, KSDATAFORMAT_WAVEFORMATEXTENSIBLE, KSDATARANGE, PCFILTER_DESCRIPTOR,
    WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM,
};

use crate::cable::{Cable, Direction};
use crate::descriptors::{
    PIN_COUNT, WAVE_CAPTURE_PIN_SYSTEM, WAVE_RENDER_PIN_SYSTEM, cable_format, wave_capture_filter,
    wave_capture_filter_default, wave_render_filter, wave_render_filter_default,
};
use crate::eventlog::EventLog;
use crate::intersect::{Negotiation, guid_eq};
use crate::stream::WaveStream;

/// Taille minimale de `cbSize` pour qu'un `WAVEFORMATEX` étendu contienne les champs
/// de `WAVEFORMATEXTENSIBLE` (`Samples`, `dwChannelMask`, `SubFormat` : 22 octets).
///
/// La même valeur que [`WAVEFORMATEXTENSIBLE_CB_SIZE`], qui est ce que notre gestionnaire
/// d'intersection **écrit** : lire et écrire d'après la même constante est ce qui empêche
/// les deux portes de diverger.
const WAVEFORMATEXTENSIBLE_CBSIZE: u16 = WAVEFORMATEXTENSIBLE_CB_SIZE;

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
/// lisible et supporté par **ce câble** (§5.4). Renvoie le format retenu.
///
/// `cable_format` est le format configuré du câble ; la liste confrontée est celle de ses
/// trois profondeurs, à sa fréquence et sur ses canaux. Le moteur audio ne devrait rien
/// proposer d'autre — c'est tout ce que le descripteur déclare —, mais `NewStream` reste
/// le seul endroit où le format est vérifié **avant** qu'un tampon ne soit dimensionné
/// dessus : le refus vaut mieux qu'une trame lue de travers.
///
/// # Safety
///
/// Contrat de [`requested_format`] sur `format`.
unsafe fn check_stream_request(
    expected_pin: u32,
    expected_capture: bool,
    pin: u32,
    capture: bool,
    cable_format: CableFormat,
    format: &KSDATAFORMAT,
) -> Result<SupportedFormat, NtStatus> {
    if pin != expected_pin || capture != expected_capture {
        return Err(STATUS_INVALID_PARAMETER);
    }
    // SAFETY: contrat relayé tel quel.
    let requested = unsafe { requested_format(format) }.ok_or(STATUS_INVALID_PARAMETER)?;
    let supportes = cable_formats(cable_format.sample_rate, cable_format.channels);
    validate(&requested, &supportes).map_err(|_| STATUS_INVALID_PARAMETER)
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
    // Mode paquets. Par défaut l'objet est composite mais n'expose RIEN :
    // `PacketInterfaces::None` fait répondre son `QueryInterface` exactement comme celui
    // d'un flux ordinaire — aucun IID de plus.
    //
    // Ce n'est plus faute de savoir servir : depuis le lot 3, les quatre méthodes de
    // `stream::WaveStream` servent. C'est faute d'avoir passé la campagne Driver Verifier —
    // ce qui n'a pas été éprouvé en machine ne se livre pas activé. Le paramètre de registre
    // `PacketMode`, à 1, expose donc des interfaces réellement servies, et il passera à 1 par
    // défaut après la campagne (voir `conduit_kmd_core::params::DEFAULT_PACKET_MODE`). Le
    // relevé `KSPROPERTY_CONDUIT_PACKETS` dit ce que le moteur audio en fait.
    //
    // Le paramètre est lu au `StartDevice` et déposé dans un atomique
    // (`registry::packet_mode`) : ce n'est pas une lecture de registre par flux, et une
    // modification dans `regedit` ne prend effet qu'au prochain démarrage du périphérique.
    //
    // Le sens : le moteur audio ÉCRIT dans le tampon d'un flux de rendu, d'où
    // `IMiniportWaveRTOutputStream` ; il LIT celui d'un flux de capture, d'où
    // `IMiniportWaveRTInputStream`. C'est la sémantique de `PacketInterfaces` (voir
    // `portcls::packet`, « sens du flux, et le cas `None` »), et l'inverser exposerait à
    // chaque flux l'interface que le moteur ne lui demandera jamais — un relevé tout à zéro
    // qu'on lirait comme un désintérêt du moteur.
    let (interfaces, exposure) = if crate::registry::packet_mode() {
        match direction {
            Direction::Render => (PacketInterfaces::Output, PacketExposure::Output),
            Direction::Capture => (PacketInterfaces::Input, PacketExposure::Input),
        }
    } else {
        (PacketInterfaces::None, PacketExposure::NotExposed)
    };
    let stream = WaveStream::new(n, direction, cable, port_stream, supported, exposure)?;
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
    /// Journal d'événements de l'adaptateur, pour les refus d'intersection (M1b-21).
    pub log: EventLog,
    /// Un refus numérique d'intersection a déjà été consigné pour ce miniport.
    pub refus_consigne: AtomicBool,
}

impl WaveRender {
    /// Le contexte de négociation de ce miniport (voir [`crate::intersect`]).
    fn negotiation(&self) -> Negotiation<'_> {
        Negotiation {
            name: "WaveRender",
            n: self.n,
            system_pin: WAVE_RENDER_PIN_SYSTEM,
            format: cable_format(self.n),
            log: self.log,
            reported: &self.refus_consigne,
        }
    }
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
        // Le descripteur de la **variante de format** du câble (M1b-05), et non plus une
        // table unique. Le trait rend une référence, pas une `Option` : le repli sur la
        // variante par défaut n'est là que pour qu'aucun chemin ne panique (même idiome
        // que `topo::TopoRender::description`). Il est inatteignable pour un format sorti
        // de `CableFormat::sanitize`, seul chemin d'écriture du magasin.
        //
        // **Le repli se dit.** Un `unwrap_or_else` nu avalait ici la seule panne que ce
        // chemin sache produire — un endpoint servant un format que personne n'a demandé —
        // et rien, nulle part, ne l'aurait signalée. Ce que ce module peut en dire est
        // limité (`kmd_log!` est vide en release) ; le garde-fou qui compte est
        // `descriptors::check_cable_pins`, appelé par `adapter::start_device` et qui, lui,
        // écrit au journal d'événements.
        let format = cable_format(self.n);
        match wave_render_filter(format) {
            Some(filtre) => filtre,
            None => {
                kmd_log!(
                    "WaveRender{} : aucune variante pour {} Hz sur {} canaux — repli sur le \
                     format par défaut, l'endpoint ne servira pas ce que le registre annonce",
                    self.n,
                    format.sample_rate,
                    format.channels
                );
                wave_render_filter_default()
            }
        }
    }

    // IRQL: PASSIVE_LEVEL
    fn data_range_intersection(
        &self,
        pin_id: u32,
        client: &KSDATARANGE,
        my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        // SAFETY: `client` et `my` viennent du thunk `DataRangeIntersection` de `portcls`,
        // qui garantit des `KSDATARANGE` suivies de leur extension `FormatSize`.
        unsafe { self.negotiation().resolve(pin_id, client, my, out) }
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
        let supported = unsafe {
            check_stream_request(
                WAVE_RENDER_PIN_SYSTEM,
                false,
                pin,
                capture,
                cable_format(self.n),
                format,
            )
        }?;
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

impl SignalModes for WaveRender {
    fn pin_count(&self) -> u32 {
        PIN_COUNT as u32
    }

    fn streaming_pin(&self) -> u32 {
        WAVE_RENDER_PIN_SYSTEM
    }

    fn trace(&self, trace: &ModesTrace<'_>) {
        kmd_log!(
            "WaveRender{}::MODES {} instance {:?} : {:?}",
            self.n,
            trace.verb,
            trace.instance,
            trace.pin
        );
    }
}

/// Miniport WaveRT du filtre `WaveCapture<n>` : l'enregistreur lit sur sa broche système.
#[derive(Debug)]
pub struct WaveCapture {
    /// Numéro du câble.
    pub n: u32,
    /// État partagé du câble (le flux capture courant y sera inscrit en M1a-08).
    pub cable: &'static Cable,
    /// Journal d'événements de l'adaptateur, pour les refus d'intersection (M1b-21).
    pub log: EventLog,
    /// Un refus numérique d'intersection a déjà été consigné pour ce miniport.
    pub refus_consigne: AtomicBool,
}

impl WaveCapture {
    /// Le contexte de négociation de ce miniport (voir [`crate::intersect`]).
    fn negotiation(&self) -> Negotiation<'_> {
        Negotiation {
            name: "WaveCapture",
            n: self.n,
            system_pin: WAVE_CAPTURE_PIN_SYSTEM,
            format: cable_format(self.n),
            log: self.log,
            reported: &self.refus_consigne,
        }
    }
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
        // Voir `WaveRender::description` : même variante, l'autre sens, et le même repli
        // qui se dit. Les deux bouts du câble lisent le **même** `cable_format(n)`, ce qui
        // est exactement ce qui les empêche de se désaccorder — et
        // `descriptors::check_cable_pins` le vérifie sur les deux au démarrage.
        let format = cable_format(self.n);
        match wave_capture_filter(format) {
            Some(filtre) => filtre,
            None => {
                kmd_log!(
                    "WaveCapture{} : aucune variante pour {} Hz sur {} canaux — repli sur le \
                     format par défaut, l'endpoint ne servira pas ce que le registre annonce",
                    self.n,
                    format.sample_rate,
                    format.channels
                );
                wave_capture_filter_default()
            }
        }
    }

    // IRQL: PASSIVE_LEVEL
    fn data_range_intersection(
        &self,
        pin_id: u32,
        client: &KSDATARANGE,
        my: &KSDATARANGE,
        out: Option<&mut [u8]>,
    ) -> Result<u32, NtStatus> {
        // SAFETY: voir `WaveRender::data_range_intersection`.
        unsafe { self.negotiation().resolve(pin_id, client, my, out) }
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
        let supported = unsafe {
            check_stream_request(
                WAVE_CAPTURE_PIN_SYSTEM,
                true,
                pin,
                capture,
                cable_format(self.n),
                format,
            )
        }?;
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

impl SignalModes for WaveCapture {
    fn pin_count(&self) -> u32 {
        PIN_COUNT as u32
    }

    fn streaming_pin(&self) -> u32 {
        WAVE_CAPTURE_PIN_SYSTEM
    }

    fn trace(&self, trace: &ModesTrace<'_>) {
        kmd_log!(
            "WaveCapture{}::MODES {} instance {:?} : {:?}",
            self.n,
            trace.verb,
            trace.instance,
            trace.pin
        );
    }
}
