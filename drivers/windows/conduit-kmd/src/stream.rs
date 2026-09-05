//! Flux WaveRT rendu ([`RenderStream`], driver-design.md §5) : l'objet que
//! `WaveRender::NewStream` crée pour la broche système, avec son tampon cyclique
//! (§5.2), sa position calculée par l'horloge (§5.1) et ses notifications (§5.3,
//! étape 5). Implémente `portcls::MiniportWaveRTStreamNotification`, donc aussi
//! `MiniportWaveRTStream`.
//!
//! # État et verrouillage
//!
//! Tout l'état mutable est dans un [`SharedStream`] (`SpinLock<StreamState>`,
//! [`crate::cable`]) : état KS, position, tampon, périodes de notification, événements
//! enregistrés. Les méthodes PortCls (`PASSIVE_LEVEL`) et `GetPosition`
//! (`<= DISPATCH_LEVEL`) le prennent brièvement ; la DPC du timer de notification
//! ([`notification_dpc`], `DISPATCH_LEVEL`) aussi. Ce qui exige `PASSIVE_LEVEL`
//! (allocation, mappage et libération des pages par `IPortWaveRTStream`,
//! `KeFlushQueuedDpcs`) se fait **hors** du verrou.
//!
//! # Tampon
//!
//! Un seul tampon par flux, alloué par `IPortWaveRTStream::AllocatePagesForMdl`
//! (taille de `conduit_kmd_core::format::buffer_bytes[_for_notifications]` : multiple
//! de la trame et de la période de notification, bornée 1–100 ms), mappé en mémoire
//! noyau (`MmCached`) et mis à zéro ; libéré par `FreeAudioBuffer` /
//! `FreeBufferWithNotification`, jamais avant l'arrêt du flux. `KSSTATE_STOP` conserve
//! le tampon : PortCls le libère lui-même.
//!
//! # Position
//!
//! `frames = position.frames_at(clock, qpc_now)` puis `byte_offset(frames, frame_bytes,
//! bytes)` : exact à la trame, sans compteur ni timer. `RUN` mémorise l'origine,
//! `PAUSE`/`ACQUIRE` depuis `RUN` accumule, `STOP` remet à zéro.
//!
//! # Notifications
//!
//! `AllocateBufferWithNotification(count)` fixe `count` ∈ {1, 2} périodes par tour ;
//! `RegisterNotificationEvent` mémorise jusqu'à [`MAX_NOTIFICATION_EVENTS`] `KEVENT`.
//! Un timer périodique de [`NOTIFICATION_TIMER_PERIOD_MS`] ([`crate::timer`]) est armé
//! quand le flux est en `RUN` avec un tampon à notification et au moins un événement,
//! désarmé sinon ; sa DPC calcule la position et signale les événements (`KeSetEvent`)
//! quand une frontière de période a été franchie (`conduit_kmd_core::notify`). En
//! M1a-07 le timer est par flux ; M1a-08 le remontera au câble pour la copie.
//!
//! # Destruction
//!
//! Le dernier `Release` (PortCls, `PASSIVE_LEVEL`, fermeture de la broche) déclenche
//! [`Drop`] : timer annulé et DPC vidées, retrait de l'emplacement du câble, libération
//! d'un tampon qui serait encore alloué (ne devrait pas arriver : journalisé).

use core::ptr::{self, NonNull};

use conduit_kmd_core::{Notifier, SupportedFormat, buffer_bytes, buffer_bytes_for_notifications};
use portcls::conduit_com::{
    NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
    STATUS_UNSUCCESSFUL,
};
use portcls::{
    AudioBuffer, MiniportWaveRTStream, MiniportWaveRTStreamNotification, PortWaveRTStream,
    physical_address,
};
use portcls_sys::{_MEMORY_CACHING_TYPE, KSRTAUDIO_HWLATENCY, KSSTATE, PKEVENT, PMDL};
use wdk_sys::ntddk::KeSetEvent;
use wdk_sys::{
    KEVENT, PAGE_SIZE, PKDPC, PVOID, STATUS_DEVICE_BUSY, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_NOT_FOUND,
};

use crate::cable::{Buffer, Cable, Direction, MAX_NOTIFICATION_EVENTS, SharedStream, StreamState};
use crate::clock;
use crate::timer::PeriodicTimer;

/// Période du timer de notification, en millisecondes (SYSVAD : 1 ms ; le tampon fait
/// au moins 1 ms, donc au plus une frontière par tick).
pub const NOTIFICATION_TIMER_PERIOD_MS: u32 = 1;

/// Borne haute d'adresse physique pour `AllocatePagesForMdl` : aucune contrainte pour
/// un périphérique virtuel. `i64::MAX` plutôt que `0xFFFF_FFFF_FFFF_FFFF`, qui serait
/// −1 dans le `QuadPart` signé de `PHYSICAL_ADDRESS`.
const HIGHEST_PHYSICAL_ADDRESS: i64 = i64::MAX;

/// Flux rendu d'un câble : voir la documentation du module.
#[derive(Debug)]
pub struct RenderStream {
    /// Numéro du câble (journalisation).
    n: u32,
    /// Câble dont l'emplacement rendu désigne `shared` tant que le flux vit.
    cable: &'static Cable,
    /// Objet d'aide de PortCls : allocation, mappage et libération du tampon.
    port_stream: PortWaveRTStream,
    /// Format retenu par `NewStream`.
    format: SupportedFormat,
    /// Octets par trame de `format`.
    frame_bytes: u32,
    /// État verrouillé (câble et DPC y accèdent).
    shared: SharedStream,
    /// Timer de notification (M1a-07 : par flux).
    timer: PeriodicTimer,
}

impl RenderStream {
    /// Flux à l'arrêt pour le câble `cable` (numéro `n`), au format `format`. Lit la
    /// fréquence du compteur de performance une fois pour toutes.
    ///
    /// Erreurs : `STATUS_INVALID_PARAMETER` si le format n'a pas de disposition de
    /// trame, `STATUS_UNSUCCESSFUL` si la fréquence QPC est nulle.
    ///
    /// L'objet rendu doit ensuite être placé à son adresse définitive (objet COM) puis
    /// [`attach`](Self::attach) avant d'être remis à PortCls.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn new(
        n: u32,
        cable: &'static Cable,
        port_stream: PortWaveRTStream,
        format: SupportedFormat,
    ) -> Result<Self, NtStatus> {
        let frame_bytes = format
            .layout()
            .ok_or(STATUS_INVALID_PARAMETER)?
            .frame_bytes();
        let clock = clock::virtual_clock(format.sample_rate).ok_or(STATUS_UNSUCCESSFUL)?;
        Ok(Self {
            n,
            cable,
            port_stream,
            format,
            frame_bytes,
            shared: SharedStream::new(StreamState::new(clock, frame_bytes)),
            timer: PeriodicTimer::new(),
        })
    }

    /// Initialise le timer (contexte : `self`) et inscrit le flux dans l'emplacement
    /// rendu du câble. `STATUS_DEVICE_BUSY` si un flux rendu est déjà ouvert sur ce
    /// câble (le timer est alors initialisé mais jamais armé ; le `Drop` s'en occupe).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `self` est à son adresse définitive (objet COM alloué) et y restera jusqu'à son
    /// `Drop`, qui annule le timer et vide les DPC avant toute libération.
    pub unsafe fn attach(&self) -> Result<(), NtStatus> {
        let context: PVOID = ptr::from_ref(self).cast_mut().cast();
        // SAFETY: `self` est à son adresse définitive jusqu'au `Drop` (contrat), et le
        // `Drop` appelle `stop_and_flush` avant de libérer quoi que ce soit : `context`
        // reste valide tant que la DPC peut s'exécuter.
        unsafe { self.timer.init(notification_dpc, context) };
        self.cable
            .attach(Direction::Render, NonNull::from(&self.shared))
            .map_err(|_| STATUS_DEVICE_BUSY)
    }

    /// Arme ou désarme le timer selon l'état : armé en `RUN` avec un tampon à
    /// notification et au moins un événement enregistré.
    ///
    /// IRQL : `<= DISPATCH_LEVEL` (appelé sous le verrou du flux).
    fn refresh_timer(&self, state: &StreamState) {
        let wanted = state.is_running()
            && state.buffer.is_some()
            && state.notifier.is_some()
            && state.has_events();
        if wanted {
            self.timer.start(NOTIFICATION_TIMER_PERIOD_MS);
        } else {
            self.timer.stop();
        }
    }

    /// Tick du timer : signale les événements si une frontière de période a été
    /// franchie depuis le dernier signal.
    ///
    /// IRQL : `DISPATCH_LEVEL` (DPC).
    fn on_timer(&self, qpc_now: u64) {
        let mut guard = self.shared.lock();
        let state: &mut StreamState = &mut guard;
        if !state.is_running() || state.buffer.is_none() {
            return;
        }
        let frames = state.frames_at(qpc_now);
        let Some(notifier) = state.notifier.as_mut() else {
            return;
        };
        if !notifier.advance(frames) {
            return;
        }
        for event in state.events.iter().flatten() {
            // SAFETY: l'événement est un `KEVENT` non paginé que PortCls garde vivant
            // jusqu'au `UnregisterNotificationEvent` correspondant, lequel le retire de
            // `events` sous ce même verrou ; `Wait = FALSE` : autorisé à `DISPATCH_LEVEL`.
            unsafe { KeSetEvent(event.as_ptr(), 0, 0) };
        }
    }

    /// Alloue le tampon cyclique (voir le module), avec `notification_count` périodes
    /// de notification par tour (`None` : `AllocateAudioBuffer`, sans notification).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn allocate(
        &self,
        notification_count: Option<u32>,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        let rate = self.format.sample_rate;
        let bytes = match notification_count {
            None => buffer_bytes(requested_bytes, self.frame_bytes, rate),
            Some(count) => {
                if !(1..=2).contains(&count) {
                    return Err(STATUS_INVALID_PARAMETER);
                }
                buffer_bytes_for_notifications(requested_bytes, self.frame_bytes, rate, count)
            }
        }
        .ok_or(STATUS_UNSUCCESSFUL)?;
        // `frame_bytes ≠ 0` (disposition valide) et `bytes` en est un multiple.
        let frames = bytes
            .checked_div(self.frame_bytes)
            .ok_or(STATUS_UNSUCCESSFUL)?;
        let notifier = match notification_count {
            None => None,
            Some(count) => Some(Notifier::new(frames, count).ok_or(STATUS_UNSUCCESSFUL)?),
        };

        // Un seul tampon par flux ; vérifié avant l'allocation (hors verrou pendant
        // celle-ci) et de nouveau à l'inscription.
        if self.shared.lock().buffer.is_some() {
            return Err(STATUS_INVALID_DEVICE_REQUEST);
        }

        let size = usize::try_from(bytes).map_err(|_| STATUS_UNSUCCESSFUL)?;
        let mdl = self
            .port_stream
            .allocate_pages_for_mdl(physical_address(HIGHEST_PHYSICAL_ADDRESS), size)?;
        // L'allocation peut être partielle : vérifier que les pages couvrent `bytes`.
        // SAFETY: `mdl` vient d'être rendue par `allocate_pages_for_mdl` de ce flux.
        let pages = match unsafe { self.port_stream.physical_pages_count(mdl) } {
            Ok(pages) => pages,
            Err(status) => {
                // SAFETY: `mdl` est vivante, non mappée, et n'est plus utilisée après.
                let _ = unsafe { self.port_stream.free_pages_from_mdl(mdl) };
                return Err(status);
            }
        };
        let covered = u64::from(pages).saturating_mul(u64::from(PAGE_SIZE));
        if covered < u64::from(bytes) {
            kmd_log!(
                "RenderStream{}::AllocateAudioBuffer : {pages} page(s) pour {bytes} octets",
                self.n
            );
            // SAFETY: idem.
            let _ = unsafe { self.port_stream.free_pages_from_mdl(mdl) };
            return Err(STATUS_INSUFFICIENT_RESOURCES);
        }
        // SAFETY: `mdl` est une MDL vivante de ce flux, pas encore mappée.
        let base = match unsafe {
            self.port_stream
                .map_allocated_pages(mdl, _MEMORY_CACHING_TYPE::MmCached)
        } {
            Ok(base) => base.cast::<u8>(),
            Err(status) => {
                // SAFETY: idem.
                let _ = unsafe { self.port_stream.free_pages_from_mdl(mdl) };
                return Err(status);
            }
        };
        // SAFETY: `base` est le début d'un mappage noyau d'au moins `bytes` octets
        // (pages vérifiées), inscriptible, que personne d'autre ne voit encore.
        unsafe { ptr::write_bytes(base, 0, size) };
        let buffer = Buffer { mdl, base, bytes };

        {
            let mut state = self.shared.lock();
            if state.buffer.is_some() {
                drop(state);
                // SAFETY: `buffer` vient d'être alloué et mappé par ce flux, et n'est
                // inscrit nulle part.
                unsafe { self.release_buffer(buffer) };
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            state.buffer = Some(buffer);
            state.notifier = notifier;
            self.refresh_timer(&state);
        }
        kmd_log!(
            "RenderStream{}::AllocateAudioBuffer : {bytes} octets ({frames} trames, demandé {requested_bytes}, notifications {notification_count:?})",
            self.n
        );
        Ok(AudioBuffer {
            mdl,
            actual_bytes: bytes,
            offset_from_first_page: 0,
            cache_type: _MEMORY_CACHING_TYPE::MmCached,
        })
    }

    /// Démappe et libère `buffer`.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `buffer` a été alloué et mappé par ce flux, n'est plus inscrit dans l'état (aucune
    /// DPC ni méthode ne peut l'atteindre) et n'est plus utilisé après l'appel.
    unsafe fn release_buffer(&self, buffer: Buffer) {
        // SAFETY: `base`/`mdl` forment un mappage vivant de ce flux, plus référencé
        // (contrat).
        if let Err(status) = unsafe {
            self.port_stream
                .unmap_allocated_pages(buffer.base.cast(), buffer.mdl)
        } {
            kmd_log!(
                "RenderStream{}::UnmapAllocatedPages : {status:#010x}",
                self.n
            );
        }
        // SAFETY: `mdl` est démappée et cédée ici (contrat).
        if let Err(status) = unsafe { self.port_stream.free_pages_from_mdl(buffer.mdl) } {
            kmd_log!("RenderStream{}::FreePagesFromMdl : {status:#010x}", self.n);
        }
    }

    /// Libère le tampon rendu par PortCls, si le flux est à l'arrêt.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn free(&self, mdl: PMDL, size: u32) {
        let (taken, ks_state) = {
            let mut state = self.shared.lock();
            if state.state != KSSTATE::KSSTATE_STOP {
                (None, state.state)
            } else {
                let taken = state.buffer.take();
                state.notifier = None;
                self.refresh_timer(&state);
                (taken, state.state)
            }
        };
        match taken {
            Some(buffer) => {
                if !ptr::eq(buffer.mdl, mdl) || buffer.bytes != size {
                    kmd_log!(
                        "RenderStream{}::FreeAudioBuffer : MDL {mdl:p}/{size} ≠ tampon {:p}/{}",
                        self.n,
                        buffer.mdl,
                        buffer.bytes
                    );
                }
                // SAFETY: `buffer` a été retiré de l'état sous le verrou : plus personne
                // ne l'atteint.
                unsafe { self.release_buffer(buffer) };
                kmd_log!(
                    "RenderStream{}::FreeAudioBuffer : {size} octets libérés",
                    self.n
                );
            }
            None => {
                kmd_log!(
                    "RenderStream{}::FreeAudioBuffer ignoré : état {ks_state}, MDL {mdl:p} (tampon conservé jusqu'à la fermeture)",
                    self.n
                );
            }
        }
    }
}

impl Drop for RenderStream {
    // IRQL: PASSIVE_LEVEL (dernier `Release`, fermeture de la broche).
    fn drop(&mut self) {
        // 1. Plus aucune DPC de ce flux après le retour.
        self.timer.stop_and_flush();
        // 2. Plus aucun lecteur du câble ne détient le pointeur d'état après le retour.
        let detached = self
            .cable
            .detach(Direction::Render, NonNull::from(&self.shared));
        // 3. Un tampon encore alloué ne devrait pas exister (PortCls appelle
        //    `FreeAudioBuffer` avant de fermer) : le libérer plutôt que le fuir.
        let leftover = {
            let mut state = self.shared.lock();
            state.notifier = None;
            state.buffer.take()
        };
        if let Some(buffer) = leftover {
            kmd_log!(
                "RenderStream{}::Drop : tampon de {} octets encore alloué, libéré",
                self.n,
                buffer.bytes
            );
            // SAFETY: retiré de l'état, timer arrêté, DPC vidées, emplacement libéré.
            unsafe { self.release_buffer(buffer) };
        }
        kmd_log!(
            "RenderStream{}::Drop (emplacement {})",
            self.n,
            if detached { "libéré" } else { "déjà vide" }
        );
    }
}

impl MiniportWaveRTStream for RenderStream {
    // IRQL: PASSIVE_LEVEL
    fn set_state(&self, state: KSSTATE::Type) -> NtStatus {
        if !matches!(
            state,
            KSSTATE::KSSTATE_STOP
                | KSSTATE::KSSTATE_ACQUIRE
                | KSSTATE::KSSTATE_PAUSE
                | KSSTATE::KSSTATE_RUN
        ) {
            return STATUS_INVALID_PARAMETER;
        }
        let now = clock::now();
        let previous = {
            let mut guard = self.shared.lock();
            let shared: &mut StreamState = &mut guard;
            let previous = shared.state;
            let clock = shared.clock;
            match state {
                KSSTATE::KSSTATE_RUN => shared.position.run(now),
                KSSTATE::KSSTATE_STOP => {
                    shared.position.reset();
                    if let Some(notifier) = shared.notifier.as_mut() {
                        notifier.reset();
                    }
                }
                // PAUSE ou ACQUIRE : depuis RUN, accumuler ; sinon sans effet.
                _ => shared.position.pause(&clock, now),
            }
            shared.state = state;
            self.refresh_timer(shared);
            previous
        };
        // Transitions attendues : voisines (STOP ↔ ACQUIRE ↔ PAUSE ↔ RUN). Les autres
        // sont tolérées, comme SYSVAD, et journalisées.
        if previous.abs_diff(state) > 1 {
            kmd_log!(
                "RenderStream{}::SetState {previous} → {state} (transition inhabituelle)",
                self.n
            );
        } else {
            kmd_log!("RenderStream{}::SetState {previous} → {state}", self.n);
        }
        STATUS_SUCCESS
    }

    // IRQL: <= DISPATCH_LEVEL
    fn position(&self) -> Result<u32, NtStatus> {
        let now = clock::now();
        let state = self.shared.lock();
        Ok(state.byte_position_at(now))
    }

    // IRQL: PASSIVE_LEVEL
    fn allocate_audio_buffer(&self, requested_bytes: u32) -> Result<AudioBuffer, NtStatus> {
        self.allocate(None, requested_bytes)
    }

    // IRQL: PASSIVE_LEVEL
    fn free_audio_buffer(&self, mdl: PMDL, size: u32) {
        self.free(mdl, size);
    }

    // IRQL: PASSIVE_LEVEL
    fn hw_latency(&self, out: &mut KSRTAUDIO_HWLATENCY) {
        // Aucune latence matérielle pour un câble virtuel.
        out.FifoSize = 0;
        out.ChipsetDelay = 0;
        out.CodecDelay = 0;
    }
}

impl MiniportWaveRTStreamNotification for RenderStream {
    // IRQL: PASSIVE_LEVEL
    fn allocate_buffer_with_notification(
        &self,
        notification_count: u32,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        self.allocate(Some(notification_count), requested_bytes)
    }

    // IRQL: PASSIVE_LEVEL
    fn free_buffer_with_notification(&self, mdl: PMDL, size: u32) {
        self.free(mdl, size);
    }

    // IRQL: PASSIVE_LEVEL
    fn register_notification_event(&self, event: PKEVENT) -> NtStatus {
        let Some(event) = NonNull::new(event.cast::<KEVENT>()) else {
            return STATUS_INVALID_PARAMETER;
        };
        let status = {
            let mut guard = self.shared.lock();
            let state: &mut StreamState = &mut guard;
            if state.events.contains(&Some(event)) {
                // Déjà enregistré : SYSVAD répond `STATUS_UNSUCCESSFUL`.
                STATUS_UNSUCCESSFUL
            } else if let Some(slot) = state.events.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(event);
                self.refresh_timer(state);
                STATUS_SUCCESS
            } else {
                STATUS_INSUFFICIENT_RESOURCES
            }
        };
        kmd_log!(
            "RenderStream{}::RegisterNotificationEvent {event:p} : {status:#010x} (max {MAX_NOTIFICATION_EVENTS})",
            self.n
        );
        status
    }

    // IRQL: PASSIVE_LEVEL
    fn unregister_notification_event(&self, event: PKEVENT) -> NtStatus {
        let Some(event) = NonNull::new(event.cast::<KEVENT>()) else {
            return STATUS_INVALID_PARAMETER;
        };
        let status = {
            let mut guard = self.shared.lock();
            let state: &mut StreamState = &mut guard;
            match state.events.iter_mut().find(|slot| **slot == Some(event)) {
                Some(slot) => {
                    *slot = None;
                    self.refresh_timer(state);
                    STATUS_SUCCESS
                }
                None => STATUS_NOT_FOUND,
            }
        };
        kmd_log!(
            "RenderStream{}::UnregisterNotificationEvent {event:p} : {status:#010x}",
            self.n
        );
        status
    }
}

/// Routine DPC du timer de notification : `context` est le [`RenderStream`].
///
/// IRQL : `DISPATCH_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par le noyau pour le timer initialisé par
/// [`RenderStream::attach`], avec le `context` qui lui a été remis ; le flux est vivant
/// tant que son `Drop` n'a pas terminé `stop_and_flush`.
unsafe extern "C" fn notification_dpc(
    _dpc: PKDPC,
    context: PVOID,
    _argument1: PVOID,
    _argument2: PVOID,
) {
    let Some(stream) = NonNull::new(context.cast::<RenderStream>()) else {
        return;
    };
    // SAFETY: `context` est le `RenderStream` à son adresse définitive, vivant pendant
    // toute DPC en cours (contrat de la fonction : `Drop` attend `KeFlushQueuedDpcs`).
    let stream = unsafe { stream.as_ref() };
    stream.on_timer(clock::now());
}
