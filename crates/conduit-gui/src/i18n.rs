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
    PatchbaySoon => "tab.patchbay.soon",
        "Le patchbay arrive au prochain jalon : nœuds, ports et liens par glisser-déposer.";
    DiagnosticSoon => "tab.diagnostic.soon",
        "La page de diagnostic arrive plus tard : xruns, latence, pilote et export de rapport.";

    CablesTitle => "cables.title", "Câbles virtuels";
    CablesAdd => "cables.add", "Ajouter un câble";
    CablesNone => "cables.none", "Aucun câble pour l'instant";
    CableVisible => "cables.visible.one", "visible par toutes les applications";
    CablesVisible => "cables.visible.many", "visibles par toutes les applications";
    CablesEmpty => "cables.empty",
        "Aucun câble. Créez-en un ci-dessous : il apparaîtra aussitôt dans les réglages audio du système.";
    CableName => "cables.name", "Nom du câble";
    CableNamePlaceholder => "cables.name.placeholder", "Nom du câble (optionnel)";
    CableActive => "cables.active", "actif";
    CableInactive => "cables.inactive", "inactif";
    CableRemoveConfirm => "cables.remove.confirm", "Supprimer ce câble ?";

    GraphDriver => "patchbay.driver", "Pilote de graphe";
    Quantum => "patchbay.quantum", "quantum";
    EngineUptime => "diag.uptime", "Moteur en marche depuis";

    ThemeLight => "theme.light", "Sericæ clair";
    ThemeDark => "theme.dark", "Sericæ sombre";

    UnitMs => "unit.ms", "ms";
    UnitS => "unit.s", "s";
    UnitMin => "unit.min", "min";
    UnitH => "unit.h", "h";
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

    Add => "action.add", "Ajouter";
    Remove => "action.remove", "Supprimer";
    Rename => "action.rename", "Renommer";
    Validate => "action.validate", "Valider";
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
        1 => juxtapose("1 câble", t(Text::CableVisible)),
        n => juxtapose(&format!("{n} câbles"), t(Text::CablesVisible)),
    }
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
