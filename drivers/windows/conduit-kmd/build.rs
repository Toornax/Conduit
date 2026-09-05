//! Script de build de `conduit-kmd` : `wdk-build` fournit à Cargo les chemins du WDK et
//! les options de l'éditeur de liens d'un pilote noyau (`/DRIVER`, `/NODEFAULTLIB`,
//! `/SUBSYSTEM:NATIVE`, `/ENTRY:DriverEntry`, `/INTEGRITYCHECK`…). Le lien vers
//! `portcls.lib` sera ajouté ici en M1a-03 (driver-design.md §2.2).

fn main() -> Result<(), wdk_build::ConfigError> {
    wdk_build::configure_wdk_binary_build()
}
