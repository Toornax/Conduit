//! `conduit-gui` — interface graphique de Conduit (F-40, SPEC §5.8).
//!
//! La GUI est un **simple client IPC** du démon (ADR-005) : elle ne contient
//! aucune logique audio et peut être fermée sans effet sur le son. Tout ce
//! qu'elle fait passe par `conduit-protocol`.
//!
//! Organisation :
//!
//! - [`i18n`] : table des textes affichés (une seule langue pour l'instant) ;
//! - [`view`] et [`cables`] : décor de la fenêtre et vue « Câbles » ;
//! - [`model`] : miroir de l'état du démon, alimenté par réduction pure des
//!   notifications ;
//! - [`ipc`] : connexion, chargement initial, boucle d'événements et
//!   reconnexion — **indépendante d'`iced`**, donc testable sans fenêtre ;
//! - [`app`] : architecture Elm d'`iced` (`Message`, `update`, `view`,
//!   `subscription`).
//!
//! Le binaire s'appelle `conduit`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod app;
pub mod cables;
pub mod i18n;
pub mod ipc;
pub mod model;
pub mod view;

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
