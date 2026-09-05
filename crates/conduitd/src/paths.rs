//! Chemins par OS (configuration, données, socket de contrôle).

use std::path::PathBuf;

/// Chemins utilisés par le démon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Fichier de configuration TOML.
    pub config_file: PathBuf,
    /// Répertoire de données (état persisté, journaux).
    pub data_dir: PathBuf,
    /// Fichier d'état persisté.
    pub state_file: PathBuf,
    /// Répertoire des journaux.
    pub log_dir: PathBuf,
    /// Socket Unix ou nom de named pipe.
    pub socket: PathBuf,
}

impl Paths {
    /// Chemins standard de l'utilisateur courant.
    pub fn standard() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("", "", "conduit")?;
        let config_dir = dirs.config_dir().to_path_buf();
        let data_dir = dirs.data_dir().to_path_buf();
        let socket = Self::default_socket(
            dirs.runtime_dir()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| data_dir.clone()),
        );
        Some(Self::in_dirs(config_dir, data_dir, socket))
    }

    /// Chemins sous un répertoire unique (tests, `--root`).
    pub fn under(root: &std::path::Path) -> Self {
        Self::in_dirs(
            root.join("config"),
            root.join("data"),
            Self::default_socket(root.to_path_buf()),
        )
    }

    fn in_dirs(config_dir: PathBuf, data_dir: PathBuf, socket: PathBuf) -> Self {
        Self {
            config_file: config_dir.join("conduit.toml"),
            state_file: data_dir.join("state.json"),
            log_dir: data_dir.join("logs"),
            data_dir,
            socket,
        }
    }

    /// Nom de socket par défaut dans un répertoire (Unix) ou nom de pipe (Windows).
    pub fn default_socket(runtime_dir: PathBuf) -> PathBuf {
        if cfg!(windows) {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
            PathBuf::from(format!(r"\\.\pipe\conduit-{user}"))
        } else {
            runtime_dir.join("conduitd.sock")
        }
    }

    /// Crée les répertoires nécessaires.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        if let Some(p) = self.config_file.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        if !cfg!(windows) {
            if let Some(p) = self.socket.parent() {
                std::fs::create_dir_all(p)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_root_and_ensure() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::under(dir.path());
        assert_eq!(p.config_file, dir.path().join("config/conduit.toml"));
        assert_eq!(p.state_file, dir.path().join("data/state.json"));
        p.ensure_dirs().unwrap();
        assert!(p.log_dir.is_dir());
        if !cfg!(windows) {
            assert_eq!(p.socket, dir.path().join("conduitd.sock"));
        }
    }

    #[test]
    fn standard_paths_exist_on_this_platform() {
        if let Some(p) = Paths::standard() {
            assert!(p.config_file.ends_with("conduit.toml"));
            assert!(p.socket.to_string_lossy().contains("conduit"));
        }
    }
}
