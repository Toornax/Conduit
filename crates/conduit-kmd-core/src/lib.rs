//! `conduit-kmd-core` — logique portable du pilote noyau Windows.
//!
//! Ce crate regroupe tout ce qui, dans le pilote de câbles `conduit-kmd`
//! ([`docs/driver-design.md`](https://github.com/ToorNax/conduit/blob/main/docs/driver-design.md)
//! §2.1, ADR-012), se raisonne et se teste **sans noyau** :
//!
//! - l'horloge virtuelle et les positions de flux ([`position`]) : trames écoulées
//!   depuis un instant du compteur de performance, position cyclique en octets ;
//! - la copie cyclique rendu → capture ([`ring`]) : tampons de tailles différentes,
//!   wrap-around des deux côtés, conversion F32 ↔ I16, silence ;
//! - la validation des formats ([`format`]) : format demandé par le moteur audio
//!   contre la liste supportée, taille de tampon bornée.
//!
//! Le pilote (workspace `drivers/windows`, jamais construit par Nix) dépend de ce
//! crate par chemin ; le workspace racine le compile, le teste (proptest, Miri,
//! couverture) depuis Linux et macOS.
//!
//! # Contraintes : code appelé à `DISPATCH_LEVEL`
//!
//! Les fonctions de ce crate sont appelées depuis `GetPosition`, le DPC de copie et
//! les gestionnaires de propriétés d'un pilote WDM, c'est-à-dire souvent à
//! `DISPATCH_LEVEL`, sous spin lock. Dans ce contexte une panique est un écran bleu
//! (`KeBugCheckEx`) et une allocation est interdite. D'où les règles, imposées par
//! les attributs de ce fichier :
//!
//! - **jamais de panique** : `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`,
//!   indexation non vérifiée (`a[i]`, `a[i..j]`) sont refusés à la compilation ; tout
//!   chemin faillible renvoie `Option` ou `Result` ;
//! - **jamais d'allocation** : `#![no_std]`, aucune dépendance, pas d'`alloc` ; les
//!   fonctions travaillent sur des tranches fournies par l'appelant ;
//! - **arithmétique explicite** : les opérateurs `+ - * / %` sur entiers sont refusés
//!   (`clippy::arithmetic_side_effects`) ; on écrit `checked_*` quand le débordement
//!   est une erreur à propager, `saturating_*` quand une borne est le comportement
//!   voulu (compteurs de trames), `wrapping_*` seulement lorsqu'un modulo est la
//!   sémantique (et on le dit en commentaire) ;
//! - **pas d'`unsafe`** (`#![forbid(unsafe_code)]`) : les transmutations vers les
//!   types du WDK vivent dans `conduit-kmd`, pas ici.
//!
//! La feature `std` (vide) ne sert qu'aux tests, au fuzz et aux outils utilisateur.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod format;
pub mod position;
pub mod ring;

pub use format::{
    buffer_bytes, validate, FormatError, RequestedFormat, SampleKind, SupportedFormat, M1A_FORMATS,
};
pub use position::{byte_offset, StreamPosition, VirtualClock};
pub use ring::{copy_frames, silence, FrameLayout, RingError, SampleFormat};
