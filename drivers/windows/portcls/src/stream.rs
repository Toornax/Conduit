//! `IMiniportWaveRTStream` et `IMiniportWaveRTStreamNotification` : un flux WaveRT (une
//! pin ouverte du filtre rendu ou capture), créé par `IMiniportWaveRT::NewStream`, avec
//! son tampon cyclique et sa position (driver-design.md §5).
//!
//! Le pilote implémente [`MiniportWaveRTStream`] (huit méthodes) et, pour les flux qui
//! signalent des événements de notification (§5.3, étape 5),
//! [`MiniportWaveRTStreamNotification`] (quatre de plus). [`new_stream_object`] et
//! [`new_stream_notification_object`] en font l'objet COM que `NewStream` rend à PortCls
//! (par [`crate::wavert::StreamObject`]). Les vtables sont les constantes associées
//! [`StreamVtbl::VTBL`] et [`StreamNotificationVtbl::VTBL`], dans l'**ordre du header
//! 26100** (`portcls-sys/tests/vtables.rs`) : `IUnknown`, `SetFormat`, `SetState`,
//! `GetPosition`, `AllocateAudioBuffer`, `FreeAudioBuffer`, `GetHWLatency`,
//! `GetPositionRegister`, `GetClockRegister`, puis pour la notification
//! `AllocateBufferWithNotification`, `FreeBufferWithNotification`,
//! `RegisterNotificationEvent`, `UnregisterNotificationEvent`. Les thunks des onze
//! premiers slots sont génériques sur la vtable `V`, donc partagés par les deux.
//!
//! `QueryInterface` : l'objet simple répond à `IID_IUnknown` et
//! `IID_IMiniportWaveRTStream` ; l'objet notification à ces deux IID plus
//! `IID_IMiniportWaveRTStreamNotification` (préfixe de vtable, `IIDS` de
//! `portcls_sys::com`). C'est ainsi que PortCls découvre la notification : il interroge
//! le flux rendu par `NewStream`.
//!
//! # IRQL
//!
//! `GetPosition` est appelable à `DISPATCH_LEVEL` (contexte de `KSPROPERTY_AUDIO_POSITION`
//! et de la DPC de boucle locale) : [`MiniportWaveRTStream::position`] n'alloue pas,
//! n'attend pas et ne touche que de la mémoire non paginée. Tout le reste est à
//! `PASSIVE_LEVEL`.

use core::ffi::c_void;

use conduit_com::{
    ComObject, ComPtr, ComVtable, NtStatus, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
    STATUS_SUCCESS,
};
use portcls_sys::{
    IMiniportWaveRTStreamNotificationVtbl, IMiniportWaveRTStreamVtbl, KSAUDIO_POSITION,
    KSDATAFORMAT, KSRTAUDIO_HWLATENCY, KSRTAUDIO_HWREGISTER, KSSTATE, MEMORY_CACHING_TYPE,
    NTSTATUS, PKEVENT, PKSAUDIO_POSITION, PKSDATAFORMAT, PMDL, ULONG,
};

use crate::status::STATUS_NOT_SUPPORTED;
use crate::unknown;

/// Résultat de `AllocateAudioBuffer` / `AllocateBufferWithNotification` : le tampon
/// cyclique alloué par le flux, tel que PortCls le mappera dans l'espace du client.
#[derive(Debug, Clone, Copy)]
pub struct AudioBuffer {
    /// MDL décrivant les pages du tampon (`IPortWaveRTStream::AllocatePagesForMdl`).
    /// PortCls la garde jusqu'à `FreeAudioBuffer`, où il la rend telle quelle.
    pub mdl: PMDL,
    /// Taille réelle du tampon en octets : **au moins** la taille demandée (sinon le
    /// moteur audio refuse le tampon), arrondie à la trame.
    pub actual_bytes: u32,
    /// Décalage du début du tampon depuis le début de la première page de la MDL
    /// (0 pour un tampon alloué page par page).
    pub offset_from_first_page: u32,
    /// Type de cache du mappage client (`MmCached` pour un tampon en mémoire système
    /// que seul le processeur touche).
    pub cache_type: MEMORY_CACHING_TYPE,
}

/// Contrat de `IMiniportWaveRTStream` (`portcls.h`), vu du pilote.
///
/// `Send + Sync + 'static` : PortCls appelle depuis n'importe quel fil (dont la DPC de
/// boucle locale pour `position`) et ne donne jamais qu'un `&self`. Le flux est créé par
/// `NewStream` à l'état `KSSTATE_STOP`, position 0 ; PortCls en détient la référence
/// principale et la rend à la fermeture de la pin.
pub trait MiniportWaveRTStream: Send + Sync + 'static {
    /// `SetFormat` : changement de format d'un flux déjà créé. Le défaut le refuse
    /// (`STATUS_NOT_SUPPORTED`, comme SYSVAD) : le format est fixé par `NewStream`.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn set_format(&self, format: &KSDATAFORMAT) -> NtStatus {
        let _ = format;
        STATUS_NOT_SUPPORTED
    }

    /// `SetState` : transition `KSSTATE_STOP` / `ACQUIRE` / `PAUSE` / `RUN`
    /// (`KSSTATE::Type`). Au passage en `RUN`, le flux mémorise son origine d'horloge
    /// (driver-design.md §5.1) et arme le timer de boucle locale ; en sortie de `RUN`, il
    /// accumule les trames jouées.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn set_state(&self, state: KSSTATE::Type) -> NtStatus;

    /// `GetPosition` : position courante en **octets depuis le début du tampon
    /// cyclique** (lecture pour le rendu, enregistrement pour la capture), calculée
    /// depuis l'horloge (§5.1). Le thunk l'écrit dans `PlayOffset` et `WriteOffset`
    /// (même valeur : un périphérique virtuel n'a pas de FIFO entre les deux).
    ///
    /// IRQL : **`<= DISPATCH_LEVEL`** : ni allocation, ni attente, ni mémoire paginée.
    fn position(&self) -> Result<u32, NtStatus>;

    /// `AllocateAudioBuffer` : alloue le tampon cyclique du flux (au moins
    /// `requested_bytes`, arrondi à la trame) par `IPortWaveRTStream::AllocatePagesForMdl`
    /// et le mémorise ; PortCls le mappera dans l'espace du client. Erreurs usuelles :
    /// `STATUS_INSUFFICIENT_RESOURCES`, `STATUS_UNSUCCESSFUL` (combinaison refusée).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus>;

    /// `FreeAudioBuffer` : PortCls rend la MDL et la taille de `allocate_audio_buffer` ;
    /// le flux libère les pages par `IPortWaveRTStream::FreePagesFromMdl` et oublie le
    /// tampon (jamais avant l'arrêt du flux : PortCls s'en assure).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn free_audio_buffer(&self, mdl: PMDL, size: u32);

    /// `GetHWLatency` : latences matérielles (`FifoSize`, `ChipsetDelay`, `CodecDelay`
    /// en octets/100 ns). Le défaut laisse les zéros : un câble virtuel n'en a aucune.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn hw_latency(&self, out: &mut KSRTAUDIO_HWLATENCY) {
        let _ = out;
    }

    /// `GetPositionRegister` : registre matériel de position lisible par le client. Le
    /// défaut, `Err(STATUS_NOT_IMPLEMENTED)`, dit à PortCls qu'il n'y en a pas ; le
    /// client passe alors par `GetPosition`.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn position_register(&self) -> Result<KSRTAUDIO_HWREGISTER, NtStatus> {
        Err(STATUS_NOT_IMPLEMENTED)
    }

    /// `GetClockRegister` : registre d'horloge matériel. Même défaut que
    /// [`position_register`](Self::position_register).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn clock_register(&self) -> Result<KSRTAUDIO_HWREGISTER, NtStatus> {
        Err(STATUS_NOT_IMPLEMENTED)
    }
}

/// Contrat de `IMiniportWaveRTStreamNotification` : un flux qui signale des
/// événements quand le tampon franchit une période de notification (driver-design.md
/// §5.3, étape 5). Sans cette interface, WASAPI en mode événementiel ne s'ouvre pas.
pub trait MiniportWaveRTStreamNotification: MiniportWaveRTStream {
    /// `AllocateBufferWithNotification` : comme
    /// [`allocate_audio_buffer`](MiniportWaveRTStream::allocate_audio_buffer), avec le
    /// nombre de notifications par tour de tampon (`notification_count` : 1 ou 2).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn allocate_buffer_with_notification(
        &self,
        notification_count: u32,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus>;

    /// `FreeBufferWithNotification` : pendant de
    /// [`allocate_buffer_with_notification`](Self::allocate_buffer_with_notification).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn free_buffer_with_notification(&self, mdl: PMDL, size: u32);

    /// `RegisterNotificationEvent` : PortCls remet un `KEVENT` (non paginé, référencé par
    /// lui) que le flux signalera à chaque période de notification ; le flux le garde
    /// jusqu'à [`unregister_notification_event`](Self::unregister_notification_event).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn register_notification_event(&self, event: PKEVENT) -> NtStatus;

    /// `UnregisterNotificationEvent` : le flux oublie `event` (jamais signalé après).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn unregister_notification_event(&self, event: PKEVENT) -> NtStatus;
}

/// Vtable `IMiniportWaveRTStream` d'un type implémenteur, constante associée (voir
/// [`PowerVtbl`](crate::power::PowerVtbl)).
pub trait StreamVtbl {
    /// Vtable complète : thunks `IUnknown` génériques puis thunks métier de `Self`.
    const VTBL: IMiniportWaveRTStreamVtbl;
}

impl<T: MiniportWaveRTStream> StreamVtbl for T {
    const VTBL: IMiniportWaveRTStreamVtbl = IMiniportWaveRTStreamVtbl {
        QueryInterface: Some(unknown::query_interface::<IMiniportWaveRTStreamVtbl, T>),
        AddRef: Some(unknown::add_ref::<IMiniportWaveRTStreamVtbl, T>),
        Release: Some(unknown::release::<IMiniportWaveRTStreamVtbl, T>),
        SetFormat: Some(set_format::<IMiniportWaveRTStreamVtbl, T>),
        SetState: Some(set_state::<IMiniportWaveRTStreamVtbl, T>),
        GetPosition: Some(get_position::<IMiniportWaveRTStreamVtbl, T>),
        AllocateAudioBuffer: Some(allocate_audio_buffer::<IMiniportWaveRTStreamVtbl, T>),
        FreeAudioBuffer: Some(free_audio_buffer::<IMiniportWaveRTStreamVtbl, T>),
        GetHWLatency: Some(get_hw_latency::<IMiniportWaveRTStreamVtbl, T>),
        GetPositionRegister: Some(get_position_register::<IMiniportWaveRTStreamVtbl, T>),
        GetClockRegister: Some(get_clock_register::<IMiniportWaveRTStreamVtbl, T>),
    };
}

/// Vtable `IMiniportWaveRTStreamNotification` d'un type implémenteur, constante
/// associée : les onze slots de [`StreamVtbl`] (mêmes thunks, instanciés pour cette
/// vtable) puis les quatre de la notification.
pub trait StreamNotificationVtbl {
    /// Vtable complète.
    const VTBL: IMiniportWaveRTStreamNotificationVtbl;
}

impl<T: MiniportWaveRTStreamNotification> StreamNotificationVtbl for T {
    const VTBL: IMiniportWaveRTStreamNotificationVtbl = IMiniportWaveRTStreamNotificationVtbl {
        QueryInterface: Some(unknown::query_interface::<IMiniportWaveRTStreamNotificationVtbl, T>),
        AddRef: Some(unknown::add_ref::<IMiniportWaveRTStreamNotificationVtbl, T>),
        Release: Some(unknown::release::<IMiniportWaveRTStreamNotificationVtbl, T>),
        SetFormat: Some(set_format::<IMiniportWaveRTStreamNotificationVtbl, T>),
        SetState: Some(set_state::<IMiniportWaveRTStreamNotificationVtbl, T>),
        GetPosition: Some(get_position::<IMiniportWaveRTStreamNotificationVtbl, T>),
        AllocateAudioBuffer: Some(
            allocate_audio_buffer::<IMiniportWaveRTStreamNotificationVtbl, T>,
        ),
        FreeAudioBuffer: Some(free_audio_buffer::<IMiniportWaveRTStreamNotificationVtbl, T>),
        GetHWLatency: Some(get_hw_latency::<IMiniportWaveRTStreamNotificationVtbl, T>),
        GetPositionRegister: Some(
            get_position_register::<IMiniportWaveRTStreamNotificationVtbl, T>,
        ),
        GetClockRegister: Some(get_clock_register::<IMiniportWaveRTStreamNotificationVtbl, T>),
        AllocateBufferWithNotification: Some(allocate_buffer_with_notification::<T>),
        FreeBufferWithNotification: Some(free_buffer_with_notification::<T>),
        RegisterNotificationEvent: Some(register_notification_event::<T>),
        UnregisterNotificationEvent: Some(unregister_notification_event::<T>),
    };
}

/// Objet COM `IMiniportWaveRTStream` possédé côté Rust (typé : `Deref<Target = T>`). À
/// effacer en [`StreamObject`](crate::wavert::StreamObject) pour le rendre à PortCls.
pub type StreamPtr<T> = ComPtr<IMiniportWaveRTStreamVtbl, T>;

/// Objet COM `IMiniportWaveRTStreamNotification` possédé côté Rust.
pub type StreamNotificationPtr<T> = ComPtr<IMiniportWaveRTStreamNotificationVtbl, T>;

/// Alloue l'objet COM `IMiniportWaveRTStream` de `inner` (compte de références 1).
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_stream_object`].
pub fn new_stream_object<T: MiniportWaveRTStream>(inner: T) -> StreamPtr<T> {
    ComObject::new(&<T as StreamVtbl>::VTBL, inner)
}

/// Comme [`new_stream_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_stream_object<T: MiniportWaveRTStream>(inner: T) -> Option<StreamPtr<T>> {
    ComObject::try_new(&<T as StreamVtbl>::VTBL, inner)
}

/// Alloue l'objet COM `IMiniportWaveRTStreamNotification` de `inner` (compte 1).
///
/// Panique via l'allocateur global en cas d'échec d'allocation : dans le pilote,
/// préférer [`try_new_stream_notification_object`].
pub fn new_stream_notification_object<T: MiniportWaveRTStreamNotification>(
    inner: T,
) -> StreamNotificationPtr<T> {
    ComObject::new(&<T as StreamNotificationVtbl>::VTBL, inner)
}

/// Comme [`new_stream_notification_object`], mais renvoie `None` si l'allocation échoue.
pub fn try_new_stream_notification_object<T: MiniportWaveRTStreamNotification>(
    inner: T,
) -> Option<StreamNotificationPtr<T>> {
    ComObject::try_new(&<T as StreamNotificationVtbl>::VTBL, inner)
}

/// Écrit les quatre sorties de `AllocateAudioBuffer`/`AllocateBufferWithNotification`.
///
/// Contrat de sortie : tous les pointeurs sont exigés non nuls (sinon
/// `STATUS_INVALID_PARAMETER`, sans appeler le trait) ; en cas d'échec du trait, la MDL de
/// sortie est mise à nul et le statut est renvoyé tel quel.
///
/// # Safety
///
/// Chaque pointeur de sortie, s'il n'est pas nul, pointe un emplacement inscriptible.
unsafe fn ecrire_tampon(
    result: Result<AudioBuffer, NtStatus>,
    mdl_out: *mut PMDL,
    actual_out: *mut ULONG,
    offset_out: *mut ULONG,
    cache_out: *mut MEMORY_CACHING_TYPE,
) -> NTSTATUS {
    match result {
        Ok(buffer) => {
            // SAFETY: `mdl_out` est non nul et inscriptible (contrat).
            unsafe { *mdl_out = buffer.mdl };
            // SAFETY: idem `actual_out`.
            unsafe { *actual_out = buffer.actual_bytes };
            // SAFETY: idem `offset_out`.
            unsafe { *offset_out = buffer.offset_from_first_page };
            // SAFETY: idem `cache_out`.
            unsafe { *cache_out = buffer.cache_type };
            STATUS_SUCCESS
        }
        Err(status) => {
            // SAFETY: `mdl_out` est non nul et inscriptible (contrat).
            unsafe { *mdl_out = core::ptr::null_mut() };
            status
        }
    }
}

/// `IMiniportWaveRTStream::SetFormat`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `format`, s'il n'est
/// pas nul, pointe une `KSDATAFORMAT` lisible (suivie de son extension `FormatSize`).
unsafe extern "C" fn set_format<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    format: PKSDATAFORMAT,
) -> NTSTATUS {
    if format.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    // SAFETY: `format` est non nul et lisible le temps de l'appel (contrat).
    let format = unsafe { &*format };
    me.set_format(format)
}

/// `IMiniportWaveRTStream::SetState`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel.
unsafe extern "C" fn set_state<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    state: KSSTATE::Type,
) -> NTSTATUS {
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    me.set_state(state)
}

/// `IMiniportWaveRTStream::GetPosition`.
///
/// IRQL : `<= DISPATCH_LEVEL`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `position`, s'il n'est
/// pas nul, pointe une `KSAUDIO_POSITION` inscriptible.
unsafe extern "C" fn get_position<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    position: PKSAUDIO_POSITION,
) -> NTSTATUS {
    if position.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    match me.position() {
        Ok(bytes) => {
            let offset = u64::from(bytes);
            // SAFETY: `position` est non nul et inscriptible (contrat).
            unsafe {
                *position = KSAUDIO_POSITION {
                    PlayOffset: offset,
                    WriteOffset: offset,
                };
            }
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// `IMiniportWaveRTStream::AllocateAudioBuffer`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; les quatre pointeurs de
/// sortie, s'ils ne sont pas nuls, pointent des emplacements inscriptibles.
unsafe extern "C" fn allocate_audio_buffer<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    requested: ULONG,
    mdl_out: *mut PMDL,
    actual_out: *mut ULONG,
    offset_out: *mut ULONG,
    cache_out: *mut MEMORY_CACHING_TYPE,
) -> NTSTATUS {
    if mdl_out.is_null() || actual_out.is_null() || offset_out.is_null() || cache_out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    let result = me.allocate_audio_buffer(requested);
    // SAFETY: les quatre sorties sont non nulles (vérifié) et inscriptibles (contrat).
    unsafe { ecrire_tampon(result, mdl_out, actual_out, offset_out, cache_out) }
}

/// `IMiniportWaveRTStream::FreeAudioBuffer`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `mdl` est celle rendue
/// par `AllocateAudioBuffer` (PortCls la cède ici).
unsafe extern "C" fn free_audio_buffer<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    mdl: PMDL,
    size: ULONG,
) {
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    me.free_audio_buffer(mdl, size);
}

/// `IMiniportWaveRTStream::GetHWLatency`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `out`, s'il n'est pas
/// nul, pointe une `KSRTAUDIO_HWLATENCY` inscriptible et exclusive le temps de l'appel.
unsafe extern "C" fn get_hw_latency<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    out: *mut KSRTAUDIO_HWLATENCY,
) {
    if out.is_null() {
        return;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    // SAFETY: `out` est non nul, inscriptible et exclusif (contrat).
    let out = unsafe { &mut *out };
    *out = KSRTAUDIO_HWLATENCY::default();
    me.hw_latency(out);
}

/// Écrit `Ok(registre)` dans `out`, ou renvoie le statut.
///
/// # Safety
///
/// `out` est non nul et inscriptible.
unsafe fn ecrire_registre(
    result: Result<KSRTAUDIO_HWREGISTER, NtStatus>,
    out: *mut KSRTAUDIO_HWREGISTER,
) -> NTSTATUS {
    match result {
        Ok(register) => {
            // SAFETY: `out` est non nul et inscriptible (contrat).
            unsafe { *out = register };
            STATUS_SUCCESS
        }
        Err(status) => status,
    }
}

/// `IMiniportWaveRTStream::GetPositionRegister`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<V, T>` vivant pendant l'appel ; `out`, s'il n'est pas
/// nul, pointe une `KSRTAUDIO_HWREGISTER` inscriptible.
unsafe extern "C" fn get_position_register<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    out: *mut KSRTAUDIO_HWREGISTER,
) -> NTSTATUS {
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    // SAFETY: `out` est non nul et inscriptible (contrat).
    unsafe { ecrire_registre(me.position_register(), out) }
}

/// `IMiniportWaveRTStream::GetClockRegister`.
///
/// # Safety
///
/// Mêmes préconditions que [`get_position_register`].
unsafe extern "C" fn get_clock_register<V: ComVtable, T: MiniportWaveRTStream>(
    this: *mut c_void,
    out: *mut KSRTAUDIO_HWREGISTER,
) -> NTSTATUS {
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<V, T>` vivant (contrat).
    let me = unsafe { ComObject::<V, T>::inner(this) };
    // SAFETY: `out` est non nul et inscriptible (contrat).
    unsafe { ecrire_registre(me.clock_register(), out) }
}

/// `IMiniportWaveRTStreamNotification::AllocateBufferWithNotification`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
/// pendant l'appel ; les quatre pointeurs de sortie, s'ils ne sont pas nuls, pointent des
/// emplacements inscriptibles.
unsafe extern "C" fn allocate_buffer_with_notification<T: MiniportWaveRTStreamNotification>(
    this: *mut c_void,
    notification_count: ULONG,
    requested: ULONG,
    mdl_out: *mut PMDL,
    actual_out: *mut ULONG,
    offset_out: *mut ULONG,
    cache_out: *mut MEMORY_CACHING_TYPE,
) -> NTSTATUS {
    if mdl_out.is_null() || actual_out.is_null() || offset_out.is_null() || cache_out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
    // (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTStreamNotificationVtbl, T>::inner(this) };
    let result = me.allocate_buffer_with_notification(notification_count, requested);
    // SAFETY: les quatre sorties sont non nulles (vérifié) et inscriptibles (contrat).
    unsafe { ecrire_tampon(result, mdl_out, actual_out, offset_out, cache_out) }
}

/// `IMiniportWaveRTStreamNotification::FreeBufferWithNotification`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
/// pendant l'appel ; `mdl` est celle rendue par `AllocateBufferWithNotification`.
unsafe extern "C" fn free_buffer_with_notification<T: MiniportWaveRTStreamNotification>(
    this: *mut c_void,
    mdl: PMDL,
    size: ULONG,
) {
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
    // (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTStreamNotificationVtbl, T>::inner(this) };
    me.free_buffer_with_notification(mdl, size);
}

/// `IMiniportWaveRTStreamNotification::RegisterNotificationEvent`.
///
/// # Safety
///
/// `this` est un `*mut ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
/// pendant l'appel ; `event`, s'il n'est pas nul, est un `KEVENT` non paginé que PortCls
/// garde vivant jusqu'au `UnregisterNotificationEvent` correspondant.
unsafe extern "C" fn register_notification_event<T: MiniportWaveRTStreamNotification>(
    this: *mut c_void,
    event: PKEVENT,
) -> NTSTATUS {
    if event.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
    // (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTStreamNotificationVtbl, T>::inner(this) };
    me.register_notification_event(event)
}

/// `IMiniportWaveRTStreamNotification::UnregisterNotificationEvent`.
///
/// # Safety
///
/// Mêmes préconditions que [`register_notification_event`].
unsafe extern "C" fn unregister_notification_event<T: MiniportWaveRTStreamNotification>(
    this: *mut c_void,
    event: PKEVENT,
) -> NTSTATUS {
    if event.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `this` est un `ComObject<IMiniportWaveRTStreamNotificationVtbl, T>` vivant
    // (contrat).
    let me = unsafe { ComObject::<IMiniportWaveRTStreamNotificationVtbl, T>::inner(this) };
    me.unregister_notification_event(event)
}
