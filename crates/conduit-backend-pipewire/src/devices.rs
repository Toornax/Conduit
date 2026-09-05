//! Table des périphériques vus dans le registre PipeWire.
//!
//! Le fil de boucle écrit ; les appels de [`Backend`](conduit_backend::Backend)
//! lisent. Un `Mutex` std suffit : rien de tout cela n'est temps réel.

use std::collections::{BTreeMap, HashMap};

use conduit_backend::{CableId, DeviceDirection, DeviceId, DeviceInfo};
use conduit_core::types::SampleRate;
use pipewire::spa::utils::dict::DictRef;

/// Fréquence annoncée pour tout nœud : celle de l'horloge du graphe PipeWire.
///
/// PipeWire rééchantillonne pour les flux qui demandent autre chose ; la valeur ne
/// dépend donc pas du périphérique.
pub const DEFAULT_SAMPLE_RATE: SampleRate = SampleRate::HZ_48000;

/// Taille de bloc annoncée : le quantum par défaut du graphe PipeWire.
pub const DEFAULT_BLOCK_FRAMES: usize = 256;

/// Canaux supposés quand le nœud ne les publie pas dans le registre.
const FALLBACK_CHANNELS: usize = 2;

/// État partagé entre le fil de boucle et les appelants du backend.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Périphériques présents, indexés par identifiant stable.
    devices: BTreeMap<DeviceId, DeviceInfo>,
    /// Identifiant global PipeWire → identifiant Conduit (pour `global_remove`).
    by_global: HashMap<u32, DeviceId>,
    /// Périphérique par défaut, indexé par [`dir_index`].
    defaults: [Option<DeviceId>; 2],
}

pub(crate) fn dir_index(direction: DeviceDirection) -> usize {
    match direction {
        DeviceDirection::Capture => 0,
        DeviceDirection::Render => 1,
    }
}

impl Shared {
    /// Insère un périphérique. Retourne sa description si c'est une nouveauté.
    pub(crate) fn insert(&mut self, global: u32, mut info: DeviceInfo) -> Option<DeviceInfo> {
        if self.devices.contains_key(&info.id) {
            return None;
        }
        info.is_default = self.defaults[dir_index(info.direction)].as_ref() == Some(&info.id);
        self.by_global.insert(global, info.id.clone());
        self.devices.insert(info.id.clone(), info.clone());
        Some(info)
    }

    /// Retire le périphérique correspondant à un global disparu.
    pub(crate) fn remove_global(&mut self, global: u32) -> Option<DeviceInfo> {
        let id = self.by_global.remove(&global)?;
        self.devices.remove(&id)
    }

    /// Change le périphérique par défaut d'un sens. Retourne `true` si ça a changé.
    pub(crate) fn set_default(&mut self, direction: DeviceDirection, id: Option<DeviceId>) -> bool {
        let slot = &mut self.defaults[dir_index(direction)];
        if *slot == id {
            return false;
        }
        if let Some(old) = slot.take() {
            if let Some(info) = self.devices.get_mut(&old) {
                info.is_default = false;
            }
        }
        if let Some(new) = &id {
            if let Some(info) = self.devices.get_mut(new) {
                info.is_default = true;
            }
        }
        *slot = id;
        true
    }

    pub(crate) fn default_device(&self, direction: DeviceDirection) -> Option<DeviceId> {
        self.defaults[dir_index(direction)].clone()
    }

    pub(crate) fn list(&self) -> Vec<DeviceInfo> {
        self.devices.values().cloned().collect()
    }

    pub(crate) fn get(&self, id: &DeviceId) -> Option<&DeviceInfo> {
        self.devices.get(id)
    }
}

/// Traduit les propriétés d'un global `Node` en [`DeviceInfo`].
///
/// Retourne `None` si le nœud n'est ni un puits ni une source audio. Le registre ne
/// publie qu'un sous-ensemble des propriétés du nœud (`node.name`, `media.class`,
/// `object.serial`, …) : les valeurs absentes prennent des replis explicites.
pub(crate) fn device_from_props(props: &DictRef) -> Option<DeviceInfo> {
    let direction = match props.get("media.class")? {
        "Audio/Sink" => DeviceDirection::Render,
        "Audio/Source" => DeviceDirection::Capture,
        _ => return None,
    };
    // `node.name` est l'identifiant stable de PipeWire ; `object.serial` ne survit
    // pas à un redémarrage du démon mais vaut mieux que rien.
    let id = props
        .get("node.name")
        .or_else(|| props.get("object.serial"))?;
    let id = DeviceId::new(id);
    let name = props
        .get("node.description")
        .or_else(|| props.get("node.nick"))
        .or_else(|| props.get("node.name"))
        .unwrap_or(id.as_str())
        .to_string();
    Some(DeviceInfo {
        id,
        name,
        direction,
        channels: channels_from_props(props),
        sample_rate: DEFAULT_SAMPLE_RATE,
        sample_rates: Vec::new(),
        default_block: DEFAULT_BLOCK_FRAMES,
        is_default: false,
        cable: props
            .get("conduit.cable")
            .and_then(|v| v.parse().ok())
            .map(CableId),
    })
}

fn channels_from_props(props: &DictRef) -> usize {
    if let Some(n) = props.get("audio.channels").and_then(|v| v.parse().ok()) {
        if n > 0 {
            return n;
        }
    }
    // `audio.position` (« FL,FR ») est souvent la seule indication publiée.
    if let Some(position) = props.get("audio.position") {
        let n = position.split(',').filter(|c| !c.trim().is_empty()).count();
        if n > 0 {
            return n;
        }
    }
    FALLBACK_CHANNELS
}

/// Extrait `name` de la valeur JSON d'une métadonnée `default.audio.*`
/// (`{ "name": "alsa_output.pci-0000_00_1f.3.analog-stereo" }`).
///
/// Analyseur minimal volontairement : dépendre d'un crate JSON pour une seule clé
/// serait disproportionné.
pub(crate) fn metadata_name(value: &str) -> Option<String> {
    let rest = value.split_once("\"name\"")?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, direction: DeviceDirection) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(id),
            name: id.to_string(),
            direction,
            channels: 2,
            sample_rate: DEFAULT_SAMPLE_RATE,
            sample_rates: Vec::new(),
            default_block: DEFAULT_BLOCK_FRAMES,
            is_default: false,
            cable: None,
        }
    }

    #[test]
    fn insert_is_idempotent_and_remove_uses_global_id() {
        let mut shared = Shared::default();
        assert!(shared
            .insert(7, info("a", DeviceDirection::Render))
            .is_some());
        assert!(shared
            .insert(7, info("a", DeviceDirection::Render))
            .is_none());
        assert_eq!(shared.list().len(), 1);
        assert_eq!(shared.remove_global(7).map(|i| i.id), Some("a".into()));
        assert!(shared.remove_global(7).is_none());
        assert!(shared.list().is_empty());
    }

    #[test]
    fn default_flag_follows_metadata_in_both_orders() {
        let mut shared = Shared::default();
        // Défaut connu avant le périphérique.
        assert!(shared.set_default(DeviceDirection::Render, Some("a".into())));
        assert!(!shared.set_default(DeviceDirection::Render, Some("a".into())));
        shared.insert(1, info("a", DeviceDirection::Render));
        assert!(shared.get(&"a".into()).unwrap().is_default);
        // Puis bascule vers un autre.
        shared.insert(2, info("b", DeviceDirection::Render));
        assert!(shared.set_default(DeviceDirection::Render, Some("b".into())));
        assert!(!shared.get(&"a".into()).unwrap().is_default);
        assert!(shared.get(&"b".into()).unwrap().is_default);
        assert_eq!(
            shared.default_device(DeviceDirection::Render),
            Some("b".into())
        );
        assert_eq!(shared.default_device(DeviceDirection::Capture), None);
    }

    #[test]
    fn metadata_name_parses_the_json_pipewire_sends() {
        assert_eq!(
            metadata_name("{ \"name\": \"Null-Sink\" }").as_deref(),
            Some("Null-Sink")
        );
        assert_eq!(metadata_name("{\"name\":\"a b\"}").as_deref(), Some("a b"));
        assert_eq!(metadata_name("null"), None);
        assert_eq!(metadata_name("{\"name\"}"), None);
        assert_eq!(metadata_name("{\"name\": 3}"), None);
    }
}
