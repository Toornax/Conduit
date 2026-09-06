//! Démarrage du démon quand il ne répond pas (F-51, M2-10).
//!
//! La GUI n'est qu'un client IPC (ADR-005) : elle ne sait rien faire du son
//! sans le démon. Quand celui-ci manque, elle propose de le lancer — et se
//! contente de le lancer. **Elle ne l'attend pas et ne le surveille pas** :
//! c'est la boucle de reconnexion de [`crate::ipc`] qui verra le démon
//! apparaître, exactement comme s'il avait été démarré depuis un terminal.
//!
//! Deux couches, comme partout dans ce crate :
//!
//! - la fonction **qui décide** — [`trouver_dans`] — est pure : elle ne
//!   connaît du monde que ce que l'appelant lui donne (le chemin de
//!   l'exécutable courant, la variable `PATH`, un prédicat d'existence), et se
//!   teste donc sans toucher au disque ;
//! - les fonctions **qui exécutent** — [`trouver`] et [`lancer`] — lisent le
//!   système et créent un processus ; aucun test ne les appelle.
//!
//! # Où le démon est cherché
//!
//! 1. **À côté de l'exécutable courant.** C'est la disposition du MSI, où
//!    `conduit.exe` et `conduitd.exe` sont installés dans le même répertoire,
//!    et c'est aussi celle d'un `cargo build`, où les deux binaires sortent
//!    dans `target/<profil>/`. Un Conduit installé deux fois lance donc bien
//!    le démon de **son** installation.
//! 2. **Dans le `PATH`**, à défaut : le démon installé par un paquet, ou
//!    fourni par un `nix develop`.
//!
//! # Ce que le lancement ne sait pas
//!
//! `conduitd` rend 0 pour une fin normale, 1 pour un échec de démarrage, 2
//! pour une erreur d'usage et **3 quand un démon tourne déjà pour cette
//! session**. Aucun de ces codes n'est lisible ici : les lire demanderait
//! d'attendre la fin du processus, c'est-à-dire d'attendre que le démon
//! s'arrête. [`lancer`] ne rend donc compte que du **lancement** — le binaire
//! a été trouvé, le système a bien créé le processus — et rien de plus.
//!
//! Ce n'est pas une perte pour le code 3 : si un démon tournait déjà, celui
//! qu'on vient de lancer sort aussitôt, et la boucle de reconnexion se
//! connecte à celui qui était là. Pour les codes 1 et 2, l'utilisateur verra
//! que rien ne vient, et le journal du démon dira pourquoi.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::i18n::{self, Text};

/// Nom du binaire du démon, avec l'extension exécutable de la plateforme.
///
/// Ce n'est **pas** un texte d'interface : un nom de fichier ne se traduit pas.
pub const BINAIRE: &str = if cfg!(windows) {
    "conduitd.exe"
} else {
    "conduitd"
};

/// `CREATE_NO_WINDOW` : le processus créé n'ouvre aucune console.
///
/// Sans ce drapeau, lancer un binaire de sous-système console depuis une
/// application graphique fait clignoter une fenêtre noire à l'écran.
#[cfg(windows)]
const SANS_CONSOLE: u32 = 0x0800_0000;

/// Ce qui a empêché le démon de démarrer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Echec {
    /// Aucun `conduitd` à côté de la GUI, ni dans le `PATH`.
    Introuvable,
    /// Le système a refusé de créer le processus ; la cause est la sienne.
    Refus(String),
}

impl Echec {
    /// Le texte affiché à l'utilisateur.
    ///
    /// La cause du système est reprise telle quelle, sans reformulation
    /// (ADR-006) : c'est elle qui connaît le détail.
    pub fn texte(&self) -> String {
        match self {
            Echec::Introuvable => i18n::t(Text::DaemonNotFound).to_string(),
            Echec::Refus(cause) => i18n::demon_non_lance(cause),
        }
    }
}

/// Où lancer le démon : à côté de `exe`, sinon dans `chemin`.
///
/// `exe` est le chemin de l'exécutable courant, `chemin` le contenu de la
/// variable d'environnement `PATH`, et `existe` dit si un chemin désigne un
/// fichier. Les répertoires vides du `PATH` sont ignorés : ils désignent le
/// répertoire courant, où un `conduitd` n'aurait rien à faire.
pub fn trouver_dans(
    exe: Option<&Path>,
    chemin: Option<&OsStr>,
    existe: impl Fn(&Path) -> bool,
) -> Result<PathBuf, Echec> {
    if let Some(voisin) = exe.and_then(Path::parent).map(|dir| dir.join(BINAIRE)) {
        if existe(&voisin) {
            return Ok(voisin);
        }
    }
    for dir in chemin.into_iter().flat_map(std::env::split_paths) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidat = dir.join(BINAIRE);
        if existe(&candidat) {
            return Ok(candidat);
        }
    }
    Err(Echec::Introuvable)
}

/// Le binaire du démon sur cette machine.
///
/// Bloquante — elle interroge le disque : à n'appeler que depuis une tâche.
pub fn trouver() -> Result<PathBuf, Echec> {
    trouver_dans(
        std::env::current_exe().ok().as_deref(),
        std::env::var_os("PATH").as_deref(),
        |chemin| chemin.is_file(),
    )
}

/// Lance le démon, détaché, sur le socket que la GUI surveille.
///
/// Le socket est passé en `--socket` : la GUI et le démon parlent ainsi du
/// même endroit, y compris quand la GUI a elle-même été lancée avec
/// `--socket`.
///
/// Les trois flux standard sont fermés et, sous Windows, aucune console n'est
/// créée. Le processus **n'est pas attendu** (voir le module) : sous Unix, le
/// démon reste dans le groupe de processus de la GUI, et son enregistrement de
/// fin reste dans la table tant que la GUI vit. Les deux sont le prix de ne pas
/// bloquer le fil de l'interface, et `#![forbid(unsafe_code)]` interdit le
/// `setsid` qui affranchirait le démon du groupe.
///
/// Bloquante — elle interroge le disque et crée un processus : à n'appeler que
/// depuis une tâche, jamais depuis `update`.
pub fn lancer(socket: &Path) -> Result<(), Echec> {
    let binaire = trouver()?;
    let mut commande = Command::new(&binaire);
    commande
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        commande.creation_flags(SANS_CONSOLE);
    }
    match commande.spawn() {
        Ok(enfant) => {
            tracing::info!(
                binaire = %binaire.display(),
                pid = enfant.id(),
                "démon lancé par la GUI"
            );
            Ok(())
        }
        Err(erreur) => Err(Echec::Refus(erreur.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;

    /// Un `PATH` bien formé pour la plateforme courante.
    fn chemin(dirs: &[&str]) -> OsString {
        std::env::join_paths(dirs).expect("PATH de test")
    }

    /// Le voisin de l'exécutable courant passe avant le `PATH` : un Conduit
    /// installé deux fois lance le démon de son installation.
    #[test]
    fn le_voisin_de_l_executable_passe_avant_le_chemin() {
        let exe = Path::new("/opt/conduit/conduit");
        let voisin = Path::new("/opt/conduit").join(BINAIRE);
        let trouve = trouver_dans(
            Some(exe),
            Some(&chemin(&["/usr/bin"])),
            // Les deux existent : la décision doit prendre le voisin.
            |_| true,
        );
        assert_eq!(trouve, Ok(voisin));
    }

    /// Sans voisin, le `PATH` est parcouru dans l'ordre, et les répertoires
    /// vides — le répertoire courant — sont ignorés.
    #[test]
    fn a_defaut_le_chemin_est_parcouru_dans_l_ordre() {
        let second = Path::new("/usr/local/bin").join(BINAIRE);
        let trouve = trouver_dans(
            Some(Path::new("/opt/conduit/conduit")),
            Some(&chemin(&["", "/usr/bin", "/usr/local/bin"])),
            |candidat| candidat == second,
        );
        assert_eq!(trouve, Ok(second));
    }

    /// Ni voisin, ni `PATH` : l'échec est nommé, et il a son texte.
    #[test]
    fn sans_voisin_ni_chemin_le_demon_est_introuvable() {
        assert_eq!(
            trouver_dans(Some(Path::new("/opt/conduit/conduit")), None, |_| false),
            Err(Echec::Introuvable),
            "sans voisin et sans PATH, il n'y a rien à lancer"
        );
        assert_eq!(
            trouver_dans(None, Some(&chemin(&["/usr/bin"])), |_| false),
            Err(Echec::Introuvable)
        );
        // Sans exécutable courant connu, seul le PATH reste.
        let dans_le_chemin = Path::new("/usr/bin").join(BINAIRE);
        assert_eq!(
            trouver_dans(None, Some(&chemin(&["/usr/bin"])), |_| true),
            Ok(dans_le_chemin)
        );
    }

    /// Le binaire porte l'extension de la plateforme, et rien d'autre.
    #[test]
    fn le_binaire_porte_l_extension_de_la_plateforme() {
        assert_eq!(
            BINAIRE,
            if cfg!(windows) {
                "conduitd.exe"
            } else {
                "conduitd"
            }
        );
    }

    /// Les deux échecs se disent, et le refus du système est repris tel quel.
    #[test]
    fn les_echecs_ont_leur_texte() {
        assert!(!Echec::Introuvable.texte().is_empty());
        let refus = Echec::Refus("permission refusée".into()).texte();
        assert!(refus.contains("permission refusée"), "{refus}");
    }
}
