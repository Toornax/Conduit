//! Instance unique : un seul démon par point de contrôle (ADR-013, M1b-35).
//!
//! Le point de contrôle est le socket Unix ou le named pipe. Il **est** le verrou :
//! pas de fichier `.pid` à côté, qui pourrait mentir (PID recyclé, fichier oublié
//! après un arrêt brutal). La question posée est toujours la même — « quelqu'un
//! répond-il là où j'allais écouter ? » — et sa réponse est forcément à jour.
//!
//! - **Unix** : le fichier de socket existe-t-il, et une connexion aboutit-elle ?
//!   Oui → un démon vit derrière. Non → le fichier est **orphelin** (démon tué), on
//!   le supprime et on continue.
//! - **Windows** : un named pipe n'existe que tant qu'un processus le détient, il n'y a
//!   donc pas d'orphelin. Ouvrir le pipe réussit (un démon écoute) ou échoue avec
//!   `ERROR_FILE_NOT_FOUND` (personne). `ERROR_PIPE_BUSY` (231, toutes les instances
//!   occupées) et `ERROR_ACCESS_DENIED` (5, un pipe de ce nom appartient à quelqu'un
//!   d'autre) valent aussi « déjà pris » : ce sont exactement les erreurs que
//!   `first_pipe_instance` renverrait à `bind`.
//!
//! Le test laisse une course possible (deux démons lancés en même temps) : le filet de
//! sécurité est [`crate::ipc::Listener::bind`], qui rend une erreur
//! [`std::io::ErrorKind::AddrInUse`] dans ce cas, et que `main` traite avec le même code
//! de retour.

use std::path::Path;

/// Message affiché quand un démon tourne déjà (ADR-006 : dire quoi faire).
pub const ALREADY_RUNNING: &str = "un démon Conduit est déjà en cours pour cette session ; \
     utilisez `conduitctl` pour lui parler, ou arrêtez-le avant d'en lancer un autre";

/// Code de retour dédié à « un démon tourne déjà » (0 = fin normale, 1 = échec de
/// démarrage, 2 = usage ou configuration).
pub const EXIT_ALREADY_RUNNING: i32 = 3;

/// Verdict du test d'instance unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instance {
    /// Personne n'écoute : le démon peut prendre le point de contrôle.
    Free,
    /// Un démon vivant tient déjà le point de contrôle.
    AlreadyRunning,
}

/// Un démon écoute-t-il déjà sur `socket` ?
///
/// Sous Unix, supprime au passage un fichier de socket orphelin.
pub fn check(socket: &Path) -> Instance {
    #[cfg(unix)]
    {
        if !socket.exists() {
            return Instance::Free;
        }
        match std::os::unix::net::UnixStream::connect(socket) {
            Ok(_) => Instance::AlreadyRunning,
            Err(_) => {
                tracing::warn!(
                    "socket orphelin {} : aucun démon derrière, suppression",
                    socket.display()
                );
                let _ = std::fs::remove_file(socket);
                Instance::Free
            }
        }
    }
    #[cfg(windows)]
    {
        let name = conduit_protocol::client::pipe_name(socket);
        // Une ouverture réussie consomme une instance du pipe : le démon en face verra un
        // client qui ne dit rien et repart (trace `debug`). C'est le prix d'une réponse
        // fiable, et il est payé une fois par démarrage.
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
        {
            Ok(_) => Instance::AlreadyRunning,
            Err(e) => match e.raw_os_error() {
                // ERROR_PIPE_BUSY, ERROR_ACCESS_DENIED : le nom est pris.
                Some(231) | Some(5) => Instance::AlreadyRunning,
                _ => Instance::Free,
            },
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = socket;
        Instance::Free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_listening_is_free() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(check(&dir.path().join("conduitd.sock")), Instance::Free);
    }

    #[cfg(unix)]
    #[test]
    fn orphan_socket_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("conduitd.sock");
        std::fs::write(&socket, b"").unwrap();
        assert_eq!(check(&socket), Instance::Free);
        assert!(!socket.exists(), "le fichier orphelin est supprimé");
    }
}
