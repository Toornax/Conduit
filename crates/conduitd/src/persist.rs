//! Persistance et restauration de l'état du graphe (F-30, M0-85).
//!
//! Les nœuds sont identifiés par leur clé stable ; les liens par (clé, nom de port).
//! Sauvegarde atomique : fichier temporaire puis renommage.

use std::collections::BTreeMap;
use std::path::Path;

use conduit_core::graph::Direction;
use conduit_core::types::Db;
use conduit_engine::{Command, Engine, EngineError, InternalKind, NodeKey};
use serde::{Deserialize, Serialize};

/// Version du format d'état.
pub const STATE_VERSION: u32 = 1;

/// Nœud interne persisté.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedInternal {
    /// Nom unique.
    pub name: String,
    /// Type et réglages initiaux.
    pub kind: InternalKind,
    /// Paramètres réglés à chaud.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

/// Réglages d'un nœud (interne ou périphérique).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedNode {
    /// Clé stable.
    pub key: NodeKey,
    /// Nom d'affichage.
    pub label: String,
    /// Gain (dB).
    pub gain_db: Db,
    /// Muet.
    pub muted: bool,
}

/// Lien persisté.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedLink {
    /// Nœud source.
    pub src: NodeKey,
    /// Port de sortie.
    pub src_port: String,
    /// Nœud destination.
    pub dst: NodeKey,
    /// Port d'entrée.
    pub dst_port: String,
    /// Gain (dB).
    pub gain_db: Db,
    /// Muet.
    pub muted: bool,
}

/// État complet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    /// Version du format.
    pub version: u32,
    /// Pilote choisi.
    pub driver: conduit_engine::DriverChoice,
    /// Nœuds internes.
    #[serde(default)]
    pub internal: Vec<PersistedInternal>,
    /// Réglages des nœuds.
    #[serde(default)]
    pub nodes: Vec<PersistedNode>,
    /// Liens.
    #[serde(default)]
    pub links: Vec<PersistedLink>,
}

/// Erreurs de persistance.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    /// Entrée/sortie.
    #[error("état : {0}")]
    Io(#[from] std::io::Error),
    /// JSON invalide.
    #[error("fichier d'état illisible ({0}) : il sera ignoré et réécrit")]
    Json(#[from] serde_json::Error),
    /// Version future.
    #[error("fichier d'état de version {0}, ce démon lit la version {STATE_VERSION} : mettez à jour Conduit")]
    Version(u32),
}

impl PersistedState {
    /// Capture l'état courant du moteur.
    pub fn capture(engine: &Engine) -> Self {
        let mut internal = Vec::new();
        let mut nodes = Vec::new();
        for n in engine.nodes() {
            if let NodeKey::Internal { name } = &n.key {
                if let Some(kind) = engine.internal_kind(n.id) {
                    internal.push(PersistedInternal {
                        name: name.clone(),
                        kind: kind.clone(),
                        params: engine.internal_params(n.id).cloned().unwrap_or_default(),
                    });
                }
            }
            nodes.push(PersistedNode {
                key: n.key.clone(),
                label: n.label.clone(),
                gain_db: n.gain_db,
                muted: n.muted,
            });
        }
        let mut links = Vec::new();
        for l in engine.links() {
            let (Some(src), Some(dst)) = (
                engine.registry().key(l.link.src.node),
                engine.registry().key(l.link.dst.node),
            ) else {
                continue;
            };
            let g = engine.graph();
            let (Ok(sn), Ok(dn)) = (g.node(l.link.src.node), g.node(l.link.dst.node)) else {
                continue;
            };
            let (Some(sp), Some(dp)) = (
                sn.outputs.get(l.link.src.index as usize),
                dn.inputs.get(l.link.dst.index as usize),
            ) else {
                continue;
            };
            links.push(PersistedLink {
                src: src.clone(),
                src_port: sp.name.clone(),
                dst: dst.clone(),
                dst_port: dp.name.clone(),
                gain_db: l.gain_db,
                muted: l.muted,
            });
        }
        Self {
            version: STATE_VERSION,
            driver: engine.config().driver.clone(),
            internal,
            nodes,
            links,
        }
    }

    /// Écrit atomiquement (fichier temporaire + renommage).
    pub fn save_atomic(&self, path: &Path) -> Result<(), PersistError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Lit un fichier ; `Ok(None)` s'il n'existe pas.
    pub fn load(path: &Path) -> Result<Option<Self>, PersistError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let state: Self = serde_json::from_str(&text)?;
        if state.version > STATE_VERSION {
            return Err(PersistError::Version(state.version));
        }
        Ok(Some(state))
    }
}

/// Résultat d'une restauration.
#[derive(Debug, Default, PartialEq)]
pub struct RestoreReport {
    /// Nœuds internes recréés.
    pub internal_nodes: usize,
    /// Réglages appliqués.
    pub settings: usize,
    /// Liens recréés.
    pub links: usize,
    /// Liens dont un nœud est absent : à appliquer quand il apparaîtra.
    pub pending: Vec<PersistedLink>,
    /// Erreurs non bloquantes (message).
    pub errors: Vec<String>,
}

/// Applique un état au moteur. Les liens vers des nœuds absents sont retournés en
/// attente.
pub fn restore(engine: &mut Engine, state: &PersistedState) -> RestoreReport {
    let mut report = RestoreReport::default();
    for i in &state.internal {
        let key = NodeKey::internal(&i.name);
        if engine.node_by_key(&key).is_some() {
            continue;
        }
        match engine.execute(Command::AddInternal {
            name: i.name.clone(),
            kind: i.kind.clone(),
        }) {
            Ok(_) => report.internal_nodes += 1,
            Err(e) => {
                report.errors.push(format!("nœud interne {} : {e}", i.name));
                continue;
            }
        }
        if let Some(id) = engine.node_by_key(&key) {
            for (name, value) in &i.params {
                if let Err(e) = engine.set_param(id, name, *value) {
                    report
                        .errors
                        .push(format!("paramètre {name} de {} : {e}", i.name));
                }
            }
        }
    }
    for n in &state.nodes {
        if let Some(id) = engine.node_by_key(&n.key) {
            let _ = engine.execute(Command::SetLabel {
                node: id,
                label: n.label.clone(),
            });
            let _ = engine.execute(Command::SetNodeGain {
                node: id,
                gain_db: Some(n.gain_db),
                muted: Some(n.muted),
            });
            report.settings += 1;
        }
    }
    for l in &state.links {
        match apply_link(engine, l) {
            Ok(true) => report.links += 1,
            Ok(false) => report.pending.push(l.clone()),
            Err(e) => report.errors.push(format!(
                "lien {}:{} → {}:{} : {e}",
                l.src, l.src_port, l.dst, l.dst_port
            )),
        }
    }
    if engine.config().driver != state.driver {
        if let Err(e) = engine.set_driver(state.driver.clone()) {
            report.errors.push(format!("pilote : {e}"));
        }
    }
    report
}

/// Tente de créer un lien persisté. `Ok(false)` si un nœud est absent.
pub fn apply_link(engine: &mut Engine, l: &PersistedLink) -> Result<bool, EngineError> {
    let (Some(src), Some(dst)) = (engine.node_by_key(&l.src), engine.node_by_key(&l.dst)) else {
        return Ok(false);
    };
    let sp = engine.port_by_name(src, Direction::Output, &l.src_port)?;
    let dp = engine.port_by_name(dst, Direction::Input, &l.dst_port)?;
    if engine.graph().find_link(sp, dp).is_some() {
        return Ok(true);
    }
    let d = engine.link(sp, dp)?;
    let _ = engine.execute(Command::SetLinkGain {
        link: d.link.id,
        gain_db: Some(l.gain_db),
        muted: Some(l.muted),
    });
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_backend::null::{NullBackend, NullDeviceSpec};
    use conduit_core::types::Quantum;
    use conduit_engine::{DriverChoice, EngineConfig, Reply};

    fn engine(null: &NullBackend) -> Engine {
        Engine::new(
            Box::new(null.clone()),
            EngineConfig {
                quantum: Quantum::new(256).unwrap(),
                driver: DriverChoice::Internal,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn capture_save_load_restore_gives_identical_graph() {
        let null = NullBackend::new();
        let spk = null.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
        let mut e1 = engine(&null);
        let Reply::Node(sine) = e1
            .execute(Command::AddInternal {
                name: "gen".into(),
                kind: InternalKind::Sine {
                    frequency: 440.0,
                    amplitude: 0.5,
                    channels: 2,
                },
            })
            .unwrap()
        else {
            panic!()
        };
        e1.set_param(sine.id, "frequency", 880.0).unwrap();
        let spk_node = e1
            .node_by_key(&NodeKey::device("null", spk.clone()))
            .unwrap();
        e1.execute(Command::LinkByName {
            src_node: sine.id,
            src_port: "FL".into(),
            dst_node: spk_node,
            dst_port: "FL".into(),
        })
        .unwrap();
        let Reply::Link(l) = e1
            .execute(Command::LinkByName {
                src_node: sine.id,
                src_port: "FR".into(),
                dst_node: spk_node,
                dst_port: "FR".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        e1.execute(Command::SetLinkGain {
            link: l.link.id,
            gain_db: Some(Db::new(-6.0)),
            muted: Some(true),
        })
        .unwrap();
        e1.execute(Command::SetNodeGain {
            node: spk_node,
            gain_db: Some(Db::new(-3.0)),
            muted: None,
        })
        .unwrap();
        e1.execute(Command::SetLabel {
            node: spk_node,
            label: "Enceintes".into(),
        })
        .unwrap();
        let state = PersistedState::capture(&e1);
        assert_eq!(state.internal.len(), 1);
        assert_eq!(state.internal[0].params.get("frequency"), Some(&880.0));
        assert_eq!(state.links.len(), 2);
        assert_eq!(state.nodes.len(), 2);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/state.json");
        state.save_atomic(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        let loaded = PersistedState::load(&path).unwrap().unwrap();
        assert_eq!(loaded, state);

        // « Redémarrage » : nouveau moteur, énumération dans un autre ordre.
        let null2 = NullBackend::new();
        let _other = null2.add_device(NullDeviceSpec::capture("Micro").layout(1, 256));
        let spk2 = null2.add_device(NullDeviceSpec::render("Haut-parleurs").layout(2, 256));
        assert_eq!(spk2, spk);
        let mut e2 = engine(&null2);
        let report = restore(&mut e2, &loaded);
        assert_eq!(report.internal_nodes, 1);
        assert_eq!(report.links, 2);
        assert!(report.pending.is_empty());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let again = PersistedState::capture(&e2);
        assert_eq!(
            again.links, state.links,
            "liens identiques par clés stables"
        );
        assert_eq!(again.internal, state.internal);
        let spk_desc = e2
            .node_descriptor(e2.node_by_key(&NodeKey::device("null", spk)).unwrap())
            .unwrap();
        assert_eq!(spk_desc.label, "Enceintes");
        assert!((spk_desc.gain_db.get() + 3.0).abs() < 0.01);
        // Restaurer deux fois est idempotent.
        let report2 = restore(&mut e2, &loaded);
        assert_eq!(report2.links, 2);
        assert_eq!(e2.links().len(), 2);
    }

    #[test]
    fn links_to_absent_devices_are_pending_then_applied() {
        let null = NullBackend::new();
        let mut e = engine(&null);
        e.execute(Command::AddInternal {
            name: "gen".into(),
            kind: InternalKind::Sine {
                frequency: 440.0,
                amplitude: 0.5,
                channels: 1,
            },
        })
        .unwrap();
        let state = PersistedState {
            version: STATE_VERSION,
            driver: DriverChoice::Internal,
            internal: vec![],
            nodes: vec![],
            links: vec![PersistedLink {
                src: NodeKey::internal("gen"),
                src_port: "MONO".into(),
                dst: NodeKey::device("null", "null:casque".into()),
                dst_port: "FL".into(),
                gain_db: Db::UNITY,
                muted: false,
            }],
        };
        let report = restore(&mut e, &state);
        assert_eq!(report.links, 0);
        assert_eq!(report.pending.len(), 1);
        null.add_device(NullDeviceSpec::render("Casque").layout(2, 256));
        e.tick();
        assert!(apply_link(&mut e, &report.pending[0]).unwrap());
        assert_eq!(e.links().len(), 1);
        // Port inexistant : erreur explicite.
        let bad = PersistedLink {
            dst_port: "FC".into(),
            ..report.pending[0].clone()
        };
        assert!(apply_link(&mut e, &bad).is_err());
    }

    #[test]
    fn load_errors_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        assert!(PersistedState::load(&path).unwrap().is_none());
        std::fs::write(&path, "{ pas du json").unwrap();
        assert!(matches!(
            PersistedState::load(&path),
            Err(PersistError::Json(_))
        ));
        std::fs::write(&path, r#"{"version": 99, "driver": {"mode": "auto"}}"#).unwrap();
        let e = PersistedState::load(&path).unwrap_err();
        assert!(e.to_string().contains("99"));
        std::fs::write(&path, r#"{"version": 1, "driver": {"mode": "auto"}}"#).unwrap();
        let s = PersistedState::load(&path).unwrap().unwrap();
        assert!(s.links.is_empty());
    }
}
