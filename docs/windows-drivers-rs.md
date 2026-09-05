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

## Recette bindgen pour `portcls-sys` (vérifiée, M1a-03, WDK 10.0.26100.0)

Source de vérité : `drivers/windows/portcls-sys/build.rs` et `wrapper.h`. Squelette :

```rust
// build.rs
use wdk_build::BuilderExt;
fn main() -> anyhow::Result<()> {
    wdk_build::configure_wdk_library_build_and_then(|config| {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
        bindgen::Builder::wdk_default(&config)?
            .header("wrapper.h") // ntddk, windef, mmreg (NOBITMAP), ks, ksmedia, punknown,
                                 // drmk, `#define INTERFACE void`, portcls
            .allowlist_file(r"(?i).*[\/](portcls|ks|ksmedia|punknown|drmk)\.h")
            .allowlist_recursively(true)
            .allowlist_type("(DEVICE_OBJECT|DRIVER_OBJECT|IRP|UNICODE_STRING)")
            .default_enum_style(bindgen::EnumVariation::ModuleConsts)
            .layout_tests(true)
            .blocklist_var("DEVPKEY_.*")           // extern static sans définition
            .blocklist_type("IPortClsVersionVtbl") // 1 slot au lieu de 4 : fixups.rs
            .blocklist_item("IPortCls(Power|RuntimePower|EtwHelper)(Vtbl)?") // C++ seulement
            // + blocklist_var de chaque GUID, émis à part en `pub const` (voir plus bas)
            .generate()?
            .write_to_file(out.join("portcls.rs"))?;
        Ok::<_, anyhow::Error>(())
    })
}
```

Ce qui a été constaté (et diffère de la recette initiale) :

- **Mode C confirmé** : `DECLARE_INTERFACE_`/`STDMETHOD_` de `basetyps.h` donnent
  `struct IMiniportWaveRT { IMiniportWaveRTVtbl *lpVtbl; }` avec une vtable plate
  incluant les méthodes héritées, dans l'ordre du header : disposition COM exacte, sans
  support C++ de bindgen. `THIS_` s'expanse en `INTERFACE *This`, d'où le
  `#define INTERFACE void` juste avant `portcls.h` (après les autres en-têtes, qui le
  définissent puis l'annulent eux-mêmes). Les tailles de 14 vtables et 27 structures
  et la valeur de 11 GUID concordent avec `cl.exe` (`tests/layout.golden`).
- **Ordre des en-têtes** : `portcls.h` a besoin de `KSDATAFORMAT_WAVEFORMATEX`, que
  `ksmedia.h` ne définit que si `WAVEFORMATEX` existe : `windef.h` puis `mmreg.h`
  (`NOBITMAP`) **avant** `ks.h`/`ksmedia.h`, sinon `portcls.h` ne compile pas.
- **GUID** : `PUT_GUIDS_HERE`/`initguid.h` est inutile, bindgen n'évalue pas les
  initialiseurs de structure ; tous les `DEFINE_GUID`/`DEFINE_GUIDSTRUCT` sortent en
  `extern static` (symboles à fournir par l'éditeur de liens). `build.rs` analyse les
  en-têtes avec une regex, émet `pub const NOM: GUID = …` dans `OUT_DIR/guids.rs`,
  bloque les `static` homonymes, et échoue si un `static … : GUID` subsiste.
- `IPortClsVersion` ne recopie pas `DEFINE_ABSTRACT_UNKNOWN()` (vtable à un slot) ;
  `IPortClsPower`, `IPortClsRuntimePower`, `IPortClsEtwHelper` n'ont ni `THIS_` ni
  `IUnknown` en C : la première est réécrite dans `src/fixups.rs`, les trois autres
  sont exclues.
- Les macros à `sizeof` (`PORT_CLASS_DEVICE_EXTENSION_SIZE`) ne sont pas générées :
  recopiées dans `fixups.rs`. Les fonctions `FORCEINLINE` ne le sont pas non plus
  (`generate_inline_functions` désactivé).
- Les `layout_tests` de bindgen indexent un tableau dans un bloc `const` : autoriser
  `clippy::indexing_slicing` et `clippy::arithmetic_side_effects` dans le module qui
  fait l'`include!`, pas ailleurs.
- Sortie : ~41 000 lignes (1,5 Mo) plus 639 GUID ; une trentaine de secondes à la
  première compilation, cache Cargo ensuite (`rerun-if-changed` sur `wrapper.h`,
  `build.rs` et les cinq en-têtes).

## Test dans la VM

`bcdedit /set testsigning on` (Secure Boot désactivé si refus), importer le `.cer` dans
*Trusted Root* et *Trusted Publishers*, `pnputil /add-driver conduit.inf /install`,
`devgen /add /hardwareid "Root\ConduitCable"`. Driver Verifier : « Create Standard
Settings » sur `conduit-kmd.sys`, WinDbg `!analyze -v`, `!verifier 3`. Journal :
`wdk::println!` → `DbgPrint` (DebugView « Capture Kernel »).
