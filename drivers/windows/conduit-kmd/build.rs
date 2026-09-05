//! Script de build de `conduit-kmd` : `wdk-build` fournit à Cargo les chemins du WDK et
//! les options de l'éditeur de liens d'un pilote noyau (`/DRIVER`, `/NODEFAULTLIB`,
//! `/SUBSYSTEM:NATIVE`, `/ENTRY:DriverEntry`, `/INTEGRITYCHECK`…), puis ce script ajoute
//! la liaison avec `portcls.lib` (driver-design.md §2.2 : le `#[link]` n'est pas dans
//! `portcls-sys`, pour qu'il reste testable en mode utilisateur).
//!
//! `portcls.lib` vit dans `Lib\<version>\km\<arch>` du WDK, dossier que `wdk-build` émet
//! déjà en `rustc-link-search` pour un pilote WDM. Par prudence, le script le vérifie et,
//! à défaut, retrouve le WDK par la clé de registre `KitsRoot10` (jamais de chemin en dur).

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use wdk_build::{Config, ConfigError};

/// Nom de la bibliothèque d'import de PortCls (`portcls.sys`).
const PORTCLS_LIB: &str = "portcls.lib";

fn main() -> Result<(), ConfigError> {
    let config = Config::from_env_auto()?;
    config.configure_binary_build()?;

    let deja_visible = config
        .library_paths()?
        .any(|dir| dir.join(PORTCLS_LIB).is_file());
    if !deja_visible {
        let dir =
            portcls_lib_dir_from_registry().ok_or_else(|| ConfigError::DirectoryNotFound {
                directory: format!("<KitsRoot10>\\Lib\\<version>\\km\\x64 contenant {PORTCLS_LIB}"),
            })?;
        println!(
            "cargo::warning=portcls.lib absent des chemins de wdk-build, ajout de {}",
            dir.display()
        );
        println!("cargo::rustc-link-search={}", dir.display());
    }

    println!("cargo::rustc-link-lib=portcls");
    Ok(())
}

/// Chemin de secours vers le dossier contenant `portcls.lib` : racine du WDK lue dans
/// `HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots\KitsRoot10`, puis
/// la version la plus récente de `Lib\*` qui possède `km\x64\portcls.lib`.
fn portcls_lib_dir_from_registry() -> Option<PathBuf> {
    let output = Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots",
            "/v",
            "KitsRoot10",
        ])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let root = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("KitsRoot10"))
        .filter_map(|rest| rest.trim().strip_prefix("REG_SZ"))
        .map(|value| PathBuf::from(value.trim()))
        .next()?;
    newest_lib_dir_with_portcls(&root.join("Lib"))
}

/// Parmi les sous-dossiers `10.0.xxxxx.0` de `Lib`, le plus récent (ordre lexical des
/// composantes numériques) qui contient `km\x64\portcls.lib`.
fn newest_lib_dir_with_portcls(lib: &Path) -> Option<PathBuf> {
    let mut candidats: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(lib)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("km").join("x64"))
        .filter(|dir| dir.join(PORTCLS_LIB).is_file())
        .filter_map(|dir| {
            let version = dir.parent()?.parent()?.file_name()?.to_str()?;
            let composantes = version
                .split('.')
                .map(str::parse)
                .collect::<Result<Vec<u32>, _>>()
                .ok()?;
            Some((composantes, dir))
        })
        .collect();
    candidats.sort();
    candidats.pop().map(|(_, dir)| dir)
}
