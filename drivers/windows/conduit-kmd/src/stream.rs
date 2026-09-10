//! Flux WaveRT d'un câble ([`WaveStream`], driver-design.md §5) : l'objet que
//! `WaveRender::NewStream` ou `WaveCapture::NewStream` crée pour la broche système, avec
//! son tampon cyclique (§5.2), sa position calculée par l'horloge (§5.1) et ses
//! notifications (§5.3, étape 5). Implémente `portcls::MiniportWaveRTStreamNotification`,
//! donc aussi `MiniportWaveRTStream`.
//!
//! # Mode paquets : servi (lot 3), exposé sous `PacketMode = 1`
//!
//! Le flux implémente `MiniportWaveRTInputStream` et `MiniportWaveRTOutputStream`
//! (`portcls::packet`), et **les quatre méthodes servent** :
//!
//! | Méthode | Ce que le flux répond |
//! |---|---|
//! | `SetWritePacket` | valide le numéro et **pose la frontière** de la copie ; déclenche un tour de boucle ([`Cable::on_write_packet`]) |
//! | `GetReadPacket` | le dernier paquet **complet** de la capture, son horodatage, et le trou s'il y en a eu |
//! | `GetOutputStreamPresentationPosition` | la position en **trames absolues** et le QPC brut, ceux de `GetPosition` |
//! | `GetPacketCount` | les paquets entièrement transférés, en **base 1**, remis à zéro en `KSSTATE_STOP` |
//!
//! Toute l'arithmétique est dans `conduit_kmd_core::packetnum` — la conversion entre le
//! `ULONG` du contrat et notre compteur 64 bits, la comparaison **modulo 2³²**, et les deux
//! refus documentés — parce que c'est la partie qu'on peut éprouver sans machine virtuelle,
//! au proptest et au fuzzer. Ce qui reste ici est l'accès à l'état, sous le verrou du flux.
//!
//! Le lot 2 exposait ces interfaces **sans les servir**, derrière `PacketMode = 1`, pour
//! mesurer ce que le moteur audio en faisait. La mesure a répondu : un client WASAPI exclusif
//! événementiel appelle `GetReadPacket` quatre cents fois par seconde dès qu'elles existent,
//! et le refus casse son transport. C'est cet état — exposer sans servir — que le lot 3 fait
//! cesser. `PacketMode` est à **1 par défaut** depuis la campagne Driver Verifier du
//! 2026-09-10 ; il ne commande plus une expérience, seulement l'exposition, et 0 n'en est
//! plus que le repli de diagnostic.
//!
//! # Ce que les quatre méthodes comptent
//!
//! Chaque appel note, dans le [`Cable`] — pas dans le flux, qui peut se fermer aussitôt
//! après — : la méthode et le sens, l'IRQL courant (`KeGetCurrentIrql`, pour **vérifier** le
//! `PASSIVE_LEVEL` que `portcls.h` promet plutôt que le supposer) et l'horodatage QPC, dont le
//! premier et le dernier sont retenus. Les `QueryInterface` sur les deux IID de paquets sont
//! comptés de la même façon, **exposés ou non** (`portcls::packet`, points d'observation).
//!
//! Les compteurs sont ceux du lot 2, inchangés : c'est ce qu'ils comptent qui a changé — des
//! appels servis, non plus des refus —, et `conduit-looptest` le dit désormais autrement.
//!
//! # IRQL, verrous et pagination
//!
//! Les quatre méthodes arrivent à `PASSIVE_LEVEL` et prennent le verrou du flux (les trois
//! premières) ou celui du câble puis du flux (`SetWritePacket`), c'est-à-dire les **mêmes**
//! verrous que la DPC du minuteur prend à `DISPATCH_LEVEL`. C'est sûr parce que
//! [`crate::sync::SpinLock::lock`] appelle `KeAcquireSpinLockRaiseToDpc` : le fil qui tient le
//! verrou est lui-même à `DISPATCH_LEVEL`, sa propre DPC ne peut donc pas le préempter sur ce
//! processeur. Un verrou qui n'élèverait pas l'IRQL ferait ici un interblocage franc — c'est
//! le point qu'il fallait vérifier avant d'écrire une ligne de ce lot.
//!
//! **Rien de paginé dans ces quatre méthodes** : elles sont à `PASSIVE_LEVEL` par contrat mais
//! sur le chemin du flux, et SYSVAD les marque explicitement non paginées. Ce workspace n'a
//! aucune section paginée — aucun `#[link_section]`, aucun segment `PAGE` : tout le binaire est
//! résident, c'est à ne pas casser, pas à faire —, et ce qu'elles exécutent se réduit à des
//! atomiques, à un `KeQueryPerformanceCounter` et à de l'arithmétique entière `no_std`.
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
//! `conduit_kmd_core::format::buffer_bytes[_for_notifications]_with_floor` : multiple de la
//! trame et de la période de notification, jamais inférieure à la demande **ni au plancher
//! `BufferMs` du registre** (M1b-05) — au-delà de 500 ms l'allocation est refusée plutôt
//! qu'écrêtée), mappé en mémoire noyau (`MmCached`) et mis
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

use conduit_kmd_core::config::PacketExposure;
use conduit_kmd_core::packetnum;
use conduit_kmd_core::{
    Notifier, SupportedFormat, buffer_bytes_for_notifications_with_floor, buffer_bytes_with_floor,
};
use portcls::conduit_com::{
    NtStatus, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
    STATUS_UNSUCCESSFUL,
};
use portcls::{
    AudioBuffer, MiniportWaveRTInputStream, MiniportWaveRTOutputStream, MiniportWaveRTStream,
    MiniportWaveRTStreamNotification, PortWaveRTStream, ReadPacket, STATUS_DEVICE_NOT_READY,
    physical_address,
};
use portcls_sys::{
    _MEMORY_CACHING_TYPE, KSAUDIO_PRESENTATION_POSITION, KSRTAUDIO_HWLATENCY, KSSTATE,
    KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY, KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM, PKEVENT,
    PMDL,
};
use wdk_sys::ntddk::KeGetCurrentIrql;
use wdk_sys::{
    KEVENT, PAGE_SIZE, STATUS_DEVICE_BUSY, STATUS_INVALID_DEVICE_REQUEST, STATUS_NOT_FOUND,
};

use crate::cable::{
    Buffer, Cable, Direction, MAX_NOTIFICATION_EVENTS, PacketMethod, SharedStream, StreamState,
};
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
    /// `format`, exposant les interfaces de paquets `packet_exposure`. Lit la fréquence du
    /// compteur de performance une fois pour toutes.
    ///
    /// `packet_exposure` est **l'image** de la `PacketInterfaces` que l'appelant passera à
    /// `try_new_packet_stream_object` : c'est `wave::open_stream` qui décide des deux, d'un
    /// seul geste, pour qu'aucun chemin ne puisse annoncer autre chose que ce qu'il expose. La
    /// valeur n'est mémorisée que pour le relevé (`KSPROPERTY_CONDUIT_PACKETS`) ; le
    /// `QueryInterface` composite, lui, ne consulte que la `PacketInterfaces` de l'objet COM.
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
        packet_exposure: PacketExposure,
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
            shared: SharedStream::new(StreamState::new(clock, layout, packet_exposure)),
        })
    }

    /// Note l'entrée dans une méthode du mode paquets et rend l'IRQL relevé.
    ///
    /// Un seul point de passage pour les quatre méthodes : le relevé d'IRQL et l'horodatage y
    /// sont écrits une fois, et aucune des quatre ne peut diverger des autres par
    /// distraction. C'est la même raison qui a fait envelopper [`Self::allocate_inner`] — un
    /// compteur qui rate un cas est pire qu'aucun compteur.
    ///
    /// L'IRQL est relevé **ici**, au plus près de l'appel : le mesurer plus loin donnerait
    /// l'IRQL de notre propre code, qui ne l'a pas changé mais qui n'est plus la réponse à la
    /// question posée. Il est rendu à l'appelant pour que sa trace le porte sans le relire —
    /// une seconde lecture donnerait une seconde valeur, et ce n'est pas celle-là qu'on
    /// mesure.
    ///
    /// IRQL : `PASSIVE_LEVEL` par contrat (`portcls.h`), et rien ici n'exige mieux ; code non
    /// paginé, sans allocation ni verrou.
    fn noter_paquet(&self, methode: PacketMethod) -> u32 {
        // SAFETY: `KeGetCurrentIrql` n'a aucune précondition et n'a pas de paramètre.
        let irql = u32::from(unsafe { KeGetCurrentIrql() });
        self.cable.note_packet_call(self.direction, methode, irql);
        irql
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
    /// de notification par tour (`None` : `AllocateAudioBuffer`, sans notification), et
    /// **compte le refus** si l'allocation échoue.
    ///
    /// # Un seul endroit qui compte, et c'est pour cela qu'il enveloppe
    ///
    /// Le travail est dans [`Self::allocate_inner`], qui a huit sorties en erreur. Les
    /// compter une par une reviendrait à en oublier une le jour où l'on en ajoute une
    /// neuvième, et un compteur de refus qui rate un refus est pire qu'aucun compteur : il
    /// **affirme** que le moteur audio n'a essuyé aucun « non ». L'enveloppe rend l'oubli
    /// impossible.
    ///
    /// Le compteur vit sur le câble et non sur ce flux : voir `cable::AllocRefusals`.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn allocate(
        &self,
        notification_count: Option<u32>,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        let issue = self.allocate_inner(notification_count, requested_bytes);
        if let Err(status) = issue {
            self.cable.note_allocation_refusal(self.direction);
            kmd_log!(
                "{}{}::Allocate REFUSÉ ({status:#010x}, notifications {notification_count:?}) : le moteur audio va retomber en scrutation sans le dire",
                self.name(),
                self.n
            );
        }
        issue
    }

    /// Le travail de [`Self::allocate`], dont chaque sortie en erreur est un refus.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn allocate_inner(
        &self,
        notification_count: Option<u32>,
        requested_bytes: u32,
    ) -> Result<AudioBuffer, NtStatus> {
        // Journaliser l'ENTRÉE, pas seulement le succès : le moteur audio demande d'abord
        // un tampon avec notifications, et un refus de notre part le fait retomber
        // silencieusement en mode scrutation. C'est exactement ce qui a coûté une journée
        // de diagnostic le 2026-09-06 : la trace ne montrait que « notifications None »,
        // sans dire qu'une demande avec notifications avait été refusée juste avant.
        // Depuis le lot 0 du mode paquets, le refus se lit **aussi** en release, par
        // `KSPROPERTY_CONDUIT_TRANSPORT` : `kmd_log!` est vide hors debug.
        kmd_log!(
            "{}{}::Allocate(notifications {notification_count:?}, {requested_bytes} octets)",
            self.name(),
            self.n
        );
        let rate = self.format.sample_rate;
        // Le plancher vient du registre (`BufferMs`, M1b-01 pour la lecture, M1b-05 pour
        // l'application) : le tampon ne descend jamais sous cette durée, quelle que soit la
        // demande du moteur audio. C'est le seul effet du paramètre, et il est ici parce
        // que c'est le seul endroit où une taille de tampon se décide.
        let floor_ms = crate::registry::buffer_ms();
        let bytes = match notification_count {
            None => buffer_bytes_with_floor(requested_bytes, self.frame_bytes, rate, floor_ms),
            // La documentation d'`AllocateBufferWithNotification` annonce « Valid values
            // are 1 or 2 », et nous n'avons jamais rien observé d'autre — l'absence
            // d'observation ne se cite pas comme une mesure. Nous restons pourtant
            // permissifs comme SYSVAD, qui n'exige que la divisibilité de la taille :
            // une borne arbitraire ici faisait échouer le mode événementiel sans laisser
            // la moindre trace, et refuser plus que ce que le contrat impose n'a rien
            // rapporté.
            Some(count) => buffer_bytes_for_notifications_with_floor(
                requested_bytes,
                self.frame_bytes,
                rate,
                count,
                floor_ms,
            ),
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
            // Ce que le moteur audio a **demandé**, pour que la propriété de transport le
            // rende tel quel : 0 quand il n'a rien demandé.
            state.notification_count = notification_count.unwrap_or(0);
            // Nouveau tampon, nouvelle géométrie de paquets : un numéro hérité de
            // l'allocation précédente ne désignerait plus les mêmes trames.
            state.reset_packets();
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
            state.notification_count = 0;
            // Sans tampon il n'y a plus de paquet : la géométrie disparaît avec la MDL, et
            // les quatre méthodes refuseront jusqu'à la prochaine allocation.
            state.reset_packets();
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
            state.notification_count = 0;
            state.reset_packets();
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
                    // Le mode paquets repart de zéro avec la position : les quatre méthodes
                    // documentent toutes cette remise à zéro, et le client recommence sa
                    // numérotation. Un dernier numéro survivant à l'arrêt ferait refuser
                    // en `STATUS_DATA_LATE_ERROR` le premier paquet du flux suivant.
                    shared.reset_packets();
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

// ---------------------------------------------------------------------------------
// Mode paquets : les quatre méthodes **servent** et **comptent** (lot 3).
//
// Elles ne sont atteignables que sous `PacketMode = 1` — le défaut depuis la campagne Driver
// Verifier du 2026-09-10 —, qui expose les interfaces du sens du flux ; sur le repli de
// diagnostic (0), PortCls n'obtient jamais l'adresse des têtes satellites et rien de ce qui
// suit ne tourne. Ce que le paramètre commande n'est plus une expérience — exposer sans
// servir — mais l'exposition seule.
//
// Elles prennent le verrou du flux (les trois premières) ou celui du câble puis du flux
// (`SetWritePacket`), n'allouent rien et n'attendent rien. Rien de paginé (voir l'en-tête de
// module). L'arithmétique est dans `conduit_kmd_core::packetnum`, éprouvée sans noyau.
// ---------------------------------------------------------------------------------

impl MiniportWaveRTInputStream for WaveStream {
    /// Le dernier paquet **complet** que la boucle locale a écrit dans le tampon de capture.
    ///
    /// C'est un *pull* : rien n'est déclenché ici, rien n'attend, et ce qui réveille le
    /// client reste l'événement de `RegisterNotificationEvent`. Le numéro est monotone et
    /// jamais rendu deux fois ; son adresse dans le tampon n'a pas de paramètre de sortie
    /// dans la vtable générée — le moteur audio la calcule lui-même par
    /// `(numéro % NotificationCount) × taille_de_paquet`, la formule que
    /// `PacketGeometry::ring_offset_frames` écrit de notre côté.
    ///
    /// Un trou — des paquets complets écrasés avant que le moteur ne vienne les chercher —
    /// se dit par `KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY` : c'est la seule information
    /// de trou que la signature laisse passer, et la taire serait mentir.
    ///
    /// `more_data` est **toujours faux** : nous rendons le paquet le plus récent, il n'y a
    /// donc rien après lui. Le champ existe pour un pilote qui rendrait le plus ancien non
    /// lu ; le nôtre suit la documentation, qui autorise à supposer que l'OS a fini de lire
    /// tous les paquets précédents.
    ///
    /// IRQL: PASSIVE_LEVEL (contrat), code non paginé.
    fn read_packet(&self) -> Result<ReadPacket, NtStatus> {
        let _ = self.noter_paquet(PacketMethod::GetReadPacket);
        let now = clock::now();
        let paquet = {
            let mut state = self.shared.lock();
            state.take_read_packet(now)
        };
        let Some((numero, trou)) = paquet else {
            // Pas d'erreur : le seul refus que `GetReadPacket` documente, et il vaut mieux
            // que la seule chose qu'elle interdise — un `Ok` sur un paquet déjà rendu.
            return Err(STATUS_DEVICE_NOT_READY);
        };
        if trou {
            kmd_log!(
                "{}{}::GetReadPacket paquet {numero} : discontinuité (des paquets complets \
                 ont été écrasés avant d'être lus)",
                self.name(),
                self.n
            );
        }
        Ok(ReadPacket {
            packet_number: packetnum::truncate(numero),
            flags: if trou {
                KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY
            } else {
                0
            },
            performance_counter: now,
            more_data: false,
        })
    }

    // IRQL: <= DISPATCH_LEVEL (celui de `QueryInterface`) — deux `fetch_add`.
    fn note_input_query(&self, rendu: bool) {
        self.cable.note_packet_query(self.direction, rendu);
    }
}

impl MiniportWaveRTOutputStream for WaveStream {
    /// Le moteur audio vient d'écrire le paquet `packet_number` dans le tampon de rendu.
    ///
    /// Un **hint**, pas un déclencheur exclusif : il n'exempte pas le pilote du minuteur, et
    /// celui-ci n'a pas changé d'une ligne. Ce qu'il apporte est une frontière **certaine**
    /// là où la boucle supposait — la copie ne lira pas au-delà de ce paquet
    /// (`StreamView::committed`) — et un second déclencheur du même corps de boucle,
    /// [`Cable::on_write_packet`], qui prend les mêmes verrous dans le même ordre.
    ///
    /// `KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM` (`ks.h`, `0x200`) est le **seul** drapeau
    /// défini : tout autre bit vaut `STATUS_INVALID_PARAMETER`, comme le contrat l'exige. Le
    /// refus des drapeaux se fait ici, hors de tout verrou — il ne dépend d'aucun état.
    ///
    /// IRQL: PASSIVE_LEVEL (contrat), code non paginé.
    fn set_write_packet(&self, packet_number: u32, flags: u32, eos_packet_length: u32) -> NtStatus {
        let irql = self.noter_paquet(PacketMethod::SetWritePacket);
        if flags & !KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM != 0 {
            kmd_log!(
                "{}{}::SetWritePacket({packet_number}) : drapeaux {flags:#010x} inconnus \
                 (seul ENDOFSTREAM est défini)",
                self.name(),
                self.n
            );
            return STATUS_INVALID_PARAMETER;
        }
        // Le champ n'a de sens qu'avec le drapeau ; sans lui, le contrat dit de l'ignorer.
        let eos_bytes =
            (flags & KSSTREAM_HEADER_OPTIONSF_ENDOFSTREAM != 0).then_some(eos_packet_length);
        let status =
            self.cable
                .on_write_packet(self.direction, packet_number, eos_bytes, clock::now());
        if status != STATUS_SUCCESS {
            kmd_log!(
                "{}{}::SetWritePacket({packet_number}) : {status:#010x} (IRQL {irql}) — le \
                 client se recalera par GetPacketCount",
                self.name(),
                self.n
            );
        }
        status
    }

    /// La position de présentation : des **trames absolues** depuis le début du flux et la
    /// valeur **brute** du compteur de performance.
    ///
    /// Ni octets ni unités de 100 ns — c'est l'erreur que `KSAUDIO_PRESENTATION_POSITION`
    /// invite à faire — et c'est exactement ce que `crate::clock` et
    /// `conduit_kmd_core::position` calculent déjà pour `GetPosition`, sans arrondi
    /// supplémentaire : les deux méthodes lisent la même position, l'une en octets
    /// cycliques, l'autre en trames absolues.
    ///
    /// L'écart avec [`packet_count`](Self::packet_count) est la latence matérielle : nulle
    /// par construction pour un pont logiciel, ce qui fait coïncider les deux ici.
    ///
    /// IRQL: PASSIVE_LEVEL (contrat), code non paginé.
    fn presentation_position(&self) -> Result<KSAUDIO_PRESENTATION_POSITION, NtStatus> {
        let _ = self.noter_paquet(PacketMethod::PresentationPosition);
        let now = clock::now();
        let frames = {
            let state = self.shared.lock();
            state.frames_at(now)
        };
        Ok(KSAUDIO_PRESENTATION_POSITION {
            u64PositionInBlocks: frames,
            u64QPCPosition: now,
        })
    }

    /// Le nombre de paquets **entièrement** transférés, en base 1.
    ///
    /// « If the packet count is 5, then 5 packets have completely transferred. That is,
    /// packets 0-4 have completely transferred. » C'est le mécanisme de resynchronisation
    /// du client, et surtout ce qu'il interroge après un `STATUS_DATA_LATE_ERROR` ou un
    /// `STATUS_DATA_OVERRUN` : un événement de notification ne dit pas quel paquet il
    /// concerne, ce compte si.
    ///
    /// `STATUS_DEVICE_NOT_READY` sans tampon à notifications : il n'existe alors aucun
    /// paquet à compter, et rendre zéro laisserait croire à un flux qui vient de démarrer.
    ///
    /// Le compte ne recule jamais, même quand la position se fige (`KSSTATE_PAUSE`) ; il
    /// repart de zéro au `KSSTATE_STOP`, comme le contrat l'exige. Tronqué à 32 bits comme
    /// le `ULONG` du contrat : c'est le même domaine que les numéros de paquet, et
    /// `packetnum` les compare modulo.
    ///
    /// IRQL: PASSIVE_LEVEL (contrat), code non paginé.
    fn packet_count(&self) -> Result<u32, NtStatus> {
        let _ = self.noter_paquet(PacketMethod::PacketCount);
        let now = clock::now();
        let compte = {
            let mut state = self.shared.lock();
            state.packets_transferred(now)
        };
        compte
            .map(packetnum::truncate)
            .ok_or(STATUS_DEVICE_NOT_READY)
    }

    // IRQL: <= DISPATCH_LEVEL (celui de `QueryInterface`) — deux `fetch_add`.
    fn note_output_query(&self, rendu: bool) {
        self.cable.note_packet_query(self.direction, rendu);
    }
}
