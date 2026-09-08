//! `conduitd` — le démon : moteur, IPC, persistance, règles.

// `deny` et non `forbid` : le seul `unsafe` du démon est dans `session_end`, sous
// `cfg(windows)` — les fenêtres Win32 n'ont pas de façade sûre (voir ce module et
// docs/dev-guide.md §6). Partout ailleurs, y compris dans le binaire, l'`unsafe` reste
// interdit.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod autoconnect;
pub mod autostart;
pub mod config;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod persist;
pub mod service;
#[cfg(windows)]
pub mod session_end;
pub mod single_instance;

use std::path::PathBuf;
use std::sync::Arc;

use conduit_backend::{Backend, CableSpec};
use conduit_core::types::ChannelCount;
use conduit_engine::{Engine, EngineConfig};
use tokio::sync::watch;

use config::Config;
use paths::Paths;
use service::{Service, ServiceOptions};

/// Nom annoncé aux clients.
pub fn server_name() -> String {
    format!("conduitd {}", env!("CARGO_PKG_VERSION"))
}

/// Erreurs de démarrage.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Configuration.
    #[error("{0}")]
    Config(#[from] config::ConfigError),
    /// Moteur.
    #[error("moteur : {0}")]
    Engine(#[from] conduit_engine::EngineError),
    /// Entrée/sortie.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Options du démon.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// Chemins.
    pub paths: Paths,
    /// Configuration.
    pub config: Config,
    /// Watchdog du fil audio.
    pub watchdog: bool,
    /// Persistance de l'état.
    pub persist: bool,
}

impl DaemonOptions {
    /// Options sous un répertoire racine (tests).
    pub fn under(root: &std::path::Path, config: Config) -> Self {
        Self {
            paths: Paths::under(root),
            config,
            watchdog: false,
            persist: true,
        }
    }
}

/// Construit le moteur : fréquence, quantum, pilote et câbles de la configuration.
pub fn build_engine(mut backend: Box<dyn Backend>, config: &Config) -> Result<Engine, DaemonError> {
    ensure_cables(backend.as_mut(), config);
    let engine_config = EngineConfig {
        sample_rate: config.sample_rate(),
        quantum: config.quantum(),
        driver: config.driver_choice(),
        ..Default::default()
    };
    Ok(Engine::new(backend, engine_config)?)
}

/// Crée les câbles manquants de la configuration et applique les alias (F-01).
///
/// « Manquant » a deux sens selon la plateforme, et les deux sont traités. Là où les
/// câbles sont créés à la demande (backend `null`), un câble absent de la liste est
/// créé. Sous Windows le pilote a une **réserve fixe** de seize câbles (SPEC §5.4) :
/// ils sont tous listés, actifs ou non, et « manquant » veut dire **déconnecté** —
/// `create` est alors la façon de le connecter (M1b-34).
fn ensure_cables(backend: &mut dyn Backend, config: &Config) {
    let Some(cc) = backend.cable_control() else {
        if !config.cables.is_empty() {
            tracing::warn!("ce backend ne gère pas les câbles : section [[cable]] ignorée");
        }
        return;
    };
    let existing = cc.list().unwrap_or_default();
    for c in &config.cables {
        let channels = ChannelCount::new(c.channels).unwrap_or_default();
        match existing.iter().find(|e| e.id.0 == c.id) {
            Some(e) => {
                // Présent mais déconnecté : le connecter en le désignant par son nom,
                // pour ne pas activer le premier câble libre à sa place.
                if !e.active {
                    match cc.create(CableSpec {
                        name: Some(e.name.clone()),
                        channels,
                    }) {
                        Ok(info) => tracing::info!("câble activé : {}", info.name),
                        Err(err) => tracing::warn!("câble {} : {err}", c.id),
                    }
                }
                if e.channels != channels {
                    let _ = cc.set_channels(e.id, channels);
                }
                if let Some(alias) = &c.alias {
                    if &e.name != alias {
                        let _ = cc.rename(e.id, alias);
                    }
                }
            }
            None => match cc.create(CableSpec {
                name: c.alias.clone(),
                channels,
            }) {
                Ok(info) => tracing::info!("câble créé : {} ({})", info.name, info.channels),
                Err(e) => tracing::warn!("câble {} : {e}", c.id),
            },
        }
    }
}

/// Démon en cours d'exécution.
pub struct Daemon {
    /// Chemin ou nom du socket.
    pub socket: PathBuf,
    /// Service (commandes directes, sans IPC).
    pub service: Arc<Service>,
    shutdown: watch::Sender<bool>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl core::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Daemon")
            .field("socket", &self.socket)
            .finish()
    }
}

impl Daemon {
    /// Démarre le moteur, le service et le serveur IPC (dans le runtime courant).
    pub fn spawn(backend: Box<dyn Backend>, options: DaemonOptions) -> Result<Self, DaemonError> {
        options.paths.ensure_dirs()?;
        let engine = build_engine(backend, &options.config)?;
        let service = Arc::new(Service::start(
            engine,
            ServiceOptions {
                state_file: options.persist.then(|| options.paths.state_file.clone()),
                rules: options.config.autoconnect.clone(),
                watchdog: options.watchdog,
                ..Default::default()
            },
        ));
        let listener = ipc::Listener::bind(&options.paths.socket)?;
        let (shutdown, rx) = watch::channel(false);
        let server = tokio::spawn(ipc::serve(
            listener,
            Arc::clone(&service),
            rx,
            server_name(),
        ));
        tracing::info!("démon prêt sur {}", options.paths.socket.display());
        Ok(Self {
            socket: options.paths.socket.clone(),
            service,
            shutdown,
            server: Some(server),
        })
    }

    /// Arrête le serveur IPC puis le service.
    ///
    /// Au retour, la boucle d'acceptation **et** toutes les sessions clientes sont
    /// terminées : le socket Unix est supprimé et plus aucune instance du named pipe
    /// n'est ouverte. Un démon peut donc être relancé aussitôt sur le même chemin, sans
    /// attente ni nouvelle tentative.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown.send(true);
        if let Some(s) = self.server.take() {
            let _ = s.await;
        }
        if let Ok(mut service) = Arc::try_unwrap(self.service) {
            service.shutdown();
        }
    }

    /// Tourne jusqu'au signal d'arrêt (Ctrl-C / SIGTERM), et sous Windows jusqu'à la
    /// fermeture de session (`WM_ENDSESSION`, voir le module `session_end` —
    /// en code et non en lien : il n'existe que sous Windows).
    pub async fn run_until_signal(self) {
        #[cfg(windows)]
        {
            self.run_until_signal_windows().await;
        }
        #[cfg(not(windows))]
        {
            wait_for_signal().await;
            tracing::info!("arrêt demandé");
            self.shutdown().await;
        }
    }

    /// Variante Windows : Ctrl-C **ou** fin de session.
    ///
    /// L'accusé de réception est envoyé après l'arrêt complet (état sauvegardé, flux
    /// fermés) : c'est lui qui laisse le système achever la fermeture de session.
    #[cfg(windows)]
    async fn run_until_signal_windows(self) {
        let session = session_end::spawn();
        match &session {
            Some(s) => {
                tokio::select! {
                    _ = wait_for_signal() => tracing::info!("arrêt demandé"),
                    _ = s.wait() => tracing::info!("fermeture de session Windows : arrêt propre"),
                }
            }
            None => {
                wait_for_signal().await;
                tracing::info!("arrêt demandé");
            }
        }
        self.shutdown().await;
        if let Some(s) = session {
            s.acknowledge();
        }
    }
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
