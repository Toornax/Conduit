//! Bindings PortCls et Kernel Streaming pour le pilote Conduit.
//!
//! Crate du workspace noyau (ADR-012), **autonome** (aucune dépendance à `wdk-sys`, qui
//! lie les bibliothèques noyau même sous `cargo test`) et donc testable en mode
//! utilisateur : `cargo test -p portcls-sys` depuis `drivers/windows`. Le crate n'émet
//! aucun `#[link]` : c'est `conduit-kmd/build.rs` qui lie `portcls.lib`.
//!
//! Contenu, généré à la compilation par `build.rs` (bindgen 0.71 sur les en-têtes du WDK
//! [`WDK_VERSION`], **mode C**, driver-design.md §2.2) :
//!
//! - structures et constantes de `ks.h`, `ksmedia.h`, `punknown.h`, `drmk.h`, `portcls.h`
//!   (`KSDATAFORMAT`, `KSDATARANGE_AUDIO`, `PCFILTER_DESCRIPTOR`, `PCPIN_DESCRIPTOR`,
//!   `PCPROPERTY_*`, `KSJACK_DESCRIPTION`, `KSRTAUDIO_*`, `KSSTATE`…), et, par
//!   récursivité, les types NT qu'elles référencent (`DRIVER_OBJECT`, `DEVICE_OBJECT`,
//!   `IRP`, `UNICODE_STRING`…) : ce sont **d'autres définitions** que celles de `wdk-sys`,
//!   de même disposition (mêmes en-têtes) ; `conduit-kmd` convertit par `cast()` ;
//! - les **vtables COM plates** issues des macros `DECLARE_INTERFACE_`/`STDMETHOD_` de
//!   `basetyps.h` : `struct IMiniportWaveRT { lpVtbl: *mut IMiniportWaveRTVtbl }` et une
//!   vtable de pointeurs de fonction `extern "C"` (`__stdcall` = ABI C sur x64) dans
//!   l'ordre du header, slots hérités compris (`IUnknown`, `IMiniport`, `IPort`…) ;
//! - les fonctions `Pc*` (`PcInitializeAdapterDriver`, `PcAddAdapterDevice`, `PcNewPort`,
//!   `PcRegisterSubdevice`…) et les types de rappel (`PCPFNSTARTDEVICE`…) ;
//! - les GUID (`IID_*`, `KSCATEGORY_*`, `KSDATAFORMAT_SUBTYPE_*`, `KSNODETYPE_*`,
//!   `KSPROPSETID_*`…) en `pub const GUID`, extraits des en-têtes par `build.rs`
//!   (bindgen les sortirait en `extern static` sans définition) ;
//! - les corrections manuelles de [`fixups`] pour les interfaces dont le mode C du WDK
//!   26100 est incomplet (`IPortClsVersion`) et les macros à `sizeof` que bindgen
//!   n'évalue pas (`PORT_CLASS_DEVICE_EXTENSION_SIZE`).
//!
//! Les énumérations C sont des modules de constantes (`KSSTATE::KSSTATE_RUN`,
//! `KSSTATE::Type`). `STATUS_SUCCESS` et les autres `STATUS_*` ne sont **pas** redéfinis
//! ici (`ntstatus.h` n'est pas dans la liste d'autorisation) : ils viennent de `wdk-sys`
//! côté pilote.
//!
//! Vérification (tests en mode utilisateur, `tests/`) : `size_of` de chaque structure
//! clé et de chaque vtable, et valeur des GUID, comparés à `tests/layout.golden` produit
//! par `cl.exe` sur les mêmes en-têtes (`tools/sizeof-probe.c`,
//! `drivers/windows/tools/regen-layout.ps1`) ; bindgen émet en outre ses propres
//! assertions de disposition (`const _: () = …`), vérifiées à la compilation.
//!
//! Feature **`com`** (désactivée par défaut) : le module [`com`] relie les vtables et
//! interfaces générées au modèle objet `conduit-com` (`ComVtable`, `ComInterface`), pour
//! le crate `portcls` ; `cargo test -p portcls-sys --features com` en vérifie la
//! disposition et les IID.

#![no_std]

#[cfg(test)]
extern crate std;

mod bindings;
#[cfg(feature = "com")]
pub mod com;
pub mod fixups;

pub use bindings::*;
pub use fixups::*;

/// Version du WDK dont les en-têtes servent de source aux bindings.
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
