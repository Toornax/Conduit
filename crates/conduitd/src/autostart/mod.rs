//! Démarrage automatique du démon à l'ouverture de session (F-51, ADR-013, M1b-35).
//!
//! Sous Windows, `conduitd` **n'est pas un service** : un service tourne en session 0,
//! sans les endpoints ni les périphériques par défaut de l'utilisateur (ADR-013). Le
//! démarrage automatique passe donc par une **tâche planifiée par utilisateur**,
//! `Conduit\conduitd`, déclenchée à l'ouverture de session.
//!
//! ## Pourquoi `schtasks.exe` et un fichier XML plutôt que l'API COM
//!
//! Le Planificateur a une API COM (`ITaskService`) que le crate `windows` expose. Elle
//! n'a pas été retenue :
//!
//! - le **XML est le format natif** du Planificateur : c'est ce que son interface
//!   graphique exporte et importe, ce qu'un utilisateur peut relire, comparer et
//!   réimporter à la main. Le générer nous donne un artefact lisible et testable sans
//!   Windows (voir [`xml`]), alors que la même définition construite en COM n'existe
//!   nulle part sous forme inspectable ;
//! - `schtasks.exe` est présent sur tout Windows depuis XP, dans `System32` (on
//!   l'appelle par son chemin absolu, jamais par le `PATH`) ;
//! - la voie COM demanderait une centaine de lignes d'`unsafe` de plus (`BSTR`,
//!   `VARIANT`, `IRegisteredTask`) dans un crate qui n'en a pas, pour trois opérations
//!   lancées une fois dans la vie d'une installation. Le coût de robustesse est ailleurs :
//!   dans le XML, qui est validé par le Planificateur lui-même à la création.
//!
//! Le prix payé est la lecture de la sortie de `schtasks /Query` pour `status` : elle est
//! **traduite** (« Prêt », « Ready »). On la restitue telle quelle plutôt que de
//! l'interpréter — l'état affiché est celui du Planificateur, mot pour mot.
//!
//! ## Codes de retour
//!
//! | Code | Sens |
//! |---|---|
//! | 0 | opération effectuée (`status` : la tâche existe) |
//! | 1 | `schtasks` a échoué (le message est repris) |
//! | 2 | plateforme sans tâche planifiée (Linux, macOS) |
//! | 4 | `status` : la tâche est absente |
//!
//! `disable` sur une tâche absente rend 0 : la désinstallation doit pouvoir l'appeler
//! deux fois.

pub mod xml;

#[cfg(windows)]
mod schtasks;

/// Nom de la tâche planifiée de Conduit.
pub const DEFAULT_TASK_NAME: &str = r"Conduit\conduitd";

/// Délai entre l'ouverture de session et le démarrage du démon, en secondes.
pub const DEFAULT_DELAY_SECS: u32 = 30;

/// Opération effectuée.
pub const EXIT_OK: i32 = 0;
/// L'outil du système a échoué.
pub const EXIT_FAILED: i32 = 1;
/// Plateforme sans tâche planifiée.
pub const EXIT_UNSUPPORTED: i32 = 2;
/// `status` : la tâche n'est pas enregistrée.
pub const EXIT_ABSENT: i32 = 4;

/// Ce que la sous-commande doit faire.
#[derive(Debug, Clone)]
pub struct Options {
    /// Nom de la tâche (`--task-name`, caché : les tests s'en servent pour ne pas
    /// toucher à la vraie tâche de l'utilisateur).
    pub task_name: String,
    /// Arguments supplémentaires passés à `conduitd` par la tâche.
    pub args: Vec<String>,
    /// Délai après l'ouverture de session, en secondes.
    pub delay_secs: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            task_name: DEFAULT_TASK_NAME.to_string(),
            args: Vec::new(),
            delay_secs: DEFAULT_DELAY_SECS,
        }
    }
}

/// Enregistre (ou remplace) la tâche de démarrage automatique.
pub fn enable(options: &Options) -> i32 {
    #[cfg(windows)]
    {
        schtasks::enable(options)
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        unsupported()
    }
}

/// Supprime la tâche de démarrage automatique.
pub fn disable(options: &Options) -> i32 {
    #[cfg(windows)]
    {
        schtasks::disable(options)
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        unsupported()
    }
}

/// Affiche l'état de la tâche de démarrage automatique.
pub fn status(options: &Options) -> i32 {
    #[cfg(windows)]
    {
        schtasks::status(options)
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        unsupported()
    }
}

/// Message des plateformes où l'autodémarrage n'est pas une tâche planifiée.
#[cfg(not(windows))]
fn unsupported() -> i32 {
    let mecanisme = if cfg!(target_os = "macos") {
        "un LaunchAgent (`~/Library/LaunchAgents/fr.conduit.conduitd.plist`), à venir en M5"
    } else {
        "une unité systemd `--user` (`systemctl --user enable --now conduitd`), à venir en M4"
    };
    eprintln!(
        "`conduitd autostart` ne gère que la tâche planifiée de Windows (ADR-013).\n\
         Sur cette plateforme, l'autodémarrage passe par {mecanisme}."
    );
    EXIT_UNSUPPORTED
}

/// Découpe une ligne de `schtasks /FO CSV`.
///
/// Format CSV de Microsoft : chaque champ est entre guillemets, un guillemet littéral
/// est doublé. Écrit ici (et non dans le module Windows) pour être testé partout.
pub fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    fields.push(current);
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_line_of_schtasks_is_split_on_unquoted_commas() {
        let line = r#""MON-PC","\Conduit\conduitd","06/09/2026 16:27:30","Prêt""#;
        assert_eq!(
            csv_fields(line),
            vec![
                "MON-PC",
                r"\Conduit\conduitd",
                "06/09/2026 16:27:30",
                "Prêt"
            ]
        );
    }

    #[test]
    fn quotes_and_commas_inside_a_field_survive() {
        // Une commande citée : `schtasks` double les guillemets internes.
        let line = r#""MON-PC","\T","C:\a b\conduitd.exe --arg ""x, y""""#;
        let f = csv_fields(line);
        assert_eq!(f.len(), 3);
        assert_eq!(f[2], r#"C:\a b\conduitd.exe --arg "x, y""#);
    }

    #[test]
    fn default_options_target_the_real_task() {
        let o = Options::default();
        assert_eq!(o.task_name, r"Conduit\conduitd");
        assert_eq!(o.delay_secs, 30);
        assert!(o.args.is_empty());
    }

    #[cfg(not(windows))]
    #[test]
    fn other_platforms_report_unsupported() {
        assert_eq!(status(&Options::default()), EXIT_UNSUPPORTED);
    }
}
