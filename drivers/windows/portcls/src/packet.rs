//! Mode **paquets** de WaveRT : `IMiniportWaveRTInputStream` et
//! `IMiniportWaveRTOutputStream` (`portcls.h`, `NTDDI_WINTHRESHOLD`), le chemin nominal
//! du transport WaveRT sur Windows 10/11.
//!
//! Dans ce mode, le moteur audio ne se contente plus de suivre la position du tampon
//! cyclique : il **signale** chaque paquet écrit (`SetWritePacket`, propriété
//! `KSPROPERTY_RTAUDIO_SETWRITEPACKET`) et **demande** chaque paquet à lire
//! (`GetReadPacket`, `KSPROPERTY_RTAUDIO_GETREADPACKET`), avec
//! `KSPROPERTY_RTAUDIO_PACKETCOUNT` et `KSPROPERTY_RTAUDIO_PRESENTATIONPOSITION`. PortCls
//! n'expose ces propriétés que si le flux répond à `QueryInterface` pour l'interface
//! correspondante ; sans elles, le moteur n'a aucun moyen d'annoncer ses écritures, ce
//! qui est exactement le symptôme observé (le pilote copie, le moteur n'écrit jamais).
//!
//! # Quatre interfaces, un seul objet
//!
//! Les deux interfaces du mode paquets dérivent de `IUnknown` **seul** : leur vtable
//! n'est pas une extension de `IMiniportWaveRTStreamVtbl` (slot 3 = `GetReadPacket` /
//! `SetWritePacket`, pas `SetFormat`, voir `portcls-sys/tests/vtables.rs`). Un flux doit
//! pourtant répondre à quatre IID : `IUnknown`, `IMiniportWaveRTStream`,
//! `IMiniportWaveRTStreamNotification` et, selon son sens,
//! `IMiniportWaveRTInputStream` ou `IMiniportWaveRTOutputStream`. Or un
//! [`ComObject`] n'a qu'**un** pointeur de vtable, à l'offset 0.
//!
//! La solution retenue est celle du C++ (et de SYSVAD, qui hérite des quatre interfaces) :
//! un **objet composite à plusieurs pointeurs de vtable dans une seule allocation**.
//! [`PacketStream<T>`] est l'état `T` du `ComObject`, et commence par deux « têtes »
//! satellites, chacune réduite à un pointeur de vtable :
//!
//! ```text
//! ComObject<IMiniportWaveRTStreamNotificationVtbl, PacketStream<T>>
//! ┌──────────────────────────────── this principal (IUnknown, …Stream, …Notification)
//! │ vtbl  ─────────────────────────► VTBL (15 slots, QueryInterface composite)
//! │ refcount                          ← l'unique compteur de l'objet
//! │ inner : PacketStream<T>
//! │   ┌──────────────────────────── this satellite « entrée »
//! │   │ vtbl ─────────────────────► INPUT_VTBL (4 slots)
//! │   ├──────────────────────────── this satellite « sortie »
//! │   │ vtbl ─────────────────────► OUTPUT_VTBL (6 slots)
//! │   │ principal : AtomicPtr ────► l'objet principal (écrit à la construction)
//! │   │ interfaces : PacketInterfaces
//! │   │ inner : T
//! ```
//!
//! Les deux autres solutions envisagées ont été écartées : des objets satellites
//! **alloués à part** créent soit un cycle de références (le principal les possède et ils
//! le possèdent), soit des « tear-offs » réalloués à chaque `QueryInterface` — allocation
//! faillible sur un chemin qui n'a pas le droit d'échouer, et pointeurs instables ;
//! étendre `ComObject` à plusieurs vtables aurait touché `conduit-com`, partagé avec le
//! workspace racine, pour un besoin propre à PortCls.
//!
//! # Comptage de références
//!
//! **Un seul compteur**, celui du `ComObject` principal, pour les trois pointeurs :
//!
//! - `QueryInterface` du principal sur un IID de paquets fait `AddRef` sur le principal
//!   et rend le pointeur de la tête satellite (adresse **stable** pour toute la vie de
//!   l'objet) : le contrat COM « une interface rendue est déjà comptée » est respecté ;
//! - `AddRef` / `Release` d'un satellite agissent sur le compteur du principal, retrouvé
//!   par le champ `principal` de l'en-tête ;
//! - `QueryInterface` d'un satellite délègue au principal, d'où la transitivité et la
//!   symétrie exigées par COM, et l'identité de `IID_IUnknown` (toujours le pointeur du
//!   principal, jamais celui d'un satellite) ;
//! - conséquence à retenir : **un `Release` sur un satellite peut détruire l'objet
//!   entier**, satellites compris. Aucun pointeur de satellite ne survit au dernier
//!   `Release`, ce qui est la règle COM habituelle.
//!
//! Le champ `principal` est écrit par [`new_packet_stream_object`] /
//! [`try_new_packet_stream_object`] juste après l'allocation — l'adresse de l'objet n'est
//! pas connue avant — et jamais ensuite ; il est lu en `Acquire`. Ces deux constructeurs
//! sont le **seul** moyen de fabriquer un `PacketStream` (ses champs sont privés), si bien
//! qu'aucun objet publié ne peut avoir de satellite non lié ; par prudence, les thunks
//! satellites traitent tout de même un `principal` nul comme une erreur plutôt que de
//! déréférencer.
//!
//! # Sens du flux
//!
//! [`PacketInterfaces`] dit à quelle famille l'objet répond : un flux de **rendu**
//! (le moteur écrit) expose `IMiniportWaveRTOutputStream`, un flux de **capture** (le
//! moteur lit) expose `IMiniportWaveRTInputStream`. Les IID non exposés sont refusés
//! comme n'importe quel IID inconnu (`STATUS_INVALID_PARAMETER`, `*out` nul).
//!
//! # IRQL
//!
//! Toutes les méthodes des deux interfaces sont à `PASSIVE_LEVEL`
//! (`_IRQL_requires_max_(PASSIVE_LEVEL)` dans `portcls.h`) ; `QueryInterface`, `AddRef` et
//! `Release` des satellites restent à `<= DISPATCH_LEVEL`, comme ceux du principal.

use core::ffi::c_void;
use core::fmt;
use core::ops::Deref;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use conduit_com::{ComObject, ComPtr, Guid, NtStatus, STATUS_INVALID_PARAMETER, STATUS_SUCCESS};
use portcls_sys::com::guid;
use portcls_sys::{
    BOOL, DWORD, IID, IID_IMiniportWaveRTInputStream, IID_IMiniportWaveRTOutputStream,
    IMiniportWaveRTInputStreamVtbl, IMiniportWaveRTOutputStreamVtbl,
    IMiniportWaveRTStreamNotificationVtbl, KSAUDIO_PRESENTATION_POSITION, KSDATAFORMAT,
    KSRTAUDIO_HWLATENCY, KSRTAUDIO_HWREGISTER, KSSTATE, NTSTATUS, PKEVENT, PMDL, PVOID, ULONG,
    ULONG64,
};

use crate::status::STATUS_NOT_SUPPORTED;
use crate::stream::{
    AudioBuffer, MiniportWaveRTStream, MiniportWaveRTStreamNotification, StreamNotificationVtbl,
};
use crate::unknown;

/// `IID_IMiniportWaveRTInputStream` sous forme [`Guid`], pour la comparaison.
const IID_INPUT_STREAM: Guid = guid(&IID_IMiniportWaveRTInputStream);

/// `IID_IMiniportWaveRTOutputStream` sous forme [`Guid`].
const IID_OUTPUT_STREAM: Guid = guid(&IID_IMiniportWaveRTOutputStream);

/// Vtable du principal, écrite une fois pour toutes : c'est celle du flux à
/// notifications, dont dépendent tous les thunks partagés.
type PrincipalVtbl = IMiniportWaveRTStreamNotificationVtbl;

/// Résultat de `IMiniportWaveRTInputStream::GetReadPacket` : le paquet **complet** le plus
/// récent que le pilote a écrit dans le tampon de capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadPacket {
    /// Numéro du paquet, monotone depuis le passage en `KSSTATE_RUN`. Sa position dans le
    /// tampon cyclique est `packet_number % nombre_de_paquets`.
    pub packet_number: u32,
    /// Drapeaux `KSSTREAM_HEADER_OPTIONSF_*` du paquet (0 pour un flux ordinaire).
    pub flags: u32,
    /// Valeur du compteur de performance (`KeQueryPerformanceCounter`) au moment où le
    /// paquet a été complété.
    pub performance_counter: u64,
    /// Vrai s'il reste au moins un paquet complet à lire après celui-ci : le moteur
    /// rappellera immédiatement au lieu d'attendre la notification suivante.
    pub more_data: bool,
}

/// Contrat de `IMiniportWaveRTInputStream` (`portcls.h`), vu du pilote : le mode paquets
/// d'un flux de **capture**, que le moteur audio lit.
///
/// Toutes les méthodes ont un défaut qui refuse (`STATUS_NOT_SUPPORTED`) : un flux peut
/// exposer l'interface sans encore la servir, ce qui suffit à faire apparaître les
/// propriétés `KSPROPERTY_RTAUDIO_*` côté PortCls et à observer si le moteur les emprunte.
pub trait MiniportWaveRTInputStream: MiniportWaveRTStream {
    /// `GetReadPacket` : numéro, drapeaux, horodatage QPC et présence de données
    /// supplémentaires du dernier paquet complet.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn read_packet(&self) -> Result<ReadPacket, NtStatus> {
        Err(STATUS_NOT_SUPPORTED)
    }
}

/// Contrat de `IMiniportWaveRTOutputStream` (`portcls.h`), vu du pilote : le mode paquets
/// d'un flux de **rendu**, dans lequel le moteur audio écrit.
///
/// Mêmes défauts que [`MiniportWaveRTInputStream`] : tout refuse tant que le pilote ne
/// sert pas le mode paquets.
pub trait MiniportWaveRTOutputStream: MiniportWaveRTStream {
    /// `SetWritePacket` : le moteur vient d'écrire le paquet `packet_number` (position
    /// `packet_number % nombre_de_paquets` dans le tampon). `flags` porte
    /// `KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM` en fin de flux, auquel cas
    /// `eos_packet_length` donne le nombre d'octets **utiles** du dernier paquet.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn set_write_packet(&self, packet_number: u32, flags: u32, eos_packet_length: u32) -> NtStatus {
        let _ = (packet_number, flags, eos_packet_length);
        STATUS_NOT_SUPPORTED
    }

    /// `GetOutputStreamPresentationPosition` : position de présentation (octets rendus
    /// depuis le début du flux) et valeur du compteur de performance associée.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn presentation_position(&self) -> Result<KSAUDIO_PRESENTATION_POSITION, NtStatus> {
        Err(STATUS_NOT_SUPPORTED)
    }

    /// `GetPacketCount` : nombre de paquets **écrits par le moteur et déjà consommés** par
    /// le pilote depuis le passage en `KSSTATE_RUN`.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn packet_count(&self) -> Result<u32, NtStatus> {
        Err(STATUS_NOT_SUPPORTED)
    }
}

/// Un flux capable de servir le mode paquets : les quatre interfaces réunies.
///
/// Alias de commodité, implémenté automatiquement ; c'est la borne des constructeurs et
/// des vtables de ce module.
pub trait PacketWaveRTStream:
    MiniportWaveRTStreamNotification + MiniportWaveRTInputStream + MiniportWaveRTOutputStream
{
}

impl<T> PacketWaveRTStream for T where
    T: MiniportWaveRTStreamNotification + MiniportWaveRTInputStream + MiniportWaveRTOutputStream
{
}

/// Interfaces du mode paquets auxquelles un objet répond, selon le sens du flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketInterfaces {
    /// Flux de **capture** : `IMiniportWaveRTInputStream` seule (le moteur lit).
    Input,
    /// Flux de **rendu** : `IMiniportWaveRTOutputStream` seule (le moteur écrit).
    Output,
    /// Les deux (tests, ou flux bidirectionnel).
    Both,
}

impl PacketInterfaces {
    /// Faut-il répondre à `IID_IMiniportWaveRTInputStream` ?
    pub const fn has_input(self) -> bool {
        matches!(self, Self::Input | Self::Both)
    }

    /// Faut-il répondre à `IID_IMiniportWaveRTOutputStream` ?
    pub const fn has_output(self) -> bool {
        matches!(self, Self::Output | Self::Both)
    }
}

/// Tête satellite `IMiniportWaveRTInputStream` : un pointeur de vtable, rien d'autre.
///
/// `repr(C)` et champ unique : son adresse est un `this` COM valide pour l'interface.
/// Le champ n'est lu que par l'appelant C, qui suit le pointeur à l'offset 0 ; côté Rust,
/// il l'est par le `Debug` de [`PacketStream`].
#[repr(C)]
struct InputHead {
    vtbl: &'static IMiniportWaveRTInputStreamVtbl,
}

/// Tête satellite `IMiniportWaveRTOutputStream` (voir [`InputHead`]).
#[repr(C)]
struct OutputHead {
    vtbl: &'static IMiniportWaveRTOutputStreamVtbl,
}

/// État d'un flux composite : les deux têtes satellites, le lien vers l'objet principal,
/// les interfaces exposées, puis le flux `T` du pilote.
///
/// `repr(C)` fixe l'ordre des champs : les décalages `INPUT_OFFSET` / `OUTPUT_OFFSET`
/// permettent aux thunks satellites de remonter à cette structure depuis leur `this`.
/// Se construit uniquement par [`new_packet_stream_object`] ou
/// [`try_new_packet_stream_object`] (voir la documentation du module).
#[repr(C)]
pub struct PacketStream<T> {
    /// Tête `IMiniportWaveRTInputStream`.
    input: InputHead,
    /// Tête `IMiniportWaveRTOutputStream`.
    output: OutputHead,
    /// Objet principal (`*mut ComObject<PrincipalVtbl, PacketStream<T>>`), écrit une seule
    /// fois juste après l'allocation, avant toute publication.
    principal: AtomicPtr<c_void>,
    /// Interfaces du mode paquets exposées par `QueryInterface`.
    interfaces: PacketInterfaces,
    /// Le flux du pilote.
    inner: T,
}

impl<T> PacketStream<T> {
    /// Décalage de la tête « entrée » dans la structure (0 : premier champ).
    const INPUT_OFFSET: usize = core::mem::offset_of!(Self, input);
    /// Décalage de la tête « sortie » dans la structure.
    const OUTPUT_OFFSET: usize = core::mem::offset_of!(Self, output);

    /// Le flux du pilote.
    pub fn get(&self) -> &T {
        &self.inner
    }

    /// Interfaces du mode paquets exposées par cet objet.
    pub fn interfaces(&self) -> PacketInterfaces {
        self.interfaces
    }
}

impl<T> Deref for PacketStream<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> fmt::Debug for PacketStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PacketStream")
            .field("interfaces", &self.interfaces)
            .field("principal", &self.principal.load(Ordering::Relaxed))
            .field("input_vtbl", &ptr::from_ref(self.input.vtbl))
            .field("output_vtbl", &ptr::from_ref(self.output.vtbl))
            .finish_non_exhaustive()
    }
}

// Les onze slots de `IMiniportWaveRTStream` et les quatre de la notification sont ceux de
// `crate::stream`, instanciés pour `PacketStream<T>` : d'où ces délégations, mécaniques.
impl<T: MiniportWaveRTStream> MiniportWaveRTStream for PacketStream<T> {
    fn set_format(&self, format: &KSDATAFORMAT) -> NtStatus {
        self.inner.set_format(format)
    }

    fn set_state(&self, state: KSSTATE::Type) -> NtStatus {
        self.inner.set_state(state)
    }

    fn position(&self) -> Result<u32, NtStatus> {
        self.inner.position()
    }

    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus> {
        self.inner.allocate_audio_buffer(requested_bytes)
    }

    fn free_audio_buffer(&self, mdl: PMDL, size: u32) {
        self.inner.free_audio_buffer(mdl, size);
    }

    fn hw_latency(&self, out: &mut KSRTAUDIO_HWLATENCY) {
        self.inner.hw_latency(out);
    }

    fn position_register(&self) -> Result<KSRTAUDIO_HWREGISTER, NtStatus> {
        self.inner.position_register()
    }

    fn clock_register(&self) -> Result<KSRTAUDIO_HWREGISTER, NtStatus> {
        self.inner.clock_register()
    }
}

impl<T: MiniportWaveRTStreamNotification> MiniportWaveRTStreamNotification for PacketStream<T> {
    fn allocate_buffer_with_notification(
        &self,
        notification_count: u32,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        self.inner
            .allocate_buffer_with_notification(notification_count, requested_bytes)
    }

    fn free_buffer_with_notification(&self, mdl: PMDL, size: u32) {
        self.inner.free_buffer_with_notification(mdl, size);
    }

    fn register_notification_event(&self, event: PKEVENT) -> NtStatus {
        self.inner.register_notification_event(event)
    }

    fn unregister_notification_event(&self, event: PKEVENT) -> NtStatus {
        self.inner.unregister_notification_event(event)
    }
}

/// Les trois vtables d'un flux composite, constantes associées au type implémenteur (voir
/// [`PowerVtbl`](crate::power::PowerVtbl)).
pub trait PacketStreamVtbl {
    /// Vtable principale : les quinze slots de `IMiniportWaveRTStreamNotification`, dont
    /// le `QueryInterface` **composite** qui rend les têtes satellites.
    const VTBL: PrincipalVtbl;
    /// Vtable de la tête `IMiniportWaveRTInputStream` (quatre slots).
    const INPUT_VTBL: IMiniportWaveRTInputStreamVtbl;
    /// Vtable de la tête `IMiniportWaveRTOutputStream` (six slots).
    const OUTPUT_VTBL: IMiniportWaveRTOutputStreamVtbl;
}

impl<T: PacketWaveRTStream> PacketStreamVtbl for T {
    const VTBL: PrincipalVtbl = PrincipalVtbl {
        // Seul le slot 0 change : les quatorze autres sont les thunks de `crate::stream`,
        // instanciés pour `PacketStream<T>` (qui délègue à `T`).
        QueryInterface: Some(query_interface_composite::<T>),
        ..<PacketStream<T> as StreamNotificationVtbl>::VTBL
    };

    const INPUT_VTBL: IMiniportWaveRTInputStreamVtbl = IMiniportWaveRTInputStreamVtbl {
        QueryInterface: Some(input_query_interface::<T>),
        AddRef: Some(input_add_ref::<T>),
        Release: Some(input_release::<T>),
        GetReadPacket: Some(get_read_packet::<T>),
    };

    const OUTPUT_VTBL: IMiniportWaveRTOutputStreamVtbl = IMiniportWaveRTOutputStreamVtbl {
        QueryInterface: Some(output_query_interface::<T>),
        AddRef: Some(output_add_ref::<T>),
        Release: Some(output_release::<T>),
        SetWritePacket: Some(set_write_packet::<T>),
        GetOutputStreamPresentationPosition: Some(get_presentation_position::<T>),
        GetPacketCount: Some(get_packet_count::<T>),
    };
}

/// Objet COM d'un flux composite, possédé côté Rust. Son `Deref` traverse
/// [`PacketStream`] jusqu'au flux `T` du pilote.
pub type PacketStreamPtr<T> = ComPtr<PrincipalVtbl, PacketStream<T>>;

/// Alloue l'objet COM composite de `inner` (compte de références 1), exposant les
/// interfaces de paquets `interfaces`.
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_packet_stream_object`].
pub fn new_packet_stream_object<T: PacketWaveRTStream>(
    inner: T,
    interfaces: PacketInterfaces,
) -> PacketStreamPtr<T> {
    let object = ComObject::new(
        &<T as PacketStreamVtbl>::VTBL,
        etat_initial(inner, interfaces),
    );
    lier_satellites(&object);
    object
}

/// Comme [`new_packet_stream_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_packet_stream_object<T: PacketWaveRTStream>(
    inner: T,
    interfaces: PacketInterfaces,
) -> Option<PacketStreamPtr<T>> {
    let object = ComObject::try_new(
        &<T as PacketStreamVtbl>::VTBL,
        etat_initial(inner, interfaces),
    )?;
    lier_satellites(&object);
    Some(object)
}

/// État d'un flux composite avant liaison : têtes garnies de leurs vtables, `principal`
/// encore nul (l'adresse de l'objet n'existe pas avant l'allocation).
fn etat_initial<T: PacketWaveRTStream>(inner: T, interfaces: PacketInterfaces) -> PacketStream<T> {
    PacketStream {
        input: InputHead {
            vtbl: &<T as PacketStreamVtbl>::INPUT_VTBL,
        },
        output: OutputHead {
            vtbl: &<T as PacketStreamVtbl>::OUTPUT_VTBL,
        },
        principal: AtomicPtr::new(ptr::null_mut()),
        interfaces,
        inner,
    }
}

/// Écrit le lien satellites → objet principal, seule écriture de ce champ, faite avant
/// que l'objet ne soit visible d'un autre fil (`Release` s'appaire avec l'`Acquire` des
/// thunks satellites).
fn lier_satellites<T: PacketWaveRTStream>(object: &PacketStreamPtr<T>) {
    object
        .object()
        .get()
        .principal
        .store(object.as_raw(), Ordering::Release);
}

// ---------------------------------------------------------------------------------
// `QueryInterface` composite du principal.
// ---------------------------------------------------------------------------------

/// `QueryInterface` de la vtable principale : les IID de paquets exposés rendent la tête
/// satellite correspondante (avec `AddRef` sur le principal), les autres passent au
/// `QueryInterface` générique de [`ComObject`].
///
/// # Safety
///
/// `this` est un `*mut ComObject<PrincipalVtbl, PacketStream<T>>` vivant pendant l'appel ;
/// `iid`, s'il n'est pas nul, pointe un `IID` lisible ; `out`, s'il n'est pas nul, pointe
/// un emplacement de pointeur inscriptible.
unsafe extern "C" fn query_interface_composite<T: PacketWaveRTStream>(
    this: *mut c_void,
    iid: *const IID,
    out: *mut PVOID,
) -> NTSTATUS {
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    if iid.is_null() {
        // SAFETY: `out` est non nul et inscriptible (contrat).
        unsafe { *out = ptr::null_mut() };
        // SAFETY: `iid` est nul, la trace ne le déréférence pas.
        unsafe { unknown::trace_query_interface(this, iid, STATUS_INVALID_PARAMETER) };
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `iid` est non nul et pointe un `IID` lisible (contrat) ; `IID` et `Guid` ont
    // la même disposition (`portcls-sys/tests/com.rs`).
    let demande = unsafe { *iid.cast::<Guid>() };
    // SAFETY: `this` est un `ComObject<PrincipalVtbl, PacketStream<T>>` vivant (contrat).
    let me = unsafe { ComObject::<PrincipalVtbl, PacketStream<T>>::inner(this) };

    let satellite: Option<PVOID> = if demande == IID_INPUT_STREAM && me.interfaces.has_input() {
        Some(ptr::from_ref(&me.input).cast_mut().cast())
    } else if demande == IID_OUTPUT_STREAM && me.interfaces.has_output() {
        Some(ptr::from_ref(&me.output).cast_mut().cast())
    } else {
        None
    };

    let status = match satellite {
        Some(tete) => {
            // SAFETY: `this` est vivant et l'appelant en détient une référence : le
            // pointeur rendu doit être compté, comme l'exige COM.
            unsafe { ComObject::<PrincipalVtbl, PacketStream<T>>::add_ref(this) };
            // SAFETY: `out` est non nul et inscriptible (contrat).
            unsafe { *out = tete };
            STATUS_SUCCESS
        }
        // SAFETY: préconditions transmises telles quelles au `QueryInterface` générique.
        None => unsafe {
            ComObject::<PrincipalVtbl, PacketStream<T>>::query_interface(
                this,
                iid.cast::<Guid>(),
                out,
            )
        },
    };
    // SAFETY: `iid` est non nul et lisible (vérifié plus haut).
    unsafe { unknown::trace_query_interface(this, iid, status) };
    status
}

// ---------------------------------------------------------------------------------
// Remontée des têtes satellites vers l'objet composite et vers le principal.
// ---------------------------------------------------------------------------------

/// L'objet composite depuis le `this` de la tête « entrée ».
///
/// # Safety
///
/// `this` est l'adresse du champ `input` d'un `PacketStream<T>` vivant pendant `'a`.
unsafe fn depuis_entree<'a, T>(this: *mut c_void) -> &'a PacketStream<T> {
    // SAFETY: `this` pointe le champ `input`, à `INPUT_OFFSET` du début de la structure,
    // dans la même allocation (contrat) : le décalage reste dans l'objet.
    let base = unsafe { this.byte_sub(PacketStream::<T>::INPUT_OFFSET) };
    // SAFETY: `base` est le début d'un `PacketStream<T>` vivant, aligné (contrat).
    unsafe { &*base.cast::<PacketStream<T>>() }
}

/// L'objet composite depuis le `this` de la tête « sortie ».
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant pendant `'a`.
unsafe fn depuis_sortie<'a, T>(this: *mut c_void) -> &'a PacketStream<T> {
    // SAFETY: voir `depuis_entree` ; `output` est à `OUTPUT_OFFSET` du début.
    let base = unsafe { this.byte_sub(PacketStream::<T>::OUTPUT_OFFSET) };
    // SAFETY: `base` est le début d'un `PacketStream<T>` vivant, aligné (contrat).
    unsafe { &*base.cast::<PacketStream<T>>() }
}

/// L'objet principal d'un composite, ou `None` s'il n'a jamais été lié (impossible pour un
/// objet construit par ce module ; vérifié plutôt que supposé).
fn principal_de<T>(me: &PacketStream<T>) -> Option<*mut c_void> {
    let raw = me.principal.load(Ordering::Acquire);
    if raw.is_null() { None } else { Some(raw) }
}

/// `AddRef` d'un satellite : incrémente le compteur **du principal**.
///
/// # Safety
///
/// `objet` est un `*mut ComObject<PrincipalVtbl, PacketStream<T>>` vivant.
unsafe fn add_ref_principal<T: Send + Sync>(objet: *mut c_void) -> ULONG {
    // SAFETY: précondition transmise telle quelle.
    unsafe { ComObject::<PrincipalVtbl, PacketStream<T>>::add_ref(objet) }
}

/// `Release` d'un satellite : décrémente le compteur **du principal**, qui peut libérer
/// l'objet entier (têtes comprises).
///
/// # Safety
///
/// `objet` est un `*mut ComObject<PrincipalVtbl, PacketStream<T>>` vivant et l'appelant
/// cède la référence qu'il détient.
unsafe fn release_principal<T: Send + Sync>(objet: *mut c_void) -> ULONG {
    // SAFETY: précondition transmise telle quelle.
    unsafe { ComObject::<PrincipalVtbl, PacketStream<T>>::release(objet) }
}

// ---------------------------------------------------------------------------------
// Tête `IMiniportWaveRTInputStream`.
// ---------------------------------------------------------------------------------

/// `IUnknown::QueryInterface` de la tête « entrée » : délégué au principal, d'où
/// l'identité de `IID_IUnknown` et l'accès aux quatre interfaces depuis n'importe laquelle.
///
/// # Safety
///
/// `this` est l'adresse du champ `input` d'un `PacketStream<T>` vivant ; `iid` et `out`
/// respectent le contrat de `QueryInterface`.
unsafe extern "C" fn input_query_interface<T: PacketWaveRTStream>(
    this: *mut c_void,
    iid: *const IID,
    out: *mut PVOID,
) -> NTSTATUS {
    // SAFETY: `this` est la tête « entrée » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_entree::<T>(this) };
    let Some(objet) = principal_de(me) else {
        // SAFETY: `out` respecte le contrat de `QueryInterface` (nul ou inscriptible).
        return unsafe { echec_non_lie(out) };
    };
    // SAFETY: `objet` est l'objet composite vivant qui contient `me` ; `iid` et `out`
    // respectent le contrat (transmis tels quels).
    unsafe { query_interface_composite::<T>(objet, iid, out) }
}

/// `IUnknown::AddRef` de la tête « entrée ».
///
/// # Safety
///
/// `this` est l'adresse du champ `input` d'un `PacketStream<T>` vivant.
unsafe extern "C" fn input_add_ref<T: PacketWaveRTStream>(this: *mut c_void) -> ULONG {
    // SAFETY: `this` est la tête « entrée » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_entree::<T>(this) };
    match principal_de(me) {
        // SAFETY: `objet` est l'objet composite vivant qui contient `me`.
        Some(objet) => unsafe { add_ref_principal::<T>(objet) },
        None => 0,
    }
}

/// `IUnknown::Release` de la tête « entrée » : peut détruire l'objet entier.
///
/// # Safety
///
/// `this` est l'adresse du champ `input` d'un `PacketStream<T>` vivant et l'appelant cède
/// la référence qu'il détient : il n'utilise plus `this` après l'appel.
unsafe extern "C" fn input_release<T: PacketWaveRTStream>(this: *mut c_void) -> ULONG {
    // SAFETY: `this` est la tête « entrée » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_entree::<T>(this) };
    match principal_de(me) {
        // SAFETY: `objet` est l'objet composite vivant qui contient `me`, et la référence
        // cédée par l'appelant l'est ici.
        Some(objet) => unsafe { release_principal::<T>(objet) },
        None => 0,
    }
}

/// `IMiniportWaveRTInputStream::GetReadPacket`.
///
/// # Safety
///
/// `this` est l'adresse du champ `input` d'un `PacketStream<T>` vivant ; les quatre
/// pointeurs de sortie, s'ils ne sont pas nuls, pointent des emplacements inscriptibles.
unsafe extern "C" fn get_read_packet<T: PacketWaveRTStream>(
    this: *mut c_void,
    packet_number: *mut ULONG,
    flags: *mut DWORD,
    performance_counter: *mut ULONG64,
    more_data: *mut BOOL,
) -> NTSTATUS {
    if packet_number.is_null()
        || flags.is_null()
        || performance_counter.is_null()
        || more_data.is_null()
    {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est la tête « entrée » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_entree::<T>(this) };
    match me.inner.read_packet() {
        Ok(paquet) => {
            // SAFETY: `packet_number` est non nul et inscriptible (vérifié, contrat).
            unsafe { *packet_number = paquet.packet_number };
            // SAFETY: idem `flags`.
            unsafe { *flags = paquet.flags };
            // SAFETY: idem `performance_counter`.
            unsafe { *performance_counter = paquet.performance_counter };
            // SAFETY: idem `more_data` (`BOOL` = `c_int`).
            unsafe { *more_data = BOOL::from(paquet.more_data) };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

// ---------------------------------------------------------------------------------
// Tête `IMiniportWaveRTOutputStream`.
// ---------------------------------------------------------------------------------

/// `IUnknown::QueryInterface` de la tête « sortie » (voir [`input_query_interface`]).
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant ; `iid` et `out`
/// respectent le contrat de `QueryInterface`.
unsafe extern "C" fn output_query_interface<T: PacketWaveRTStream>(
    this: *mut c_void,
    iid: *const IID,
    out: *mut PVOID,
) -> NTSTATUS {
    // SAFETY: `this` est la tête « sortie » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_sortie::<T>(this) };
    let Some(objet) = principal_de(me) else {
        // SAFETY: `out` respecte le contrat de `QueryInterface` (nul ou inscriptible).
        return unsafe { echec_non_lie(out) };
    };
    // SAFETY: `objet` est l'objet composite vivant qui contient `me` ; `iid` et `out`
    // respectent le contrat (transmis tels quels).
    unsafe { query_interface_composite::<T>(objet, iid, out) }
}

/// `IUnknown::AddRef` de la tête « sortie ».
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant.
unsafe extern "C" fn output_add_ref<T: PacketWaveRTStream>(this: *mut c_void) -> ULONG {
    // SAFETY: `this` est la tête « sortie » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_sortie::<T>(this) };
    match principal_de(me) {
        // SAFETY: `objet` est l'objet composite vivant qui contient `me`.
        Some(objet) => unsafe { add_ref_principal::<T>(objet) },
        None => 0,
    }
}

/// `IUnknown::Release` de la tête « sortie » : peut détruire l'objet entier.
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant et l'appelant cède
/// la référence qu'il détient.
unsafe extern "C" fn output_release<T: PacketWaveRTStream>(this: *mut c_void) -> ULONG {
    // SAFETY: `this` est la tête « sortie » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_sortie::<T>(this) };
    match principal_de(me) {
        // SAFETY: `objet` est l'objet composite vivant qui contient `me`, et la référence
        // cédée par l'appelant l'est ici.
        Some(objet) => unsafe { release_principal::<T>(objet) },
        None => 0,
    }
}

/// `IMiniportWaveRTOutputStream::SetWritePacket`.
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant.
unsafe extern "C" fn set_write_packet<T: PacketWaveRTStream>(
    this: *mut c_void,
    packet_number: ULONG,
    flags: DWORD,
    eos_packet_length: ULONG,
) -> NTSTATUS {
    // SAFETY: `this` est la tête « sortie » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_sortie::<T>(this) };
    me.inner
        .set_write_packet(packet_number, flags, eos_packet_length)
}

/// `IMiniportWaveRTOutputStream::GetOutputStreamPresentationPosition`.
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant ; `out`, s'il
/// n'est pas nul, pointe une `KSAUDIO_PRESENTATION_POSITION` inscriptible.
unsafe extern "C" fn get_presentation_position<T: PacketWaveRTStream>(
    this: *mut c_void,
    out: *mut KSAUDIO_PRESENTATION_POSITION,
) -> NTSTATUS {
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est la tête « sortie » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_sortie::<T>(this) };
    match me.inner.presentation_position() {
        Ok(position) => {
            // SAFETY: `out` est non nul et inscriptible (vérifié, contrat).
            unsafe { *out = position };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// `IMiniportWaveRTOutputStream::GetPacketCount`.
///
/// # Safety
///
/// `this` est l'adresse du champ `output` d'un `PacketStream<T>` vivant ; `out`, s'il
/// n'est pas nul, pointe un `ULONG` inscriptible.
unsafe extern "C" fn get_packet_count<T: PacketWaveRTStream>(
    this: *mut c_void,
    out: *mut ULONG,
) -> NTSTATUS {
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est la tête « sortie » d'un `PacketStream<T>` vivant (contrat).
    let me = unsafe { depuis_sortie::<T>(this) };
    match me.inner.packet_count() {
        Ok(count) => {
            // SAFETY: `out` est non nul et inscriptible (vérifié, contrat).
            unsafe { *out = count };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// Réponse d'un satellite dont le lien vers le principal n'a jamais été écrit : ne peut
/// pas arriver sur un objet construit par ce module (voir la documentation), mais vaut
/// mieux qu'un déréférencement de pointeur nul en noyau.
///
/// # Safety
///
/// `out` est nul, ou pointe un emplacement de pointeur inscriptible.
unsafe fn echec_non_lie(out: *mut PVOID) -> NTSTATUS {
    if !out.is_null() {
        // SAFETY: `out` est non nul (vérifié) et inscriptible (contrat de l'appelant).
        unsafe { *out = ptr::null_mut() };
    }
    STATUS_INVALID_PARAMETER
}
