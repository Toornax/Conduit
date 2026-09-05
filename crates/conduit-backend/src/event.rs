//! Événements de périphériques.

use std::sync::mpsc;

use crate::cable::CableInfo;
use crate::device::{DeviceDirection, DeviceId, DeviceInfo};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Changement côté OS.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(tag = "type", rename_all = "snake_case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DeviceEvent {
    /// Un périphérique est apparu.
    Added(DeviceInfo),
    /// Un périphérique a disparu.
    Removed {
        /// Identifiant.
        id: DeviceId,
    },
    /// Le périphérique par défaut d'un sens a changé.
    DefaultChanged {
        /// Sens concerné.
        direction: DeviceDirection,
        /// Nouveau périphérique par défaut (`None` si aucun).
        id: Option<DeviceId>,
    },
    /// Un câble a été créé, modifié ou supprimé (`None` = supprimé).
    CableChanged {
        /// Identifiant.
        id: crate::cable::CableId,
        /// Nouvel état.
        info: Option<CableInfo>,
    },
}

/// Récepteur d'événements d'un backend.
pub type EventReceiver = mpsc::Receiver<DeviceEvent>;

/// Émetteur partagé par les backends pour diffuser à tous les abonnés.
#[derive(Debug, Default)]
pub struct EventBroadcaster {
    senders: Vec<mpsc::Sender<DeviceEvent>>,
}

impl EventBroadcaster {
    /// Crée un abonnement.
    pub fn subscribe(&mut self) -> EventReceiver {
        let (tx, rx) = mpsc::channel();
        self.senders.push(tx);
        rx
    }

    /// Diffuse à tous les abonnés vivants ; les abonnés disparus sont oubliés.
    pub fn send(&mut self, event: DeviceEvent) {
        self.senders.retain(|s| s.send(event.clone()).is_ok());
    }

    /// Nombre d'abonnés.
    pub fn subscribers(&self) -> usize {
        self.senders.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_reaches_all_and_drops_dead() {
        let mut b = EventBroadcaster::default();
        let a = b.subscribe();
        let c = b.subscribe();
        b.send(DeviceEvent::Removed { id: "x".into() });
        assert_eq!(
            a.try_recv().unwrap(),
            DeviceEvent::Removed { id: "x".into() }
        );
        assert_eq!(
            c.try_recv().unwrap(),
            DeviceEvent::Removed { id: "x".into() }
        );
        drop(a);
        b.send(DeviceEvent::DefaultChanged {
            direction: DeviceDirection::Render,
            id: None,
        });
        assert_eq!(b.subscribers(), 1);
        assert!(c.try_recv().is_ok());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn event_serde_tag() {
        let e = DeviceEvent::Removed { id: "abc".into() };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"type":"removed","id":"abc"}"#);
    }
}
