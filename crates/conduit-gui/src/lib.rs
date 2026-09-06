//! `conduit-gui` — interface graphique de Conduit (F-40, SPEC §5.8).
//!
//! La GUI est un **simple client IPC** du démon (ADR-005) : elle ne contient
//! aucune logique audio et peut être fermée sans effet sur le son. Tout ce
//! qu'elle fait passe par `conduit-protocol`.
//!
//! Organisation :
//!
//! - [`i18n`] : table des textes affichés (une seule langue pour l'instant) ;
//! - [`theme`] : jetons de couleur et thèmes « Sericæ » clair et sombre ;
//! - [`typo`] : les trois familles embarquées et les capitales espacées ;
//! - [`style`] : les closures de style des widgets `iced` ;
//! - [`mod@format`] : mise en forme française des grandeurs affichées ;
//! - [`shell`], [`cables`], [`patchbay`] et [`diagnostic`] : coquille de la
//!   fenêtre (barre latérale, en-tête, bandeau de notice) et les trois vues
//!   qui l'habitent ;
//! - [`etats`] : les deux écrans qui remplacent la vue — démon absent, premier
//!   lancement ;
//! - [`demarrage`] : où trouver le démon, comment le lancer quand il manque
//!   (F-51) et où il écrit son journal ;
//! - [`erreurs`] : le conseil qui complète chaque message d'erreur (ADR-006) ;
//! - [`preferences`] : ce que la fenêtre retient d'une session à l'autre ;
//! - [`model`] : miroir de l'état du démon, alimenté par réduction pure des
//!   notifications ;
//! - [`ipc`] : connexion, chargement initial, boucle d'événements et
//!   reconnexion — **indépendante d'`iced`**, donc testable sans fenêtre ;
//! - [`app`] : architecture Elm d'`iced` (`Message`, `update`, `view`,
//!   `subscription`).
//!
//! Le design system est décrit dans `docs/design-system.md` ; les polices
//! embarquées et leur licence, dans l'ADR-014.
//!
//! Le binaire s'appelle `conduit`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod app;
pub mod cables;
pub mod demarrage;
pub mod diagnostic;
pub mod erreurs;
pub mod etats;
pub mod format;
pub mod i18n;
pub mod ipc;
pub mod model;
pub mod patchbay;
pub mod preferences;
pub mod shell;
pub mod style;
pub mod theme;
pub mod typo;

pub use app::run;

/// Initialise la journalisation (`RUST_LOG`, sortie d'erreur standard).
///
/// Sans effet si un abonné `tracing` est déjà installé.
pub fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
