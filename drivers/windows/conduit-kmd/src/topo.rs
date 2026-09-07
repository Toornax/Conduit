//! Miniports topologie d'un câble (driver-design.md §4) : [`TopoRender`] et
//! [`TopoCapture`], un par sens, implémentent `portcls::MiniportTopology` et
//! `portcls::AudioNodes`.
//!
//! Ils exposent le descripteur de filtre **de leur câble**
//! (`descriptors::topo_render_filter(n)`, `topo_capture_filter(n)` : broche bridge,
//! broche endpoint nommée par le GUID du câble, nœud de volume, nœud de
//! sourdine) et servent les propriétés de ces nœuds en déléguant au [`NodeState`] du bon
//! sens dans le câble. Ils implémentent aussi `portcls::JackInfo`, la propriété de
//! **filtre** `KSPROPERTY_JACK_DESCRIPTION` (M1b-03). `Init` ne conserve rien : le port
//! topologie et la liste de ressources reçus sont relâchés en sortie (`Drop` des
//! enveloppes). `DataRangeIntersection` reste au défaut (PortCls intersecte lui-même).
//!
//! # Le jack : quelle broche, et quelle cartographie
//!
//! Deux valeurs seulement distinguent les deux sens, et toutes deux sont des pièges.
//!
//! **La broche.** [`JackInfo::jack_pin`] désigne la broche qui **fait face à
//! l'extérieur** — celle dont Windows tire l'endpoint, celle qui porte le nom du câble :
//! [`TOPO_RENDER_PIN_ENDPOINT`] (1) au rendu, [`TOPO_CAPTURE_PIN_ENDPOINT`] (0) à la
//! capture. Ce n'est **pas** la broche que `descriptors::bridge_pin` construit, malgré le
//! vocabulaire de la documentation Microsoft, qui appelle « bridge pin » la broche à prise.
//! Se tromper ne casse rien de visible : la propriété répond, mais par une description
//! vide, et l'endpoint garde l'état de connexion par défaut de Windows. D'où les
//! assertions `const` ci-dessous.
//!
//! **La cartographie.** `KSJACK_DESCRIPTION::ChannelMapping` doit être non nul « *only for
//! analog rendering pins* » : `KSAUDIO_SPEAKER_STEREO` au rendu, **0** à la capture.
//!
//! L'état de connexion, lui, est **par câble** et commun aux deux sens
//! ([`Cable::is_connected`]) : un câble débranché l'est de ses deux bouts.
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
use portcls::{
    AudioNodes, JackInfo, JackTrace, MiniportTopology, PortTopology, ResourceList, Trace,
};
use portcls_sys::{IUnknown, KSAUDIO_SPEAKER_STEREO, PCFILTER_DESCRIPTOR, ULONG};

use crate::cable::{Cable, Direction, MAX_CHANNELS, NodeState};
use crate::descriptors::{
    CHANNELS, PIN_COUNT, TOPO_CAPTURE_PIN_ENDPOINT, TOPO_RENDER_PIN_ENDPOINT, topo_capture_filter,
    topo_capture_filter_0, topo_render_filter, topo_render_filter_0,
};

const _: () = assert!(
    CHANNELS as usize <= MAX_CHANNELS,
    "le nœud de volume déclare plus de canaux que `NodeState` n'en mémorise"
);

/// Nombre de broches d'un filtre de topologie, du type que `JackInfo::pin_count` rend.
///
/// Doit valoir `PCFILTER_DESCRIPTOR::PinCount` : c'est lui qui sépare « broche existante
/// sans prise » (réponse vide, `STATUS_SUCCESS`) de « broche inexistante »
/// (`STATUS_INVALID_PARAMETER`).
const BROCHES: u32 = PIN_COUNT as u32;

/// `KSJACK_DESCRIPTION::ChannelMapping` du sens **rendu** : les deux canaux avant.
///
/// `KSAUDIO_SPEAKER_STEREO` vaut `SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT` (3) : autant de
/// bits que [`CHANNELS`], ce que l'assertion ci-dessous vérifie. Une divergence — un câble
/// à six canaux qui annoncerait encore une cartographie stéréo — se verrait dans le nom des
/// canaux affiché par Windows, et nulle part ailleurs.
const MAPPING_RENDU: ULONG = KSAUDIO_SPEAKER_STEREO;

/// `KSJACK_DESCRIPTION::ChannelMapping` du sens **capture** : nul, comme la documentation
/// l'exige pour toute broche qui n'est pas une broche de rendu analogique.
const MAPPING_CAPTURE: ULONG = 0;

const _: () = {
    // La broche à prise est l'endpoint, celle qui fait face à l'extérieur — jamais la
    // broche bridge vers le filtre WaveRT (voir l'en-tête de module).
    assert!(TOPO_RENDER_PIN_ENDPOINT == 1 && TOPO_CAPTURE_PIN_ENDPOINT == 0);
    assert!(TOPO_RENDER_PIN_ENDPOINT < BROCHES && TOPO_CAPTURE_PIN_ENDPOINT < BROCHES);
    // Autant de bits à 1 dans la cartographie que de canaux déclarés.
    assert!(MAPPING_RENDU.count_ones() == CHANNELS);
    assert!(MAPPING_CAPTURE == 0);
};

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

impl JackInfo for TopoRender {
    // IRQL: PASSIVE_LEVEL
    fn pin_count(&self) -> u32 {
        BROCHES
    }

    // IRQL: PASSIVE_LEVEL — la broche endpoint, pas la broche bridge (en-tête de module).
    fn jack_pin(&self) -> u32 {
        TOPO_RENDER_PIN_ENDPOINT
    }

    // IRQL: PASSIVE_LEVEL — broche de rendu analogique : cartographie non nulle.
    fn channel_mapping(&self) -> u32 {
        MAPPING_RENDU
    }

    // IRQL: quelconque — l'état de connexion est par câble, commun aux deux sens.
    fn is_connected(&self) -> bool {
        self.cable.is_connected()
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &JackTrace<'_>) {
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

impl JackInfo for TopoCapture {
    // IRQL: PASSIVE_LEVEL
    fn pin_count(&self) -> u32 {
        BROCHES
    }

    // IRQL: PASSIVE_LEVEL — l'endpoint est ici la broche d'**entrée** (en-tête de module).
    fn jack_pin(&self) -> u32 {
        TOPO_CAPTURE_PIN_ENDPOINT
    }

    // IRQL: PASSIVE_LEVEL — broche de capture : cartographie **nulle**, la documentation
    // l'exige.
    fn channel_mapping(&self) -> u32 {
        MAPPING_CAPTURE
    }

    // IRQL: quelconque — le même atomique que le sens rendu, un câble ayant deux bouts.
    fn is_connected(&self) -> bool {
        self.cable.is_connected()
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &JackTrace<'_>) {
        kmd_log!("TopoCapture{} : {trace:?}", self.n);
    }
}
