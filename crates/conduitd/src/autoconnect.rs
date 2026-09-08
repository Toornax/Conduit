//! Règles d'auto-connexion déclaratives (F-32).
//!
//! Une règle relie, dès qu'il apparaît, un nœud correspondant à `match` au nœud
//! `target`. Les motifs acceptent `*` (n'importe quelle suite de caractères) et
//! sont comparés au nom d'affichage et à la clé du nœud.

use conduit_backend::DeviceDirection;
use conduit_protocol::api::{NodeDescriptor, NodeKey};
use serde::{Deserialize, Serialize};

/// Sélection du nœud déclencheur.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeMatch {
    /// Numéro de câble Conduit.
    pub cable: Option<u32>,
    /// `"capture"` ou `"render"` (côté du câble ou sens du périphérique).
    pub direction: Option<String>,
    /// Motif sur le nom ou la clé (`"Micro*"`, `"null:*"`).
    pub node: Option<String>,
}

/// Cible du lien.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTarget {
    /// Motif du nœud cible.
    pub node: String,
    /// Ports de la cible, dans l'ordre d'appariement.
    pub ports: Vec<String>,
    /// Ports du nœud déclencheur (défaut : ses ports dans l'ordre).
    #[serde(default)]
    pub source_ports: Option<Vec<String>>,
}

/// Une règle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoConnectRule {
    /// Déclencheur.
    #[serde(rename = "match")]
    pub matcher: NodeMatch,
    /// Cible.
    pub target: LinkTarget,
}

/// Paire de ports à relier : (nœud source, port source, nœud destination, port destination).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedLink {
    /// Nœud possédant la sortie.
    pub src: conduit_core::graph::NodeId,
    /// Nom du port de sortie.
    pub src_port: String,
    /// Nœud possédant l'entrée.
    pub dst: conduit_core::graph::NodeId,
    /// Nom du port d'entrée.
    pub dst_port: String,
}

impl AutoConnectRule {
    /// Vérifie la cohérence de la règle.
    pub fn validate(&self) -> Result<(), String> {
        if self.matcher.cable.is_none() && self.matcher.node.is_none() {
            return Err("match doit préciser cable ou node".into());
        }
        if let Some(d) = &self.matcher.direction {
            if d != "capture" && d != "render" {
                return Err(format!("direction {d:?} inconnue : capture ou render"));
            }
        }
        if self.target.node.trim().is_empty() {
            return Err("target.node est vide".into());
        }
        if self.target.ports.is_empty() {
            return Err("target.ports est vide".into());
        }
        Ok(())
    }

    /// Vrai si le nœud correspond à `match`.
    pub fn matches(&self, node: &NodeDescriptor) -> bool {
        let m = &self.matcher;
        if let Some(cable) = m.cable {
            match node.device.as_ref().and_then(|d| d.cable) {
                Some(id) if id.0 == cable => {}
                _ => return false,
            }
        }
        if let Some(dir) = &m.direction {
            let want = if dir == "capture" {
                DeviceDirection::Capture
            } else {
                DeviceDirection::Render
            };
            let actual =
                node.device
                    .as_ref()
                    .map(|d| d.direction)
                    .unwrap_or(if node.outputs.is_empty() {
                        DeviceDirection::Render
                    } else {
                        DeviceDirection::Capture
                    });
            if actual != want {
                return false;
            }
        }
        if let Some(pat) = &m.node {
            if !node_matches(pat, node) {
                return false;
            }
        }
        true
    }

    /// Calcule les liens à créer entre un nœud déclencheur et une cible trouvée.
    /// Le sens est déduit : le nœud qui a des sorties alimente celui qui a des entrées.
    pub fn plan(&self, trigger: &NodeDescriptor, target: &NodeDescriptor) -> Vec<PlannedLink> {
        let trigger_is_source = !trigger.outputs.is_empty() && !target.inputs.is_empty();
        let (src, dst) = if trigger_is_source {
            (trigger, target)
        } else {
            (target, trigger)
        };
        // Ports côté déclencheur (source_ports) et côté cible (ports).
        let trigger_ports: Vec<String> = match &self.target.source_ports {
            Some(p) => p.clone(),
            None => {
                let list = if trigger_is_source {
                    &trigger.outputs
                } else {
                    &trigger.inputs
                };
                list.iter().map(|p| p.name.clone()).collect()
            }
        };
        let target_ports = &self.target.ports;
        trigger_ports
            .iter()
            .zip(target_ports.iter())
            .filter_map(|(tp, gp)| {
                let (src_port, dst_port) = if trigger_is_source {
                    (tp, gp)
                } else {
                    (gp, tp)
                };
                let ok = src.outputs.iter().any(|p| &p.name == src_port)
                    && dst.inputs.iter().any(|p| &p.name == dst_port);
                ok.then(|| PlannedLink {
                    src: src.id,
                    src_port: src_port.clone(),
                    dst: dst.id,
                    dst_port: dst_port.clone(),
                })
            })
            .collect()
    }

    /// Vrai si `node` peut être la cible de cette règle.
    pub fn target_matches(&self, node: &NodeDescriptor) -> bool {
        node_matches(&self.target.node, node)
    }
}

/// Compare un motif au nom d'affichage, à la clé et, pour un périphérique, à son
/// identifiant OS et son nom.
pub fn node_matches(pattern: &str, node: &NodeDescriptor) -> bool {
    let mut candidates = vec![node.label.as_str(), &node.type_name];
    let key = node.key.to_string();
    candidates.push(&key);
    let internal_name = match &node.key {
        NodeKey::Internal { name } => Some(name.as_str()),
        NodeKey::Device { .. } => None,
    };
    if let Some(n) = internal_name {
        candidates.push(n);
    }
    let (dev_id, dev_name) = match &node.device {
        Some(d) => (Some(d.id.to_string()), Some(d.name.as_str())),
        None => (None, None),
    };
    if let Some(id) = &dev_id {
        candidates.push(id);
    }
    if let Some(n) = dev_name {
        candidates.push(n);
    }
    candidates.iter().any(|c| glob_match(pattern, c))
}

/// Correspondance avec `*` uniquement, insensible à la casse.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn rec(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| rec(rest, &t[i..])),
            Some((c, rest)) => t
                .split_first()
                .is_some_and(|(tc, trest)| tc == c && rec(rest, trest)),
        }
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    rec(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_backend::{CableId, DeviceInfo};
    use conduit_core::graph::NodeId;
    use conduit_core::node::PortSpec;
    use conduit_core::types::{Db, SampleRate};
    use conduit_protocol::api::NodeState;

    fn device(
        id: &str,
        name: &str,
        dir: DeviceDirection,
        ch: usize,
        cable: Option<u32>,
    ) -> NodeDescriptor {
        let (inputs, outputs) = match dir {
            DeviceDirection::Capture => (vec![], PortSpec::layout(ch)),
            DeviceDirection::Render => (PortSpec::layout(ch), vec![]),
        };
        NodeDescriptor {
            id: NodeId::new(id.len() as u32, 0),
            key: NodeKey::device("null", id.into()),
            label: name.into(),
            type_name: "device".into(),
            inputs,
            outputs,
            state: NodeState::Active,
            gain_db: Db::UNITY,
            muted: false,
            device: Some(DeviceInfo {
                id: id.into(),
                name: name.into(),
                direction: dir,
                channels: ch,
                sample_rate: SampleRate::HZ_48000,
                sample_rates: vec![],
                default_block: 256,
                is_default: false,
                cable: cable.map(CableId),
            }),
        }
    }

    #[test]
    fn glob() {
        assert!(glob_match("Micro*", "Micro USB"));
        assert!(glob_match("*usb*", "Micro USB"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("Micro", "Micro USB"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn spec_rule_links_cable_capture_to_speakers() {
        let rule: AutoConnectRule = toml::from_str(
            r#"match = { cable = 1, direction = "capture" }
target = { node = "Haut-parleurs", ports = ["FL", "FR"] }"#,
        )
        .unwrap();
        rule.validate().unwrap();
        let cap = device(
            "null:cable1:capture",
            "Conduit 1",
            DeviceDirection::Capture,
            2,
            Some(1),
        );
        let ren = device(
            "null:cable1:render",
            "Conduit 1",
            DeviceDirection::Render,
            2,
            Some(1),
        );
        let spk = device("null:hp", "Haut-parleurs", DeviceDirection::Render, 2, None);
        assert!(rule.matches(&cap));
        assert!(!rule.matches(&ren), "mauvais côté");
        assert!(!rule.matches(&spk));
        assert!(rule.target_matches(&spk));
        assert!(!rule.target_matches(&cap));
        let plan = rule.plan(&cap, &spk);
        assert_eq!(plan.len(), 2);
        assert_eq!(
            plan[0],
            PlannedLink {
                src: cap.id,
                src_port: "FL".into(),
                dst: spk.id,
                dst_port: "FL".into()
            }
        );
        assert_eq!(plan[1].dst_port, "FR");
    }

    /// **Une règle `cable = N` suit le câble, pas son nom.**
    ///
    /// Après `conduitctl cable rename 1 Musique`, les endpoints du câble 1 s'appellent
    /// « Musique » : leur description a changé, mais le dorsal les rattache par la
    /// **marque** que le service a écrite, et `DeviceInfo::cable` vaut toujours 1. La
    /// règle de connexion automatique de la configuration continue donc de mordre —
    /// sans quoi renommer un câble aurait silencieusement débranché tout le montage.
    #[test]
    fn une_regle_sur_le_cable_survit_au_renommage() {
        let rule: AutoConnectRule = toml::from_str(
            r#"match = { cable = 1, direction = "capture" }
target = { node = "Haut-parleurs", ports = ["FL", "FR"] }"#,
        )
        .unwrap();
        rule.validate().unwrap();
        // Le nom affiché ne contient plus « Conduit » : seul `cable` l'identifie.
        let renomme = device(
            "{0.0.1.00000000}.{c615a124}",
            "Musique (Conduit — câbles audio virtuels)",
            DeviceDirection::Capture,
            2,
            Some(1),
        );
        assert!(rule.matches(&renomme), "{}", renomme.label);
        // Et un autre câble, renommé lui aussi, ne se fait pas prendre pour le premier.
        let autre = device(
            "{0.0.1.00000000}.{d726b235}",
            "Musique (Conduit — câbles audio virtuels)",
            DeviceDirection::Capture,
            2,
            Some(2),
        );
        assert!(!rule.matches(&autre));
    }

    #[test]
    fn reverse_direction_and_port_filtering() {
        // Le déclencheur est un périphérique de rendu : la cible (capture) l'alimente.
        let rule = AutoConnectRule {
            matcher: NodeMatch {
                cable: None,
                direction: Some("render".into()),
                node: Some("Casque*".into()),
            },
            target: LinkTarget {
                node: "null:micro".into(),
                ports: vec!["MONO".into(), "MONO".into()],
                source_ports: None,
            },
        };
        rule.validate().unwrap();
        let casque = device(
            "null:casque",
            "Casque USB",
            DeviceDirection::Render,
            2,
            None,
        );
        let mic = device("null:micro", "Micro", DeviceDirection::Capture, 1, None);
        assert!(rule.matches(&casque));
        let plan = rule.plan(&casque, &mic);
        assert_eq!(plan.len(), 2, "mono dupliqué sur FL et FR");
        assert!(plan.iter().all(|l| l.src == mic.id && l.src_port == "MONO"));
        assert_eq!(plan[1].dst_port, "FR");
        // Port inexistant ignoré.
        let rule2 = AutoConnectRule {
            matcher: NodeMatch {
                cable: None,
                direction: None,
                node: Some("casque usb".into()),
            },
            target: LinkTarget {
                node: "Micro".into(),
                ports: vec!["MONO".into()],
                source_ports: Some(vec!["FC".into()]),
            },
        };
        assert!(rule2.plan(&casque, &mic).is_empty());
    }

    #[test]
    fn validation_errors() {
        let bad = AutoConnectRule {
            matcher: NodeMatch::default(),
            target: LinkTarget {
                node: "x".into(),
                ports: vec!["a".into()],
                source_ports: None,
            },
        };
        assert!(bad.validate().unwrap_err().contains("cable ou node"));
        let bad = AutoConnectRule {
            matcher: NodeMatch {
                direction: Some("sideways".into()),
                node: Some("x".into()),
                cable: None,
            },
            target: LinkTarget {
                node: "x".into(),
                ports: vec!["a".into()],
                source_ports: None,
            },
        };
        assert!(bad.validate().unwrap_err().contains("direction"));
        let bad = AutoConnectRule {
            matcher: NodeMatch {
                node: Some("x".into()),
                ..Default::default()
            },
            target: LinkTarget {
                node: " ".into(),
                ports: vec![],
                source_ports: None,
            },
        };
        assert!(bad.validate().unwrap_err().contains("target.node"));
    }
}
