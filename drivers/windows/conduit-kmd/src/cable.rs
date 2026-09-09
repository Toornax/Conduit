//! État partagé d'un câble (driver-design.md §2.3, §5.3) : les flux rendu et capture
//! courants, l'état de la boucle locale, le timer haute résolution qui la fait tourner,
//! et l'état de chaque flux tel que le câble le voit ([`StreamState`]).
//!
//! Un [`Cable`] par câble ; ses deux emplacements ([`CableState`], un par sens) mémorisent
//! le pointeur vers l'état verrouillé ([`SharedStream`]) du flux ouvert sur la broche
//! système du filtre WaveRT correspondant, nul quand la broche est fermée. Les miniports
//! WaveRT (`wave::WaveRender`, `wave::WaveCapture`) reçoivent un `&'static Cable` à leur
//! création dans `StartDevice` (`adapter`) ; `NewStream` y inscrit le flux qu'il crée
//! ([`Cable::attach`]) et le `Drop` du flux l'en retire ([`Cable::detach`]).
//!
//! # Durée de vie et allocation
//!
//! Les câbles sont un **tableau `static`** ([`CABLES`], un élément par câble) : leur
//! construction est `const`, ils vivent dans la section de données du pilote — non
//! paginée, comme tout le binaire d'un pilote WDM qui ne marque pas ses sections
//! `PAGE` —, depuis le chargement jusqu'au déchargement, sans allocation, sans fuite de
//! pool à déclarer à Driver Verifier, sans `Drop`. Les cycles `StartDevice`/`StopDevice`
//! les réutilisent ([`Cable::start`]) ; seuls leurs timers sont alloués par le noyau, une
//! fois au premier `StartDevice` et supprimés au déchargement ([`shutdown`]).
//!
//! # Les trois verbes du cycle de vie (M1b-06)
//!
//! | Verbe | Quand | Timer | `device` |
//! |---|---|---|---|
//! | [`Cable::start`] | `StartDevice` | **créé** s'il ne l'est pas | posé |
//! | [`Cable::suspend`] | `PowerChangeState` hors `D0` | **désarmé** (`ExCancelTimer`) | intact |
//! | [`Cable::stop`] | `DriverUnload`, par [`shutdown`] | **supprimé** (`ExDeleteTimer`, attente) | remis à nul |
//!
//! La distinction entre les deux derniers est le cœur de M1b-06 : une veille n'est pas un
//! arrêt. En `D3` l'objet de périphérique reste valide et l'utilisateur doit retrouver ses
//! câbles au réveil, alors `suspend` ne touche ni au `device` ni à l'existence du timer ;
//! seul `stop` défait ce que `start` a fait, et c'est le seul des trois à exiger
//! `PASSIVE_LEVEL`. Voir `crate::power` pour le pourquoi de chaque colonne.
//!
//! # Synchronisation et contrat des emplacements
//!
//! Les emplacements sont protégés par le **spin lock du câble** ([`Cable::state`]) :
//!
//! - le pointeur d'un emplacement n'est valide que **tant que le flux vit** ; il désigne
//!   le `SpinLock<StreamState>` logé dans l'objet COM du flux (pool non paginé) ;
//! - le flux se retire de l'emplacement **avant** sa destruction (`Drop` de
//!   `stream::WaveStream`), en prenant ce même spin lock : un lecteur qui tient la garde
//!   des emplacements a donc la garantie que le flux qu'elle désigne ne sera pas détruit
//!   avant qu'il la relâche. C'est aussi ce qui remplace le `KeFlushQueuedDpcs` de
//!   M1a-07 : au retour de `detach`, aucun tick ne détient plus le pointeur ;
//! - tout lecteur ([`Cable::on_tick`]) doit **tenir la garde des emplacements pendant
//!   tout son usage du pointeur**, y compris pendant qu'il prend le verrou du flux.
//!   Ordre de verrouillage, fixe : **câble puis flux** ; jamais l'inverse (le `Drop` du
//!   flux ne tient pas son propre verrou en prenant celui du câble, et `set_state`
//!   relâche le sien avant [`Cable::refresh_timer`]). Entre les deux flux, l'ordre est
//!   rendu puis capture ; seul `on_tick` prend les deux.
//!
//! Les sections critiques du câble sont courtes (lecture de deux pointeurs, calcul de
//! position, copie d'une avance de 2 ms) : `GetPosition` d'un flux ne prend que le verrou
//! du flux, pas celui du câble.
//!
//! Le câble porte un **second** spin lock, celui des destinataires d'événement
//! ([`Cable::attach_jack_events`]) : il n'entre dans aucun ordre de verrouillage, parce
//! qu'il n'est jamais tenu en même temps qu'un autre — ni dans un sens ni dans l'autre.
//! Voir la documentation du champ `jack_events`.
//!
//! # Les nœuds KS, et pourquoi ils sont ici
//!
//! Le câble porte aussi, par sens, l'état des nœuds de volume et de sourdine de son filtre
//! de topologie ([`NodeState`], driver-design.md §5.5). C'est le seul objet que les deux
//! côtés atteignent déjà : les miniports topologie (`topo::TopoRender`,
//! `topo::TopoCapture`) reçoivent le même `&'static Cable` que les miniports WaveRT, et
//! M1b-03 (jack) comme M1b-04 (propriété de configuration) auront besoin du même chemin.
//! Le loger dans le miniport de topologie marcherait aujourd'hui et coûterait un
//! déménagement demain.
//!
//! Depuis M1b-03, il porte de la même façon son **état de connexion**
//! ([`Cable::is_connected`]) : `KSJACK_DESCRIPTION::IsConnected`, ce qui fait la différence
//! entre un endpoint listé dans les réglages Son et un endpoint rangé sous
//! « Périphériques déconnectés ». Il est **par câble** et non par sens : un câble
//! débranché l'est de ses deux bouts, et les deux filtres de topologie lisent le même
//! atomique.
//!
//! # L'état actif est persisté (M1b-04)
//!
//! Depuis M1b-04, cet état est **modifiable** par le jeu de propriétés privé
//! `KSPROPSETID_Conduit` ([`Cable::set_connected`]) et **survit au redémarrage** sans le
//! démon (F-05) : une unique valeur `REG_DWORD` `ActiveCables` dans la clé matérielle du
//! périphérique porte le masque de bits des seize câbles (voir
//! `conduit_kmd_core::config::ACTIVE_CABLES_VALUE_NAME` pour le pourquoi d'un masque plutôt
//! que de seize valeurs). `StartDevice` la lit et l'applique par [`apply_active_mask`] ;
//! chaque écriture réussie de la propriété la réécrit entière depuis [`active_mask`].
//!
//! Un changement d'état **se signale** : [`Cable::set_connected`] émet
//! `KSEVENT_PINCAPS_JACKINFOCHANGE` sur les deux filtres de topologie du câble
//! ([`Cable::notify_jack_change`]) avant de persister. Sans cet événement, Windows ne
//! relit jamais `KSPROPERTY_JACK_DESCRIPTION` : la propriété rendrait la bonne valeur et
//! le panneau de son ne bougerait pas (voir `portcls::event`).
//!
//! La constante `CONNECTED_BY_DEFAULT` de M1b-03 a disparu : l'état de départ ne se déduit
//! plus du numéro de câble, il vient du registre. Ce qu'il en reste est le **défaut du
//! masque**, `conduit_kmd_core::config::ACTIVE_CABLES_DEFAULT` (0x3, « Conduit 1 » et
//! « Conduit 2 »), sur lequel la lecture se replie et que l'INF écrit à l'installation.
//!
//! Ces champs sont des **atomiques, pas des champs sous le verrou du câble** : le
//! gestionnaire de propriété KS tourne à `PASSIVE_LEVEL` sur un fil quelconque, la lecture
//! éventuelle viendrait du tick à `DISPATCH_LEVEL`, un chargement 32 bits aligné ne se
//! déchire pas, et il n'y a aucune relation d'ordre à établir avec un autre champ — d'où
//! `Relaxed` partout. Prendre le spin lock du câble pour lire quatre octets élèverait
//! l'IRQL et se sérialiserait avec la boucle locale pour rien.

use core::ffi::c_void;
use core::fmt;
use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, AtomicU64, Ordering};

use conduit_kmd_core::config::{self, ACTIVE_CABLES_DEFAULT};
use conduit_kmd_core::{
    FrameLayout, Loopback, Notifier, SilenceCause, StreamPosition, StreamView, VirtualClock,
    byte_offset, copy_frames, silence,
};
use portcls::conduit_com::{NtStatus, STATUS_INSUFFICIENT_RESOURCES};
use portcls::{JackTarget, JackTargets, PortEvents, VOLUME_MAX};
use portcls_sys::{KSSTATE, PDEVICE_OBJECT, PMDL};
use wdk_sys::ntddk::KeSetEvent;
use wdk_sys::{KEVENT, PEX_TIMER, PVOID};

use crate::clock;
use crate::eventlog::EventLog;
use crate::registry;
use crate::sync::{SpinLock, SpinLockGuard};
use crate::timer::ExTimer;

/// `STATUS_DEVICE_NOT_READY` (`0xC00000A3`) : le câble n'a pas encore vu de `StartDevice`,
/// donc il n'a pas d'objet de périphérique et rien à persister. Ne devrait pas se produire
/// (une propriété KS suppose un sous-périphérique enregistré), mais un statut nommé vaut
/// mieux qu'un pointeur nul déréférencé.
const STATUS_DEVICE_NOT_READY: NtStatus = 0xC000_00A3_u32 as NtStatus;

/// Nombre maximal d'événements de notification par flux
/// (`RegisterNotificationEvent`) : PortCls n'en enregistre qu'un par client, deux
/// laissent une marge (SYSVAD utilise une liste sans borne).
pub const MAX_NOTIFICATION_EVENTS: usize = 2;

/// Période du timer du câble, en millisecondes (SYSVAD : 1 ms ; le tampon fait au moins
/// 1 ms, donc au plus une frontière de notification par tick, et l'avance de copie de
/// `conduit_kmd_core::loopback` couvre deux ticks).
pub const TICK_PERIOD_MS: u32 = 1;

/// Nombre de ticks entre deux lignes de journal de la boucle locale (debug seulement :
/// `kmd_log!` est vide en release). À 1 ms par tick, une ligne par seconde.
#[cfg(debug_assertions)]
const LOG_EVERY_TICKS: u64 = 1000;

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

/// État d'un flux WaveRT ouvert, tel que le câble et le tick le voient : protégé par
/// le spin lock du flux ([`SharedStream`]), en mémoire non paginée (objet COM du flux).
///
/// Les champs immuables après création (`clock`, `layout`) y figurent pour que
/// [`Cable::on_tick`] calcule la position et la disposition des trames des deux flux avec
/// le seul pointeur de l'emplacement.
#[derive(Debug)]
pub struct StreamState {
    /// Horloge virtuelle du flux (fréquence QPC lue à la création, fréquence
    /// d'échantillonnage du format retenu).
    pub clock: VirtualClock,
    /// Disposition des trames du format retenu (canaux et format d'échantillon).
    pub layout: FrameLayout,
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
    pub const fn new(clock: VirtualClock, layout: FrameLayout) -> Self {
        Self {
            clock,
            layout,
            state: KSSTATE::KSSTATE_STOP,
            position: StreamPosition::new(),
            buffer: None,
            notifier: None,
            events: [None; MAX_NOTIFICATION_EVENTS],
        }
    }

    /// Octets par trame du format retenu (2 à 32).
    pub const fn frame_bytes(&self) -> u32 {
        self.layout.frame_bytes()
    }

    /// Vrai en `KSSTATE_RUN`.
    pub const fn is_running(&self) -> bool {
        self.state == KSSTATE::KSSTATE_RUN
    }

    /// Vrai si le flux tourne avec un tampon : le tick a quelque chose à faire pour lui.
    pub const fn is_live(&self) -> bool {
        self.is_running() && self.buffer.is_some()
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
        byte_offset(self.frames_at(qpc_now), self.frame_bytes(), buffer.bytes).unwrap_or(0)
    }

    /// Vue du flux pour [`conduit_kmd_core::loopback`] à l'instant `qpc_now` : `None`
    /// s'il ne tourne pas, n'a pas de tampon, ou si le tampon ne fait pas au moins une
    /// trame entière (impossible : il est alloué en multiple de la trame).
    fn view(&self, qpc_now: u64) -> Option<StreamView> {
        let buffer = self.buffer?;
        if !self.is_running() {
            return None;
        }
        let buffer_frames = buffer.bytes.checked_div(self.frame_bytes())?;
        (buffer_frames > 0).then(|| StreamView {
            frames: self.frames_at(qpc_now),
            buffer_frames: u64::from(buffer_frames),
        })
    }

    /// Tampon du flux vu comme (base, longueur en octets, disposition), pour les
    /// tranches de [`Cable::on_tick`]. `None` sans tampon ou si la taille ne tient pas
    /// dans un `usize`.
    fn buffer_view(&self) -> Option<(*mut u8, usize, FrameLayout)> {
        let buffer = self.buffer?;
        let len = usize::try_from(buffer.bytes).ok()?;
        Some((buffer.base, len, self.layout))
    }

    /// Signale les événements de notification si une frontière de période a été franchie
    /// depuis le dernier signal (driver-design.md §5.3, étape 5).
    ///
    /// IRQL : `DISPATCH_LEVEL` (sous le verrou du flux, depuis le tick du câble).
    fn signal_notifications(&mut self, qpc_now: u64) {
        if !self.is_live() {
            return;
        }
        let frames = self.frames_at(qpc_now);
        let Some(notifier) = self.notifier.as_mut() else {
            return;
        };
        if !notifier.advance(frames) {
            return;
        }
        for event in self.events.iter().flatten() {
            // SAFETY: l'événement est un `KEVENT` non paginé que PortCls garde vivant
            // jusqu'au `UnregisterNotificationEvent` correspondant, lequel le retire de
            // `events` sous ce même verrou ; `Wait = FALSE` : autorisé à `DISPATCH_LEVEL`.
            unsafe { KeSetEvent(event.as_ptr(), 0, 0) };
        }
    }
}

/// État verrouillé d'un flux : ce que les emplacements du câble désignent.
pub type SharedStream = SpinLock<StreamState>;

/// Nombre maximal de canaux d'un nœud de volume : le plafond de M1b-05 (1 à 8 canaux
/// selon la configuration du câble).
///
/// Le tableau est dimensionné une fois pour toutes plutôt qu'au format courant : il coûte
/// 32 octets par sens dans la section de données, la construction reste `const`, et le
/// jour où le format d'un câble change, rien à redimensionner.
pub const MAX_CHANNELS: usize = 8;

/// Niveau par canal et sourdine des nœuds KS d'un sens du câble (driver-design.md §5.5).
///
/// # Mémorisé, jamais appliqué — et c'est le mécanisme, pas un raccourci
///
/// Le nœud de volume existe pour une raison unique : **faire renoncer Windows à son APO
/// logiciel**. Sans nœud dans notre topologie, le moteur audio insère le sien et applique
/// aux trames le volume par défaut qu'il donne à tout endpoint neuf — mesuré à 64 %, soit
/// une amplitude de 0,229 pour 0,500 demandée. Exposer le nœud suffit à ce que ce gain ne
/// soit plus appliqué avant que les trames n'atteignent notre tampon.
///
/// La seconde moitié est contre-intuitive et tout aussi indispensable : **ne pas appliquer
/// la valeur**. Windows pousse sa valeur par défaut dans notre nœud dès la création de
/// l'endpoint ; « initialiser à 0 dB » ne suffirait donc pas, la valeur serait écrasée dans
/// la seconde qui suit. C'est en mémorisant sans appliquer qu'on obtient 0,500 pour 0,500.
///
/// Rien dans le pilote ne lit ces champs pour transformer le signal :
/// [`Cable::on_tick`] appelle `copy_frames` qui recopie les trames **inchangées, octet pour
/// octet**. C'est la seule réponse cohérente avec la raison d'être du produit — un câble
/// dont la promesse est la transparence bit à bit ne peut pas atténuer ce qu'il transporte.
///
/// **Conséquence assumée : le curseur de volume d'un endpoint Conduit est décoratif.** Le
/// déplacer change ce que la propriété KS relit, et rien d'autre. C'est volontaire.
///
/// # Synchronisation
///
/// Atomiques en `Relaxed`, sans verrou : voir la documentation du module.
///
/// # Un état par sens, pas par câble numéroté
///
/// Le contenu réellement *par câble* n'est pas ici mais dans le descripteur (le GUID
/// `KsPinDescriptor.Name`). Les nœuds, leurs tables d'automatisation et leurs gestionnaires
/// sont des `static` partagées par tous les câbles ; c'est le `MajorTarget` de la requête
/// qui ramène le gestionnaire au bon miniport, donc au bon `NodeState`.
#[derive(Debug)]
pub struct NodeState {
    /// Niveau de chaque canal, en unités de 1/65536 dB (échelle de `portcls::audio`).
    levels: [AtomicI32; MAX_CHANNELS],
    /// Sourdine du nœud, tous canaux confondus : 0 ou 1 (un `AtomicBool` ferait l'affaire,
    /// mais l'aligné 32 bits se relit sans surprise dans un vidage mémoire).
    muted: AtomicU32,
}

impl NodeState {
    /// Nœuds au repos : tous les canaux à 0 dB, sourdine levée.
    ///
    /// Ces valeurs ne survivent pas à la création de l'endpoint — Windows y pousse les
    /// siennes aussitôt (voir la documentation du type) —, mais elles rendent la première
    /// lecture cohérente si un client interroge avant lui.
    pub const fn new() -> Self {
        Self {
            // Expression `const` répétée : `[AtomicI32::new(0); N]` exigerait `Copy`.
            levels: [const { AtomicI32::new(VOLUME_MAX) }; MAX_CHANNELS],
            muted: AtomicU32::new(0),
        }
    }

    /// Niveau du canal `channel`, en unités de 1/65536 dB.
    ///
    /// `portcls::audio` a déjà validé `channel` contre le nombre de canaux déclarés ; le
    /// repli 0 dB au-delà du dernier canal n'est là que pour qu'aucun chemin ne panique.
    ///
    /// IRQL : quelconque.
    pub fn volume(&self, channel: u32) -> i32 {
        match Self::slot(&self.levels, channel) {
            Some(level) => level.load(Ordering::Relaxed),
            None => VOLUME_MAX,
        }
    }

    /// Mémorise le niveau du canal `channel` (déjà borné et arrondi par `portcls::audio`).
    /// **N'applique rien au signal** : voir la documentation du type.
    ///
    /// IRQL : quelconque.
    pub fn set_volume(&self, channel: u32, level: i32) {
        if let Some(slot) = Self::slot(&self.levels, channel) {
            slot.store(level, Ordering::Relaxed);
        }
    }

    /// État de la sourdine.
    ///
    /// IRQL : quelconque.
    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed) != 0
    }

    /// Mémorise l'état de la sourdine. **N'applique rien au signal.**
    ///
    /// IRQL : quelconque.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(u32::from(muted), Ordering::Relaxed);
    }

    /// L'atomique du canal `channel`, ou `None` au-delà du dernier.
    fn slot(levels: &[AtomicI32; MAX_CHANNELS], channel: u32) -> Option<&AtomicI32> {
        usize::try_from(channel).ok().and_then(|i| levels.get(i))
    }
}

/// Sens d'un flux sur un câble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Broche système de `WaveRender<n>` : le lecteur écrit.
    Render,
    /// Broche système de `WaveCapture<n>` : l'enregistreur lit.
    Capture,
}

impl Direction {
    /// Nom du type de flux pour la journalisation.
    pub const fn stream_name(self) -> &'static str {
        match self {
            Self::Render => "RenderStream",
            Self::Capture => "CaptureStream",
        }
    }

    /// Nom du sens pour la journalisation (« rendu », « capture »), quand ce n'est pas
    /// d'un flux qu'on parle mais du sens lui-même — les filtres de topologie, par
    /// exemple, n'ont pas de flux.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Render => "rendu",
            Self::Capture => "capture",
        }
    }
}

/// L'état du câble sous son spin lock : les deux emplacements, le plan de la boucle
/// locale et l'état d'armement du timer.
pub struct CableState {
    render: *const SharedStream,
    capture: *const SharedStream,
    /// Curseur et lien rendu ↔ capture de la boucle locale.
    loopback: Loopback,
    /// Vrai si le timer est armé : évite un `ExSetTimer` (qui réinitialise l'échéance) à
    /// chaque transition d'état d'un flux déjà en `RUN`.
    armed: bool,
}

// SAFETY: les pointeurs ne sont lus et écrits que sous le spin lock du câble, et leur
// validité est garantie par le contrat des emplacements (module) : pas d'accès non
// synchronisé possible.
unsafe impl Send for CableState {}

impl CableState {
    const fn new() -> Self {
        Self {
            render: ptr::null_mut(),
            capture: ptr::null_mut(),
            loopback: Loopback::new(),
            armed: false,
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
    pub fn get(&self, direction: Direction) -> Option<NonNull<SharedStream>> {
        let raw = match direction {
            Direction::Render => self.render,
            Direction::Capture => self.capture,
        };
        NonNull::new(raw.cast_mut())
    }
}

impl fmt::Debug for CableState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CableState")
            .field("render", &self.render)
            .field("capture", &self.capture)
            .field("loopback", &self.loopback)
            .field("armed", &self.armed)
            .finish()
    }
}

/// Compteurs de la boucle locale, hors verrou (journalisation et diagnostic).
///
/// Remis à zéro à chaque [`Cable::start`] : le pilote ne se décharge pas entre deux
/// cycles de périphérique, et une trace qui additionnerait les cycles précédents ferait
/// croire à une image obsolète du pilote (plusieurs heures perdues ainsi le 2026-09-06).
/// Une trace qui ment coûte plus cher qu'une trace absente.
///
/// # Un compteur par cause, pas un compteur par geste (M1b-07)
///
/// Le silence a **deux causes** (`conduit_kmd_core::loopback::SilenceCause`) que le
/// même `ring::silence` sert : la capture sans producteur, régime permanent, et le rendu
/// plus jeune que son lien, transitoire de quelques trames. Un compteur unique les
/// additionnait, donc n'en démontrait aucune : une valeur qui monte ne disait pas si le
/// rendu était absent ou s'il venait d'ouvrir. Ils sont désormais séparés.
///
/// Symétriquement, le cas « rendu seul » ne laissait **aucune** trace — ni copie, ni
/// silence, ni débordement — et ne se distinguait donc pas d'un câble au repos.
/// [`Counters::discarded_ticks`] est le témoin qui manquait : c'est lui, resté seul à
/// monter pendant que `copied` et les deux silences restent à zéro, qui **démontre** la
/// non-accumulation demandée par M1b-07.
#[derive(Debug)]
struct Counters {
    /// Ticks du timer du câble depuis le dernier `StartDevice`.
    ticks: AtomicU64,
    /// Trames copiées du rendu vers la capture.
    copied: AtomicU64,
    /// Ticks trop en retard pour rattraper (`Plan::overrun`) : un trou dans la capture.
    overruns: AtomicU64,
    /// Trames de silence écrites faute de rendu en `RUN` : « l'entrée sans producteur lit
    /// du silence ». Monte tant que la capture tourne seule, à raison de l'avance de copie
    /// par tick.
    silenced_no_render: AtomicU64,
    /// Trames de silence écrites alors qu'un rendu tourne, pour des trames antérieures à
    /// son départ. Borné par le décalage du lien : quelques trames par lien, pas un
    /// régime.
    silenced_before_render: AtomicU64,
    /// Ticks où le rendu tournait **sans capture** : ses trames sont jetées, rien n'est
    /// accumulé. Le seul compteur qui ne compte pas des trames écrites, mais des trames
    /// qu'on a choisi de ne pas garder.
    discarded_ticks: AtomicU64,
}

impl Counters {
    const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
            copied: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            silenced_no_render: AtomicU64::new(0),
            silenced_before_render: AtomicU64::new(0),
            discarded_ticks: AtomicU64::new(0),
        }
    }

    /// Remet les six compteurs à zéro (nouveau cycle de périphérique).
    ///
    /// IRQL : `PASSIVE_LEVEL`, timer arrêté : aucun tick ne peut incrémenter en
    /// parallèle, `Relaxed` suffit.
    fn reset(&self) {
        self.ticks.store(0, Ordering::Relaxed);
        self.copied.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
        self.silenced_no_render.store(0, Ordering::Relaxed);
        self.silenced_before_render.store(0, Ordering::Relaxed);
        self.discarded_ticks.store(0, Ordering::Relaxed);
    }

    /// Ajoute `frames` trames de silence au compteur de sa **cause**.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`, hors de tout verrou.
    fn add_silence(&self, cause: SilenceCause, frames: u64) {
        let compteur = match cause {
            SilenceCause::NoRender => &self.silenced_no_render,
            SilenceCause::BeforeRenderStart => &self.silenced_before_render,
        };
        compteur.fetch_add(frames, Ordering::Relaxed);
    }
}

/// Erreur de [`Cable::attach`] : un flux est déjà ouvert dans ce sens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotOccupied;

/// Un câble : son numéro, ses deux flux courants, sa boucle locale et son timer.
#[derive(Debug)]
pub struct Cable {
    /// Numéro du câble, de 0 à [`CABLE_COUNT`] − 1 (« Conduit *n+1* » pour
    /// l'utilisateur).
    pub index: u32,
    /// L'état du câble, sous son spin lock.
    state: SpinLock<CableState>,
    /// Timer haute résolution de la boucle locale (créé au premier `StartDevice`).
    timer: ExTimer,
    /// Compteurs atomiques de la boucle (hors verrou).
    counters: Counters,
    /// Nœuds volume et sourdine du filtre `TopoRender<n>` (hors verrou, [`NodeState`]).
    render_nodes: NodeState,
    /// Nœuds volume et sourdine du filtre `TopoCapture<n>`.
    capture_nodes: NodeState,
    /// Les **deux** filtres de topologie du câble, tels qu'ils se sont fait connaître dans
    /// leur `Init` : c'est par eux que [`Cable::set_connected`] fait relire la prise à
    /// Windows (`portcls::JackTargets`).
    ///
    /// # Un verrou à part, et lequel
    ///
    /// Ni atomique (une `PortEvents` est une référence comptée, pas un mot) ni sous le
    /// verrou du câble : un **spin lock dédié**, jamais pris en même temps qu'un autre —
    /// ni le verrou du câble, ni celui d'un flux ne sont pris pendant qu'on le tient, et
    /// réciproquement. Il n'entre donc dans aucun ordre de verrouillage, ce qui est la
    /// façon la plus simple de ne pas se tromper d'ordre. Il ne protège que deux `Option`,
    /// et la section critique la plus longue est une copie qui prend deux références COM.
    ///
    /// C'est ce verrou qui rend impossible la course entre le signalement et le démontage
    /// d'un miniport : voir `portcls::JackTargets` et [`Cable::notify_jack_change`].
    ///
    /// # Pourquoi `ManuallyDrop`
    ///
    /// Une `JackTarget` possède une référence COM, donc une glu de destruction — et un
    /// [`Cable`] n'a pas le droit d'en avoir une. Les câbles sont un tableau `static`
    /// construit **à la compilation** ([`cables`]), par un `const fn` qui **affecte** chaque
    /// élément : une affectation détruit la valeur remplacée, et l'évaluation `const`
    /// refuse toute destruction (`E0493`). La même règle interdirait les assertions qui
    /// vérifient la numérotation des câbles. Le choix est donc entre « pas de glu » et
    /// « pas de vérification à la compilation », et ce module a déjà tranché ailleurs.
    ///
    /// Ce n'est pas une fuite : un [`Cable`] **n'est jamais détruit** — il vit dans la
    /// section de données du pilote, du chargement au déchargement (voir l'en-tête de
    /// module) —, si bien que la glu ne s'exécuterait de toute façon jamais. Les références
    /// COM, elles, sont rendues explicitement : par [`Cable::detach_jack_events`] au `Drop`
    /// de chaque miniport, et par [`shutdown`] au déchargement, pour ce qui aurait
    /// survécu. `ManuallyDrop` ne fait qu'écrire ce qui était déjà vrai.
    jack_events: SpinLock<ManuallyDrop<JackTargets>>,
    /// `KSJACK_DESCRIPTION::IsConnected` des **deux** filtres de topologie du câble : 0 ou
    /// 1 (hors verrou, comme [`NodeState`] ; un `AtomicU32` plutôt qu'un `AtomicBool` pour
    /// que l'aligné 32 bits se relise sans surprise dans un vidage mémoire).
    connected: AtomicU32,
    /// L'objet de périphérique de l'adaptateur qui a démarré ce câble, nul avant le
    /// premier [`Cable::start`] : c'est par lui que [`Cable::set_connected`] atteint le
    /// registre et le journal d'événements.
    ///
    /// # Pourquoi le câble le porte
    ///
    /// La persistance de M1b-04 se déclenche depuis un **gestionnaire de propriété KS**,
    /// qui ne reçoit ni objet de périphérique ni IRP de PnP : il n'a que le `MajorTarget`
    /// de sa requête, donc le miniport, donc ce câble. Il fallait bien que l'objet de
    /// périphérique soit joignable depuis ici. Le loger dans le câble plutôt que dans une
    /// `static` de module n'ajoute aucune hypothèse : il est réécrit à chaque
    /// `StartDevice`, comme le timer et les compteurs, et suit donc le cycle de vie qui
    /// existe déjà. (Une seconde instance d'adaptateur partagerait de toute façon le
    /// tableau `static` [`CABLES`] tout entier — c'est une limite du pilote, pas de ce
    /// champ.)
    ///
    /// # Validité
    ///
    /// PortCls détient une référence sur l'objet de périphérique tant que l'adaptateur
    /// vit, et il ne route une propriété vers un de nos miniports que pendant cette
    /// période : entre le `StartDevice` qui pose ce pointeur et le retrait du
    /// périphérique, il désigne un objet vivant. [`shutdown`] le remet à nul au
    /// déchargement.
    device: AtomicPtr<c_void>,
}

impl Cable {
    /// Câble `index`, sans flux ni timer, nœuds au repos, état actif au **défaut du
    /// masque**.
    ///
    /// L'état de départ n'est plus une propriété du numéro de câble : il vient du registre
    /// (`ActiveCables`), que `StartDevice` lit et applique par [`apply_active_mask`] avant
    /// d'enregistrer le moindre sous-périphérique. La valeur posée ici est donc le repli
    /// [`ACTIVE_CABLES_DEFAULT`], celui-là même sur lequel la lecture se rabat, et elle
    /// n'est observable que si l'on interrogeait un câble avant tout `StartDevice` — ce
    /// qu'aucun chemin ne fait, faute d'endpoint.
    pub const fn new(index: u32) -> Self {
        Self {
            index,
            state: SpinLock::new(CableState::new()),
            timer: ExTimer::new(),
            counters: Counters::new(),
            render_nodes: NodeState::new(),
            capture_nodes: NodeState::new(),
            jack_events: SpinLock::new(ManuallyDrop::new(JackTargets::new())),
            connected: AtomicU32::new(if config::is_active(ACTIVE_CABLES_DEFAULT, index) {
                1
            } else {
                0
            }),
            device: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// L'état de connexion du câble, celui que `KSJACK_DESCRIPTION::IsConnected` porte et
    /// que `KSPROPERTY_CONDUIT_CABLE_STATE` rend.
    ///
    /// `false` range l'endpoint sous « Périphériques déconnectés » dans les réglages Son,
    /// des deux côtés du câble à la fois.
    ///
    /// IRQL : quelconque.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed) != 0
    }

    /// Fixe l'état de connexion du câble **et le persiste** (M1b-04).
    ///
    /// L'état en mémoire est appliqué d'abord et **toujours** ; le `Err` rendu ne décrit
    /// que l'échec de l'écriture au registre, que l'appelant ne doit pas propager à sa
    /// propriété (un disque plein ne doit pas empêcher d'activer un câble). L'échec est
    /// déjà journalisé par [`crate::registry::write_active_cables`].
    ///
    /// La persistance écrit le masque **entier**, recalculé depuis l'état vivant des seize
    /// câbles : c'est ce qui fait de l'écriture une seule valeur indivisible du point de
    /// vue d'un lecteur (voir `conduit_kmd_core::config::ACTIVE_CABLES_VALUE_NAME`). Deux
    /// `SET` concurrents sur deux câbles différents peuvent donc se recouvrir — le dernier
    /// écrit gagne, et il écrit un masque cohérent avec l'état en mémoire au moment où il
    /// l'a lu. Sérialiser coûterait un verrou à `PASSIVE_LEVEL` pour une course qui,
    /// au pire, rejoue une écriture de registre au démarrage suivant.
    ///
    /// # `KSEVENT_PINCAPS_JACKINFOCHANGE`, entre l'état et la persistance
    ///
    /// Les deux filtres de topologie du câble (`TopoRender<n>` et `TopoCapture<n>`)
    /// reçoivent l'événement **après** le `store` et **avant** la persistance
    /// ([`Cable::notify_jack_change`]) : sans lui, Windows n'irait jamais relire
    /// `KSPROPERTY_JACK_DESCRIPTION` et l'interface resterait figée sur l'état du démarrage
    /// (voir `portcls::jack`) — « la propriété KS rend la bonne valeur mais le panneau de
    /// son ne bouge pas ».
    ///
    /// L'ordre compte dans un seul sens : le signalement doit suivre le `store`, puisque
    /// c'est la valeur enregistrée que Windows relira. Le placer avant la persistance,
    /// plutôt qu'après, fait que l'interface bouge même si l'écriture au registre échoue —
    /// l'état en mémoire, lui, a bien changé.
    ///
    /// # IRQL
    ///
    /// **`PASSIVE_LEVEL`**, et pour deux raisons indépendantes : l'écriture au registre
    /// l'exige (`IoOpenDeviceRegistryKey`, `ZwSetValueKey`), et le `Drop` de la copie des
    /// destinataires peut relâcher la dernière référence sur un objet port, ce qui ne se
    /// fait pas à IRQL élevé. Le signalement lui-même n'est pas le facteur limitant :
    /// `GenerateEventList` tolère `<= DISPATCH_LEVEL` sans réserve (voir
    /// `portcls::event`). Le spin lock des destinataires, lui, n'est tenu que le temps
    /// d'une copie, à `DISPATCH_LEVEL`.
    pub fn set_connected(&self, connected: bool) -> Result<(), NtStatus> {
        self.connected
            .store(u32::from(connected), Ordering::Relaxed);
        self.notify_jack_change();
        self.persist_active_mask()
    }

    /// Inscrit le filtre de topologie du sens `direction` comme destinataire de
    /// `KSEVENT_PINCAPS_JACKINFOCHANGE`, avec la broche endpoint de **ce** filtre.
    ///
    /// Appelé par `topo::TopoRender::init` / `topo::TopoCapture::init`, seuls endroits où
    /// l'objet port est visible. La `JackTarget` porte la référence COM obtenue par
    /// `QueryInterface` ; c'est le câble qui la possède désormais, et
    /// [`detach_jack_events`](Self::detach_jack_events) qui la rendra.
    ///
    /// IRQL : `PASSIVE_LEVEL` (`Init`).
    pub fn attach_jack_events(&self, direction: Direction, target: JackTarget) {
        let ancienne = {
            let mut cibles = self.jack_events.lock();
            match direction {
                Direction::Render => cibles.set_render(target),
                Direction::Capture => cibles.set_capture(target),
            }
        };
        // Hors du verrou : le `Drop` d'une cible relâche une référence COM, et la dernière
        // détruirait l'objet port — jamais à `DISPATCH_LEVEL`.
        if ancienne.is_some() {
            kmd_log!(
                "câble {} : une cible d'événement {} était déjà inscrite (remplacée)",
                self.index,
                direction.name()
            );
        }
        drop(ancienne);
    }

    /// Retire le destinataire du sens `direction` et **relâche sa référence COM**.
    ///
    /// Appelé par le `Drop` du miniport de topologie : c'est le dernier instant où la
    /// cible est encore joignable, et le premier où la relâcher est sûr. Après le retour,
    /// un signalement concurrent ne peut plus trouver ce sens (il l'a vu avant, et il en
    /// tient alors une copie comptée : voir `portcls::JackTargets`).
    ///
    /// IRQL : `PASSIVE_LEVEL` (`Release` final du miniport, depuis PortCls).
    pub fn detach_jack_events(&self, direction: Direction) {
        let cible = {
            let mut cibles = self.jack_events.lock();
            match direction {
                Direction::Render => cibles.take_render(),
                Direction::Capture => cibles.take_capture(),
            }
        };
        // Verrou relâché : voir `attach_jack_events`.
        drop(cible);
    }

    /// L'`IPortEvents` du filtre de topologie du sens `direction`, s'il s'est fait
    /// connaître : ce que `portcls::EventSource::port_events` rend au verbe `ADD`.
    ///
    /// La copie prend une référence de plus, sous le verrou : l'appelant peut la garder
    /// aussi longtemps qu'il veut sans risquer que le miniport la démonte sous lui.
    ///
    /// IRQL : `PASSIVE_LEVEL` (verbe `ADD` d'un événement KS).
    pub fn jack_port_events(&self, direction: Direction) -> Option<PortEvents> {
        let cibles = self.jack_events.lock();
        match direction {
            Direction::Render => cibles.render(),
            Direction::Capture => cibles.capture(),
        }
        .map(|cible| cible.port_events().clone())
    }

    /// Signale `KSEVENT_PINCAPS_JACKINFOCHANGE` aux **deux** filtres de topologie du
    /// câble, chacun sur sa broche endpoint.
    ///
    /// Un câble dont aucun miniport ne s'est fait connaître ne notifie rien et ne
    /// s'en plaint pas : c'est l'état normal avant `StartDevice`.
    ///
    /// # Pourquoi la copie
    ///
    /// La copie est prise **sous** le verrou et le signalement fait **dehors**. Sous le
    /// verrou, chaque copie prend une référence COM de plus : le port ne peut donc pas
    /// être détruit pendant qu'on le signale, même si le miniport se démonte au même
    /// instant — il devra passer par ce même verrou pour retirer sa cible, et il ne
    /// trouvera plus qu'un compteur qui ne tombe pas à zéro. Dehors, parce que
    /// `GenerateEventList` appelle PortCls et que le `Drop` de la copie peut détruire un
    /// objet port : ni l'un ni l'autre n'a sa place dans une section critique à
    /// `DISPATCH_LEVEL`.
    ///
    /// IRQL : `PASSIVE_LEVEL` (voir [`set_connected`](Self::set_connected)).
    fn notify_jack_change(&self) {
        // `JackTargets::clone` et non `garde.clone()` : la garde donne un
        // `ManuallyDrop<JackTargets>`, dont le `Clone` rendrait une copie **sans** glu de
        // destruction — deux références COM prises et jamais rendues.
        let cibles = {
            let garde = self.jack_events.lock();
            JackTargets::clone(&garde)
        };
        cibles.notify_jack_change();
    }

    /// Écrit le masque des câbles actifs dans la clé matérielle du périphérique.
    ///
    /// `Err(STATUS_DEVICE_NOT_READY)` si le câble n'a pas encore vu de `StartDevice` : il
    /// n'y a alors aucun objet de périphérique, donc aucune clé à ouvrir. Ce cas ne devrait
    /// pas se produire — une propriété KS suppose un sous-périphérique enregistré — mais il
    /// vaut mieux un statut nommé qu'un déréférencement de pointeur nul.
    fn persist_active_mask(&self) -> Result<(), NtStatus> {
        let device = self.device.load(Ordering::Relaxed);
        if device.is_null() {
            kmd_log!(
                "câble {} : pas d'objet de périphérique, masque non persisté",
                self.index
            );
            return Err(STATUS_DEVICE_NOT_READY);
        }
        // SAFETY: `device` est l'objet de périphérique posé par `start` et vivant tant que
        // l'adaptateur l'est (voir la documentation du champ `device`) ; l'`EventLog` ne
        // sert que dans cet appel.
        let log = unsafe { EventLog::new(device.cast()) };
        // SAFETY: idem ; la propriété est traitée à `PASSIVE_LEVEL`, ce qu'exigent
        // `IoOpenDeviceRegistryKey` et `ZwSetValueKey`.
        unsafe { registry::write_active_cables(device.cast(), active_mask(), log) }
    }

    /// Les nœuds volume et sourdine du sens `direction`.
    ///
    /// Hors du spin lock du câble, et volontairement : voir [`NodeState`].
    ///
    /// IRQL : quelconque.
    pub const fn nodes(&self, direction: Direction) -> &NodeState {
        match direction {
            Direction::Render => &self.render_nodes,
            Direction::Capture => &self.capture_nodes,
        }
    }

    /// Prend le spin lock du câble et rend son état. Tant que la garde vit, aucun flux
    /// désigné ne peut être détruit (voir le contrat du module) ; l'ordre de
    /// verrouillage est câble puis flux.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    fn state(&self) -> SpinLockGuard<'_, CableState> {
        self.state.lock()
    }

    /// Prépare le câble pour un `StartDevice` : oublie les flux et le plan de la boucle
    /// (les broches sont fermées avant tout arrêt du périphérique, les emplacements
    /// devraient déjà être vides — journalisé sinon), remet les compteurs à zéro, puis
    /// crée le timer haute résolution s'il ne l'est pas déjà.
    ///
    /// Échec de `ExAllocateTimer` (pool épuisé) : `STATUS_INSUFFICIENT_RESOURCES`,
    /// **sans repli** sur `KeSetTimerEx`. Décision documentée dans
    /// [`crate::timer`] et driver-design.md §5.3 : un `KTIMER` de 1 ms ne se réveille
    /// qu'à la résolution de l'horloge système (15,6 ms), ce qui ferait déborder la
    /// boucle à chaque tick et manquerait toutes les notifications d'un tampon de
    /// 10 ms. Un câble qui s'énumère et ne délivre que des trous est plus difficile à
    /// diagnostiquer qu'un `StartDevice` qui échoue proprement (driver-design.md §7).
    ///
    /// Les [`NodeState`] ne sont **pas** remis à zéro : Windows repousse sa valeur par
    /// défaut dans le nœud dès qu'il recrée l'endpoint, et ce que le nœud mémorise
    /// n'agit de toute façon sur rien. L'état de connexion non plus : c'est un réglage de
    /// l'utilisateur, pas un état de session, et il doit survivre à un cycle
    /// `StopDevice`/`StartDevice` (une mise en veille, par exemple). `start_device` l'a de
    /// toute façon déjà relu du registre par [`apply_active_mask`], **avant** cet appel.
    ///
    /// `device` est mémorisé : c'est par lui que [`Cable::set_connected`] atteindra ensuite
    /// le registre et le journal d'événements (voir le champ `device`).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    ///
    /// # Safety
    ///
    /// `device` est l'objet de périphérique remis à `StartDevice`, vivant au moins jusqu'au
    /// retrait du périphérique — donc au-delà de toute propriété KS routée vers un miniport
    /// de ce câble.
    pub unsafe fn start(&self, device: PDEVICE_OBJECT) -> Result<(), NtStatus> {
        self.device.store(device.cast(), Ordering::Relaxed);
        {
            let mut state = self.state();
            if !state.render.is_null() || !state.capture.is_null() {
                kmd_log!(
                    "câble {} : un flux était encore inscrit au démarrage (oublié)",
                    self.index
                );
            }
            *state = CableState::new();
            // `armed` repart à faux : le timer d'un cycle précédent (qui devrait déjà
            // être désarmé, les broches étant fermées) doit l'être vraiment.
            self.timer.stop();
            // Les compteurs aussi : le pilote reste chargé d'un cycle à l'autre, et une
            // trace qui cumulerait les ticks des cycles précédents ferait croire à une
            // image obsolète du pilote.
            self.counters.reset();
        }
        // Les cibles d'événement ne sont **pas** effacées : elles sont posées par les
        // `Init` des miniports de topologie, qui suivent ce `start`, et retirées par leur
        // `Drop`, qui précède le `StopDevice` suivant. En trouver une ici signalerait un
        // miniport survivant à son cycle — jamais vu, mais silencieux si on n'y regarde
        // pas.
        let cible_restante = {
            let cibles = self.jack_events.lock();
            !cibles.is_empty()
        };
        if cible_restante {
            kmd_log!(
                "câble {} : une cible d'événement était encore inscrite au démarrage",
                self.index
            );
        }
        let context: PVOID = ptr::from_ref(self).cast_mut().cast();
        // SAFETY: `self` est un élément du `static` `CABLES` : le contexte reste valide
        // jusqu'au déchargement du pilote, où `shutdown` supprime le timer.
        if unsafe { self.timer.create(cable_tick, context) } {
            return Ok(());
        }
        kmd_log!(
            "câble {} : ExAllocateTimer (haute résolution) a échoué",
            self.index
        );
        Err(STATUS_INSUFFICIENT_RESOURCES)
    }

    /// Arrête le câble : **supprime** le minuteur (en attendant la fin du tick en cours)
    /// et **remet l'objet de périphérique à nul**. Symétrique de [`Cable::start`].
    ///
    /// # Ce que cette fonction corrige
    ///
    /// Le champ `device` borne explicitement sa validité « jusqu'au retrait du
    /// périphérique » (voir sa documentation) — et jusqu'à M1b-06, **rien ne l'annulait**
    /// à ce moment-là. Un pointeur périmé y aurait survécu à l'objet de périphérique, et
    /// [`Cable::persist_active_mask`] ne traite que le cas **nul** (par
    /// `STATUS_DEVICE_NOT_READY`), pas le cas périmé : il l'aurait passé à
    /// `IoOpenDeviceRegistryKey`. Il existe désormais **une** opération qui rend le câble
    /// à l'état d'avant son premier démarrage, et c'est elle qui doit être appelée partout
    /// où le périphérique s'en va.
    ///
    /// # Ce qu'elle ne fait pas
    ///
    /// Elle ne vide pas les emplacements de flux : c'est le `Drop` de chaque
    /// `stream::WaveStream` qui les retire, et il court avant, PortCls fermant les broches
    /// avant tout arrêt du périphérique. Elle n'efface ni les [`NodeState`], ni l'état de
    /// connexion — pour les mêmes raisons que [`Cable::start`] ne les remet pas à zéro.
    ///
    /// # Ce n'est **pas** le chemin de la veille
    ///
    /// Une transition `D3` n'est pas un arrêt : l'objet de périphérique reste valide, le
    /// pilote reste chargé, et l'utilisateur s'attend à retrouver ses câbles au réveil.
    /// La veille passe par [`Cable::suspend`], qui se contente de **désarmer** le
    /// minuteur. Appeler `stop` en `D3` annulerait la persistance de l'état actif pour
    /// tout le reste de la vie du périphérique — `start` est le seul à reposer `device`,
    /// et il ne court pas au réveil.
    ///
    /// # IRQL
    ///
    /// **`PASSIVE_LEVEL`**, hors de tout spin lock : `ExDeleteTimer(…, Wait = TRUE, …)`
    /// **attend** la fin du rappel en cours, ce qui exige `<= APC_LEVEL`
    /// ([`crate::timer::ExTimer::delete`]).
    pub fn stop(&self) {
        self.timer.delete();
        {
            // `armed` repart à faux avec le reste de l'état : le câble arrêté doit être
            // indiscernable d'un câble qui n'a jamais démarré, sans quoi le prochain
            // `refresh_timer` croirait le minuteur déjà armé.
            let mut state = self.state();
            state.armed = false;
        }
        // Après le minuteur, jamais avant : un tick qui s'achèverait entre les deux ne
        // toucherait de toute façon pas à `device`, mais l'ordre « plus rien ne tourne,
        // puis plus rien n'est joignable » est celui qu'on peut relire.
        self.device.store(ptr::null_mut(), Ordering::Relaxed);
    }

    /// Le câble entre en veille (`PowerDeviceD3` et tout état qui n'est pas `D0`) :
    /// **désarme** le minuteur sans le supprimer, et oublie qu'il était armé.
    ///
    /// # Pourquoi désarmer et non supprimer
    ///
    /// Deux raisons, et la première suffit. `ExCancelTimer` est autorisé jusqu'à
    /// `DISPATCH_LEVEL` et **n'attend rien** ; `ExDeleteTimer(…, Wait = TRUE, …)` attend
    /// la fin du tick en cours et exige `<= APC_LEVEL`. Or l'IRQL exact des rappels
    /// `IAdapterPowerManagement` n'est pas écrit dans la documentation de PortCls (voir
    /// `crate::power`) : un désarmement reste correct même si la supposition
    /// `PASSIVE_LEVEL` était fausse, une suppression bloquante non. La seconde raison est
    /// le choix déjà pris en tête de module : l'objet minuteur est alloué une fois et
    /// vit jusqu'au déchargement, pour qu'aucun réveil ne dépende d'une allocation de
    /// pool qui pourrait échouer.
    ///
    /// # Pourquoi remettre `armed` à faux
    ///
    /// [`Cable::refresh_timer`] est un différentiel : il ne fait rien quand l'état voulu
    /// est celui qu'il croit avoir. Désarmer sans le lui dire laisserait `armed` à vrai,
    /// et le `refresh_timer` du réveil ne réarmerait **jamais** un flux resté en `RUN` —
    /// le câble se réveillerait muet. C'est le seul piège de cette paire.
    ///
    /// # IRQL
    ///
    /// `<= DISPATCH_LEVEL` (le spin lock du câble est pris ; `ExCancelTimer` l'admet).
    pub fn suspend(&self) {
        let mut state = self.state();
        state.armed = false;
        self.timer.stop();
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
        let mut cable = self.state();
        let slot = cable.slot_mut(direction);
        if !slot.is_null() {
            return Err(SlotOccupied);
        }
        *slot = state.as_ptr();
        // Le flux entre à l'arrêt : le plan repart de zéro plutôt que de reprendre un
        // curseur hérité du flux précédent.
        cable.loopback.reset();
        Ok(())
    }

    /// Retire `state` de l'emplacement du sens `direction` s'il y est encore ; rend
    /// vrai dans ce cas. À appeler par le flux **avant** sa destruction : au retour,
    /// plus aucun tick ne détient le pointeur.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn detach(&self, direction: Direction, state: NonNull<SharedStream>) -> bool {
        let mut cable = self.state();
        let slot = cable.slot_mut(direction);
        if ptr::eq(*slot, state.as_ptr()) {
            *slot = ptr::null_mut();
            cable.loopback.reset();
            true
        } else {
            false
        }
    }

    /// Arme le timer si au moins un flux du câble tourne avec un tampon, le désarme
    /// sinon. À appeler **hors** du verrou d'un flux (ordre câble puis flux) après toute
    /// transition qui peut changer la réponse : `SetState`, allocation ou libération de
    /// tampon, `Drop` d'un flux.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn refresh_timer(&self) {
        // Décision et action sous le verrou du câble : deux flux qui changent d'état en
        // même temps ne peuvent pas laisser le timer désarmé alors que l'un tourne.
        let mut cable = self.state();
        let wanted = [Direction::Render, Direction::Capture]
            .into_iter()
            .filter_map(|d| cable.get(d))
            .any(|shared| {
                // SAFETY: le pointeur de l'emplacement désigne le `SpinLock<StreamState>`
                // d'un flux vivant tant que la garde du câble est détenue (contrat du
                // module) ; l'ordre câble puis flux est respecté.
                unsafe { shared.as_ref() }.lock().is_live()
            });
        if wanted == cable.armed {
            return;
        }
        cable.armed = wanted;
        if wanted {
            self.timer.start(TICK_PERIOD_MS);
        } else {
            self.timer.stop();
        }
    }

    /// Un tick du timer (driver-design.md §5.3, étapes 1 à 5) : plan de copie
    /// ([`conduit_kmd_core::loopback`]), exécution (copie, silence, ou rien), puis
    /// notifications des deux flux.
    ///
    /// IRQL : `DISPATCH_LEVEL` (rappel `EXT_CALLBACK`). Aucune allocation, aucune
    /// attente, aucune mémoire paginée.
    fn on_tick(&self, qpc_now: u64) {
        let mut cable = self.state();
        // SAFETY: les pointeurs des emplacements désignent le `SpinLock<StreamState>` de
        // flux vivants tant que cette garde est détenue : un flux se retire de son
        // emplacement sous ce même verrou avant d'être détruit (contrat du module). Les
        // références obtenues ne sortent pas de cette fonction, donc pas de la garde.
        let render_lock = cable
            .get(Direction::Render)
            .map(|shared| unsafe { shared.as_ref() });
        // SAFETY: idem pour l'emplacement capture.
        let capture_lock = cable
            .get(Direction::Capture)
            .map(|shared| unsafe { shared.as_ref() });
        // Ordre de verrouillage interne au câble : rendu puis capture (seul ce tick prend
        // les deux verrous de flux).
        let mut render = render_lock.map(SpinLock::lock);
        let mut capture = capture_lock.map(SpinLock::lock);

        let render_view = render.as_deref().and_then(|s| s.view(qpc_now));
        let capture_view = capture.as_deref().and_then(|s| s.view(qpc_now));
        // L'avance se compte en trames de la **capture** : c'est elle qu'on écrit.
        let lead = capture
            .as_deref()
            .map_or(0, |s| Loopback::lead_frames(s.clock.sample_rate()));
        let plan = cable.loopback.plan(render_view, capture_view, lead);

        let mut copied = 0u64;
        let mut silenced = 0u64;
        // La capture a forcément un tampon dès que le plan demande d'écrire (le plan est
        // vide sans vue de capture, et la vue exige un tampon) ; `and_then` évite tout
        // de même de le supposer.
        if let Some((dst_base, dst_len, dst_layout)) = (plan.silence.is_some()
            || plan.copy.is_some())
        .then(|| capture.as_deref().and_then(StreamState::buffer_view))
        .flatten()
        {
            // SAFETY: `dst_base`/`dst_len` décrivent le mappage noyau du tampon de la
            // capture (`MapAllocatedPages`, mémoire non paginée d'au moins `dst_len`
            // octets), retiré de l'état seulement sous le verrou du flux que cette
            // fonction détient : il reste mappé pendant toute la tranche. C'est un autre
            // tampon que celui du rendu (deux MDL distinctes, jamais partagées §5.2),
            // donc pas d'alias avec `src`. Le client voit la même mémoire depuis le mode
            // utilisateur : les octets sont lus et écrits sans hypothèse d'atomicité ni
            // de cohérence trame par trame — c'est le modèle WaveRT, où le moteur audio
            // et le « matériel » se croisent sur un tampon partagé.
            let dst = unsafe { core::slice::from_raw_parts_mut(dst_base, dst_len) };
            if let Some(op) = plan.silence
                && silence(dst, dst_layout, op.dst_start, op.count).is_ok()
            {
                silenced = op.count;
            }
            if let Some(op) = plan.copy
                && let Some((src_base, src_len, src_layout)) =
                    render.as_deref().and_then(StreamState::buffer_view)
            {
                // SAFETY: mêmes garanties que pour `dst`, côté rendu et en lecture seule
                // (le verrou du flux rendu est détenu, le tampon est un mappage noyau
                // vivant d'au moins `src_len` octets, distinct de celui de la capture).
                let src = unsafe { core::slice::from_raw_parts(src_base.cast_const(), src_len) };
                if copy_frames(
                    src,
                    src_layout,
                    op.src_start,
                    dst,
                    dst_layout,
                    op.dst_start,
                    op.count,
                )
                .is_ok()
                {
                    copied = op.count;
                }
            }
        }

        // Étape 5 : notifications des deux flux, toujours sous les mêmes verrous.
        if let Some(state) = render.as_deref_mut() {
            state.signal_notifications(qpc_now);
        }
        if let Some(state) = capture.as_deref_mut() {
            state.signal_notifications(qpc_now);
        }

        // Verrous relâchés avant les compteurs et le journal : section critique courte.
        drop(capture);
        drop(render);
        drop(cable);

        let ticks = self.counters.ticks.fetch_add(1, Ordering::Relaxed);
        if copied > 0 {
            self.counters.copied.fetch_add(copied, Ordering::Relaxed);
        }
        // Le silence est compté sous **sa** cause : le plan la porte, `ring::silence` n'en
        // a que faire, et un compteur unique ne démontrerait ni l'une ni l'autre.
        if let Some(op) = plan.silence
            && silenced > 0
        {
            self.counters.add_silence(op.cause, silenced);
        }
        if plan.overrun {
            self.counters.overruns.fetch_add(1, Ordering::Relaxed);
        }
        // Rendu sans capture : le seul cas où le tick n'écrit rien **et** n'est pas au
        // repos. Sans ce compteur, la non-accumulation ne se lirait nulle part.
        if plan.discarded {
            self.counters
                .discarded_ticks
                .fetch_add(1, Ordering::Relaxed);
        }
        self.log_counters(ticks.wrapping_add(1));
    }

    /// Journalise les compteurs toutes les [`LOG_EVERY_TICKS`] ticks. Vide en release
    /// (`kmd_log!`), donc le calcul lui-même est réservé au profil debug.
    ///
    /// IRQL : `DISPATCH_LEVEL`, hors de tout verrou.
    #[cfg(debug_assertions)]
    fn log_counters(&self, ticks: u64) {
        if ticks.checked_rem(LOG_EVERY_TICKS) != Some(0) {
            return;
        }
        kmd_log!(
            "câble {} : {ticks} ticks, {} trames copiées, {} silences sans rendu, \
             {} silences avant le départ du rendu, {} ticks jetés (rendu sans capture), \
             {} débordements",
            self.index,
            self.counters.copied.load(Ordering::Relaxed),
            self.counters.silenced_no_render.load(Ordering::Relaxed),
            self.counters.silenced_before_render.load(Ordering::Relaxed),
            self.counters.discarded_ticks.load(Ordering::Relaxed),
            self.counters.overruns.load(Ordering::Relaxed)
        );
    }

    /// Sans journalisation en release.
    #[cfg(not(debug_assertions))]
    fn log_counters(&self, _ticks: u64) {}

    /// Arrête le câble ([`Cable::stop`] : timer supprimé, objet de périphérique oublié)
    /// puis relâche ce qui pourrait survivre au déchargement du pilote. Les broches sont
    /// fermées et les emplacements vides à ce stade.
    ///
    /// IRQL : `PASSIVE_LEVEL`, hors de tout spin lock.
    fn shutdown(&self) {
        // Le timer et l'objet de périphérique partent ensemble, par l'opération
        // symétrique de `start` : c'est elle qui porte le pourquoi.
        self.stop();
        // Les cibles d'événement devraient déjà avoir été retirées par le `Drop` de leur
        // miniport ; les relâcher ici garantit qu'aucune référence sur un objet port ne
        // survit au déchargement. Hors du verrou, comme partout ailleurs : le `Drop` d'une
        // cible peut détruire l'objet port.
        let restantes = {
            let mut cibles = self.jack_events.lock();
            cibles.take_all()
        };
        drop(restantes);
        kmd_log!(
            "câble {} : timer supprimé après {} ticks depuis le dernier StartDevice ({} trames copiées, {} silences sans rendu, {} silences avant le départ du rendu, {} ticks jetés, {} débordements)",
            self.index,
            self.counters.ticks.load(Ordering::Relaxed),
            self.counters.copied.load(Ordering::Relaxed),
            self.counters.silenced_no_render.load(Ordering::Relaxed),
            self.counters.silenced_before_render.load(Ordering::Relaxed),
            self.counters.discarded_ticks.load(Ordering::Relaxed),
            self.counters.overruns.load(Ordering::Relaxed)
        );
    }
}

/// Rappel du timer haute résolution d'un câble (`EXT_CALLBACK`) : `context` est le
/// [`Cable`] `static` passé à `ExAllocateTimer`.
///
/// IRQL : `DISPATCH_LEVEL`.
///
/// # Safety
///
/// Appelée uniquement par le noyau pour le timer créé par [`Cable::start`], avec le
/// `context` qui lui a été remis.
unsafe extern "C" fn cable_tick(_timer: PEX_TIMER, context: PVOID) {
    let Some(cable) = NonNull::new(context.cast::<Cable>()) else {
        return;
    };
    // SAFETY: `context` est le `&'static Cable` remis à `ExAllocateTimer` (contrat) :
    // vivant tant que le pilote est chargé, et `ExDeleteTimer(…, Wait = TRUE)` attend ce
    // rappel avant le déchargement.
    let cable = unsafe { cable.as_ref() };
    cable.on_tick(clock::now());
}

/// Les câbles, construits en `const` : `CABLES[n].index == n`.
///
/// `Cable` n'est ni `Copy` ni `Clone` (spin lock, timer, atomiques) et chaque élément
/// doit connaître son index : d'où la boucle `while`, sans `unsafe` ni `MaybeUninit` —
/// `Cable::new(0)` est une valeur `const` valide, dont `[const { … }; N]` remplit le
/// tableau avant que la boucle ne renumérote.
#[allow(clippy::indexing_slicing)] // évalué à la compilation
const fn cables() -> [Cable; CABLE_COUNT as usize] {
    let mut cables = [const { Cable::new(0) }; CABLE_COUNT as usize];
    let mut i = 1usize;
    while i < cables.len() {
        cables[i] = Cable::new(i as u32);
        i = i.wrapping_add(1);
    }
    cables
}

const _: () = {
    // Les index sont ordonnés, 0, 1, … : c'est ce qui autorise `cable(index)` à indexer
    // directement, et ce qui apparie chaque câble au bon GUID de nom de broche
    // (`descriptors::PIN_NAMES`) et aux bons noms de sous-périphérique. Motif de tranche
    // — n'indexe ni ne calcule, donc traverse `indexing_slicing` et
    // `arithmetic_side_effects` sans aucun `allow`.
    let cables = cables();
    let mut reste: &[Cable] = &cables;
    let mut attendu = 0u32;
    while let [premier, suite @ ..] = reste {
        assert!(
            premier.index == attendu,
            "les câbles doivent être numérotés dans l'ordre"
        );
        reste = suite;
        attendu = attendu.wrapping_add(1);
    }
    assert!(attendu == CABLE_COUNT, "un câble par index");
};

/// Les câbles servis par ce pilote, dans la section de données du pilote (voir
/// « Durée de vie et allocation » en tête de module).
static CABLES: [Cable; CABLE_COUNT as usize] = cables();

/// Le câble `index`, ou `None` au-delà du dernier.
pub fn cable(index: u32) -> Option<&'static Cable> {
    usize::try_from(index).ok().and_then(|i| CABLES.get(i))
}

/// Nombre de câbles servis par ce pilote (`portcls::CABLE_COUNT`, la source unique :
/// c'est elle qui dimensionne aussi les noms de sous-périphériques, les GUID de noms de
/// broche et les blocs de l'INF).
///
/// M1b-01 la rendra configurable (lecture du registre au démarrage) ; elle reste une
/// constante pour l'instant.
pub const CABLE_COUNT: u32 = portcls::CABLE_COUNT as u32;

// Le masque du contrat portable adresse exactement les câbles de ce pilote. Une
// divergence perdrait silencieusement l'état des câbles au-delà du plus petit des deux :
// leur bit ne serait jamais écrit (masque trop court) ou jamais relu (tableau trop court).
const _: () = assert!(
    config::CABLE_MAX == CABLE_COUNT,
    "le masque ActiveCables et le tableau des câbles doivent couvrir les mêmes câbles"
);
// Le défaut n'allume que des câbles qui existent : « Conduit 1 » et « Conduit 2 ».
const _: () = {
    assert!(config::is_active(ACTIVE_CABLES_DEFAULT, 0));
    assert!(config::is_active(ACTIVE_CABLES_DEFAULT, 1));
    assert!(!config::is_active(ACTIVE_CABLES_DEFAULT, 2));
    assert!(!config::is_active(
        ACTIVE_CABLES_DEFAULT,
        CABLE_COUNT.wrapping_sub(1)
    ));
};

/// L'état actif des seize câbles, en masque de bits : le bit *n* vaut « câble *n*
/// connecté ».
///
/// C'est exactement ce que [`crate::registry::write_active_cables`] persiste, et ce que
/// [`apply_active_mask`] a posé au démarrage. Reconstruit à la demande depuis les atomiques
/// plutôt que tenu à jour en double : un second exemplaire du même état est un second
/// exemplaire à garder cohérent.
///
/// IRQL : quelconque.
pub fn active_mask() -> u32 {
    let mut masque = 0;
    for cable in &CABLES {
        masque = config::with_active(masque, cable.index, cable.is_connected());
    }
    masque
}

/// Applique un masque lu au registre à l'état de tous les câbles.
///
/// Appelé une fois par `StartDevice`, **avant** l'enregistrement du moindre
/// sous-périphérique : c'est ce qui fait qu'un endpoint apparaît d'emblée dans le bon état
/// plutôt que de basculer sous les yeux de l'utilisateur. Les câbles au-delà de la réserve
/// sont réglés eux aussi — ils ne produisent aucun endpoint, mais garder leur bit cohérent
/// évite qu'une réduction puis une remise à seize de `ReserveSize` ne perde leur état.
///
/// Ne persiste rien : le masque **vient** du registre.
///
/// IRQL : `PASSIVE_LEVEL`.
pub fn apply_active_mask(masque: u32) {
    for cable in &CABLES {
        cable.connected.store(
            u32::from(config::is_active(masque, cable.index)),
            Ordering::Relaxed,
        );
    }
    kmd_log!("câbles : masque actif {masque:#06x} appliqué");
}

/// Supprime les timers de tous les câbles : à appeler une fois au déchargement du
/// pilote, avant de rendre la main à PortCls.
///
/// IRQL : `PASSIVE_LEVEL`, hors de tout spin lock.
pub fn shutdown() {
    for cable in &CABLES {
        cable.shutdown();
    }
}
