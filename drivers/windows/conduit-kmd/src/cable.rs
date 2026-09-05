//! État partagé d'un câble (driver-design.md §2.3, §5.3) : les flux rendu et capture
//! courants, que la boucle locale (M1a-08) lira sous le timer/DPC du câble.
//!
//! M1a-06 ne livre que la structure : un [`Cable`] par câble, deux [`Slot`] (un par
//! sens) qui mémorisent le pointeur d'état du flux ouvert sur la broche système du
//! filtre WaveRT correspondant ; `None` quand la broche est fermée. Les miniports WaveRT
//! (`wave::WaveRender`, `wave::WaveCapture`) reçoivent un `&'static Cable` à leur
//! création dans `StartDevice` (`adapter`).
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
//! # Synchronisation
//!
//! Chaque [`Slot`] est un `AtomicPtr` : `NewStream` (`PASSIVE_LEVEL`) y dépose le pointeur
//! d'état du flux, la fermeture du flux le retire, la DPC de copie (`DISPATCH_LEVEL`) le
//! lit sans verrou. Ce qui n'est **pas** garanti par l'atomique seul : que l'état visé
//! survive à la lecture. M1a-08 devra retirer le pointeur du slot **avant** de libérer
//! l'état et attendre la fin de la DPC en cours (`KeCancelTimer` + `KeFlushQueuedDpcs`,
//! ou un spin lock `KSPIN_LOCK` autour de la lecture et de la libération) ; le choix
//! est laissé à la tâche qui écrit la DPC.

use core::fmt;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

/// État d'un flux WaveRT ouvert, tel que le câble le voit.
///
/// M1a-06 : type opaque, jamais instancié (`NewStream` refuse encore les flux). M1a-07 y
/// mettra le format retenu, le tampon cyclique (MDL, base mappée, taille) et la position
/// (`conduit_kmd_core::StreamPosition`), en mémoire non paginée.
#[derive(Debug)]
pub struct StreamState {
    _opaque: (),
}

/// Le flux courant d'un sens d'un câble : pointeur d'état ou nul.
pub struct Slot(AtomicPtr<StreamState>);

impl Slot {
    /// Emplacement vide.
    pub const fn new() -> Self {
        Self(AtomicPtr::new(ptr::null_mut()))
    }

    /// Le pointeur d'état courant (nul si aucun flux n'est ouvert).
    ///
    /// IRQL : tout niveau (lecture atomique).
    pub fn get(&self) -> *mut StreamState {
        self.0.load(Ordering::Acquire)
    }

    /// Remplace le pointeur d'état et rend l'ancien (nul s'il n'y en avait pas).
    ///
    /// IRQL : tout niveau (échange atomique).
    pub fn swap(&self, state: *mut StreamState) -> *mut StreamState {
        self.0.swap(state, Ordering::AcqRel)
    }

    /// Vide l'emplacement et rend l'ancien pointeur.
    ///
    /// IRQL : tout niveau.
    pub fn take(&self) -> *mut StreamState {
        self.swap(ptr::null_mut())
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Slot").field(&self.get()).finish()
    }
}

/// Un câble : son numéro et ses deux flux courants.
#[derive(Debug)]
pub struct Cable {
    /// Numéro du câble (0 pour le seul câble de M1a ; « Conduit 1 » pour l'utilisateur).
    pub index: u32,
    /// Flux ouvert sur la broche système de `WaveRender<index>`.
    pub render: Slot,
    /// Flux ouvert sur la broche système de `WaveCapture<index>`.
    pub capture: Slot,
}

impl Cable {
    /// Câble `index`, sans flux.
    pub const fn new(index: u32) -> Self {
        Self {
            index,
            render: Slot::new(),
            capture: Slot::new(),
        }
    }

    /// Oublie les deux flux (à l'entrée de `StartDevice` : les broches sont fermées
    /// avant tout arrêt du périphérique, les emplacements devraient déjà être vides).
    /// Rend vrai si l'un d'eux ne l'était pas, pour journalisation.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    pub fn reset(&self) -> bool {
        let render = self.render.take();
        let capture = self.capture.take();
        !render.is_null() || !capture.is_null()
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
