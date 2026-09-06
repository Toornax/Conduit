//! Textes de l'interface, centralisés en une table (préparation de M2-11).
//!
//! Une seule langue est fournie pour l'instant (français). Les appelants
//! désignent un texte par une variante de [`Text`], jamais par une chaîne
//! littérale : le jour où plusieurs langues existeront (M2-11, `fluent`), seule
//! la résolution [`t`] changera, et [`TABLE`] servira de catalogue à traduire.

use std::time::Duration;

macro_rules! textes {
    ($($variant:ident => $key:literal, $fr:literal;)*) => {
        /// Un texte affiché par l'interface.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Text {
            $(
                #[doc = $fr]
                $variant,
            )*
        }

        impl Text {
            /// Clé stable du texte, indépendante de la langue.
            pub const fn key(self) -> &'static str {
                match self {
                    $(Text::$variant => $key,)*
                }
            }

            /// Texte français.
            pub const fn fr(self) -> &'static str {
                match self {
                    $(Text::$variant => $fr,)*
                }
            }
        }

        /// Catalogue complet : `(clé, texte français)`, dans l'ordre de [`Text`].
        pub const TABLE: &[(&str, &str)] = &[$(($key, $fr),)*];
    };
}

textes! {
    AppTitle => "app.title", "Conduit";
    Starting => "conn.starting", "Démarrage…";
    Connecting => "conn.connecting", "Connexion au démon…";
    Connected => "conn.connected", "Connecté";
    DaemonMissing => "conn.missing", "Démon absent";
    DaemonMissingHint => "conn.missing.hint",
        "Démarrez le démon avec « conduitd », puis attendez la reconnexion.";
    Socket => "conn.socket", "Socket";
    Server => "conn.server", "Démon";
    Waiting => "conn.waiting", "En attente du démon…";

    BrandTagline => "brand.tagline", "Câble audio virtuel";
    DaemonGone => "state.daemon.over", "Démon arrêté";
    DaemonGoneTitle => "state.daemon.title", "Le démon conduitd ne répond pas.";
    DaemonGoneDriver => "state.daemon.driver",
        "Vos câbles, eux, continuent de fonctionner en boucle locale : ce qui entre dans leur sortie ressort dans leur entrée, car c'est le pilote qui les tient, pas le démon. Le routage vers votre carte son, les gains et les VU-mètres sont suspendus jusqu'à son redémarrage.";
    DaemonGoneLinux => "state.daemon.linux",
        "Vos câbles disparaissent avec lui : ce sont des nœuds PipeWire que le démon crée, et rien ne les tient en son absence. Ils reviendront à son redémarrage, avec le routage, les gains et les VU-mètres.";
    OpenLog => "state.daemon.log", "Ouvrir le journal";
    DaemonNoLog => "state.daemon.log.nowhere",
        "Aucun répertoire de données : le démon n'a nulle part où écrire son journal.";
    WelcomeOver => "state.welcome.over", "Installation terminée";
    WelcomeTitle => "state.welcome.title", "Bienvenue dans Conduit.";
    WelcomeStart => "state.welcome.start", "Commencer";
    WelcomePatchbay => "state.welcome.patchbay", "Ouvrir le patchbay";
    DaemonRunning => "daemon.running", "conduitd en marche";
    DaemonStarting => "daemon.starting", "conduitd démarre…";
    DaemonStopped => "daemon.stopped", "conduitd arrêté";
    DaemonStart => "daemon.start", "Démarrer le démon";
    DaemonStartPending => "daemon.start.pending", "Démarrage…";
    DaemonNotFound => "daemon.notfound",
        "conduitd est introuvable : ni à côté de Conduit, ni dans le PATH.";
    DriverWindows => "driver.windows", "Pilote noyau conduit-kmd";
    DriverMacos => "driver.macos", "Plugin HAL conduit-hal";
    DriverLinux => "driver.linux", "Nœuds PipeWire (sans pilote)";
    DriverNone => "driver.none", "aucun";
    DriverInternal => "driver.internal", "horloge interne";

    TabCables => "tab.cables", "Câbles";
    TabPatchbay => "tab.patchbay", "Patchbay";
    TabDiagnostic => "tab.diagnostic", "Diagnostic";

    CablesAdd => "cables.add", "Ajouter un câble";
    CablesNone => "cables.none", "Aucun câble pour l'instant";
    CableVisible => "cables.visible.one", "visible par toutes les applications";
    CablesVisible => "cables.visible.many", "visibles par toutes les applications";
    CablesEmpty => "cables.empty",
        "Aucun câble. « Ajouter un câble » en crée un : il apparaîtra aussitôt dans les réglages audio du système.";
    ColumnDevice => "cables.column.device", "Périphérique";
    ColumnAlias => "cables.column.alias", "Alias";
    ColumnChannels => "cables.column.channels", "Canaux";
    ColumnLevel => "cables.column.level", "Niveau";
    ColumnState => "cables.column.state", "État";
    CableDuplex => "cables.duplex", "Sortie + entrée";
    CableAlias => "cables.alias.placeholder", "Sans alias";
    CableAliasNote => "cables.alias.note",
        "Les alias sont propres à Conduit ; le nom système reste « Conduit N ».";
    CableLevelInert => "cables.level.inert",
        "Le démon ne diffuse aucun niveau : les barres restent à zéro tant que les VU-mètres n'existent pas.";
    CableActive => "cables.active", "Actif";
    CableInactive => "cables.inactive", "Inactif";
    CableRemoveConfirm => "cables.remove.confirm", "Supprimer ?";

    PatchbayHint => "patchbay.hint",
        "Glissez d'une sortie (droite) vers une entrée (gauche) pour lier · cliquez un lien pour le sélectionner · Suppr pour le retirer";
    PatchbayEmpty => "patchbay.empty", "Aucun nœud dans le graphe.";
    NodeDriver => "patchbay.node.driver", "Pilote";
    NodeSuspended => "patchbay.node.suspended", "Suspendu";
    NodeCable => "patchbay.node.cable", "Câble";
    NodeHardware => "patchbay.node.hardware", "Matériel";
    NodeUtility => "patchbay.node.utility", "Utilitaire";
    PatchbayLinkRemove => "patchbay.link.remove", "Supprimer le lien";
    PatchbayGenerator => "patchbay.generator", "Générateur de test";
    Mute => "action.mute", "M";

    GraphDriver => "patchbay.driver", "Pilote de graphe";
    Quantum => "patchbay.quantum", "quantum";
    EngineUptime => "diag.uptime", "Moteur en marche depuis";

    DiagXruns => "diag.tile.xruns", "Xruns";
    DiagXrunsGoal => "diag.tile.xruns.goal", "Objectif : 0";
    DiagXrunsReset => "diag.xruns.reset", "Remettre à zéro";
    DiagLatency => "diag.tile.latency", "Latence estimée";
    DiagLatencyPath => "diag.tile.latency.note", "Câble → carte son";
    DiagCycle => "diag.tile.cycle", "Temps de cycle";
    DiagLoad => "diag.tile.load", "Charge CPU";
    DiagCore => "diag.tile.load.core", "un cœur";
    DiagMin => "diag.min", "min";
    DiagMax => "diag.max", "max";
    DiagBudget => "diag.budget", "budget";
    ColumnNode => "diag.column.node", "Nœud";
    ColumnLatency => "diag.column.latency", "Latence";
    ColumnBuffer => "diag.column.buffer", "Tampon";
    ColumnRatio => "diag.column.ratio", "Ratio";
    NodeStateInternal => "node.state.internal", "Interne";
    NodeStateActive => "node.state.active", "Actif";
    NodeStateDriver => "node.state.driver", "Pilote";
    NodeStateSuspended => "node.state.suspended", "Suspendu";
    DiagNoDevice => "diag.devices.empty",
        "Aucun périphérique ouvert : le moteur tourne sur son horloge interne.";
    DiagLatencyNote => "diag.latency.note",
        "La latence est celle du moteur — deux quanta —, la même pour tous les nœuds : le protocole n'en mesure aucune par nœud.";
    DiagBufferNote => "diag.buffer.note",
        "Le remplissage est un nombre de trames ; la barre le rapporte au plus grand remplissage du tableau, faute de capacité annoncée.";
    DiagEngine => "diag.engine", "Moteur";
    DriverAuto => "driver.auto", "automatique";
    DiagDriverHot => "diag.driver.hot",
        "Le changement se fait à chaud : un court silence est possible.";
    DiagQuantum => "diag.quantum", "Quantum";
    DiagSampleRate => "diag.samplerate", "Fréquence";
    DiagReadOnly => "diag.readonly",
        "Réglés au démarrage du démon : le protocole n'a aucune commande pour les changer.";
    DiagPlatform => "diag.platform", "Pilote de plateforme";
    DiagPlatformWindows => "diag.platform.windows",
        "Un pilote noyau installe les câbles comme des cartes son du système ; toute application les voit dans les réglages audio de Windows.";
    DiagPlatformMacos => "diag.platform.macos",
        "Un plugin HAL, chargé par coreaudiod, installe les câbles comme des périphériques du système ; aucune extension noyau n'est requise.";
    DiagPlatformLinux => "diag.platform.linux",
        "Aucun pilote n'est installé : les câbles sont des nœuds PipeWire, que le démon crée dans le graphe du serveur audio.";
    DiagExport => "diag.export", "Exporter un rapport";
    DiagNoDirectory => "diag.export.nowhere",
        "Aucun répertoire où écrire : ni Documents, ni répertoire de données.";

    ThemeLight => "theme.light", "Sericæ clair";
    ThemeDark => "theme.dark", "Sericæ sombre";

    UnitMs => "unit.ms", "ms";
    UnitS => "unit.s", "s";
    UnitMin => "unit.min", "min";
    UnitH => "unit.h", "h";
    UnitHz => "unit.hz", "Hz";
    UnitKhz => "unit.khz", "kHz";
    UnitDb => "unit.db", "dB";
    UnitPercent => "unit.percent", "%";
    UnitRatio => "unit.ratio", "×";
    UnitUs => "unit.us", "µs";
    UnitFrame => "unit.frame", "trame";
    UnitFrames => "unit.frames", "trames";
    Silence => "unit.silence", "−∞";
    Xrun => "diag.xrun", "xrun";
    Xruns => "diag.xruns", "xruns";
    NoXrun => "diag.xrun.none", "aucun xrun";
    Inconnu => "num.unknown", "—";
    Separateur => "ui.separator", "·";

    // Les conseils qui complètent un message d'erreur (voir `crate::erreurs`).
    // Le message du démon dit ce qui a échoué ; le conseil dit quoi faire
    // ensuite (ADR-006).
    AdviceNotFound => "error.advice.notfound",
        "Le graphe a changé depuis l'affichage : laissez la vue se remettre à jour, puis recommencez.";
    AdviceWouldCycle => "error.advice.wouldcycle",
        "Supprimez d'abord un lien du chemin de retour, puis refaites celui-ci.";
    AdviceInvalid => "error.advice.invalid",
        "Un lien va d'une sortie vers une entrée, et une seule fois : vérifiez ses deux extrémités.";
    AdviceDevice => "error.advice.device",
        "Vérifiez que le périphérique est branché et qu'aucune autre application ne le monopolise ; la vue Diagnostic dit lesquels sont ouverts.";
    AdviceCable => "error.advice.cable",
        "Supprimez un câble devenu inutile, et vérifiez que le pilote Conduit est installé.";
    AdviceBusy => "error.advice.busy",
        "Le moteur est occupé : attendez un instant, puis réessayez.";
    AdviceUnsupported => "error.advice.unsupported",
        "Ce démon ne sait pas faire cela : mettez Conduit à jour, la GUI et le démon ensemble.";
    AdviceInternal => "error.advice.internal",
        "Consultez le journal du démon ; si cela se reproduit, exportez un rapport depuis la vue Diagnostic.";
    AdviceDaemonMissing => "error.advice.daemon.missing",
        "Réinstallez Conduit, ou lancez « conduitd » vous-même dans un terminal.";
    AdviceDaemonRefused => "error.advice.daemon.refused",
        "Consultez le journal du démon, puis réessayez.";
    AdviceReport => "error.advice.report",
        "Vérifiez les droits d'écriture du dossier, puis réessayez.";

    Remove => "action.remove", "Supprimer";
    RemoveIcon => "action.remove.icon", "×";
    Minus => "action.minus", "−";
    Plus => "action.plus", "+";
    Cancel => "action.cancel", "Annuler";
    Dismiss => "action.dismiss", "Fermer";
}

/// Résout un texte dans la langue courante.
pub const fn t(text: Text) -> &'static str {
    text.fr()
}

/// « Reconnexion dans N s… », tronqué à la seconde, au minimum 1 s.
pub fn reconnecting_in(delay: Duration) -> String {
    let secs = delay.as_secs().max(1);
    format!("Reconnexion dans {secs} s…")
}

/// « 1 canal » ou « N canaux ».
pub fn channels_label(channels: u8) -> String {
    if channels <= 1 {
        format!("{channels} canal")
    } else {
        format!("{channels} canaux")
    }
}

/// Deux fragments séparés par le point médian : « 3 câbles · 48 kHz ».
fn juxtapose(gauche: &str, droite: &str) -> String {
    format!("{gauche} {} {droite}", t(Text::Separateur))
}

/// Sous-titre de la vue « Câbles » : « 3 câbles · visibles par toutes les
/// applications ».
///
/// Le nombre maximal de câbles n'est pas exposé au client : le sous-titre ne
/// dit que le compte courant.
pub fn cables_subtitle(count: usize) -> String {
    match count {
        0 => t(Text::CablesNone).to_string(),
        1 => juxtapose(&cables_count(1), t(Text::CableVisible)),
        n => juxtapose(&cables_count(n), t(Text::CablesVisible)),
    }
}

/// Le compte de câbles, accordé : « Aucun câble », « 1 câble », « 3 câbles ».
///
/// Le pied de la vue « Câbles » n'annonce que ce compte ; le sous-titre de
/// l'en-tête lui ajoute la phrase de visibilité (voir [`cables_subtitle`]).
pub fn cables_count(count: usize) -> String {
    match count {
        0 => "Aucun câble".to_string(),
        1 => "1 câble".to_string(),
        n => format!("{n} câbles"),
    }
}

/// « Conduit 3 créé. Il apparaît dans les réglages audio du système. »
///
/// `cable` est le **nom système** du câble (`CableId`), pas son alias : c'est
/// sous ce nom que le système d'exploitation le montre.
pub fn cable_created(cable: &str) -> String {
    format!("{cable} créé. Il apparaît dans les réglages audio du système.")
}

/// « Conduit 3 supprimé. »
pub fn cable_removed(cable: &str) -> String {
    format!("{cable} supprimé.")
}

/// « Conduit 3 passe à 4 canaux : réactivation du câble, court silence. »
///
/// Changer les canaux recrée le périphérique côté système : l'avertissement
/// dit le silence qui vient, il ne constate pas un changement déjà fait.
pub fn channels_changed(cable: &str, channels: u8) -> String {
    format!(
        "{cable} passe à {} : réactivation du câble, court silence.",
        channels_label(channels)
    )
}

/// « Lien refusé : il créerait une boucle (« Conduit 1 » alimente déjà
/// « Lecteur de musique »). »
///
/// Le lien irait de `source` vers `destination` ; il est refusé parce que
/// `destination` alimente déjà `source`, directement ou par un détour. La
/// phrase nomme les deux nœuds dans cet ordre-là : c'est le chemin qui existe
/// déjà, pas celui qu'on demandait.
pub fn lien_refuse_boucle(source: &str, destination: &str) -> String {
    format!("Lien refusé : il créerait une boucle (« {destination} » alimente déjà « {source} »).")
}

/// « Lien supprimé entre « Lecteur de musique » et « Conduit 1 ». »
pub fn lien_supprime(source: &str, destination: &str) -> String {
    format!("Lien supprimé entre « {source} » et « {destination} ».")
}

/// « « Haut-parleurs » coupé : plus aucun son n'en sort. » ou
/// « « Haut-parleurs » rétabli. »
///
/// La coupure mérite une phrase — elle fait taire quelque chose, et rien
/// d'autre à l'écran ne le crie —, là où un réglage de gain n'en mérite
/// aucune : c'est un geste continu, une phrase par mouvement serait du bruit.
pub fn noeud_coupe(nom: &str, coupe: bool) -> String {
    if coupe {
        format!("« {nom} » coupé : plus aucun son n'en sort.")
    } else {
        format!("« {nom} » rétabli.")
    }
}

/// « Lien coupé entre « Lecteur de musique » et « Conduit 1 ». » ou
/// « Lien rétabli entre … ».
pub fn lien_coupe(source: &str, destination: &str, coupe: bool) -> String {
    let quoi = if coupe { "coupé" } else { "rétabli" };
    format!("Lien {quoi} entre « {source} » et « {destination} ».")
}

/// « « Générateur de test » créé : sinus à 440 Hz, −12,0 dB. Reliez sa sortie
/// à une entrée pour l'entendre. »
///
/// La fréquence et le niveau arrivent déjà mis en forme (voir
/// [`crate::format`]).
pub fn generateur_cree(nom: &str, frequence: &str, niveau: &str) -> String {
    format!(
        "« {nom} » créé : sinus à {frequence}, {niveau}. \
         Reliez sa sortie à une entrée pour l'entendre."
    )
}

/// Sous-titre de la vue « Patchbay » : « Pilote de graphe : horloge interne ·
/// 48 kHz · quantum 256 ».
///
/// Les trois valeurs arrivent déjà mises en forme (voir [`crate::format`]) :
/// ce module ne compose que des mots.
pub fn patchbay_subtitle(pilote: &str, frequence: &str, quantum: &str) -> String {
    let graphe = format!("{} : {pilote}", t(Text::GraphDriver));
    let quantum = format!("{} {quantum}", t(Text::Quantum));
    juxtapose(&juxtapose(&graphe, frequence), &quantum)
}

/// Sous-titre de la vue « Diagnostic » : « Moteur en marche depuis 3 min ·
/// 2 xruns ».
pub fn diagnostic_subtitle(duree: &str, xruns: &str) -> String {
    juxtapose(&format!("{} {duree}", t(Text::EngineUptime)), xruns)
}

/// Le compte de nœuds, accordé : « aucun nœud », « 1 nœud », « 7 nœuds ».
pub fn noeuds_count(count: usize) -> String {
    match count {
        0 => "aucun nœud".to_string(),
        1 => "1 nœud".to_string(),
        n => format!("{n} nœuds"),
    }
}

/// Note de la tuile « Charge CPU » : « 7 nœuds · un cœur ».
///
/// Le moteur exécute son graphe sur **un** fil : la charge est celle d'un
/// cœur, pas celle de la machine. La note le dit plutôt que de laisser croire
/// à une mesure système.
pub fn charge_note(noeuds: usize) -> String {
    juxtapose(&noeuds_count(noeuds), t(Text::DiagCore))
}

/// Note de la tuile « Temps de cycle » : « min 511 µs · max 3 704 µs ·
/// budget 5 333 µs ».
///
/// Les trois durées arrivent déjà mises en forme (voir [`crate::format`]).
pub fn cycle_note(min: &str, max: &str, budget: &str) -> String {
    let min = format!("{} {min}", t(Text::DiagMin));
    let max = format!("{} {max}", t(Text::DiagMax));
    let budget = format!("{} {budget}", t(Text::DiagBudget));
    juxtapose(&juxtapose(&min, &max), &budget)
}

/// « Rapport écrit dans /home/lea/Documents/conduit-diagnostic-2026-09-06.txt »
///
/// La notice nomme le chemin : c'est la seule façon pour l'utilisateur de
/// retrouver le fichier, l'interface n'ouvrant aucun explorateur.
pub fn rapport_ecrit(chemin: &str) -> String {
    format!("Rapport écrit dans {chemin}")
}

/// Le paragraphe d'accueil, accordé au compte réel de câbles du miroir.
///
/// Le démon en crée deux par défaut (ADR-006 : « ça marche à l'installation »),
/// mais la phrase ne l'affirme pas : elle dit ce que le miroir montre.
pub fn accueil_cables(count: usize) -> String {
    match count {
        0 => "Aucun câble n'existe pour l'instant. « Ajouter un câble », dans la vue Câbles, \
              en crée un : il apparaît aussitôt dans les réglages audio du système."
            .to_string(),
        1 => "Un câble a été créé. Il apparaît déjà comme sortie et comme entrée dans les \
              réglages audio du système : il n'y a rien à configurer."
            .to_string(),
        n => format!(
            "{n} câbles ont été créés. Ils apparaissent déjà comme sorties et comme entrées \
             dans les réglages audio du système : il n'y a rien à configurer."
        ),
    }
}

/// La troisième ligne de contrôle de l'accueil : « 2 câbles · 48 kHz ·
/// quantum 256 ».
///
/// La fréquence et le quantum arrivent déjà mis en forme (voir
/// [`crate::format`]).
pub fn accueil_reglages(cables: usize, frequence: &str, quantum: &str) -> String {
    let quantum = format!("{} {quantum}", t(Text::Quantum));
    juxtapose(&juxtapose(&cables_count(cables), frequence), &quantum)
}

/// « Journal du démon : /home/lea/.local/share/conduit/logs — un fichier
/// conduitd.log par jour. »
///
/// La notice **dit le chemin** plutôt que d'ouvrir l'explorateur du système :
/// ouvrir un fichier demanderait une dépendance de plus pour un geste que
/// l'utilisateur fait très bien lui-même.
pub fn journal_du_demon(chemin: &str) -> String {
    format!("Journal du démon : {chemin} — un fichier conduitd.log par jour.")
}

/// Un message d'erreur suivi de son conseil : « nœud inconnu : 3 — Le graphe a
/// changé depuis l'affichage… ».
///
/// Le message reste **tel quel et en premier** (ADR-006, ADR-010) : c'est lui
/// qui connaît le détail, et il vient de qui l'a constaté. Le conseil suit,
/// séparé par un tiret cadratin — ou par une simple espace quand le message se
/// termine déjà par une ponctuation forte, deux phrases n'ayant pas besoin
/// d'un tiret pour se suivre. Un message vide laisse le conseil seul.
pub fn erreur_conseillee(message: &str, conseil: &str) -> String {
    let message = message.trim_end();
    if message.is_empty() {
        return conseil.to_string();
    }
    if message.ends_with(['.', '!', '?', '…']) {
        format!("{message} {conseil}")
    } else {
        format!("{message} — {conseil}")
    }
}

/// « Démon non lancé : permission refusée »
///
/// La cause vient du système : elle est reprise telle quelle, sans
/// reformulation (ADR-006).
pub fn demon_non_lance(cause: &str) -> String {
    format!("Démon non lancé : {cause}")
}

/// « Rapport non écrit : permission refusée. »
///
/// La cause vient du système : elle est reprise telle quelle, sans
/// reformulation (ADR-006).
pub fn rapport_non_ecrit(erreur: &str) -> String {
    format!("Rapport non écrit : {erreur}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_texts_non_empty() {
        let mut keys: Vec<&str> = TABLE.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "clés dupliquées dans la table i18n");
        assert!(TABLE.iter().all(|(_, v)| !v.is_empty()));
    }

    #[test]
    fn resolution_matches_the_table() {
        assert_eq!(t(Text::AppTitle), "Conduit");
        assert_eq!(
            TABLE
                .iter()
                .find(|(k, _)| *k == Text::Connected.key())
                .map(|(_, v)| *v),
            Some(t(Text::Connected))
        );
    }

    #[test]
    fn channels_label_agrees_in_number() {
        assert_eq!(channels_label(1), "1 canal");
        assert_eq!(channels_label(8), "8 canaux");
    }

    /// Le sous-titre des câbles s'accorde en nombre et ne suppose aucune
    /// limite.
    #[test]
    fn cables_subtitle_agrees_in_number() {
        assert_eq!(cables_subtitle(0), "Aucun câble pour l'instant");
        assert_eq!(
            cables_subtitle(1),
            "1 câble · visible par toutes les applications"
        );
        assert_eq!(
            cables_subtitle(3),
            "3 câbles · visibles par toutes les applications"
        );
        assert!(
            !cables_subtitle(3).contains("sur"),
            "aucune limite inventée"
        );
    }

    /// Le compte du pied s'accorde, sans la phrase de visibilité.
    #[test]
    fn cables_count_agrees_in_number() {
        assert_eq!(cables_count(0), "Aucun câble");
        assert_eq!(cables_count(1), "1 câble");
        assert_eq!(cables_count(12), "12 câbles");
        assert!(!cables_count(2).contains("visible"));
    }

    /// Les notices des trois actions nomment le câble par son nom système et
    /// s'accordent en nombre.
    #[test]
    fn les_notices_nomment_le_cable_et_s_accordent() {
        assert_eq!(
            cable_created("Conduit 3"),
            "Conduit 3 créé. Il apparaît dans les réglages audio du système."
        );
        assert_eq!(cable_removed("Conduit 2"), "Conduit 2 supprimé.");
        assert_eq!(
            channels_changed("Conduit 2", 4),
            "Conduit 2 passe à 4 canaux : réactivation du câble, court silence."
        );
        assert_eq!(
            channels_changed("Conduit 1", 1),
            "Conduit 1 passe à 1 canal : réactivation du câble, court silence."
        );
    }

    /// Les notices du patchbay nomment les nœuds entre guillemets français.
    #[test]
    fn les_notices_du_patchbay_nomment_les_noeuds() {
        assert_eq!(
            lien_refuse_boucle("Lecteur de musique", "Conduit 1"),
            "Lien refusé : il créerait une boucle \
             (« Conduit 1 » alimente déjà « Lecteur de musique »)."
        );
        assert_eq!(
            lien_supprime("Lecteur de musique", "Conduit 1"),
            "Lien supprimé entre « Lecteur de musique » et « Conduit 1 »."
        );
        assert_eq!(
            generateur_cree("Générateur de test", "440 Hz", "−12,0 dB"),
            "« Générateur de test » créé : sinus à 440 Hz, −12,0 dB. \
             Reliez sa sortie à une entrée pour l'entendre."
        );
    }

    /// Les deux notices de coupure disent ce qui se tait, et ce qui revient.
    #[test]
    fn les_notices_de_coupure_disent_les_deux_sens() {
        assert_eq!(
            noeud_coupe("Haut-parleurs", true),
            "« Haut-parleurs » coupé : plus aucun son n'en sort."
        );
        assert_eq!(
            noeud_coupe("Haut-parleurs", false),
            "« Haut-parleurs » rétabli."
        );
        assert_eq!(
            lien_coupe("Lecteur de musique", "Conduit 1", true),
            "Lien coupé entre « Lecteur de musique » et « Conduit 1 »."
        );
        assert_eq!(
            lien_coupe("Lecteur de musique", "Conduit 1", false),
            "Lien rétabli entre « Lecteur de musique » et « Conduit 1 »."
        );
    }

    /// Les deux autres sous-titres juxtaposent leurs valeurs déjà mises en
    /// forme.
    #[test]
    fn the_other_subtitles_juxtapose_their_values() {
        assert_eq!(
            patchbay_subtitle("horloge interne", "48 kHz", "256"),
            "Pilote de graphe : horloge interne · 48 kHz · quantum 256"
        );
        assert_eq!(
            diagnostic_subtitle("3 min", "2 xruns"),
            "Moteur en marche depuis 3 min · 2 xruns"
        );
    }

    #[test]
    fn reconnecting_never_announces_zero_second() {
        assert_eq!(
            reconnecting_in(Duration::from_secs(2)),
            "Reconnexion dans 2 s…"
        );
        assert_eq!(
            reconnecting_in(Duration::from_millis(200)),
            "Reconnexion dans 1 s…"
        );
    }
}
