//! État partagé d'un câble (driver-design.md §2.3, §5.3) : les flux rendu et capture
//! courants, que la boucle locale (M1a-08) lira sous le timer/DPC du câble, et l'état
//! de chaque flux tel que le câble le voit ([`StreamState`]).
//!
//! Un [`Cable`] par câble ; ses deux emplacements ([`Slots`], un par sens) mémorisent
//! le pointeur vers l'état verrouillé ([`SharedStream`]) du flux ouvert sur la broche
//! système du filtre WaveRT correspondant, nul quand la broche est fermée. Les miniports
//! WaveRT (`wave::WaveRender`, `wave::WaveCapture`) reçoivent un `&'static Cable` à leur
//! création dans `StartDevice` (`adapter`) ; `NewStream` y inscrit le flux qu'il crée
//! ([`Cable::attach`]) et le `Drop` du flux l'en retire ([`Cable::detach`]).
//!
//! # Durée de vie et allocation
//!
//! Le câble du spike est une **`static`** ([`CABLE_0`]) : sa construction est `const`,
//! il vit dans la section de données du pilote — non paginée, comme tout le binaire
//! d'un pilote WDM qui ne marque pas ses sections `PAGE` —, depuis le chargement jusqu'au
//! déchargement, sans allocation, sans fuite de pool à déclarer à Driver Verifier, sans
//! `Drop`. Les cycles `StartDevice`/`StopDevice` le réutilisent ([`Cable::reset`]).
//! M1b-02 en fera un tableau de `MAX_CABLES` entrées, toujours `static`.
//!
//! # Synchronisation et contrat des emplacements
//!
//! Les emplacements sont protégés par le **spin lock du câble** ([`Cable::slots`]) :
//!
//! - le pointeur d'un emplacement n'est valide que **tant que le flux vit** ; il désigne
//!   le `SpinLock<StreamState>` logé dans l'objet COM du flux (pool non paginé) ;
//! - le flux se retire de l'emplacement **avant** sa destruction (`Drop` de
//!   `stream::RenderStream`, après avoir arrêté son propre timer), en prenant ce même
//!   spin lock : un lecteur qui tient la garde des emplacements a donc la garantie que
//!   le flux qu'elle désigne ne sera pas détruit avant qu'il la relâche ;
//! - tout lecteur (la DPC de copie de M1a-08) doit **tenir la garde des emplacements
//!   pendant tout son usage du pointeur**, y compris pendant qu'il prend le verrou du
//!   flux. Ordre de verrouillage, fixe : **câble puis flux** ; jamais l'inverse (le
//!   `Drop` du flux ne tient pas son propre verrou en prenant celui du câble).
//!
//! Les sections critiques du câble sont courtes (lecture de deux pointeurs, calcul de
//! position, copie d'une période) : `GetPosition` d'un flux ne prend que le verrou du
//! flux, pas celui du câble.

use core::fmt;
use core::ptr::{self, NonNull};

use conduit_kmd_core::{Notifier, StreamPosition, VirtualClock, byte_offset};
use portcls_sys::{KSSTATE, PMDL};
use wdk_sys::KEVENT;

use crate::sync::{SpinLock, SpinLockGuard};

/// Nombre maximal d'événements de notification par flux
/// (`RegisterNotificationEvent`) : PortCls n'en enregistre qu'un par client, deux
/// laissent une marge (SYSVAD utilise une liste sans borne).
pub const MAX_NOTIFICATION_EVENTS: usize = 2;

/// Tampon cyclique d'un flux, alloué par `IPortWaveRTStream::AllocatePagesForMdl` et
/// mappé en mémoire noyau (driver-design.md §5.2).
#[derive(Debug, Clone, Copy)]
pub struct Buffer {
    /// MDL rendue à PortCls, qui la rend à `FreeAudioBuffer`.
    pub mdl: PMDL,
    /// Adresse virtuelle noyau du premier octet (`MapAllocatedPages`, `MmCached`).
    pub base: *mut u8,
    /// Taille en octets : multiple de la trame (et de la période de notification).
    pub bytes: u32,
}

/// État d'un flux WaveRT ouvert, tel que le câble et les DPC le voient : protégé par
/// le spin lock du flux ([`SharedStream`]), en mémoire non paginée (objet COM du flux).
///
/// Les champs immuables après création (`clock`, `frame_bytes`) y figurent pour que la
/// DPC de copie (M1a-08) calcule la position des deux flux avec le seul pointeur de
/// l'emplacement.
#[derive(Debug)]
pub struct StreamState {
    /// Horloge virtuelle du flux (fréquence QPC lue à la création, fréquence
    /// d'échantillonnage du format retenu).
    pub clock: VirtualClock,
    /// Octets par trame du format retenu.
    pub frame_bytes: u32,
    /// État KS courant (`KSSTATE_STOP` à la création).
    pub state: KSSTATE::Type,
    /// Position en trames (pauses accumulées).
    pub position: StreamPosition,
    /// Tampon cyclique, `None` entre `FreeAudioBuffer` et le prochain
    /// `AllocateAudioBuffer`.
    pub buffer: Option<Buffer>,
    /// Périodes de notification (`AllocateBufferWithNotification`), `None` pour un
    /// tampon sans notification.
    pub notifier: Option<Notifier>,
    /// Événements enregistrés par `RegisterNotificationEvent` (PortCls les garde vivants
    /// jusqu'au `UnregisterNotificationEvent` correspondant, ou jusqu'à la fermeture du
    /// flux).
    pub events: [Option<NonNull<KEVENT>>; MAX_NOTIFICATION_EVENTS],
}

// SAFETY: les pointeurs (`Buffer::mdl`, `Buffer::base`, `events`) désignent de la
// mémoire non paginée possédée par le flux ou par PortCls ; ils ne sont manipulés que
// sous le spin lock du flux, depuis n'importe quel fil ou DPC, sans référence Rust
// durable : les envoyer d'un fil à l'autre est sûr.
unsafe impl Send for StreamState {}

impl StreamState {
    /// Flux à l'arrêt, position 0, sans tampon ni événement.
    pub const fn new(clock: VirtualClock, frame_bytes: u32) -> Self {
        Self {
            clock,
            frame_bytes,
            state: KSSTATE::KSSTATE_STOP,
            position: StreamPosition::new(),
            buffer: None,
            notifier: None,
            events: [None; MAX_NOTIFICATION_EVENTS],
        }
    }

    /// Vrai en `KSSTATE_RUN`.
    pub const fn is_running(&self) -> bool {
        self.state == KSSTATE::KSSTATE_RUN
    }

    /// Position absolue en trames à l'instant `qpc_now`.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn frames_at(&self, qpc_now: u64) -> u64 {
        self.position.frames_at(&self.clock, qpc_now)
    }

    /// Position cyclique en octets dans le tampon à l'instant `qpc_now` ; 0 sans tampon
    /// (ou si les tailles sont incohérentes, ce qui ne peut pas arriver : le tampon est
    /// alloué en multiple de la trame).
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn byte_position_at(&self, qpc_now: u64) -> u32 {
        let Some(buffer) = self.buffer else {
            return 0;
        };
        byte_offset(self.frames_at(qpc_now), self.frame_bytes, buffer.bytes).unwrap_or(0)
    }

    /// Vrai si au moins un événement de notification est enregistré.
    pub fn has_events(&self) -> bool {
        self.events.iter().any(Option::is_some)
    }
}

/// État verrouillé d'un flux : ce que les emplacements du câble désignent.
pub type SharedStream = SpinLock<StreamState>;

/// Sens d'un flux sur un câble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Broche système de `WaveRender<n>` : le lecteur écrit.
    Render,
    /// Broche système de `WaveCapture<n>` : l'enregistreur lit.
    #[expect(dead_code, reason = "inscrit par le flux capture de M1a-08")]
    Capture,
}

/// Les deux emplacements d'un câble, sous son spin lock.
pub struct Slots {
    render: *const SharedStream,
    capture: *const SharedStream,
}

// SAFETY: les pointeurs ne sont lus et écrits que sous le spin lock du câble, et leur
// validité est garantie par le contrat des emplacements (module) : pas d'accès non
// synchronisé possible.
unsafe impl Send for Slots {}

impl Slots {
    const fn new() -> Self {
        Self {
            render: ptr::null_mut(),
            capture: ptr::null_mut(),
        }
    }

    fn slot_mut(&mut self, direction: Direction) -> &mut *const SharedStream {
        match direction {
            Direction::Render => &mut self.render,
            Direction::Capture => &mut self.capture,
        }
    }

    /// Le flux du sens `direction`, s'il y en a un. Valide tant que la garde qui a
    /// donné accès à `self` est détenue (contrat du module).
    #[expect(dead_code, reason = "lu par la DPC de copie de M1a-08")]
    pub fn get(&self, direction: Direction) -> Option<NonNull<SharedStream>> {
        let raw = match direction {
            Direction::Render => self.render,
            Direction::Capture => self.capture,
        };
        NonNull::new(raw.cast_mut())
    }
}

impl fmt::Debug for Slots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slots")
            .field("render", &self.render)
            .field("capture", &self.capture)
            .finish()
    }
}

/// Erreur de [`Cable::attach`] : un flux est déjà ouvert dans ce sens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotOccupied;

/// Un câble : son numéro et ses deux flux courants.
#[derive(Debug)]
pub struct Cable {
    /// Numéro du câble (0 pour le seul câble de M1a ; « Conduit 1 » pour l'utilisateur).
    pub index: u32,
    /// Les emplacements, sous le spin lock du câble.
    slots: SpinLock<Slots>,
}

impl Cable {
    /// Câble `index`, sans flux.
    pub const fn new(index: u32) -> Self {
        Self {
            index,
            slots: SpinLock::new(Slots::new()),
        }
    }

    /// Prend le spin lock du câble et rend les emplacements. Tant que la garde vit,
    /// aucun flux désigné ne peut être détruit (voir le contrat du module) ; l'ordre
    /// de verrouillage est câble puis flux.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    #[expect(dead_code, reason = "lu par la DPC de copie de M1a-08")]
    pub fn slots(&self) -> SpinLockGuard<'_, Slots> {
        self.slots.lock()
    }

    /// Inscrit `state` comme flux du sens `direction`. [`SlotOccupied`] si l'emplacement
    /// est déjà pris (un seul flux par sens : la broche déclare une instance).
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn attach(
        &self,
        direction: Direction,
        state: NonNull<SharedStream>,
    ) -> Result<(), SlotOccupied> {
        let mut slots = self.slots.lock();
        let slot = slots.slot_mut(direction);
        if !slot.is_null() {
            return Err(SlotOccupied);
        }
        *slot = state.as_ptr();
        Ok(())
    }

    /// Retire `state` de l'emplacement du sens `direction` s'il y est encore ; rend
    /// vrai dans ce cas. À appeler par le flux **avant** sa destruction : au retour,
    /// plus aucun lecteur ne détient le pointeur.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn detach(&self, direction: Direction, state: NonNull<SharedStream>) -> bool {
        let mut slots = self.slots.lock();
        let slot = slots.slot_mut(direction);
        if ptr::eq(*slot, state.as_ptr()) {
            *slot = ptr::null_mut();
            true
        } else {
            false
        }
    }

    /// Oublie les deux flux (à l'entrée de `StartDevice` : les broches sont fermées
    /// avant tout arrêt du périphérique, les emplacements devraient déjà être vides).
    /// Rend vrai si l'un d'eux ne l'était pas, pour journalisation.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn reset(&self) -> bool {
        let mut slots = self.slots.lock();
        let occupied = !slots.render.is_null() || !slots.capture.is_null();
        *slots = Slots::new();
        occupied
    }
}

/// Le câble 0, unique câble de M1a.
static CABLE_0: Cable = Cable::new(0);

/// Le câble `index`, ou `None` au-delà du dernier câble (M1a : un seul).
pub fn cable(index: u32) -> Option<&'static Cable> {
    (index == 0).then_some(&CABLE_0)
}

/// Nombre de câbles servis par ce pilote.
pub const CABLE_COUNT: u32 = 1;
