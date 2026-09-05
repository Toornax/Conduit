//! Identifiants générationnels.

use core::fmt;

use super::Direction;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Index brut dans une table d'emplacements.
pub type RawIndex = u32;

/// Index d'un port dans la liste des ports (d'une direction) d'un nœud.
pub type PortIndex = u16;

macro_rules! gen_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        pub struct $name {
            index: RawIndex,
            generation: u32,
        }

        impl $name {
            /// Construit à partir d'un index et d'une génération.
            pub const fn new(index: RawIndex, generation: u32) -> Self {
                Self { index, generation }
            }

            /// Index d'emplacement.
            pub const fn index(self) -> RawIndex {
                self.index
            }

            /// Génération de l'emplacement.
            pub const fn generation(self) -> u32 {
                self.generation
            }

            /// Encodage compact en `u64` (index en poids faible).
            pub const fn to_u64(self) -> u64 {
                ((self.generation as u64) << 32) | self.index as u64
            }

            /// Décodage depuis [`to_u64`](Self::to_u64).
            pub const fn from_u64(v: u64) -> Self {
                Self { index: v as u32, generation: (v >> 32) as u32 }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}.{}", $prefix, self.index, self.generation)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }
    };
}

gen_id!(
    /// Identifiant d'un nœud. Un identifiant n'est jamais réutilisé après suppression.
    NodeId,
    "n"
);
gen_id!(
    /// Identifiant d'un lien.
    LinkId,
    "l"
);

/// Identifiant d'un port : nœud, direction et index dans cette direction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PortId {
    /// Nœud propriétaire.
    pub node: NodeId,
    /// Direction du port.
    pub direction: Direction,
    /// Index dans les ports de cette direction.
    pub index: PortIndex,
}

impl PortId {
    /// Construit.
    pub const fn new(node: NodeId, direction: Direction, index: PortIndex) -> Self {
        Self {
            node,
            direction,
            index,
        }
    }
}

impl fmt::Display for PortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = match self.direction {
            Direction::Input => "in",
            Direction::Output => "out",
        };
        write!(f, "{}:{}{}", self.node, d, self.index)
    }
}

impl fmt::Debug for PortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_u64_roundtrip() {
        let n = NodeId::new(7, 3);
        assert_eq!(n.to_string(), "n7.3");
        assert_eq!(NodeId::from_u64(n.to_u64()), n);
        assert_eq!(format!("{n:?}"), "n7.3");
        let l = LinkId::new(1, 0);
        assert_eq!(l.to_string(), "l1.0");
        let p = PortId::new(n, Direction::Output, 2);
        assert_eq!(p.to_string(), "n7.3:out2");
        assert_eq!(PortId::new(n, Direction::Input, 0).to_string(), "n7.3:in0");
        assert!(NodeId::new(0, 0) < NodeId::new(1, 0));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip() {
        let p = PortId::new(NodeId::new(3, 1), Direction::Input, 4);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(
            json,
            r#"{"node":{"index":3,"generation":1},"direction":"input","index":4}"#
        );
        let back: PortId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        let l: LinkId = serde_json::from_str(r#"{"index":9,"generation":2}"#).unwrap();
        assert_eq!(l, LinkId::new(9, 2));
    }
}
