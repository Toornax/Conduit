//! Bindings PortCls et Kernel Streaming pour le pilote Conduit.
//!
//! Crate du workspace noyau (ADR-012), conçu pour rester **testable en mode utilisateur** :
//! il ne dépend pas de `wdk-sys` (qui lie les bibliothèques noyau même sous `cargo test`)
//! et n'émet aucun `#[link]` ; c'est `conduit-kmd/build.rs` qui lie `portcls.lib`.
//!
//! État (M1a-01) : crate vide qui prouve que le workspace compile et que
//! `cargo test -p portcls-sys` fonctionne. La tâche M1a-03 y ajoutera, selon
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
//! - les déclarations des fonctions PortCls (`PcInitializeAdapterDriver`,
//!   `PcAddAdapterDevice`, `PcNewPort`, `PcRegisterSubdevice`…) ;
//! - des tests de `size_of` et de nombre de slots de vtable contre des valeurs de
//!   référence obtenues par un programme C (`tools/sizeof-probe.c`).
//!
//! Tant que ces bindings n'existent pas, `#![forbid(unsafe_code)]` reste en place ; il
//! sera relâché en M1a-03 pour les déclarations `extern "system"` générées.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

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
