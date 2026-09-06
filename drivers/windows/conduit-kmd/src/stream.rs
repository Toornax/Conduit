//! Flux WaveRT d'un câble ([`WaveStream`], driver-design.md §5) : l'objet que
//! `WaveRender::NewStream` ou `WaveCapture::NewStream` crée pour la broche système, avec
//! son tampon cyclique (§5.2), sa position calculée par l'horloge (§5.1) et ses
//! notifications (§5.3, étape 5). Implémente `portcls::MiniportWaveRTStreamNotification`,
//! donc aussi `MiniportWaveRTStream`.
//!
//! Un seul type pour les deux sens : rendu et capture ne diffèrent que par leur
//! [`Direction`] — l'emplacement du câble qu'ils occupent, et le rôle que la boucle
//! locale leur donne ([`crate::cable::Cable::on_tick`] lit le tampon du rendu et écrit
//! celui de la capture). Côté PortCls, les deux répondent exactement pareil : c'est le
//! moteur audio qui écrit dans le tampon du rendu et lit celui de la capture.
//!
//! # État et verrouillage
//!
//! Tout l'état mutable est dans un [`SharedStream`] (`SpinLock<StreamState>`,
//! [`crate::cable`]) : état KS, position, tampon, périodes de notification, événements
//! enregistrés. Les méthodes PortCls (`PASSIVE_LEVEL`) et `GetPosition`
//! (`<= DISPATCH_LEVEL`) le prennent brièvement ; le tick du câble (`DISPATCH_LEVEL`)
//! aussi. Ce qui exige `PASSIVE_LEVEL` (allocation, mappage et libération des pages par
//! `IPortWaveRTStream`) se fait **hors** du verrou, et [`Cable::refresh_timer`] aussi :
//! il prend le verrou du câble, qui précède toujours celui d'un flux.
//!
//! # Tampon
//!
//! Un seul tampon par flux, alloué par `IPortWaveRTStream::AllocatePagesForMdl` (taille de
//! `conduit_kmd_core::format::buffer_bytes[_for_notifications]` : multiple de la trame et
//! de la période de notification, jamais inférieure à la demande — au-delà de 500 ms
//! l'allocation est refusée plutôt qu'écrêtée), mappé en mémoire noyau (`MmCached`) et mis
//! à zéro ; libéré par `FreeAudioBuffer` / `FreeBufferWithNotification`, **sans condition
//! d'état** : c'est PortCls qui décide du moment, le miniport libère. `SetState` ne touche
//! pas au tampon — `KSSTATE_STOP` le conserve, en attendant que PortCls le rende.
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
//! Depuis M1a-08 le flux n'a **plus de timer à lui** : c'est le timer haute résolution du
//! câble (1 ms, [`crate::timer::ExTimer`]) qui, à chaque tick, calcule la position et
//! signale les événements (`KeSetEvent`) quand une frontière de période a été franchie
//! (`conduit_kmd_core::notify`). Le flux se contente de demander au câble de réviser
//! l'armement du timer après chaque transition ([`Cable::refresh_timer`]).
//!
//! # Destruction
//!
//! Le dernier `Release` (PortCls, `PASSIVE_LEVEL`, fermeture de la broche) déclenche
//! [`Drop`] : retrait de l'emplacement du câble — qui garantit, sous le spin lock du
//! câble, qu'aucun tick ne détient plus le pointeur —, révision du timer, puis libération
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
use wdk_sys::{
    KEVENT, PAGE_SIZE, STATUS_DEVICE_BUSY, STATUS_INVALID_DEVICE_REQUEST, STATUS_NOT_FOUND,
};

use crate::cable::{Buffer, Cable, Direction, MAX_NOTIFICATION_EVENTS, SharedStream, StreamState};
use crate::clock;

/// Borne haute d'adresse physique pour `AllocatePagesForMdl` : aucune contrainte pour
/// un périphérique virtuel. `i64::MAX` plutôt que `0xFFFF_FFFF_FFFF_FFFF`, qui serait
/// −1 dans le `QuadPart` signé de `PHYSICAL_ADDRESS`.
const HIGHEST_PHYSICAL_ADDRESS: i64 = i64::MAX;

/// Flux d'un câble, dans un sens ou dans l'autre : voir la documentation du module.
#[derive(Debug)]
pub struct WaveStream {
    /// Numéro du câble (journalisation).
    n: u32,
    /// Sens du flux : l'emplacement du câble qu'il occupe.
    direction: Direction,
    /// Câble dont l'emplacement `direction` désigne `shared` tant que le flux vit.
    cable: &'static Cable,
    /// Objet d'aide de PortCls : allocation, mappage et libération du tampon.
    port_stream: PortWaveRTStream,
    /// Format retenu par `NewStream`.
    format: SupportedFormat,
    /// Octets par trame de `format`.
    frame_bytes: u32,
    /// État verrouillé (le tick du câble y accède).
    shared: SharedStream,
}

impl WaveStream {
    /// Flux à l'arrêt du sens `direction` pour le câble `cable` (numéro `n`), au format
    /// `format`. Lit la fréquence du compteur de performance une fois pour toutes.
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
        direction: Direction,
        cable: &'static Cable,
        port_stream: PortWaveRTStream,
        format: SupportedFormat,
    ) -> Result<Self, NtStatus> {
        let layout = format.layout().ok_or(STATUS_INVALID_PARAMETER)?;
        let clock = clock::virtual_clock(format.sample_rate).ok_or(STATUS_UNSUCCESSFUL)?;
        Ok(Self {
            n,
            direction,
            cable,
            port_stream,
            format,
            frame_bytes: layout.frame_bytes(),
            shared: SharedStream::new(StreamState::new(clock, layout)),
        })
    }

    /// Nom du flux pour la journalisation (`RenderStream` ou `CaptureStream`).
    fn name(&self) -> &'static str {
        self.direction.stream_name()
    }

    /// Inscrit le flux dans l'emplacement `direction` du câble. `STATUS_DEVICE_BUSY` si
    /// un flux de ce sens est déjà ouvert sur ce câble.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `self` est à son adresse définitive (objet COM alloué) et y restera jusqu'à son
    /// `Drop`, qui se retire de l'emplacement avant toute libération.
    pub unsafe fn attach(&self) -> Result<(), NtStatus> {
        self.cable
            .attach(self.direction, NonNull::from(&self.shared))
            .map_err(|_| STATUS_DEVICE_BUSY)
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
        // Journaliser l'ENTRÉE, pas seulement le succès : le moteur audio demande d'abord
        // un tampon avec notifications, et un refus de notre part le fait retomber
        // silencieusement en mode scrutation. C'est exactement ce qui a coûté une journée
        // de diagnostic le 2026-09-06 : la trace ne montrait que « notifications None »,
        // sans dire qu'une demande avec notifications avait été refusée juste avant.
        kmd_log!(
            "{}{}::Allocate(notifications {notification_count:?}, {requested_bytes} octets)",
            self.name(),
            self.n
        );
        let rate = self.format.sample_rate;
        let bytes = match notification_count {
            None => buffer_bytes(requested_bytes, self.frame_bytes, rate),
            // La documentation d'`AllocateBufferWithNotification` annonce « Valid values
            // are 1 or 2 », et nous n'avons jamais rien observé d'autre — l'absence
            // d'observation ne se cite pas comme une mesure. Nous restons pourtant
            // permissifs comme SYSVAD, qui n'exige que la divisibilité de la taille :
            // une borne arbitraire ici faisait échouer le mode événementiel sans laisser
            // la moindre trace, et refuser plus que ce que le contrat impose n'a rien
            // rapporté.
            Some(count) => {
                buffer_bytes_for_notifications(requested_bytes, self.frame_bytes, rate, count)
            }
        };
        let Some(bytes) = bytes else {
            kmd_log!(
                "{}{}::Allocate refusé : taille impossible pour {requested_bytes} octets, trame {}, {notification_count:?} période(s)",
                self.name(),
                self.n,
                self.frame_bytes
            );
            return Err(STATUS_UNSUCCESSFUL);
        };
        // `frame_bytes ≠ 0` (disposition valide) et `bytes` en est un multiple.
        let Some(frames) = bytes.checked_div(self.frame_bytes) else {
            kmd_log!("{}{}::Allocate refusé : trame nulle", self.name(), self.n);
            return Err(STATUS_UNSUCCESSFUL);
        };
        let notifier = match notification_count {
            None => None,
            Some(count) => match Notifier::new(frames, count) {
                Some(n) => Some(n),
                None => {
                    kmd_log!(
                        "{}{}::Allocate refusé : {frames} trames indivisibles par {count} période(s)",
                        self.name(),
                        self.n
                    );
                    return Err(STATUS_UNSUCCESSFUL);
                }
            },
        };

        // Un seul tampon par flux ; vérifié avant l'allocation (hors verrou pendant
        // celle-ci) et de nouveau à l'inscription.
        if self.shared.lock().buffer.is_some() {
            kmd_log!(
                "{}{}::Allocate refusé : un tampon est déjà alloué pour ce flux",
                self.name(),
                self.n
            );
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
                "{}{}::AllocateAudioBuffer : {pages} page(s) pour {bytes} octets",
                self.name(),
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
        }
        // Hors du verrou du flux : `refresh_timer` prend celui du câble, qui le précède.
        self.cable.refresh_timer();
        kmd_log!(
            "{}{}::AllocateAudioBuffer : {bytes} octets ({frames} trames, demandé {requested_bytes}, notifications {notification_count:?})",
            self.name(),
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
    /// `buffer` a été alloué et mappé par ce flux, n'est plus inscrit dans l'état (aucun
    /// tick ni méthode ne peut l'atteindre) et n'est plus utilisé après l'appel.
    unsafe fn release_buffer(&self, buffer: Buffer) {
        // SAFETY: `base`/`mdl` forment un mappage vivant de ce flux, plus référencé
        // (contrat).
        if let Err(status) = unsafe {
            self.port_stream
                .unmap_allocated_pages(buffer.base.cast(), buffer.mdl)
        } {
            kmd_log!(
                "{}{}::UnmapAllocatedPages : {status:#010x}",
                self.name(),
                self.n
            );
        }
        // SAFETY: `mdl` est démappée et cédée ici (contrat).
        if let Err(status) = unsafe { self.port_stream.free_pages_from_mdl(buffer.mdl) } {
            kmd_log!(
                "{}{}::FreePagesFromMdl : {status:#010x}",
                self.name(),
                self.n
            );
        }
    }

    /// Libère le tampon rendu par PortCls, **quel que soit l'état du flux**.
    ///
    /// La documentation de `FreeAudioBuffer` / `FreeBufferWithNotification` ne pose
    /// aucune condition d'état : quand PortCls appelle, il considère la MDL rendue et le
    /// miniport libère. Conditionner la libération à `KSSTATE_STOP` laissait deux dégâts :
    /// le tampon restait inscrit dans l'état, donc `allocate` refusait toute réallocation
    /// avec `STATUS_INVALID_DEVICE_REQUEST` — flux définitivement inutilisable — et la
    /// propriété de la MDL divergeait (rendue pour PortCls, gardée jusqu'au `Drop` pour
    /// nous). Un appel hors `KSSTATE_STOP` reste journalisé : la trace est intéressante,
    /// la garde ne l'était pas.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn free(&self, mdl: PMDL, size: u32) {
        let (taken, ks_state) = {
            let mut state = self.shared.lock();
            let taken = state.buffer.take();
            state.notifier = None;
            (taken, state.state)
        };
        // Hors du verrou du flux (ordre câble puis flux).
        self.cable.refresh_timer();
        if ks_state != KSSTATE::KSSTATE_STOP {
            kmd_log!(
                "{}{}::FreeAudioBuffer hors arrêt : état {ks_state} (tampon libéré quand même)",
                self.name(),
                self.n
            );
        }
        match taken {
            Some(buffer) => {
                if !ptr::eq(buffer.mdl, mdl) || buffer.bytes != size {
                    kmd_log!(
                        "{}{}::FreeAudioBuffer : MDL {mdl:p}/{size} ≠ tampon {:p}/{}",
                        self.name(),
                        self.n,
                        buffer.mdl,
                        buffer.bytes
                    );
                }
                // SAFETY: `buffer` a été retiré de l'état sous le verrou : plus personne
                // ne l'atteint.
                unsafe { self.release_buffer(buffer) };
                kmd_log!(
                    "{}{}::FreeAudioBuffer : {size} octets libérés",
                    self.name(),
                    self.n
                );
            }
            None => {
                kmd_log!(
                    "{}{}::FreeAudioBuffer : aucun tampon inscrit, MDL {mdl:p}/{size} (double libération ?)",
                    self.name(),
                    self.n
                );
            }
        }
    }
}

impl Drop for WaveStream {
    // IRQL: PASSIVE_LEVEL (dernier `Release`, fermeture de la broche).
    fn drop(&mut self) {
        // 1. Plus aucun tick du câble ne détient le pointeur d'état après le retour :
        //    `detach` prend le spin lock du câble, que `on_tick` tient pendant tout son
        //    usage des emplacements (contrat de `cable`).
        let detached = self
            .cable
            .detach(self.direction, NonNull::from(&self.shared));
        // 2. Désarmer le timer si plus aucun flux du câble ne tourne.
        self.cable.refresh_timer();
        // 3. Un tampon encore alloué ne devrait pas exister (PortCls appelle
        //    `FreeAudioBuffer` avant de fermer) : le libérer plutôt que le fuir.
        let leftover = {
            let mut state = self.shared.lock();
            state.notifier = None;
            state.buffer.take()
        };
        if let Some(buffer) = leftover {
            kmd_log!(
                "{}{}::Drop : tampon de {} octets encore alloué, libéré",
                self.name(),
                self.n,
                buffer.bytes
            );
            // SAFETY: retiré de l'état, emplacement libéré (plus aucun tick ne peut
            // atteindre ce flux).
            unsafe { self.release_buffer(buffer) };
        }
        kmd_log!(
            "{}{}::Drop (emplacement {})",
            self.name(),
            self.n,
            if detached { "libéré" } else { "déjà vide" }
        );
    }
}

impl MiniportWaveRTStream for WaveStream {
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
            previous
        };
        // Hors du verrou du flux : `refresh_timer` prend celui du câble, qui le précède.
        self.cable.refresh_timer();
        // Transitions attendues : voisines (STOP ↔ ACQUIRE ↔ PAUSE ↔ RUN). Les autres
        // sont tolérées, comme SYSVAD, et journalisées.
        if previous.abs_diff(state) > 1 {
            kmd_log!(
                "{}{}::SetState {previous} → {state} (transition inhabituelle)",
                self.name(),
                self.n
            );
        } else {
            kmd_log!("{}{}::SetState {previous} → {state}", self.name(), self.n);
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

impl MiniportWaveRTStreamNotification for WaveStream {
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
                STATUS_SUCCESS
            } else {
                STATUS_INSUFFICIENT_RESOURCES
            }
        };
        kmd_log!(
            "{}{}::RegisterNotificationEvent {event:p} : {status:#010x} (max {MAX_NOTIFICATION_EVENTS})",
            self.name(),
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
                    STATUS_SUCCESS
                }
                None => STATUS_NOT_FOUND,
            }
        };
        kmd_log!(
            "{}{}::UnregisterNotificationEvent {event:p} : {status:#010x}",
            self.name(),
            self.n
        );
        status
    }
}
