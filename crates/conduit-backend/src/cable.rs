//! Contrôle des câbles virtuels de la plateforme.

use core::fmt;

use conduit_core::types::ChannelCount;

use crate::device::DeviceId;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Identifiant d'un câble (numéro stable, 1 = « Conduit 1 »).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableId(pub u32);

impl fmt::Display for CableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Conduit {}", self.0)
    }
}

/// Demande de création ou de modification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableSpec {
    /// Nom OS souhaité (`None` = `Conduit N`).
    pub name: Option<String>,
    /// Canaux.
    pub channels: ChannelCount,
}

impl Default for CableSpec {
    fn default() -> Self {
        Self {
            name: None,
            channels: ChannelCount::STEREO,
        }
    }
}

/// Description d'un câble existant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CableInfo {
    /// Identifiant.
    pub id: CableId,
    /// Nom OS.
    pub name: String,
    /// Canaux.
    pub channels: ChannelCount,
    /// Vrai si actif (visible des applications).
    pub active: bool,
    /// Périphérique de rendu (les applications y jouent).
    pub render: DeviceId,
    /// Périphérique de capture (les applications y lisent).
    pub capture: DeviceId,
}

/// Erreurs de contrôle des câbles. Les messages disent quoi faire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CableError {
    /// Nombre maximal de câbles atteint.
    #[error("limite de {max} câbles atteinte : supprimez un câble avant d'en créer un autre")]
    LimitReached {
        /// Limite de la plateforme.
        max: usize,
    },
    /// Câble inconnu.
    #[error("câble inconnu : {0}")]
    NotFound(CableId),
    /// Droits insuffisants.
    #[error("droits insuffisants : {0}")]
    PermissionDenied(String),
    /// Opération non supportée par cette plateforme.
    #[error("non supporté sur cette plateforme : {0}")]
    Unsupported(String),
    /// Nom refusé.
    #[error("nom invalide : {0}")]
    InvalidName(String),
    /// Le service ou le démon dont dépend le contrôle des câbles ne répond pas.
    ///
    /// Distinct de [`Self::Driver`] : le pilote n'a rien refusé, on n'a pas pu lui
    /// parler. Sous Windows c'est le service d'assistance `ConduitHelper` qui manque
    /// (M1b-20) ; le message dit alors quoi installer.
    #[error("service indisponible : {0}")]
    Unavailable(String),
    /// Erreur du pilote ou de l'OS.
    #[error("erreur du pilote : {0}")]
    Driver(String),
}

/// Longueur maximale d'un nom de câble (caractères).
pub const MAX_CABLE_NAME_LEN: usize = 64;

/// Valide un nom de câble : non vide, ≤ [`MAX_CABLE_NAME_LEN`] caractères,
/// imprimable, sans caractère de contrôle ni `/ \ : * ? " < > |`.
pub fn validate_cable_name(name: &str) -> Result<(), CableError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CableError::InvalidName("le nom est vide".into()));
    }
    if name.chars().count() > MAX_CABLE_NAME_LEN {
        return Err(CableError::InvalidName(format!(
            "plus de {MAX_CABLE_NAME_LEN} caractères"
        )));
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_control() || "/\\:*?\"<>|".contains(*c))
    {
        return Err(CableError::InvalidName(format!("caractère interdit {c:?}")));
    }
    Ok(())
}

/// Contrôle des câbles d'une plateforme.
pub trait CableControl: fmt::Debug {
    /// Nombre maximal de câbles.
    fn max_cables(&self) -> usize;

    /// Câbles existants, actifs ou non.
    fn list(&self) -> Result<Vec<CableInfo>, CableError>;

    /// Crée (ou active) un câble.
    fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError>;

    /// Supprime (ou désactive) un câble.
    fn remove(&mut self, id: CableId) -> Result<(), CableError>;

    /// Change le nombre de canaux (peut réactiver le câble : court silence).
    fn set_channels(
        &mut self,
        id: CableId,
        channels: ChannelCount,
    ) -> Result<CableInfo, CableError>;

    /// Renomme côté OS.
    fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError>;

    /// Câble par identifiant.
    fn get(&self, id: CableId) -> Result<CableInfo, CableError> {
        self.list()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or(CableError::NotFound(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(validate_cable_name("Musique").is_ok());
        assert!(validate_cable_name("Conduit 1").is_ok());
        assert!(matches!(
            validate_cable_name("  "),
            Err(CableError::InvalidName(_))
        ));
        assert!(matches!(
            validate_cable_name("a/b"),
            Err(CableError::InvalidName(_))
        ));
        assert!(matches!(
            validate_cable_name("a\nb"),
            Err(CableError::InvalidName(_))
        ));
        let long = "x".repeat(MAX_CABLE_NAME_LEN + 1);
        assert!(matches!(
            validate_cable_name(&long),
            Err(CableError::InvalidName(_))
        ));
        assert!(validate_cable_name(&"é".repeat(MAX_CABLE_NAME_LEN)).is_ok());
    }

    #[test]
    fn display_and_errors() {
        assert_eq!(CableId(3).to_string(), "Conduit 3");
        assert!(CableError::LimitReached { max: 16 }
            .to_string()
            .contains("16"));
        assert!(CableError::NotFound(CableId(9))
            .to_string()
            .contains("Conduit 9"));
        // Un service absent ne se confond pas avec un refus du pilote : ce sont deux
        // conduites différentes pour l'utilisateur (installer, ou rapporter un bogue).
        let indisponible = CableError::Unavailable("le service n'est pas démarré".into());
        assert!(indisponible.to_string().starts_with("service indisponible"));
        assert_ne!(
            indisponible,
            CableError::Driver("le service n'est pas démarré".into())
        );
        assert_eq!(CableSpec::default().channels, ChannelCount::STEREO);
    }
}
