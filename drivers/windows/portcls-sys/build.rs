//! Génération des bindings PortCls / Kernel Streaming par `bindgen`, en **mode C**
//! (driver-design.md §2.2, recette dans windows-drivers-rs.md).
//!
//! Deux passes :
//!
//! 1. **bindgen** sur `wrapper.h` (commité) : `wdk-build` fournit les chemins d'include du
//!    WDK (`km/crt`, `km`, `shared`), les défines d'un pilote WDM x64 (`_WIN64`, `_AMD64_`,
//!    `AMD64`, `_KERNEL_MODE`) et les flags clang de compatibilité Microsoft ;
//!    `bindgen::Builder::wdk_default` les assemble. Sortie : `OUT_DIR/portcls.rs`.
//! 2. **GUID** : bindgen n'évalue pas les initialiseurs de structure, donc chaque
//!    `DEFINE_GUID`/`DEFINE_GUIDSTRUCT` sortirait en `extern static` (symbole à fournir par
//!    l'éditeur de liens : `ksguid.lib` ou une unité C avec `INITGUID`, impossible ici).
//!    Ce script extrait donc les valeurs des en-têtes eux-mêmes (`DEFINE_GUID(nom, l, w1,
//!    w2, b1…b8)` et `DEFINE_GUIDSTRUCT("xxxxxxxx-…", nom)`), les émet en `pub const nom:
//!    GUID` dans `OUT_DIR/guids.rs` et met les `static` homonymes en liste de blocage.
//!    Un `static … : GUID` restant dans la sortie de bindgen fait échouer le build : la
//!    liste des GUID reste ainsi complète par construction. Les valeurs de référence sont
//!    vérifiées contre `cl.exe` par `tests/layout.rs` (`tools/sizeof-probe.c`).
//!
//! Les deux fichiers sont inclus par `src/bindings.rs`.

// Script de build (mode utilisateur, jamais dans le noyau) : les lints anti-panique du
// workspace y sont sans objet, une erreur arrête la compilation avec son message.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::unwrap_used
)]

use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow, bail};
use regex::Regex;
use wdk_build::BuilderExt;

/// Regex (bindgen ancre en `^…$`) des en-têtes dont les éléments sont autorisés :
/// insensible à la casse et au séparateur de chemin (`\` ou `/`).
const ALLOWLIST_HEADERS: &str = r"(?i).*[\\/](portcls|ks|ksmedia|punknown|drmk)\.h";

/// En-têtes d'où sont extraits les GUID (mêmes que l'allowlist ; `ks.h` et `ksmedia.h`
/// sont dans `shared`, les trois autres dans `km`).
const GUID_HEADERS: [&str; 5] = ["ks.h", "ksmedia.h", "punknown.h", "drmk.h", "portcls.h"];

/// GUID au format de `guiddef.h` (`Data1`, `Data2`, `Data3`, `Data4[8]`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

fn main() -> anyhow::Result<()> {
    println!("cargo::rerun-if-changed=wrapper.h");
    println!("cargo::rerun-if-changed=build.rs");

    wdk_build::configure_wdk_library_build_and_then(|config| {
        let out_dir = PathBuf::from(env::var("OUT_DIR")?);
        let wrapper = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("wrapper.h");

        let include_paths: Vec<PathBuf> = config.include_paths()?.collect();
        let guids = extraire_guids(&include_paths)?;
        fs::write(out_dir.join("guids.rs"), rendre_guids(&guids))?;

        let mut builder = bindgen::Builder::wdk_default(&config)?
            .header(wrapper.to_string_lossy())
            .allowlist_file(ALLOWLIST_HEADERS)
            .allowlist_recursively(true)
            // Les types NT que PortCls référence sortent par récursivité sous leur nom de
            // structure (`_DEVICE_OBJECT`, `_DRIVER_OBJECT`) : PortCls ne nomme que les
            // pointeurs (`PDEVICE_OBJECT`). Les alias `typedef` de `wdm.h` sont ajoutés
            // explicitement pour que `conduit-kmd` et les tests parlent le même nom que
            // `wdk-sys`. `DEVICE_CAPABILITIES` : paramètre de
            // `IAdapterPowerManagement::QueryDeviceCapabilities`, nommé par le trait
            // `portcls::AdapterPowerManagement`.
            .allowlist_type(
                "(DEVICE_OBJECT|DRIVER_OBJECT|IRP|UNICODE_STRING|IO_STACK_LOCATION|DEVICE_CAPABILITIES)",
            )
            .default_enum_style(bindgen::EnumVariation::ModuleConsts)
            .layout_tests(true)
            .generate_comments(false)
            // `DEFINE_DEVPROPKEY` (ksmedia.h) : mêmes `extern static` sans définition ;
            // inutiles au pilote, bloqués plutôt que laissés en symboles non résolus.
            .blocklist_var("DEVPKEY_.*")
            // Piège du WDK 26100 en mode C : `IPortClsVersion` ne recopie pas
            // `DEFINE_ABSTRACT_UNKNOWN()`, sa vtable C n'aurait qu'un slot au lieu de
            // quatre. Définie à la main dans `src/fixups.rs` (IUnknown + GetVersion).
            .blocklist_type("IPortClsVersionVtbl")
            // `IPortClsPower`, `IPortClsRuntimePower`, `IPortClsEtwHelper` : déclarées
            // pour C++ seulement (ni `THIS_` ni `IUnknown` en C) : slots et signatures
            // faux, exclues entièrement. À réécrire à la main si M1b en a besoin.
            .blocklist_item("IPortClsPower(Vtbl)?")
            .blocklist_item("PPORTCLSPOWER")
            .blocklist_item("IPortClsRuntimePower(Vtbl)?")
            .blocklist_item("PPORTCLSRUNTIMEPOWER")
            .blocklist_item("IPortClsEtwHelper(Vtbl)?")
            .blocklist_item("PPORTCLSETWHELPER");
        for nom in guids.keys() {
            builder = builder.blocklist_var(nom);
        }

        let bindings = builder.generate()?.to_string();
        verifier_aucun_guid_static(&bindings)?;
        fs::write(out_dir.join("portcls.rs"), bindings)?;
        Ok::<(), anyhow::Error>(())
    })
}

/// Lit les cinq en-têtes dans les chemins d'include du WDK et en extrait tous les GUID.
fn extraire_guids(include_paths: &[PathBuf]) -> anyhow::Result<BTreeMap<String, Guid>> {
    let mut guids = BTreeMap::new();
    for nom in GUID_HEADERS {
        let chemin = include_paths
            .iter()
            .map(|dir| dir.join(nom))
            .find(|p| p.is_file())
            .ok_or_else(|| anyhow!("{nom} introuvable dans les chemins d'include du WDK"))?;
        println!("cargo::rerun-if-changed={}", chemin.display());
        let texte = fs::read_to_string(&chemin)
            .with_context(|| format!("lecture de {}", chemin.display()))?;
        for (nom_guid, guid) in parser_guids(&texte, &chemin)? {
            if let Some(existant) = guids.insert(nom_guid.clone(), guid) {
                if existant != guid {
                    bail!("{nom_guid} défini deux fois avec des valeurs différentes ({nom})");
                }
            }
        }
    }
    Ok(guids)
}

/// Extrait les `DEFINE_GUID(nom, l, w1, w2, b1, …, b8)` (multi-lignes) et les
/// `DEFINE_GUIDSTRUCT("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx", nom)` d'un en-tête.
///
/// Les blocs `#if` ne sont pas interprétés : un GUID exclu par `NTDDI_VERSION` devient
/// une constante en trop, jamais une constante fausse. Les commentaires `//` sont
/// retirés ligne par ligne pour ne pas prendre un exemple commenté pour une définition.
fn parser_guids(texte: &str, chemin: &Path) -> anyhow::Result<Vec<(String, Guid)>> {
    let sans_commentaires: String = texte
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    let define_guid = Regex::new(r"(?s)\bDEFINE_GUID\s*\(\s*(\w+)\s*,([^)]*)\)")?;
    let define_guidstruct = Regex::new(
        r#"\bDEFINE_GUIDSTRUCT\s*\(\s*"([0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12})"\s*,\s*(\w+)\s*\)"#,
    )?;

    let mut resultat = Vec::new();
    for capture in define_guid.captures_iter(&sans_commentaires) {
        let nom = capture[1].to_string();
        let composantes: Vec<u64> = capture[2]
            .split(',')
            .map(|c| parser_entier_c(c.trim()))
            .collect::<anyhow::Result<_>>()
            .with_context(|| format!("DEFINE_GUID({nom}) dans {}", chemin.display()))?;
        if composantes.len() != 11 {
            bail!(
                "DEFINE_GUID({nom}) : {} composantes au lieu de 11 ({})",
                composantes.len(),
                chemin.display()
            );
        }
        let mut data4 = [0u8; 8];
        for (i, octet) in data4.iter_mut().enumerate() {
            *octet = u8::try_from(composantes[3 + i])
                .with_context(|| format!("DEFINE_GUID({nom}) : octet {i} hors u8"))?;
        }
        resultat.push((
            nom.clone(),
            Guid {
                data1: u32::try_from(composantes[0])
                    .with_context(|| format!("DEFINE_GUID({nom}) : Data1 hors u32"))?,
                data2: u16::try_from(composantes[1])
                    .with_context(|| format!("DEFINE_GUID({nom}) : Data2 hors u16"))?,
                data3: u16::try_from(composantes[2])
                    .with_context(|| format!("DEFINE_GUID({nom}) : Data3 hors u16"))?,
                data4,
            },
        ));
    }
    for capture in define_guidstruct.captures_iter(&sans_commentaires) {
        let nom = capture[2].to_string();
        let guid = parser_guid_texte(&capture[1])
            .with_context(|| format!("DEFINE_GUIDSTRUCT({nom}) dans {}", chemin.display()))?;
        resultat.push((nom, guid));
    }
    Ok(resultat)
}

/// Entier C tel qu'écrit dans les en-têtes : `0x1234L`, `0x0a`, `12`, avec suffixe `L`/`U`
/// éventuel.
fn parser_entier_c(texte: &str) -> anyhow::Result<u64> {
    let sans_suffixe = texte.trim_end_matches(['L', 'l', 'U', 'u']);
    let valeur = if let Some(hex) = sans_suffixe
        .strip_prefix("0x")
        .or_else(|| sans_suffixe.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        sans_suffixe.parse::<u64>()
    };
    valeur.with_context(|| format!("entier C invalide : « {texte} »"))
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` → GUID (les deux derniers groupes forment
/// `Data4`, octet par octet, dans l'ordre d'écriture).
fn parser_guid_texte(texte: &str) -> anyhow::Result<Guid> {
    let groupes: Vec<&str> = texte.split('-').collect();
    if groupes.len() != 5 {
        bail!("GUID textuel invalide : « {texte} »");
    }
    let mut data4 = [0u8; 8];
    let octets = format!("{}{}", groupes[3], groupes[4]);
    for (i, octet) in data4.iter_mut().enumerate() {
        *octet = u8::from_str_radix(&octets[2 * i..2 * i + 2], 16)?;
    }
    Ok(Guid {
        data1: u32::from_str_radix(groupes[0], 16)?,
        data2: u16::from_str_radix(groupes[1], 16)?,
        data3: u16::from_str_radix(groupes[2], 16)?,
        data4,
    })
}

/// Source Rust des constantes : `pub const NOM: GUID = GUID { … };` (le type `GUID` vient
/// des bindings, les deux fichiers étant inclus dans le même module).
fn rendre_guids(guids: &BTreeMap<String, Guid>) -> String {
    let mut source = String::from(
        "// GUID extraits des en-têtes du WDK par build.rs (DEFINE_GUID, DEFINE_GUIDSTRUCT).\n",
    );
    for (nom, g) in guids {
        let data4 = g
            .data4
            .iter()
            .map(|b| format!("{b:#04x}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            source,
            "pub const {nom}: GUID = GUID {{ Data1: {:#010x}, Data2: {:#06x}, Data3: {:#06x}, \
             Data4: [{data4}] }};",
            g.data1, g.data2, g.data3
        );
    }
    source
}

/// Échoue si bindgen a encore émis un `static … : GUID` : un GUID que l'extraction n'a
/// pas vu (nouvelle forme de macro dans un WDK futur) doit être traité, pas ignoré.
fn verifier_aucun_guid_static(bindings: &str) -> anyhow::Result<()> {
    let restants: Vec<&str> = bindings
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("pub static") && l.ends_with(": GUID;"))
        .collect();
    if restants.is_empty() {
        Ok(())
    } else {
        bail!(
            "GUID non extraits des en-têtes (restés en `extern static`) :\n{}",
            restants.join("\n")
        )
    }
}
