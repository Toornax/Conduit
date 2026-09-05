//! Périphériques, flux et rappel audio.

use core::fmt;
use std::sync::Arc;

use conduit_core::types::SampleRate;

use crate::cable::CableControl;
use crate::event::EventReceiver;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Identifiant stable d'un périphérique, fourni par l'OS (identifiant d'endpoint
/// WASAPI, nom de nœud PipeWire, UID CoreAudio). Ne dépend pas de l'ordre
/// d'énumération.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(transparent))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceId(Arc<str>);

impl DeviceId {
    /// Construit.
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    /// Valeur brute.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({:?})", &*self.0)
    }
}

impl From<&str> for DeviceId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Sens d'un périphérique, vu de l'application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "snake_case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DeviceDirection {
    /// Le périphérique fournit de l'audio (micro, entrée de câble).
    Capture,
    /// Le périphérique consomme de l'audio (haut-parleurs, sortie de câble).
    Render,
}

impl fmt::Display for DeviceDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeviceDirection::Capture => "capture",
            DeviceDirection::Render => "rendu",
        })
    }
}

/// Description d'un périphérique énuméré.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceInfo {
    /// Identifiant stable.
    pub id: DeviceId,
    /// Nom lisible.
    pub name: String,
    /// Sens.
    pub direction: DeviceDirection,
    /// Nombre de canaux natif.
    pub channels: usize,
    /// Fréquence native (celle utilisée si le format demandé n'est pas disponible).
    pub sample_rate: SampleRate,
    /// Fréquences acceptées (vide = seulement `sample_rate`).
    pub sample_rates: Vec<SampleRate>,
    /// Taille de bloc par défaut (trames par rappel).
    pub default_block: usize,
    /// Vrai si c'est le périphérique par défaut de l'OS pour son sens.
    pub is_default: bool,
    /// Identifiant du câble Conduit si ce périphérique en est un côté.
    pub cable: Option<crate::cable::CableId>,
}

impl DeviceInfo {
    /// Vrai si la fréquence est acceptée.
    pub fn supports_rate(&self, rate: SampleRate) -> bool {
        rate == self.sample_rate || self.sample_rates.contains(&rate)
    }
}

/// Format demandé ou obtenu pour un flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StreamFormat {
    /// Fréquence.
    pub sample_rate: SampleRate,
    /// Canaux (trames entrelacées).
    pub channels: usize,
    /// Trames par rappel (le backend peut arrondir).
    pub block_frames: usize,
}

/// Position d'horloge fournie à chaque rappel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClockInfo {
    /// Trames écoulées depuis l'ouverture, dans l'horloge du périphérique, au début
    /// de ce rappel.
    pub position: u64,
    /// Horodatage monotone du rappel, en nanosecondes (base arbitraire).
    pub timestamp_ns: u64,
    /// Trames de ce rappel.
    pub frames: usize,
}

/// Tampons d'un rappel : entrée (capture) et/ou sortie (rendu), entrelacés.
#[derive(Debug)]
pub struct StreamIo<'a> {
    /// Trames capturées, si le flux est en capture.
    pub input: Option<&'a [f32]>,
    /// Trames à rendre, si le flux est en rendu. Le rappel doit tout écrire.
    pub output: Option<&'a mut [f32]>,
}

impl StreamIo<'_> {
    /// Met la sortie au silence, s'il y en a une.
    pub fn silence_output(&mut self) {
        if let Some(o) = self.output.as_deref_mut() {
            o.fill(0.0);
        }
    }
}

/// Rappel audio temps réel.
pub type AudioCallback = Box<dyn FnMut(&mut StreamIo<'_>, &ClockInfo) + Send + 'static>;

/// Erreurs d'un backend.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// Le périphérique n'existe pas (ou plus).
    #[error("périphérique introuvable : {0}")]
    NotFound(DeviceId),
    /// Le périphérique est déjà ouvert.
    #[error("périphérique déjà ouvert : {0}")]
    Busy(DeviceId),
    /// Format refusé.
    #[error("format non supporté par {device} : {reason}")]
    UnsupportedFormat {
        /// Périphérique concerné.
        device: DeviceId,
        /// Explication.
        reason: String,
    },
    /// Le périphérique a disparu pendant l'utilisation.
    #[error("périphérique débranché : {0}")]
    Disconnected(DeviceId),
    /// Erreur de l'API de plateforme.
    #[error("erreur du backend : {0}")]
    Platform(String),
}

/// Un flux ouvert. Le fermer = le détruire.
pub trait DeviceHandle: Send + fmt::Debug {
    /// Description du périphérique.
    fn info(&self) -> &DeviceInfo;

    /// Format effectivement obtenu.
    fn format(&self) -> StreamFormat;

    /// Démarre les rappels.
    fn start(&mut self) -> Result<(), BackendError>;

    /// Arrête les rappels. Aucun rappel n'est en cours au retour.
    fn stop(&mut self) -> Result<(), BackendError>;

    /// Vrai si les rappels sont actifs.
    fn is_running(&self) -> bool;

    /// Position d'horloge courante (hors rappel).
    fn clock(&self) -> ClockInfo;
}

/// Un backend de plateforme.
pub trait Backend: Send + fmt::Debug {
    /// Nom (`"null"`, `"wasapi"`, …).
    fn name(&self) -> &'static str;

    /// Énumère les périphériques présents.
    fn devices(&self) -> Result<Vec<DeviceInfo>, BackendError>;

    /// Périphérique par défaut pour un sens.
    fn default_device(&self, direction: DeviceDirection) -> Option<DeviceId>;

    /// Ouvre un flux. Le rappel est appelé après [`DeviceHandle::start`].
    fn open(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<Box<dyn DeviceHandle>, BackendError>;

    /// S'abonne aux événements de périphériques.
    fn subscribe(&mut self) -> EventReceiver;

    /// Contrôle des câbles, si la plateforme le permet.
    fn cable_control(&mut self) -> Option<&mut dyn CableControl>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_display_and_eq() {
        let a = DeviceId::from("x");
        let b = DeviceId::new(String::from("x"));
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "x");
        assert_eq!(format!("{a:?}"), "DeviceId(\"x\")");
        assert_eq!(a.as_str(), "x");
        assert_eq!(DeviceDirection::Render.to_string(), "rendu");
    }

    #[test]
    fn stream_io_silence() {
        let mut out = [1.0f32; 4];
        let mut io = StreamIo {
            input: None,
            output: Some(&mut out),
        };
        io.silence_output();
        assert_eq!(out, [0.0; 4]);
        let mut none = StreamIo {
            input: Some(&[]),
            output: None,
        };
        none.silence_output();
    }

    #[test]
    fn errors_display() {
        let e = BackendError::UnsupportedFormat {
            device: "d".into(),
            reason: "96 kHz".into(),
        };
        assert!(e.to_string().contains("96 kHz"));
        assert!(BackendError::NotFound("z".into()).to_string().contains('z'));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn device_info_serde() {
        let d = DeviceInfo {
            id: "abc".into(),
            name: "Casque".into(),
            direction: DeviceDirection::Render,
            channels: 2,
            sample_rate: SampleRate::HZ_48000,
            sample_rates: vec![SampleRate::HZ_44100],
            default_block: 256,
            is_default: true,
            cable: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"id\":\"abc\""));
        assert!(json.contains("\"direction\":\"render\""));
        let back: DeviceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
        assert!(d.supports_rate(SampleRate::HZ_44100));
        assert!(!d.supports_rate(SampleRate::HZ_96000));
    }
}
