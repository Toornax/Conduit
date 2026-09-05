# Fiche outillage : `windows-drivers-rs` et `cargo-wdk`

Synthèse de la recherche du 2026-09-05 (sources : dépôt GitHub `microsoft/windows-drivers-rs`,
crates.io, learn.microsoft.com). À relire avant M1a-01 et M1a-03. Ce qui n'a pas été
compilé est marqué « non vérifié ».

## Versions épinglées

| Élément | Version | Pourquoi |
|---|---|---|
| Rust | stable 1.96.1 MSVC (celui du dépôt) | MSRV `wdk-build` 0.5.1 = 1.85 ; CI du projet en stable ; pas de `-Z build-std` |
| `wdk-sys`, `wdk-build`, `wdk-macros` | 0.5.1 (2025-11-14) | dernières publiées, celles des templates `cargo wdk new` |
| `wdk`, `wdk-alloc`, `wdk-panic` | 0.4.1 (2025-11-14) | idem |
| `cargo-wdk` | 0.1.1 (2025-11-14) | commandes `new`, `build` ; pas de `test` ni `deploy` |
| bindgen (build-dep de `portcls-sys`) | `^0.71` | contrainte de `wdk-build` 0.5.1 |
| LLVM / libclang | **17.0.6** | seule version testée en CI ; **LLVM 22 casse les bindings** avec les crates publiés (issue #659, corrigé sur `main` seulement, non publié) ; LLVM 18 casse ARM64 |
| WDK | 10.0.26100 (≥ .4204 ; le QFE .3323 avait un bug, issue #424) | même build que le SDK, obligatoire |
| Édition Rust du workspace noyau | 2024 | imposée depuis `wdk` 0.4 |

Le dépôt amont est actif (commits jusqu'en 2026-08) mais rien n'est publié depuis
2025-11 : `KeBugCheckEx` dans `wdk-panic`, correctif LLVM 22, gating des `#[link]` en
test, `-fno-builtin` sont sur `main` seulement. On reste sur les versions publiées ;
revoir à la prochaine publication.

## Prérequis machine

- Build Tools 2022 avec MSVC, SDK 26100, bibliothèques Spectre (exigées par l'installeur
  WDK, pas par `wdk-build`). `link.exe` obligatoire : `rust-lld` produit un `.sys`
  inutilisable (issue #182).
- WDK 26100 par winget (`Microsoft.WindowsWDK.10.0.26100`). `wdk-build` le trouve par le
  registre (`Installed Roots\KitsRoot10`) ou `WDKContentRoot`. **L'extension VSIX WDK
  n'est pas nécessaire** (elle sert aux projets MSBuild C++).
- LLVM 17.0.6 : `winget install --id LLVM.LLVM -e --version 17.0.6 --force`. bindgen
  cherche `libclang` via `LIBCLANG_PATH` puis le PATH.
- Outils du WDK utilisés par `cargo wdk build` : `stampinf`, `inf2cat`, `infverif`,
  `signtool`, `makecert`, `certmgr` (dans `bin\10.0.26100.0\x64` et `x86`) ; `devgen`
  et `devcon` dans `Tools\10.0.26100.0\x64`.

## Squelette d'un crate WDM

```toml
[package]
name = "conduit-kmd"
edition = "2024"

[package.metadata.wdk.driver-model]
driver-type = "WDM"

[lib]
crate-type = ["cdylib"]

[build-dependencies]
wdk-build = "0.5.1"

[dependencies]
wdk = "0.4.1"
wdk-alloc = "0.4.1"
wdk-sys = "0.5.1"

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
lto = true
```

`.cargo/config.toml` : `[build] rustflags = ["-C", "target-feature=+crt-static"]`
(sinon `wdk-build` refuse : `StaticCrtNotEnabled`).
`build.rs` : `fn main() -> Result<(), wdk_build::ConfigError> { wdk_build::configure_wdk_binary_build() }`.
`src/lib.rs` : `#![no_std]`, `extern crate alloc`, `#[global_allocator] static A: wdk_alloc::WdkAllocator`,
`#[unsafe(export_name = "DriverEntry")] pub unsafe extern "system" fn driver_entry(driver: PDRIVER_OBJECT, registry_path: PCUNICODE_STRING) -> NTSTATUS`.
Flags émis par `wdk-build` : `/DRIVER /NODEFAULTLIB /SUBSYSTEM:NATIVE /KERNEL /ENTRY:DriverEntry
/INTEGRITYCHECK /NXCOMPAT /DYNAMICBASE /OPT:REF,ICF /MANIFEST:NO`, liaison de `ntoskrnl`,
`hal`, `wmilib`, `BufferOverflowFastFailK`.

`cargo wdk build [--profile release]` produit `target\<profile>\conduit-kmd-package\` :
`.sys`, `.pdb`, `.map`, `.inf` (depuis le **`.inx`** obligatoire, issue #518), `.cat`,
`WDRLocalTestCert.cer` ; signature de test automatique (`signtool /s WDRTestCertStore`).

## Pièges connus

- `wdk-alloc` : `ExAllocatePool2(POOL_FLAG_NON_PAGED, …, 'rust')`, IRQL ≤ DISPATCH,
  **ignore les alignements > défaut** (PR #353 en attente). Tampons audio via MDL.
- `wdk-panic` 0.4.1 = `loop {}`. Écrire son propre `#[panic_handler]` (`KeBugCheckEx`).
- `wdk-sys` 0.5.1 lie les bibliothèques noyau même en `cargo test` (issue #502) : ne pas
  tester `conduit-kmd` en mode utilisateur ; tester `conduit-kmd-core` et `portcls-sys`
  (qui ne dépend pas de `wdk-sys`).
- Exports parasites `_fltused`, `__CxxFrameHandler3` (issue #294) : contournement `/DEF:`.
- `_NT_TARGET_VERSION` non configurable (issue #149) : en-têtes à la version par défaut.
- Aucun `portcls.h`, `ks.h`, `ksmedia.h` dans `wdk-sys` (features : `gpio`, `hid`,
  `parallel-ports`, `spb`, `storage`, `usb`, `test-stubs`). Aucun pilote PortCls/WaveRT
  en Rust connu publiquement : Conduit essuie les plâtres, SYSVAD (C++) reste la
  référence de comportement.

## Recette bindgen pour `portcls-sys` (non vérifiée)

```rust
// build.rs
use wdk_build::BuilderExt;
fn main() -> anyhow::Result<()> {
    wdk_build::configure_wdk_library_build_and_then(|config| {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("portcls.rs");
        bindgen::Builder::wdk_default(&config)?
            .header_contents("portcls-input.h", concat!(
                "#define PUT_GUIDS_HERE\n#include <initguid.h>\n",
                "#include <ntddk.h>\n#include <ks.h>\n#include <ksmedia.h>\n",
                "#include <punknown.h>\n#include <drmk.h>\n",
                "#define INTERFACE void\n#include <portcls.h>\n"))
            .allowlist_file(".*(portcls|ks|ksmedia|punknown|drmk)\\.h")
            .generate()?
            .write_to_file(out)?;
        Ok::<_, anyhow::Error>(())
    })
}
```

En mode C, `DECLARE_INTERFACE_`/`STDMETHOD_` de `basetyps.h` donnent
`struct IMiniportWaveRT { const IMiniportWaveRTVtbl *lpVtbl; }` avec une vtable plate
incluant les méthodes héritées : disposition COM exacte sans support C++ de bindgen.
`THIS_` s'expanse en `INTERFACE *This`, d'où le `#define INTERFACE void` juste avant
`portcls.h` (après les autres en-têtes qui le définissent puis l'annulent eux-mêmes).
Les fonctions `FORCEINLINE` ne sont pas générées (`generate_inline_functions` désactivé).

## Test dans la VM

`bcdedit /set testsigning on` (Secure Boot désactivé si refus), importer le `.cer` dans
*Trusted Root* et *Trusted Publishers*, `pnputil /add-driver conduit.inf /install`,
`devgen /add /hardwareid "Root\ConduitCable"`. Driver Verifier : « Create Standard
Settings » sur `conduit-kmd.sys`, WinDbg `!analyze -v`, `!verifier 3`. Journal :
`wdk::println!` → `DbgPrint` (DebugView « Capture Kernel »).
