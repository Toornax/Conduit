//! Binaire `conduitd`.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::Parser;
use conduit_backend::null::NullBackend;
use conduit_backend::Backend;
use conduitd::config::Config;
use conduitd::paths::Paths;
use conduitd::{Daemon, DaemonOptions};

/// Démon audio Conduit.
#[derive(Debug, Parser)]
#[command(name = "conduitd", version, about)]
struct Args {
    /// Fichier de configuration TOML (défaut : répertoire de configuration de l'utilisateur).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Répertoire racine unique pour configuration, données et socket (tests, portable).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Chemin du socket Unix ou nom du named pipe.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Backend audio : `auto` ou `null` (simulé).
    #[arg(long, default_value = "auto")]
    backend: String,
    /// Niveau de journalisation (remplace [log] level).
    #[arg(long)]
    log_level: Option<String>,
    /// Désactive le watchdog du fil audio.
    #[arg(long)]
    no_watchdog: bool,
    /// Désactive la persistance de l'état.
    #[arg(long)]
    no_persist: bool,
}

fn main() {
    let args = Args::parse();
    let mut paths = match &args.root {
        Some(root) => Paths::under(root),
        None => {
            Paths::standard().unwrap_or_else(|| Paths::under(&std::env::temp_dir().join("conduit")))
        }
    };
    if let Some(c) = &args.config {
        paths.config_file = c.clone();
    }
    if let Some(s) = &args.socket {
        paths.socket = s.clone();
    }
    let config = match Config::load(&paths.config_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("impossible de créer les répertoires : {e}");
        std::process::exit(2);
    }
    let level = args
        .log_level
        .clone()
        .unwrap_or_else(|| config.log.level.clone());
    let _guard =
        conduitd::logging::init(&level, config.log.file.then_some(paths.log_dir.as_path()));
    let backend: Box<dyn Backend> = match args.backend.as_str() {
        "null" => Box::new(NullBackend::new()),
        "auto" => {
            tracing::warn!("aucun backend natif disponible sur cette plateforme dans cette version : backend null (simulé)");
            Box::new(NullBackend::new())
        }
        other => {
            eprintln!("backend inconnu « {other} » : auto ou null");
            std::process::exit(2);
        }
    };
    let options = DaemonOptions {
        paths,
        config,
        watchdog: !args.no_watchdog,
        persist: !args.no_persist,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime tokio");
    runtime.block_on(async move {
        match Daemon::spawn(backend, options) {
            Ok(daemon) => daemon.run_until_signal().await,
            Err(e) => {
                tracing::error!("{e}");
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    });
}
