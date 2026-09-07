//! Miniports topologie d'un câble (driver-design.md §4) : [`TopoRender`] et
//! [`TopoCapture`], un par sens, implémentent `portcls::MiniportTopology` et
//! `portcls::AudioNodes`.
//!
//! Ils exposent le descripteur de filtre **de leur câble**
//! (`descriptors::topo_render_filter(n)`, `topo_capture_filter(n)` : broche bridge,
//! broche endpoint nommée par le GUID du câble, nœud de volume, nœud de
//! sourdine) et servent les propriétés de ces nœuds en déléguant au [`NodeState`] du bon
//! sens dans le câble. `Init` ne conserve rien : le port topologie et la liste de
//! ressources reçus sont relâchés en sortie (`Drop` des enveloppes). Jack : M1b-03 ;
//! `DataRangeIntersection` reste au défaut (PortCls intersecte lui-même).
//!
//! # Le volume est mémorisé, pas appliqué
//!
//! Les deux moitiés du mécanisme sont documentées sur [`NodeState`] et dans
//! driver-design.md §5.5 : **exposer** le nœud fait renoncer Windows à son APO logiciel,
//! **ne pas appliquer** la valeur qu'il y pousse aussitôt est ce qui rend le câble
//! transparent. Ces implémentations sont donc de purs accesseurs sur des atomiques ;
//! aucune ne touche au signal.
//!
//! # Le nombre de canaux
//!
//! [`AudioNodes::channels`] doit s'accorder avec le format des broches : c'est
//! [`descriptors::CHANNELS`], la valeur unique de M1a (2). Une divergence rendrait
//! `STATUS_INVALID_PARAMETER` sur un canal pourtant déclaré par le format, ou
//! l'inverse — d'où l'assertion `const` ci-dessous contre le plafond de
//! [`cable::MAX_CHANNELS`].
//!
//! # La trace
//!
//! [`AudioNodes::trace`] est câblée sur `kmd_log!` en une ligne. Elle existe pour rendre
//! vérifiable en une minute, au premier essai en machine, le seul décalage du plan dont
//! l'erreur serait silencieuse : le `Channel` en tête de `Instance` (voir la documentation
//! de `portcls::audio`). Attendu sur un endpoint stéréo : Windows interroge le canal `0`
//! puis le canal `1`, et écrit avec le canal `-1`. Si la trace ne montre que des
//! « canal 0 », c'est le `Reserved` qu'on lit. Vide en release (`kmd_log!`).

use portcls::conduit_com::{ComRef, NtStatus, STATUS_SUCCESS};
use portcls::{AudioNodes, MiniportTopology, PortTopology, ResourceList, Trace};
use portcls_sys::{IUnknown, PCFILTER_DESCRIPTOR};

use crate::cable::{Cable, Direction, MAX_CHANNELS, NodeState};
use crate::descriptors::{
    CHANNELS, topo_capture_filter, topo_capture_filter_0, topo_render_filter, topo_render_filter_0,
};

const _: () = assert!(
    CHANNELS as usize <= MAX_CHANNELS,
    "le nœud de volume déclare plus de canaux que `NodeState` n'en mémorise"
);

/// Miniport topologie du filtre `TopoRender<n>`.
#[derive(Debug)]
pub struct TopoRender {
    /// Numéro du câble.
    pub n: u32,
    /// L'état partagé du câble, qui porte les nœuds du sens rendu.
    pub cable: &'static Cable,
}

impl TopoRender {
    /// Les nœuds volume et sourdine du sens rendu.
    fn nodes(&self) -> &'static NodeState {
        self.cable.nodes(Direction::Render)
    }
}

impl MiniportTopology for TopoRender {
    // IRQL: PASSIVE_LEVEL
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        _port: PortTopology,
    ) -> NtStatus {
        kmd_log!("TopoRender{}::Init", self.n);
        STATUS_SUCCESS
    }

    // IRQL: PASSIVE_LEVEL
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        // Les seize filtres ne diffèrent que par le GUID de nom de leur broche endpoint,
        // d'où l'indexation par `self.n`. Le trait rend une **référence**, pas une
        // `Option` : le repli sur le câble 0 n'est là que pour qu'aucun chemin ne panique
        // (même idiome que `cable::NodeState::volume`). `n < CABLE_COUNT` est garanti par
        // l'appelant : `adapter::install_cable` ne construit ce miniport que pour un
        // câble dont `cable::cable(n)` et `portcls::subdevice_names(n)` ont répondu.
        topo_render_filter(self.n).unwrap_or_else(topo_render_filter_0)
    }
}

impl AudioNodes for TopoRender {
    // IRQL: PASSIVE_LEVEL
    fn channels(&self) -> u32 {
        CHANNELS
    }

    // IRQL: PASSIVE_LEVEL
    fn volume(&self, channel: u32) -> i32 {
        self.nodes().volume(channel)
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien (voir l'en-tête de module).
    fn set_volume(&self, channel: u32, level: i32) {
        self.nodes().set_volume(channel, level);
    }

    // IRQL: PASSIVE_LEVEL
    fn muted(&self) -> bool {
        self.nodes().muted()
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien.
    fn set_muted(&self, muted: bool) {
        self.nodes().set_muted(muted);
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &Trace<'_>) {
        kmd_log!("TopoRender{} : {trace:?}", self.n);
    }
}

/// Miniport topologie du filtre `TopoCapture<n>`.
#[derive(Debug)]
pub struct TopoCapture {
    /// Numéro du câble.
    pub n: u32,
    /// L'état partagé du câble, qui porte les nœuds du sens capture.
    pub cable: &'static Cable,
}

impl TopoCapture {
    /// Les nœuds volume et sourdine du sens capture.
    fn nodes(&self) -> &'static NodeState {
        self.cable.nodes(Direction::Capture)
    }
}

impl MiniportTopology for TopoCapture {
    // IRQL: PASSIVE_LEVEL
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        _port: PortTopology,
    ) -> NtStatus {
        kmd_log!("TopoCapture{}::Init", self.n);
        STATUS_SUCCESS
    }

    // IRQL: PASSIVE_LEVEL
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        // Repli documenté sur le câble 0 : voir `TopoRender::description`.
        topo_capture_filter(self.n).unwrap_or_else(topo_capture_filter_0)
    }
}

impl AudioNodes for TopoCapture {
    // IRQL: PASSIVE_LEVEL
    fn channels(&self) -> u32 {
        CHANNELS
    }

    // IRQL: PASSIVE_LEVEL
    fn volume(&self, channel: u32) -> i32 {
        self.nodes().volume(channel)
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien (voir l'en-tête de module).
    fn set_volume(&self, channel: u32, level: i32) {
        self.nodes().set_volume(channel, level);
    }

    // IRQL: PASSIVE_LEVEL
    fn muted(&self) -> bool {
        self.nodes().muted()
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien.
    fn set_muted(&self, muted: bool) {
        self.nodes().set_muted(muted);
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &Trace<'_>) {
        kmd_log!("TopoCapture{} : {trace:?}", self.n);
    }
}
