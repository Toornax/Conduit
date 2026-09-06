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
    DaemonRunning => "daemon.running", "conduitd en marche";
    DaemonStarting => "daemon.starting", "conduitd démarre…";
    DaemonStopped => "daemon.stopped", "conduitd arrêté";
    DriverWindows => "driver.windows", "Pilote noyau conduit-kmd";
    DriverMacos => "driver.macos", "Plugin HAL conduit-hal";
    DriverLinux => "driver.linux", "Nœuds PipeWire (sans pilote)";
    DriverNone => "driver.none", "aucun";
    DriverInternal => "driver.internal", "horloge interne";

    TabCables => "tab.cables", "Câbles";
    TabPatchbay => "tab.patchbay", "Patchbay";
    TabDiagnostic => "tab.diagnostic", "Diagnostic";
    DiagnosticSoon => "tab.diagnostic.soon",
        "La page de diagnostic arrive plus tard : xruns, latence, pilote et export de rapport.";

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
    Silence => "unit.silence", "−∞";
    Xrun => "diag.xrun", "xrun";
    Xruns => "diag.xruns", "xruns";
    NoXrun => "diag.xrun.none", "aucun xrun";
    Inconnu => "num.unknown", "—";
    Separateur => "ui.separator", "·";

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
