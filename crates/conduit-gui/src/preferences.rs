//! Préférences d'interface, retenues d'une session à l'autre.
//!
//! Ce fichier ne contient **aucun réglage audio** : le démon est seul maître
//! du graphe (ADR-005). On n'y trouve que ce que la fenêtre a besoin de se
//! rappeler — pour l'instant, l'endroit où l'utilisateur a posé chaque carte
//! du patchbay.
//!
//! Il vit dans `config_dir()/interface.json`, en JSON parce qu'il se relit à
//! la main. Trois principes :
//!
//! - **rien n'est une erreur** : un fichier absent, illisible ou invalide fait
//!   simplement repartir sur la disposition automatique, sans notice ;
//! - **on n'écrit qu'au relâchement** d'une carte, jamais pendant le
//!   déplacement, et jamais depuis `update` — l'écriture passe par une
//!   `Task` ([`ecrire`]) ;
//! - **on ne perd rien** : les clés que le fichier contient et que la session
//!   ne connaît pas — un périphérique débranché — sont conservées à
//!   l'écriture (voir [`Positions::fusion`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::patchbay::Positions;

/// Nom du fichier, dans le répertoire de configuration de Conduit.
pub const FICHIER: &str = "interface.json";

/// Ce que la fenêtre retient d'une session à l'autre.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Positions des cartes du patchbay, par clé stable de nœud.
    pub patchbay: Positions,
}

/// Chemin du fichier de préférences, si le système en désigne un.
pub fn chemin() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "conduit").map(|dirs| dirs.config_dir().join(FICHIER))
}

/// Lit les préférences ; rend celles par défaut si quoi que ce soit manque.
///
/// Bloquante : à n'appeler que depuis une tâche, jamais depuis `update`.
pub fn lire() -> Preferences {
    chemin()
        .and_then(|chemin| std::fs::read_to_string(chemin).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Écrit les positions du patchbay, en conservant les clés déjà sur le disque.
///
/// Le fichier est relu juste avant d'être réécrit : une clé qu'une autre
/// session — ou une autre journée — y a laissée n'est pas effacée par
/// celle-ci. Une écriture qui échoue est sans conséquence : la disposition
/// repartira automatiquement.
///
/// Bloquante : à n'appeler que depuis une tâche, jamais depuis `update`.
pub fn ecrire(patchbay: &Positions) -> std::io::Result<()> {
    let Some(chemin) = chemin() else {
        return Ok(());
    };
    let preferences = Preferences {
        patchbay: lire().patchbay.fusion(patchbay),
    };
    let json = serde_json::to_string_pretty(&preferences)
        .map_err(|erreur| std::io::Error::new(std::io::ErrorKind::InvalidData, erreur))?;
    if let Some(parent) = chemin.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(chemin, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    use iced::Point;

    /// Le fichier vaut ce qu'il dit, et rien de plus.
    #[test]
    fn les_preferences_font_l_aller_retour_json() {
        let mut patchbay = Positions::default();
        patchbay.set("internal:mix", Point::new(24.0, 48.0));
        let preferences = Preferences { patchbay };
        let json = serde_json::to_string(&preferences).expect("sérialisation");
        assert_eq!(json, r#"{"patchbay":{"internal:mix":[24.0,48.0]}}"#);
        assert_eq!(
            serde_json::from_str::<Preferences>(&json).expect("désérialisation"),
            preferences
        );
    }

    /// Un fichier absent, tronqué ou d'une autre version ne fait pas d'erreur :
    /// il rend les préférences par défaut.
    #[test]
    fn un_fichier_invalide_rend_les_preferences_par_defaut() {
        for source in ["", "{", "null", "[]", r#"{"patchbay":"non"}"#] {
            assert_eq!(
                serde_json::from_str::<Preferences>(source).unwrap_or_default(),
                Preferences::default(),
                "source : {source:?}"
            );
        }
        // Une clé que cette version ne connaît pas ne fait pas échouer la
        // lecture, et le patchbay absent retombe sur ses positions vides.
        let futur = r#"{"patchbay":{"internal:a":[1.0,2.0]},"inconnu":42}"#;
        let lues: Preferences = serde_json::from_str(futur).expect("désérialisation");
        assert_eq!(lues.patchbay.get("internal:a"), Some(Point::new(1.0, 2.0)));
        let sans: Preferences = serde_json::from_str("{}").expect("désérialisation");
        assert!(sans.patchbay.is_empty());
    }

    /// Le chemin est celui de la configuration de Conduit.
    #[test]
    fn le_chemin_est_dans_la_configuration_de_conduit() {
        let Some(chemin) = chemin() else {
            return;
        };
        assert!(chemin.ends_with(FICHIER));
        assert!(
            chemin.to_string_lossy().contains("conduit"),
            "chemin : {}",
            chemin.display()
        );
    }
}
