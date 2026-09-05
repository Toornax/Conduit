//! Clés stables de nœuds : indépendantes de l'ordre d'énumération et des
//! identifiants générationnels du graphe (F-30).

use std::collections::HashMap;

use conduit_core::graph::NodeId;
pub use conduit_protocol::api::NodeKey;

/// Table bidirectionnelle clé ↔ identifiant de graphe.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    by_key: HashMap<NodeKey, NodeId>,
    by_id: HashMap<NodeId, NodeKey>,
}

impl Registry {
    /// Enregistre une association. Remplace une association existante pour la clé.
    pub fn insert(&mut self, key: NodeKey, id: NodeId) {
        if let Some(old) = self.by_key.insert(key.clone(), id) {
            self.by_id.remove(&old);
        }
        self.by_id.insert(id, key);
    }

    /// Retire par identifiant ; retourne la clé.
    pub fn remove_id(&mut self, id: NodeId) -> Option<NodeKey> {
        let key = self.by_id.remove(&id)?;
        self.by_key.remove(&key);
        Some(key)
    }

    /// Retire par clé ; retourne l'identifiant.
    pub fn remove_key(&mut self, key: &NodeKey) -> Option<NodeId> {
        let id = self.by_key.remove(key)?;
        self.by_id.remove(&id);
        Some(id)
    }

    /// Identifiant pour une clé.
    pub fn id(&self, key: &NodeKey) -> Option<NodeId> {
        self.by_key.get(key).copied()
    }

    /// Clé pour un identifiant.
    pub fn key(&self, id: NodeId) -> Option<&NodeKey> {
        self.by_id.get(&id)
    }

    /// Nombre d'entrées.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Vrai si vide.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Itère sur les associations.
    pub fn iter(&self) -> impl Iterator<Item = (&NodeKey, NodeId)> + '_ {
        self.by_key.iter().map(|(k, &v)| (k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_display_and_accessors() {
        let d = NodeKey::device("null", "null:micro".into());
        assert_eq!(d.to_string(), "null:null:micro");
        assert_eq!(d.device_id().unwrap().as_str(), "null:micro");
        let i = NodeKey::internal("sine");
        assert_eq!(i.to_string(), "internal:sine");
        assert!(i.device_id().is_none());
        assert_ne!(d, i);
    }

    #[test]
    fn registry_roundtrip_and_replacement() {
        let mut r = Registry::default();
        assert!(r.is_empty());
        let k = NodeKey::internal("a");
        r.insert(k.clone(), NodeId::new(0, 0));
        assert_eq!(r.id(&k), Some(NodeId::new(0, 0)));
        assert_eq!(r.key(NodeId::new(0, 0)), Some(&k));
        // Même clé, nouvel identifiant (réapparition) : l'ancien est oublié.
        r.insert(k.clone(), NodeId::new(0, 1));
        assert_eq!(r.key(NodeId::new(0, 0)), None);
        assert_eq!(r.id(&k), Some(NodeId::new(0, 1)));
        assert_eq!(r.len(), 1);
        assert_eq!(r.iter().count(), 1);
        assert_eq!(r.remove_key(&k), Some(NodeId::new(0, 1)));
        assert!(r.remove_id(NodeId::new(0, 1)).is_none());
        r.insert(k.clone(), NodeId::new(5, 0));
        assert_eq!(r.remove_id(NodeId::new(5, 0)), Some(k));
        assert!(r.is_empty());
    }

    #[test]
    fn keys_survive_reenumeration_order() {
        // Deux redémarrages simulés énumèrent dans un ordre différent : les clés sont
        // identiques, donc les liens persistés se retrouvent.
        let first = ["null:a", "null:b"].map(|s| NodeKey::device("null", s.into()));
        let second = ["null:b", "null:a"].map(|s| NodeKey::device("null", s.into()));
        let mut r1 = Registry::default();
        let mut r2 = Registry::default();
        for (i, k) in first.iter().enumerate() {
            r1.insert(k.clone(), NodeId::new(i as u32, 0));
        }
        for (i, k) in second.iter().enumerate() {
            r2.insert(k.clone(), NodeId::new(i as u32, 0));
        }
        assert_ne!(r1.id(&first[0]), r2.id(&first[0]));
        assert!(r2.id(&first[0]).is_some() && r2.id(&first[1]).is_some());
    }
}
