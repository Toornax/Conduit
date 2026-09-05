//! Bindings PortCls et Kernel Streaming pour le pilote Conduit.
//!
//! Crate du workspace noyau (ADR-012), conçu pour rester **testable en mode utilisateur** :
//! `wdk-sys` (qui lie les bibliothèques noyau même sous `cargo test`) n'est qu'une
//! dépendance optionnelle derrière la feature `kernel`, activée par `conduit-kmd` seul ;
//! `cargo test -p portcls-sys` tourne sans elle. Le crate n'émet aucun `#[link]` : c'est
//! `conduit-kmd/build.rs` qui lie `portcls.lib`.
//!
//! État (M1a-02) : le module [`functions`] déclare **à la main, provisoirement**, les deux
//! fonctions PortCls dont le pilote minimal a besoin (`PcInitializeAdapterDriver`,
//! `PcAddAdapterDevice`) et les types qu'elles exigent. La tâche M1a-03 remplace ces
//! déclarations par les bindings générés selon
//! [driver-design.md §2.2](../../../docs/driver-design.md) :
//!
//! - les bindings générés par `bindgen` (`wdk_build::BuilderExt::wdk_default`) sur
//!   `ks.h`, `ksmedia.h`, `punknown.h`, `drmk.h` et `portcls.h` en mode C, avec une liste
//!   d'autorisation explicite : structures (`KSDATAFORMAT`, `KSDATARANGE_AUDIO`,
//!   `PCFILTER_DESCRIPTOR`, `PCPIN_DESCRIPTOR`, `PCNODE_DESCRIPTOR`, `PCPROPERTY_*`,
//!   `KSJACK_DESCRIPTION`, `KSRTAUDIO_*`…), GUID (`KSCATEGORY_*`, `KSDATAFORMAT_*`,
//!   `KSNODETYPE_*`, `KSPROPSETID_*`) et constantes ;
//! - les vtables COM **plates** produites par bindgen depuis les macros
//!   `DECLARE_INTERFACE_`/`STDMETHOD_` (`IMiniportWaveRT`, `IMiniportWaveRTStream`,
//!   `IMiniportTopology`, `IPortWaveRT`, `IAdapterPowerManagement`…), grâce au
//!   `#define INTERFACE void` placé avant `portcls.h` ;
//! - les déclarations des autres fonctions PortCls (`PcNewPort`, `PcRegisterSubdevice`,
//!   `PcRegisterPhysicalConnection`, `PcNewResourceList`…) ;
//! - des tests de `size_of` et de nombre de slots de vtable contre des valeurs de
//!   référence obtenues par un programme C (`tools/sizeof-probe.c`).
//!
//! `unsafe` n'est autorisé ici que pour les blocs `extern` (déclarations de fonctions
//! externes) ; `unsafe_op_in_unsafe_fn` reste en `deny` (lints du workspace).

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(feature = "kernel")]
pub mod functions;

/// Version du WDK dont les en-têtes servent de source aux bindings (M1a-03).
///
/// Épinglée dans `packaging/windows/versions.json` (`wdk`) ; exposée ici pour que
/// `conduit-kmd` puisse la journaliser au chargement.
pub const WDK_VERSION: &str = "10.0.26100.0";

#[cfg(test)]
mod tests {
    use super::WDK_VERSION;

    /// Prouve que le crate se compile et se teste en mode utilisateur sans lier au noyau.
    #[test]
    fn wdk_version_epinglee() {
        assert!(WDK_VERSION.starts_with("10.0.26100"));
    }
}
