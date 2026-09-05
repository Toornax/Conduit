//! Banc de test : un démon PipeWire headless, sans matériel.
//!
//! `pipewire -c tests/pipewire-test.conf` suffit à obtenir un `Dummy-Driver` et un
//! `Null-Sink`. Deux points sont indispensables dans cette configuration :
//! `core.daemon = true` (sans quoi aucune socket n'est créée) et
//! `libpipewire-module-access` (sans quoi les clients ne voient aucun nœud).
//!
//! Le démon seul ne **relie** pas les flux : c'est le rôle d'un gestionnaire de
//! session. [`PwDaemon::start_with_session`] lance donc `wireplumber` en plus, ce qui
//! est nécessaire dès qu'un test veut voir le rappel `process` s'exécuter.
//!
//! Si les binaires manquent (hors devshell Nix), les fonctions retournent `None` et
//! les tests se sautent proprement.

#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Configuration validée, embarquée dans le binaire de test.
const CONF: &str = include_str!("../pipewire-test.conf");

/// Délai maximal d'apparition de la socket du démon.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

static COUNTER: AtomicU32 = AtomicU32::new(0);
static RUNTIME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Répertoire d'exécution commun à tous les démons de ce binaire de test.
///
/// PipeWire résout le chemin de la socket depuis `XDG_RUNTIME_DIR`, qui est une
/// variable de processus : un seul répertoire pour tout le binaire, et un nom de
/// démon différent par test. Le chemin reste court : une socket Unix ne dépasse pas
/// 108 octets.
pub fn runtime_dir() -> &'static Path {
    RUNTIME_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("conduit-pw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("répertoire d'exécution");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        // Le backend reçoit le nom du démon en argument : la variable ne doit pas
        // désigner un autre démon.
        std::env::remove_var("PIPEWIRE_REMOTE");
        dir
    })
}

/// Cherche un exécutable dans le `PATH`.
pub fn find_binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Un démon PipeWire de test, tué à la destruction.
pub struct PwDaemon {
    name: String,
    conf: PathBuf,
    pipewire: Child,
    session: Option<Child>,
}

impl PwDaemon {
    /// Démarre un démon seul. `None` si `pipewire` est introuvable.
    pub fn start() -> Option<Self> {
        Self::spawn(false)
    }

    /// Démarre un démon **et** un gestionnaire de session (`wireplumber`), sans
    /// lequel aucun flux client n'est relié ni cadencé. `None` si l'un des deux
    /// binaires manque.
    pub fn start_with_session() -> Option<Self> {
        Self::spawn(true)
    }

    fn spawn(with_session: bool) -> Option<Self> {
        let pipewire_bin = find_binary("pipewire")?;
        let session_bin = if with_session {
            Some(find_binary("wireplumber")?)
        } else {
            None
        };
        let dir = runtime_dir();
        let name = format!(
            "pipewire-conduit-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let conf = dir.join(format!("{name}.conf"));
        std::fs::write(
            &conf,
            CONF.replace("core.name = pipewire-0", &format!("core.name = {name}")),
        )
        .expect("configuration du démon");

        let pipewire = Command::new(pipewire_bin)
            .arg("-c")
            .arg(&conf)
            .env("XDG_RUNTIME_DIR", dir)
            .stdin(Stdio::null())
            .stdout(log_file(dir, &name, "out"))
            .stderr(log_file(dir, &name, "err"))
            .spawn()
            .expect("démarrage du démon PipeWire");

        let mut daemon = PwDaemon {
            name,
            conf,
            pipewire,
            session: None,
        };
        if !daemon.wait_for_socket() {
            eprintln!(
                "démon PipeWire non démarré, journal :\n{}",
                daemon.log().unwrap_or_default()
            );
            return None;
        }

        if let Some(bin) = session_bin {
            let session = Command::new(bin)
                .env("XDG_RUNTIME_DIR", dir)
                .env("PIPEWIRE_REMOTE", &daemon.name)
                // Empêche WirePlumber d'écrire dans la configuration de l'utilisateur.
                .env("XDG_CONFIG_HOME", dir)
                .env("XDG_STATE_HOME", dir)
                .env("XDG_CACHE_HOME", dir)
                .stdin(Stdio::null())
                .stdout(log_file(dir, &daemon.name, "wp-out"))
                .stderr(log_file(dir, &daemon.name, "wp-err"))
                .spawn()
                .expect("démarrage de WirePlumber");
            daemon.session = Some(session);
        }
        Some(daemon)
    }

    /// Nom du démon, à passer à `PipewireBackend::connect`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Lance un outil PipeWire (`pw-loopback`, `pw-cli`, …) contre ce démon.
    pub fn tool(&self, program: &str, args: &[&str]) -> Option<Child> {
        let bin = find_binary(program)?;
        Command::new(bin)
            .args(args)
            .env("XDG_RUNTIME_DIR", runtime_dir())
            .env("PIPEWIRE_REMOTE", &self.name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    fn socket(&self) -> PathBuf {
        runtime_dir().join(&self.name)
    }

    fn wait_for_socket(&mut self) -> bool {
        let deadline = Instant::now() + SOCKET_TIMEOUT;
        while Instant::now() < deadline {
            if self.socket().exists() {
                return true;
            }
            if matches!(self.pipewire.try_wait(), Ok(Some(_))) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn log(&self) -> Option<String> {
        let dir = runtime_dir();
        let mut out = String::new();
        for suffix in ["out", "err"] {
            if let Ok(text) = std::fs::read_to_string(dir.join(format!("{}.{suffix}", self.name))) {
                out.push_str(&text);
            }
        }
        Some(out)
    }
}

fn log_file(dir: &Path, name: &str, suffix: &str) -> Stdio {
    match File::create(dir.join(format!("{name}.{suffix}"))) {
        Ok(file) => Stdio::from(file),
        Err(_) => Stdio::null(),
    }
}

impl Drop for PwDaemon {
    fn drop(&mut self) {
        if let Some(session) = &mut self.session {
            let _ = session.kill();
            let _ = session.wait();
        }
        let _ = self.pipewire.kill();
        let _ = self.pipewire.wait();
        let _ = std::fs::remove_file(&self.conf);
        let _ = std::fs::remove_file(self.socket());
    }
}

/// Attend qu'une condition devienne vraie, ou abandonne après `timeout`.
pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}
