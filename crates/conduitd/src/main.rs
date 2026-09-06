//! Binaire `conduitd`.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand};
use conduit_backend::null::NullBackend;
use conduit_backend::Backend;
use conduitd::autostart;
use conduitd::config::Config;
use conduitd::paths::Paths;
use conduitd::single_instance::{self, Instance};
use conduitd::{Daemon, DaemonOptions};

/// Démon audio Conduit.
///
/// Sans sous-commande, `conduitd` démarre le démon et tourne jusqu'à Ctrl-C (ou, sous
/// Windows, jusqu'à la fermeture de session).
///
/// Codes de retour : 0 fin normale, 1 échec de démarrage, 2 usage ou configuration,
/// 3 un démon tourne déjà pour cette session.
#[derive(Debug, Parser)]
#[command(name = "conduitd", version, about)]
struct Args {
    /// Sous-commande d'administration (par défaut : démarrer le démon).
    #[command(subcommand)]
    command: Option<Sub>,
    /// Fichier de configuration TOML (défaut : répertoire de configuration de l'utilisateur).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Répertoire racine unique pour configuration, données et socket (tests, portable).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Chemin du socket Unix ou nom du named pipe.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Backend audio : `auto`, `null` (simulé) ou `wasapi` (Windows).
    ///
    /// `auto` choisit le backend natif de la plateforme (WASAPI sous Windows) et se
    /// replie sur `null` s'il manque ou ne démarre pas.
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

/// Sous-commandes d'administration.
#[derive(Debug, Subcommand)]
enum Sub {
    /// Démarrage automatique du démon à l'ouverture de session.
    ///
    /// Sous Windows, une tâche planifiée par utilisateur nommée `Conduit\conduitd`
    /// (déclencheur « à l'ouverture de session de cet utilisateur », 30 s de délai,
    /// redémarrage en cas d'échec, pas d'arrêt sur batterie, pas de limite de durée).
    /// Ce n'est volontairement pas un service Windows : un service tourne en session 0,
    /// sans l'audio de l'utilisateur (ADR-013).
    ///
    /// Ailleurs, l'autodémarrage passe par systemd `--user` ou un LaunchAgent : la
    /// sous-commande le dit et sort en 2.
    ///
    /// Codes de retour : 0 fait (`status` : tâche présente), 1 l'outil du système a
    /// échoué, 2 plateforme non concernée, 4 `status` : tâche absente.
    Autostart {
        /// Ce qu'il faut faire de la tâche planifiée.
        #[command(subcommand)]
        action: AutostartAction,
    },
}

/// `enable` / `disable` / `status`.
#[derive(Debug, Subcommand)]
enum AutostartAction {
    /// Enregistre (ou remplace) la tâche planifiée.
    Enable(EnableArgs),
    /// Supprime la tâche planifiée.
    Disable(TaskArgs),
    /// Affiche si la tâche existe, son état et sa prochaine exécution.
    Status(TaskArgs),
}

/// Options communes : la tâche visée.
#[derive(Debug, ClapArgs)]
struct TaskArgs {
    /// Nom de la tâche planifiée (réservé aux tests : viser une tâche jetable plutôt
    /// que celle de l'utilisateur).
    #[arg(long, default_value = autostart::DEFAULT_TASK_NAME, hide = true)]
    task_name: String,
}

/// Options de `autostart enable`.
#[derive(Debug, ClapArgs)]
struct EnableArgs {
    #[command(flatten)]
    task: TaskArgs,
    /// Argument supplémentaire passé à `conduitd` par la tâche (répétable).
    ///
    /// Les arguments commençant par un tiret sont acceptés :
    /// `--arg --backend --arg null`.
    #[arg(long = "arg", value_name = "ARG", allow_hyphen_values = true)]
    args: Vec<String>,
    /// Délai entre l'ouverture de session et le démarrage, en secondes.
    #[arg(long, value_name = "SECONDES", default_value_t = autostart::DEFAULT_DELAY_SECS)]
    delay: u32,
}

impl AutostartAction {
    /// Exécute l'action et rend son code de retour.
    fn run(&self) -> i32 {
        match self {
            Self::Enable(a) => autostart::enable(&autostart::Options {
                task_name: a.task.task_name.clone(),
                args: a.args.clone(),
                delay_secs: a.delay,
            }),
            Self::Disable(a) => autostart::disable(&autostart::Options {
                task_name: a.task_name.clone(),
                ..Default::default()
            }),
            Self::Status(a) => autostart::status(&autostart::Options {
                task_name: a.task_name.clone(),
                ..Default::default()
            }),
        }
    }
}

/// Valeurs acceptées par `--backend`, pour le message d'erreur.
#[cfg(windows)]
const BACKEND_CHOICES: &str = "auto, null ou wasapi";
#[cfg(not(windows))]
const BACKEND_CHOICES: &str = "auto ou null";

/// Backend natif de la plateforme : WASAPI sous Windows.
///
/// `None` si la plateforme n'en a pas dans cette version, ou si son démarrage
/// échoue — l'erreur est alors journalisée et l'appelant se replie sur le backend
/// null : le démon doit démarrer quand même (F-51), quitte à ne servir aucun
/// périphérique réel.
#[cfg(windows)]
fn native_backend() -> Option<Box<dyn Backend>> {
    match conduit_backend_wasapi::WasapiBackend::new() {
        Ok(wasapi) => {
            tracing::info!("backend wasapi (MMDevice + WASAPI, mode partagé)");
            Some(Box::new(wasapi))
        }
        Err(e) => {
            tracing::error!(
                "le backend wasapi n'a pas démarré : {e} — le service audio Windows \
                 (AudioSrv) tourne-t-il ? Aucun périphérique réel ne sera disponible"
            );
            None
        }
    }
}

#[cfg(not(windows))]
fn native_backend() -> Option<Box<dyn Backend>> {
    None
}

fn main() {
    let args = Args::parse();
    if let Some(Sub::Autostart { action }) = &args.command {
        std::process::exit(action.run());
    }
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
    // Instance unique (ADR-013) : un démon par session, donc par point de contrôle.
    // Vérifié avant d'ouvrir le backend audio, pour ne rien réserver au périphérique
    // pendant qu'un autre démon travaille.
    if single_instance::check(&paths.socket) == Instance::AlreadyRunning {
        eprintln!("{}", single_instance::ALREADY_RUNNING);
        std::process::exit(single_instance::EXIT_ALREADY_RUNNING);
    }
    // Le backend null est cadencé en temps réel par son fil timer (mode manuel réservé aux tests).
    let null_timer = || {
        let mut null = NullBackend::new();
        null.start_timer();
        null
    };
    let backend: Box<dyn Backend> = match args.backend.as_str() {
        "null" => Box::new(null_timer()),
        "auto" => match native_backend() {
            Some(native) => native,
            None => {
                if cfg!(windows) {
                    tracing::warn!("repli sur le backend null (simulé)");
                } else {
                    tracing::warn!("aucun backend natif disponible sur cette plateforme dans cette version : backend null (simulé)");
                }
                Box::new(null_timer())
            }
        },
        #[cfg(windows)]
        "wasapi" => native_backend().unwrap_or_else(|| {
            tracing::warn!("repli sur le backend null (simulé)");
            Box::new(null_timer())
        }),
        other => {
            eprintln!("backend inconnu « {other} » : {}", BACKEND_CHOICES);
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
            // Le point de contrôle a été pris entre la vérification et l'ouverture :
            // deux démons lancés en même temps. Même verdict, même code de retour.
            Err(conduitd::DaemonError::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::error!("{e}");
                eprintln!("{}", single_instance::ALREADY_RUNNING);
                std::process::exit(single_instance::EXIT_ALREADY_RUNNING);
            }
            Err(e) => {
                tracing::error!("{e}");
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    });
}
