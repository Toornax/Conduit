//! Miniports topologie d'un câble (driver-design.md §4) : [`TopoRender`] et
//! [`TopoCapture`], un par sens, implémentent `portcls::MiniportTopology`.
//!
//! M1a-06 : ils ne font qu'exposer leur descripteur de filtre
//! (`descriptors::TOPO_RENDER_FILTER`, `TOPO_CAPTURE_FILTER` : broche bridge et broche
//! endpoint, connexion directe), ce qui suffit au générateur d'endpoints. `Init` ne
//! conserve rien : le port topologie et la liste de ressources reçus sont relâchés en
//! sortie (`Drop` des enveloppes). Nœuds volume/mute et jack : M1b-03 ;
//! `DataRangeIntersection` reste au défaut (PortCls intersecte lui-même).

use portcls::conduit_com::{ComRef, NtStatus, STATUS_SUCCESS};
use portcls::{MiniportTopology, PortTopology, ResourceList};
use portcls_sys::{IUnknown, PCFILTER_DESCRIPTOR};

use crate::descriptors::{TOPO_CAPTURE_FILTER, TOPO_RENDER_FILTER};

/// Miniport topologie du filtre `TopoRender<n>`.
#[derive(Debug)]
pub struct TopoRender {
    /// Numéro du câble.
    pub n: u32,
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
        TOPO_RENDER_FILTER.get()
    }
}

/// Miniport topologie du filtre `TopoCapture<n>`.
#[derive(Debug)]
pub struct TopoCapture {
    /// Numéro du câble.
    pub n: u32,
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
        TOPO_CAPTURE_FILTER.get()
    }
}
