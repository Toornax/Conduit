//! XML de définition d'une tâche planifiée Windows (schéma
//! `http://schemas.microsoft.com/windows/2004/02/mit/task`, version 1.2).
//!
//! Ce module ne fait que produire du **texte**, puis l'encoder : il ne dépend d'aucune
//! plateforme et se teste partout. C'est ce fichier que
//! `schtasks.exe /Create /XML` reçoit, et c'est exactement ce que le Planificateur de
//! tâches réexporte : un utilisateur peut le lire, le comparer, l'importer à la main.

use std::fmt::Write as _;

/// Espace de noms du schéma du Planificateur de tâches.
pub const SCHEMA: &str = "http://schemas.microsoft.com/windows/2004/02/mit/task";

/// Définition de la tâche qui démarre `conduitd` à l'ouverture de session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDefinition {
    /// Chemin complet de la tâche, avec sa barre oblique inverse initiale
    /// (`\Conduit\conduitd`).
    pub uri: String,
    /// Auteur affiché par le Planificateur.
    pub author: String,
    /// Description affichée par le Planificateur.
    pub description: String,
    /// Utilisateur pour qui la tâche s'exécute (`DOMAINE\utilisateur` ou un SID).
    pub user_id: String,
    /// Délai après l'ouverture de session, en secondes.
    pub delay_seconds: u32,
    /// Chemin absolu de l'exécutable.
    pub command: String,
    /// Ligne d'arguments (vide = élément omis).
    pub arguments: String,
    /// Répertoire de travail (vide = élément omis).
    pub working_directory: String,
    /// Nombre de redémarrages après un échec.
    pub restart_count: u32,
    /// Intervalle entre deux redémarrages, en minutes.
    pub restart_interval_minutes: u32,
}

impl TaskDefinition {
    /// Définition par défaut de Conduit : `uri` complet, utilisateur, exécutable.
    pub fn new(uri: &str, user_id: &str, command: &str) -> Self {
        Self {
            uri: uri.to_string(),
            author: user_id.to_string(),
            description: "Démon audio Conduit — démarré à l'ouverture de session \
                          (voir docs/user/windows.md)"
                .to_string(),
            user_id: user_id.to_string(),
            delay_seconds: super::DEFAULT_DELAY_SECS,
            command: command.to_string(),
            arguments: String::new(),
            working_directory: String::new(),
            restart_count: 3,
            restart_interval_minutes: 1,
        }
    }
}

/// Échappe le texte d'un nœud ou d'un attribut XML.
///
/// Un chemin Windows peut contenir `&` (`C:\Program Files\A & B\conduitd.exe`) : sans
/// échappement, `schtasks` refuse le fichier. Les guillemets et l'apostrophe sont
/// échappés aussi, pour que la même fonction serve aux attributs.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Rend le XML de définition de la tâche.
///
/// Choix explicites, dans l'ordre du fichier :
///
/// - **`LogonTrigger`** avec `UserId` : « à l'ouverture de session de cet utilisateur »,
///   pas de tous. `Delay` = 30 s par défaut, pour ne pas disputer le disque à
///   l'ouverture de session (ADR-013).
/// - **`InteractiveToken` / `LeastPrivilege`** : le démon tourne dans la session
///   interactive, aux droits de l'utilisateur, sans élévation (SPEC §5.10).
/// - **`MultipleInstancesPolicy = StopExisting`** : « Arrêter la tâche existante », ce
///   qui rejoint l'instance unique.
/// - **`ExecutionTimeLimit = PT0S`** : aucune limite de durée — un démon tourne tant que
///   la session dure.
/// - **`DisallowStartIfOnBatteries` / `StopIfGoingOnBatteries` = `false`** : l'audio ne
///   s'arrête pas parce qu'on débranche le portable.
/// - **`RestartOnFailure`** : trois reprises espacées d'une minute.
/// - **`Priority = 5`** : classe de priorité **normale**. Le défaut du Planificateur
///   (7) donne `BELOW_NORMAL_PRIORITY_CLASS`, ce qu'un démon audio ne peut pas se
///   permettre.
pub fn task_xml(def: &TaskDefinition) -> String {
    let mut x = String::with_capacity(2048);
    let _ = writeln!(x, r#"<?xml version="1.0" encoding="UTF-16"?>"#);
    let _ = writeln!(x, r#"<Task version="1.2" xmlns="{SCHEMA}">"#);
    let _ = writeln!(x, "  <RegistrationInfo>");
    let _ = writeln!(x, "    <Author>{}</Author>", escape(&def.author));
    let _ = writeln!(
        x,
        "    <Description>{}</Description>",
        escape(&def.description)
    );
    let _ = writeln!(x, "    <URI>{}</URI>", escape(&def.uri));
    let _ = writeln!(x, "  </RegistrationInfo>");
    let _ = writeln!(x, "  <Principals>");
    let _ = writeln!(x, r#"    <Principal id="Author">"#);
    let _ = writeln!(x, "      <UserId>{}</UserId>", escape(&def.user_id));
    let _ = writeln!(x, "      <LogonType>InteractiveToken</LogonType>");
    let _ = writeln!(x, "      <RunLevel>LeastPrivilege</RunLevel>");
    let _ = writeln!(x, "    </Principal>");
    let _ = writeln!(x, "  </Principals>");
    let _ = writeln!(x, "  <Settings>");
    let _ = writeln!(x, "    <AllowHardTerminate>true</AllowHardTerminate>");
    let _ = writeln!(x, "    <AllowStartOnDemand>true</AllowStartOnDemand>");
    let _ = writeln!(
        x,
        "    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"
    );
    let _ = writeln!(x, "    <Enabled>true</Enabled>");
    let _ = writeln!(x, "    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>");
    let _ = writeln!(x, "    <Hidden>false</Hidden>");
    // Explicites, même si `RunOnlyIfIdle` est faux : sans elles, le Planificateur écrit
    // ses propres valeurs par défaut (`StopOnIdleEnd` vrai) dans la définition stockée.
    let _ = writeln!(x, "    <IdleSettings>");
    let _ = writeln!(x, "      <StopOnIdleEnd>false</StopOnIdleEnd>");
    let _ = writeln!(x, "      <RestartOnIdle>false</RestartOnIdle>");
    let _ = writeln!(x, "    </IdleSettings>");
    let _ = writeln!(
        x,
        "    <MultipleInstancesPolicy>StopExisting</MultipleInstancesPolicy>"
    );
    let _ = writeln!(x, "    <Priority>5</Priority>");
    let _ = writeln!(x, "    <RestartOnFailure>");
    let _ = writeln!(x, "      <Count>{}</Count>", def.restart_count);
    let _ = writeln!(
        x,
        "      <Interval>PT{}M</Interval>",
        def.restart_interval_minutes
    );
    let _ = writeln!(x, "    </RestartOnFailure>");
    let _ = writeln!(x, "    <RunOnlyIfIdle>false</RunOnlyIfIdle>");
    let _ = writeln!(
        x,
        "    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>"
    );
    let _ = writeln!(x, "    <StartWhenAvailable>false</StartWhenAvailable>");
    let _ = writeln!(
        x,
        "    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"
    );
    let _ = writeln!(x, "    <WakeToRun>false</WakeToRun>");
    let _ = writeln!(x, "  </Settings>");
    let _ = writeln!(x, "  <Triggers>");
    let _ = writeln!(x, "    <LogonTrigger>");
    let _ = writeln!(x, "      <Enabled>true</Enabled>");
    let _ = writeln!(x, "      <UserId>{}</UserId>", escape(&def.user_id));
    let _ = writeln!(x, "      <Delay>PT{}S</Delay>", def.delay_seconds);
    let _ = writeln!(x, "    </LogonTrigger>");
    let _ = writeln!(x, "  </Triggers>");
    let _ = writeln!(x, r#"  <Actions Context="Author">"#);
    let _ = writeln!(x, "    <Exec>");
    let _ = writeln!(x, "      <Command>{}</Command>", escape(&def.command));
    if !def.arguments.is_empty() {
        let _ = writeln!(x, "      <Arguments>{}</Arguments>", escape(&def.arguments));
    }
    if !def.working_directory.is_empty() {
        let _ = writeln!(
            x,
            "      <WorkingDirectory>{}</WorkingDirectory>",
            escape(&def.working_directory)
        );
    }
    let _ = writeln!(x, "    </Exec>");
    let _ = writeln!(x, "  </Actions>");
    let _ = writeln!(x, "</Task>");
    x
}

/// Encode le XML en UTF-16 petit-boutien **avec BOM**.
///
/// `schtasks.exe /Create /XML` refuse un fichier qui n'est pas en Unicode
/// (« The task XML is malformed ») : la déclaration `encoding="UTF-16"` doit
/// correspondre au contenu réel du fichier.
pub fn utf16le(xml: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + xml.len() * 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TaskDefinition {
        TaskDefinition::new(
            r"\Conduit\conduitd",
            r"MON-PC\Nathan",
            r"C:\Program Files\A & B\conduitd.exe",
        )
    }

    #[test]
    fn escapes_the_five_xml_entities() {
        assert_eq!(
            escape(r#"a & b < c > d " e ' f"#),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
        assert_eq!(
            escape(r"C:\Program Files\conduitd.exe"),
            r"C:\Program Files\conduitd.exe"
        );
    }

    #[test]
    fn xml_carries_the_settings_the_roadmap_asks_for() {
        let x = task_xml(&sample());
        assert!(
            x.starts_with(r#"<?xml version="1.0" encoding="UTF-16"?>"#),
            "{x}"
        );
        assert!(
            x.contains(&format!(r#"<Task version="1.2" xmlns="{SCHEMA}">"#)),
            "{x}"
        );
        // Déclencheur : ouverture de session de cet utilisateur, 30 s de délai.
        assert!(x.contains("<LogonTrigger>"), "{x}");
        assert!(x.contains(r"<UserId>MON-PC\Nathan</UserId>"), "{x}");
        assert!(x.contains("<Delay>PT30S</Delay>"), "{x}");
        // Options demandées par M1b-35.
        assert!(
            x.contains("<MultipleInstancesPolicy>StopExisting</MultipleInstancesPolicy>"),
            "{x}"
        );
        assert!(
            x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"),
            "{x}"
        );
        assert!(
            x.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"),
            "{x}"
        );
        assert!(
            x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"),
            "{x}"
        );
        assert!(x.contains("<Count>3</Count>"), "{x}");
        assert!(x.contains("<Interval>PT1M</Interval>"), "{x}");
        assert!(x.contains("<StopOnIdleEnd>false</StopOnIdleEnd>"), "{x}");
        // Sans élévation, dans la session interactive (SPEC §5.10).
        assert!(x.contains("<LogonType>InteractiveToken</LogonType>"), "{x}");
        assert!(x.contains("<RunLevel>LeastPrivilege</RunLevel>"), "{x}");
        // Priorité normale, pas le défaut « inférieure à la normale » du Planificateur.
        assert!(x.contains("<Priority>5</Priority>"), "{x}");
    }

    #[test]
    fn command_path_is_escaped_and_optional_elements_are_omitted() {
        let x = task_xml(&sample());
        assert!(
            x.contains(r"<Command>C:\Program Files\A &amp; B\conduitd.exe</Command>"),
            "{x}"
        );
        assert!(
            !x.contains("<Arguments>"),
            "arguments vides : élément omis\n{x}"
        );
        assert!(!x.contains("<WorkingDirectory>"), "{x}");
        let mut def = sample();
        def.arguments = "--backend null --log-level debug".into();
        def.working_directory = r"C:\Program Files\A & B".into();
        let x = task_xml(&def);
        assert!(
            x.contains("<Arguments>--backend null --log-level debug</Arguments>"),
            "{x}"
        );
        assert!(
            x.contains(r"<WorkingDirectory>C:\Program Files\A &amp; B</WorkingDirectory>"),
            "{x}"
        );
    }

    #[test]
    fn every_open_tag_is_closed() {
        // Contrôle grossier mais utile : autant de « < » que de « > », et le document
        // se termine par la balise fermante de la racine.
        let x = task_xml(&sample());
        assert_eq!(x.matches('<').count(), x.matches('>').count(), "{x}");
        assert!(x.trim_end().ends_with("</Task>"), "{x}");
    }

    #[test]
    fn utf16le_starts_with_a_bom_and_round_trips() {
        let bytes = utf16le("<Task/>é");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "BOM petit-boutien");
        assert_eq!(bytes.len(), 2 + "<Task/>é".encode_utf16().count() * 2);
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "<Task/>é");
    }
}
