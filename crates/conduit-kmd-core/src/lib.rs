//! `conduit-kmd-core` — logique portable du pilote noyau Windows.
//!
//! Ce crate regroupe tout ce qui, dans le pilote de câbles `conduit-kmd`
//! ([`docs/driver-design.md`](https://github.com/ToorNax/conduit/blob/main/docs/driver-design.md)
//! §2.1, ADR-012), se raisonne et se teste **sans noyau** :
//!
//! - l'horloge virtuelle et les positions de flux ([`position`]) : trames écoulées
//!   depuis un instant du compteur de performance, position cyclique en octets ;
//! - la copie cyclique rendu → capture ([`ring`]) : tampons de tailles différentes,
//!   wrap-around des deux côtés, conversion entre F32, PCM24 et I16 (les neuf couples),
//!   silence ;
//! - la validation des formats ([`format`]) : format demandé par le moteur audio
//!   contre la liste supportée — une fréquence et un nombre de canaux par câble, les
//!   trois profondeurs toujours (M1b-05) —, taille de tampon bornée ;
//! - la négociation de format ([`wavefmt`]) : l'intersection de deux plages audio
//!   (`IMiniport::DataRangeIntersection`), le choix entre `WAVEFORMATEX` simple et
//!   `WAVEFORMATEXTENSIBLE`, et le masque de haut-parleurs qui va avec le nombre de
//!   canaux ;
//! - les périodes de notification ([`notify`]) : quand signaler les événements
//!   enregistrés par `IMiniportWaveRTStreamNotification`, à partir de la position
//!   absolue, bouclage compris ;
//! - les bornes des paramètres de registre ([`params`]) : réserve de câbles, canaux et
//!   durée de tampon, écrêtés vers leur borne avec un rapport de ce qui a été corrigé,
//!   pour qu'un registre aberrant ne fasse jamais échouer le chargement (M1b-01) ;
//! - le contrat du jeu de propriétés KS privé de configuration ([`config`]) : le GUID du
//!   jeu, la structure d'échange et **tout** son parseur, plus le masque de bits qui
//!   persiste l'état actif des câbles (M1b-04, M1b-08) ;
//! - le plan de copie de la boucle locale ([`loopback`]) : à chaque tick du timer du
//!   câble, quelles trames de rendu écrire à quelles trames de capture, quand écrire
//!   du silence, et quand le tick est trop en retard pour rattraper.
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

pub mod config;
pub mod format;
pub mod loopback;
pub mod notify;
pub mod params;
pub mod position;
pub mod ring;
pub mod wavefmt;

pub use config::{
    cable_bit, is_active, sanitize_mask, with_active, CableCounters, CableFormat, CableFormatFix,
    CableState, ConfigError, ConfigGuid, CountersError, FormatCodeError, MaskFix,
    ACTIVE_CABLES_DEFAULT, ACTIVE_CABLES_LABEL, ACTIVE_CABLES_MASK, ACTIVE_CABLES_VALUE_NAME,
    CABLE_COUNTERS_BYTES, CABLE_FORMAT_DEFAULT, CABLE_FORMAT_LABEL, CABLE_FORMAT_VALUE_NAMES,
    CABLE_MAX, CABLE_STATE_BYTES, CONFIG_VERSION, KSPROPERTY_CONDUIT_CABLE_STATE,
    KSPROPERTY_CONDUIT_COUNTERS, KSPROPERTY_CONDUIT_VERSION, KSPROPSETID_CONDUIT,
};
pub use format::{
    buffer_bytes, buffer_bytes_for_notifications, buffer_bytes_for_notifications_with_floor,
    buffer_bytes_with_floor, cable_formats, sample_rate_at, sample_rate_index, validate,
    variant_channels, variant_index, variant_of, variant_rate, FormatError, RequestedFormat,
    SampleKind, SupportedFormat, FORMATS_PER_CABLE, MAX_CHANNELS_PER_CABLE, SAMPLE_DEPTHS,
    SAMPLE_RATES, VARIANT_COUNT,
};
pub use loopback::{CopyOp, Loopback, Plan, SilenceCause, SilenceOp, StreamView, LEAD_MS};
pub use notify::{align_frames, boundaries_crossed, Notifier};
pub use params::{
    decode_dword, sanitize, Correction, Fix, Param, Params, RawParams, Report, REG_DWORD,
    REG_DWORD_BYTES,
};
pub use position::{byte_offset, StreamPosition, VirtualClock};
pub use ring::{copy_frames, silence, FrameLayout, RingError, SampleFormat};
pub use wavefmt::{
    intersect, needs_extensible, speaker_mask, AudioRange, NoMatch, WaveFormat,
    KSDATAFORMAT_WAVEFORMATEXTENSIBLE_BYTES, KSDATAFORMAT_WAVEFORMATEX_BYTES,
    WAVEFORMATEXTENSIBLE_CB_SIZE, WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM,
};
