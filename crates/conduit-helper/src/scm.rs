//! Le service Windows : installation, désinstallation, et le dispatcher qui le fait
//! tourner.
//!
//! # Par l'API, pas par `sc.exe`
//!
//! `CreateServiceW` et `DeleteService` plutôt qu'un `sc.exe create …` lancé en
//! sous-processus. Trois raisons : le code d'erreur revient tel quel au lieu d'un texte
//! localisé à analyser, il n'y a pas de chemin à échapper dans une ligne de commande
//! (`C:\Program Files\…` en contient), et le binaire ne dépend de rien qu'il ne porte
//! lui-même — ce qui compte dans un MSI (M1b-40) où l'ordre d'installation des
//! composants n'est pas garanti.
//!
//! # Ce que l'installation pose
//!
//! | Réglage | Valeur | Pourquoi |
//! |---|---|---|
//! | Nom | `ConduitHelper` | ce que `sc query` et le MSI nomment |
//! | Nom affiché | « Conduit — service d'assistance » | ce que la console des services montre |
//! | Type | `SERVICE_WIN32_OWN_PROCESS` | un processus pour ce service seul |
//! | Démarrage | `SERVICE_AUTO_START` | le démon peut être lancé avant l'ouverture de session |
//! | Compte | `LocalSystem` | le seul qui **détienne** `SeLoadDriverPrivilege` |
//! | Ligne de commande | `"<chemin du binaire>" service` | la sous-commande qui appelle le dispatcher |
//!
//! Le chemin est **cité** : `CreateServiceW` prend une ligne de commande, pas un chemin,
//! et un `C:\Program Files\Conduit\conduit-helper.exe` non cité s'interprète comme
//! `C:\Program.exe` avec des arguments — le défaut « chemin de service non cité », qui
//! est aussi une élévation de privilège classique.
//!
//! # Aucun test n'installe quoi que ce soit
//!
//! Contrainte absolue du dépôt. Les tests de ce module ne touchent qu'aux fonctions
//! **pures** : la ligne de commande construite, les noms, la traduction des états. Rien
//! n'ouvre le contrôleur de services.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_SERVICE_DOES_NOT_EXIST};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerExW,
    SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_CREATE_SERVICE, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS,
    SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

use crate::journal::{Journal, Niveau};
use crate::tube::{self, Arret};

/// Nom du service, tel que le contrôleur de services le connaît.
///
/// Gravé : le MSI (M1b-40), les scripts de la VM et toute installation déjà faite s'y
/// donnent rendez-vous.
pub const NOM_SERVICE: &str = "ConduitHelper";

/// Nom affiché dans la console des services.
pub const NOM_AFFICHE: &str = "Conduit — service d'assistance";

/// Description affichée dans la console des services.
///
/// Elle dit **pourquoi** le service existe : un administrateur qui le découvre doit
/// pouvoir décider s'il peut l'arrêter sans lire le dépôt.
pub const DESCRIPTION: &str = "Écrit la configuration des câbles virtuels Conduit dans le pilote \
     audio, pour le compte du démon de l'utilisateur. Le pilote exige le privilège de \
     chargement de pilote, qu'une session ordinaire ne détient pas. Arrêter ce service \
     empêche d'activer ou de désactiver un câble ; les câbles déjà actifs continuent de \
     fonctionner.";

/// La sous-commande que le service se lance à lui-même.
pub const SOUS_COMMANDE_SERVICE: &str = "service";

/// Construit la ligne de commande d'un service, chemin **cité**.
///
/// Fonction pure, testée : c'est la moitié du défaut « chemin de service non cité », et
/// l'autre moitié est de s'en souvenir.
#[must_use]
pub fn ligne_de_commande(binaire: &std::path::Path) -> String {
    format!("\"{}\" {SOUS_COMMANDE_SERVICE}", binaire.display())
}

/// Convertit une chaîne Rust en chaîne large terminée par NUL.
fn large(texte: &str) -> Vec<u16> {
    texte.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Le dernier code d'erreur du fil courant.
fn dernier_code() -> u32 {
    // SAFETY: `GetLastError` ne prend aucun paramètre et lit le code du fil courant.
    unsafe { GetLastError() }.0
}

/// Ce qui a empêché d'installer, de désinstaller ou d'interroger le service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErreurService {
    /// L'appel a échoué ; le code Win32 est rendu tel quel.
    Appel {
        /// La fonction fautive.
        appel: &'static str,
        /// Le code Win32.
        code: u32,
    },
    /// Le service n'est pas installé.
    Absent,
    /// Le chemin du binaire n'a pas pu être déterminé.
    Binaire {
        /// La cause, en français.
        cause: String,
    },
}

impl core::fmt::Display for ErreurService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Appel { appel, code: 5 } => write!(
                f,
                "{appel} : accès refusé — cette commande demande une invite élevée \
                 (« Exécuter en tant qu'administrateur »)"
            ),
            Self::Appel { appel, code: 1073 } => write!(
                f,
                "{appel} : le service {NOM_SERVICE} est déjà installé — désinstallez-le \
                 d'abord (« conduit-helper desinstaller »)"
            ),
            Self::Appel { appel, code } => write!(f, "{appel} : erreur Win32 {code}"),
            Self::Absent => write!(
                f,
                "le service {NOM_SERVICE} n'est pas installé (« conduit-helper installer »)"
            ),
            Self::Binaire { cause } => write!(f, "chemin du binaire indéterminable : {cause}"),
        }
    }
}

impl std::error::Error for ErreurService {}

/// Un handle du contrôleur de services, fermé à la destruction.
struct PoigneeScm(SC_HANDLE);

impl Drop for PoigneeScm {
    fn drop(&mut self) {
        if self.0.is_invalid() {
            return;
        }
        // SAFETY: handle rendu par `OpenSCManagerW`, `CreateServiceW` ou `OpenServiceW`,
        // fermé une seule fois (le type n'est ni `Copy` ni clonable).
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

/// Ouvre le contrôleur de services avec les accès demandés.
fn ouvrir_scm(acces: u32) -> Result<PoigneeScm, ErreurService> {
    // SAFETY: les deux `PCWSTR::null` sont la façon documentée de dire « la machine
    // locale, la base de données active ».
    let handle =
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), acces) }.map_err(|_| {
            ErreurService::Appel {
                appel: "OpenSCManagerW",
                code: dernier_code(),
            }
        })?;
    Ok(PoigneeScm(handle))
}

/// Ouvre le service avec les accès demandés.
fn ouvrir_service(scm: &PoigneeScm, acces: u32) -> Result<PoigneeScm, ErreurService> {
    let nom = large(NOM_SERVICE);
    // SAFETY: `scm` est un handle valide vivant pendant l'appel ; `nom` est une chaîne
    // large terminée par NUL, vivante elle aussi.
    let handle = unsafe { OpenServiceW(scm.0, PCWSTR(nom.as_ptr()), acces) }.map_err(|_| {
        let code = dernier_code();
        if code == ERROR_SERVICE_DOES_NOT_EXIST.0 {
            ErreurService::Absent
        } else {
            ErreurService::Appel {
                appel: "OpenServiceW",
                code,
            }
        }
    })?;
    Ok(PoigneeScm(handle))
}

/// Installe le service : `LocalSystem`, démarrage automatique.
///
/// Le binaire installé est **celui qui exécute cette commande** : il n'y a rien à
/// copier, et pas de chemin à saisir qui pourrait viser autre chose.
///
/// # Erreurs
///
/// [`ErreurService`], dont le cas le plus fréquent est `ERROR_ACCESS_DENIED` (5) sur une
/// invite non élevée.
pub fn installer() -> Result<(), ErreurService> {
    let binaire = std::env::current_exe().map_err(|e| ErreurService::Binaire {
        cause: e.to_string(),
    })?;
    let scm = ouvrir_scm(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;
    let nom = large(NOM_SERVICE);
    let affiche = large(NOM_AFFICHE);
    let commande = large(&ligne_de_commande(&binaire));
    // `LocalSystem` est le seul compte qui **détienne** `SeLoadDriverPrivilege` sans
    // configuration : c'est toute la raison d'être de ce service (voir `crate::cables`).
    let compte = large("LocalSystem");
    // SAFETY: toutes les chaînes larges sont terminées par NUL et vivent pendant l'appel.
    // Aucun groupe d'ordre, aucune étiquette, aucune dépendance, aucun mot de passe :
    // `LocalSystem` n'en prend pas.
    let service = unsafe {
        CreateServiceW(
            scm.0,
            PCWSTR(nom.as_ptr()),
            PCWSTR(affiche.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(commande.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR(compte.as_ptr()),
            PCWSTR::null(),
        )
    }
    .map_err(|_| ErreurService::Appel {
        appel: "CreateServiceW",
        code: dernier_code(),
    })?;
    let service = PoigneeScm(service);

    // La description est un réglage à part : `CreateServiceW` ne la prend pas. Son échec
    // n'annule pas l'installation — un service sans description marche.
    let mut description = large(DESCRIPTION);
    let bloc = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(description.as_mut_ptr()),
    };
    // SAFETY: `service` est un handle valide ouvert en `SERVICE_ALL_ACCESS` ;
    // `description` vit pendant l'appel et `bloc` la pointe ; le type d'information
    // annoncé est bien celui de la structure transmise.
    let _ = unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some((&raw const bloc).cast()),
        )
    };
    Ok(())
}

/// Démarre le service.
///
/// # Erreurs
///
/// [`ErreurService`] ; `ERROR_SERVICE_ALREADY_RUNNING` (1056) si le service tourne déjà.
pub fn demarrer() -> Result<(), ErreurService> {
    let scm = ouvrir_scm(SC_MANAGER_CONNECT)?;
    let service = ouvrir_service(&scm, windows::Win32::System::Services::SERVICE_START)?;
    // SAFETY: `service` est un handle valide ouvert avec `SERVICE_START` ; aucun
    // argument n'est passé au service, d'où `None`.
    unsafe { StartServiceW(service.0, None) }.map_err(|_| ErreurService::Appel {
        appel: "StartServiceW",
        code: dernier_code(),
    })
}

/// Arrête le service s'il tourne, puis le supprime.
///
/// L'arrêt d'abord : `DeleteService` sur un service en marche ne fait que le **marquer**
/// pour suppression, et il ne disparaît qu'au prochain redémarrage — ce qui donne un
/// « le service existe encore » incompréhensible à qui vient de le désinstaller.
///
/// # Erreurs
///
/// [`ErreurService::Absent`] si le service n'est pas installé, sinon le code Win32.
pub fn desinstaller() -> Result<(), ErreurService> {
    let scm = ouvrir_scm(SC_MANAGER_CONNECT)?;
    let service = ouvrir_service(&scm, SERVICE_ALL_ACCESS)?;
    let mut statut = SERVICE_STATUS::default();
    // SAFETY: `service` est un handle valide ; `statut` est une variable de cette pile,
    // écrite par l'appelé seul. L'échec est toléré : un service déjà arrêté rend
    // `ERROR_SERVICE_NOT_ACTIVE`, ce qui est exactement l'état voulu.
    let _ = unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut statut) };
    // SAFETY: `service` est un handle valide ouvert en `SERVICE_ALL_ACCESS`, qui
    // contient `DELETE`.
    unsafe { DeleteService(service.0) }.map_err(|_| ErreurService::Appel {
        appel: "DeleteService",
        code: dernier_code(),
    })
}

/// L'état du service, tel que le contrôleur de services le rapporte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtatService {
    /// Pas installé.
    Absent,
    /// Installé, à l'arrêt.
    Arrete,
    /// En cours de démarrage.
    Demarre,
    /// En marche.
    EnMarche,
    /// En cours d'arrêt.
    EnArret,
    /// Un état que ce module ne nomme pas ; le code brut est conservé.
    Autre(u32),
}

impl EtatService {
    /// Traduit un `SERVICE_STATUS_CURRENT_STATE`.
    ///
    /// Fonction pure, testée : c'est la seule interprétation du code d'état, et un état
    /// inconnu garde son numéro plutôt que d'être replié sur « arrêté » — un service
    /// annoncé arrêté alors qu'il est en pause serait un diagnostic faux.
    #[must_use]
    pub const fn depuis_code(code: SERVICE_STATUS_CURRENT_STATE) -> Self {
        match code.0 {
            1 => Self::Arrete,
            2 => Self::Demarre,
            3 => Self::EnArret,
            4 => Self::EnMarche,
            autre => Self::Autre(autre),
        }
    }
}

impl core::fmt::Display for EtatService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Absent => f.write_str("non installé"),
            Self::Arrete => f.write_str("arrêté"),
            Self::Demarre => f.write_str("en cours de démarrage"),
            Self::EnMarche => f.write_str("en marche"),
            Self::EnArret => f.write_str("en cours d'arrêt"),
            Self::Autre(code) => write!(f, "état {code}"),
        }
    }
}

/// Interroge l'état du service.
///
/// # Erreurs
///
/// [`ErreurService`] pour tout ce qui n'est pas « le service n'existe pas » — ce cas-là
/// rend [`EtatService::Absent`], parce que c'est un renseignement et non une panne.
pub fn etat() -> Result<EtatService, ErreurService> {
    let scm = ouvrir_scm(SC_MANAGER_CONNECT)?;
    let service = match ouvrir_service(&scm, windows::Win32::System::Services::SERVICE_QUERY_STATUS)
    {
        Ok(service) => service,
        Err(ErreurService::Absent) => return Ok(EtatService::Absent),
        Err(erreur) => return Err(erreur),
    };
    let mut statut = SERVICE_STATUS::default();
    // SAFETY: `service` est un handle valide ouvert avec `SERVICE_QUERY_STATUS` ;
    // `statut` est une variable de cette pile, écrite par l'appelé seul.
    unsafe { QueryServiceStatus(service.0, &mut statut) }.map_err(|_| ErreurService::Appel {
        appel: "QueryServiceStatus",
        code: dernier_code(),
    })?;
    Ok(EtatService::depuis_code(statut.dwCurrentState))
}

// ---------------------------------------------------------------------------------
// Le dispatcher : ce que le contrôleur de services appelle.
// ---------------------------------------------------------------------------------

/// Le handle de statut du service, posé par [`service_main`].
///
/// Un `AtomicIsize` plutôt qu'un `OnceLock<SERVICE_STATUS_HANDLE>` : le gestionnaire de
/// contrôle est une fonction `extern "system"` appelée par un fil du système, et un
/// entier atomique est ce qu'on peut lui offrir sans verrou ni allocation.
static HANDLE_STATUT: AtomicIsize = AtomicIsize::new(0);

/// Ce que le gestionnaire de contrôle a besoin d'atteindre.
///
/// `OnceLock` : posé une fois par [`service_main`], lu par le gestionnaire de contrôle
/// et par la boucle.
static CONTEXTE: OnceLock<Contexte> = OnceLock::new();

/// L'état partagé du service en marche.
struct Contexte {
    journal: Arc<Journal>,
    arret: Arc<Arret>,
}

/// Publie l'état du service auprès du contrôleur de services.
///
/// Un service qui n'annonce pas `SERVICE_RUNNING` est tué au bout du délai de démarrage,
/// et un service qui n'annonce pas `SERVICE_STOPPED` laisse la console des services
/// bloquée sur « arrêt en cours » jusqu'au redémarrage du poste.
fn publier_etat(etat: SERVICE_STATUS_CURRENT_STATE, code_sortie: u32) {
    let handle =
        SERVICE_STATUS_HANDLE(HANDLE_STATUT.load(Ordering::SeqCst) as *mut core::ffi::c_void);
    if handle.0.is_null() {
        return;
    }
    let statut = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: etat,
        // On accepte l'arrêt et la fermeture du poste, et rien d'autre : ni pause, ni
        // reprise, ni changement de matériel. Annoncer un contrôle qu'on ne sait pas
        // servir ferait attendre le système pour rien.
        dwControlsAccepted: if etat == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: code_sortie,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        // Le délai annoncé pour les états transitoires : le canal se ferme en quelques
        // millisecondes, cinq secondes sont très larges.
        dwWaitHint: if etat == SERVICE_STOP_PENDING || etat == SERVICE_START_PENDING {
            5_000
        } else {
            0
        },
    };
    // SAFETY: `handle` vient de `RegisterServiceCtrlHandlerExW` (non nul, vérifié
    // ci-dessus) et reste valide jusqu'à la fin du processus ; `statut` est une variable
    // de cette pile, lue par l'appelé pendant l'appel seulement.
    let _ = unsafe { SetServiceStatus(handle, &raw const statut) };
}

/// Le gestionnaire de contrôle, appelé par un fil du système.
///
/// Il ne fait **rien de long** : lever le drapeau d'arrêt et publier l'état. Tout travail
/// ici bloquerait le contrôleur de services.
extern "system" fn gestionnaire_controle(
    controle: u32,
    _type_evenement: u32,
    _donnees: *mut core::ffi::c_void,
    _contexte: *mut core::ffi::c_void,
) -> u32 {
    if controle == SERVICE_CONTROL_STOP || controle == SERVICE_CONTROL_SHUTDOWN {
        publier_etat(SERVICE_STOP_PENDING, 0);
        if let Some(contexte) = CONTEXTE.get() {
            contexte.journal.info(if controle == SERVICE_CONTROL_STOP {
                "arrêt demandé par le contrôleur de services"
            } else {
                "fermeture du poste : arrêt du service"
            });
            contexte.arret.demander();
        }
    }
    // `NO_ERROR` : le contrôle a été pris en compte. Les contrôles qu'on n'accepte pas
    // n'arrivent jamais ici, le système les filtre sur `dwControlsAccepted`.
    0
}

/// Le point d'entrée que le contrôleur de services appelle.
extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let nom = large(NOM_SERVICE);
    // SAFETY: `nom` est une chaîne large terminée par NUL, vivante pendant tout l'appel ;
    // `gestionnaire_controle` est une fonction `extern "system"` de ce module, valide
    // pour toute la durée du processus. Aucun contexte n'est transmis : l'état partagé
    // passe par `CONTEXTE`.
    let handle = match unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(nom.as_ptr()), Some(gestionnaire_controle), None)
    } {
        Ok(handle) => handle,
        // Sans handle de statut, il n'y a rien à publier et rien à servir : on rend la
        // main, le contrôleur de services conclura à un échec de démarrage.
        Err(_) => return,
    };
    HANDLE_STATUT.store(handle.0 as isize, Ordering::SeqCst);
    publier_etat(SERVICE_START_PENDING, 0);

    // Le journal du service va dans un fichier : un service n'a pas de console, et sa
    // sortie d'erreur ne va nulle part.
    let journal =
        match Journal::vers_fichier(&crate::journal::chemin_par_defaut(), Niveau::Info, false) {
            Ok(journal) => Arc::new(journal),
            // Journal impossible à ouvrir : on sert quand même, sans trace. Refuser de
            // démarrer parce qu'on ne peut pas écrire un fichier priverait l'utilisateur de
            // ses câbles pour une raison secondaire.
            Err(_) => Arc::new(Journal::vers_stderr(Niveau::Info)),
        };
    let arret = match Arret::nouveau() {
        Ok(arret) => Arc::new(arret),
        Err(erreur) => {
            // Sans l'événement d'arrêt, le service ne saurait pas s'arrêter proprement :
            // on ne démarre pas plutôt que de laisser la console des services bloquée sur
            // « arrêt en cours » jusqu'au redémarrage du poste.
            journal.erreur(&format!("événement d'arrêt impossible à créer : {erreur}"));
            publier_etat(SERVICE_STOPPED, 1);
            return;
        }
    };
    let _ = CONTEXTE.set(Contexte {
        journal: Arc::clone(&journal),
        arret: Arc::clone(&arret),
    });

    publier_etat(SERVICE_RUNNING, 0);
    let code = match tube::servir(&journal, &arret) {
        Ok(()) => 0,
        Err(erreur) => {
            journal.erreur(&format!("le service n'a pas pu servir : {erreur}"));
            // `ERROR_SERVICE_SPECIFIC_ERROR` serait plus précis mais exigerait un code
            // propre au service ; `ERROR_INVALID_FUNCTION` (1) dit simplement « ça n'a
            // pas marché » et se lit dans l'Observateur d'événements.
            1
        }
    };
    journal.info("service arrêté");
    publier_etat(SERVICE_STOPPED, code);
}

/// Appelle le dispatcher : c'est ce que fait la sous-commande `service`.
///
/// **Ne rend la main qu'à l'arrêt du service.** Lancé depuis une console ordinaire,
/// `StartServiceCtrlDispatcherW` échoue avec
/// `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT` (1063) — c'est la façon dont le système dit
/// « ce processus n'a pas été lancé par le contrôleur de services », et le message le
/// traduit plutôt que de laisser un 1063 nu.
///
/// # Erreurs
///
/// [`ErreurService::Appel`] quand le dispatcher refuse.
pub fn lancer_dispatcher() -> Result<(), ErreurService> {
    let mut nom = large(NOM_SERVICE);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(nom.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        // L'entrée nulle qui termine la table, exigée par l'API.
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::null(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: `table` est un tableau de cette pile, terminé par l'entrée nulle exigée, et
    // il vit pendant tout l'appel — qui ne rend la main qu'à l'arrêt du service. `nom`
    // vit aussi longtemps que `table`, qui la pointe.
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }.map_err(|_| ErreurService::Appel {
        appel: "StartServiceCtrlDispatcherW",
        code: dernier_code(),
    })
}

/// Sert le canal **en avant-plan**, sans contrôleur de services.
///
/// C'est le mode `console` : la même boucle, le même descripteur de sécurité, le même
/// protocole, mais journal sur la sortie d'erreur et arrêt par `Ctrl+C`. Il sert au
/// diagnostic dans la VM, quand on veut voir les lignes défiler plutôt que d'aller lire
/// un fichier.
///
/// **Attention** : lancé sous un compte administrateur ordinaire plutôt qu'en
/// `LocalSystem`, il arme `SeLoadDriverPrivilege` dans **ce** jeton — ce qui marche,
/// puisqu'un administrateur le détient. C'est une commodité de diagnostic, pas le mode
/// d'exploitation.
///
/// # Erreurs
///
/// [`crate::tube::ErreurTube`] si le canal ne peut pas être créé.
pub fn servir_en_console(verbeux: bool) -> Result<(), crate::tube::ErreurTube> {
    let plancher = if verbeux { Niveau::Debug } else { Niveau::Info };
    let journal = Arc::new(Journal::vers_stderr(plancher));
    let arret = Arc::new(Arret::nouveau()?);
    tube::servir(&journal, &arret)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Le chemin du binaire est **cité** dans la ligne de commande du service.
    ///
    /// Un chemin non cité contenant un espace est l'élévation de privilège classique du
    /// « unquoted service path » : `C:\Program Files\…` s'interprète comme
    /// `C:\Program.exe` avec des arguments, et quiconque peut écrire à la racine de `C:`
    /// prend la place du service.
    #[test]
    fn le_chemin_du_service_est_cite() {
        let commande = ligne_de_commande(std::path::Path::new(
            "C:\\Program Files\\Conduit\\conduit-helper.exe",
        ));
        assert_eq!(
            commande,
            "\"C:\\Program Files\\Conduit\\conduit-helper.exe\" service"
        );
        assert!(commande.starts_with('"'), "{commande}");
        // La sous-commande est bien celle que le dispatcher attend.
        assert!(commande.ends_with(SOUS_COMMANDE_SERVICE), "{commande}");
        assert_eq!(SOUS_COMMANDE_SERVICE, "service");

        // Un chemin sans espace est cité quand même : une seule forme, pas deux.
        let simple = ligne_de_commande(std::path::Path::new("C:\\conduit-helper.exe"));
        assert_eq!(simple, "\"C:\\conduit-helper.exe\" service");
    }

    /// Les noms du service sont gravés : le MSI et les scripts de la VM s'y donnent
    /// rendez-vous.
    #[test]
    fn les_noms_du_service_sont_graves() {
        assert_eq!(NOM_SERVICE, "ConduitHelper");
        // Pas d'espace dans le nom court : c'est celui que `sc` prend en argument.
        assert!(!NOM_SERVICE.contains(' '), "{NOM_SERVICE}");
        assert!(NOM_AFFICHE.contains("Conduit"), "{NOM_AFFICHE}");
        // La description dit ce que coûte un arrêt du service, pas seulement ce qu'il
        // fait.
        assert!(DESCRIPTION.contains("privilège"), "{DESCRIPTION}");
        assert!(DESCRIPTION.contains("Arrêter"), "{DESCRIPTION}");
    }

    /// Table des états : chaque code du système a son nom, et un code inconnu garde son
    /// numéro.
    #[test]
    fn etat_table() {
        let cas: [(u32, EtatService); 5] = [
            (1, EtatService::Arrete),
            (2, EtatService::Demarre),
            (3, EtatService::EnArret),
            (4, EtatService::EnMarche),
            (7, EtatService::Autre(7)),
        ];
        for (code, attendu) in cas {
            assert_eq!(
                EtatService::depuis_code(SERVICE_STATUS_CURRENT_STATE(code)),
                attendu,
                "code {code}"
            );
        }
        // Les constantes du système sont bien celles qu'on traduit.
        assert_eq!(SERVICE_STOPPED.0, 1);
        assert_eq!(SERVICE_START_PENDING.0, 2);
        assert_eq!(SERVICE_STOP_PENDING.0, 3);
        assert_eq!(SERVICE_RUNNING.0, 4);

        // Chaque état a un nom en français, et aucun n'est vide.
        for etat in [
            EtatService::Absent,
            EtatService::Arrete,
            EtatService::Demarre,
            EtatService::EnMarche,
            EtatService::EnArret,
            EtatService::Autre(9),
        ] {
            assert!(!etat.to_string().is_empty(), "{etat:?}");
        }
        assert!(EtatService::Autre(9).to_string().contains('9'));
    }

    /// Les erreurs disent quoi faire : l'accès refusé nomme l'élévation, le doublon
    /// nomme la désinstallation.
    #[test]
    fn les_erreurs_de_service_disent_quoi_faire() {
        let refuse = ErreurService::Appel {
            appel: "CreateServiceW",
            code: 5,
        }
        .to_string();
        assert!(refuse.contains("administrateur"), "{refuse}");

        let doublon = ErreurService::Appel {
            appel: "CreateServiceW",
            code: 1073,
        }
        .to_string();
        assert!(doublon.contains("desinstaller"), "{doublon}");

        let absent = ErreurService::Absent.to_string();
        assert!(absent.contains("installer"), "{absent}");

        let autre = ErreurService::Appel {
            appel: "DeleteService",
            code: 1234,
        }
        .to_string();
        assert!(autre.contains("1234"), "{autre}");
    }
}
