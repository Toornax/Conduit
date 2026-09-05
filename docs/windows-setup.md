# Poste Windows : installation et passation (M1a)

Note de passation pour une session de travail sous Windows (Claude Code Windows ou
développeur). État du dépôt au 2026-09-05 : M0 clos (`v0.1.0-core`), M3 (PipeWire) et
M2 (GUI) en cours sur Linux. Le pilote Windows (M1a) n'a pas commencé.

## 1. Installer (PowerShell administrateur)

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --add Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre --includeRecommended"
winget install --id Microsoft.WindowsWDK.10.0.26100 -e
winget install --id LLVM.LLVM -e
winget install --id Rustlang.Rustup -e
winget install --id Git.Git -e
winget install --id Microsoft.WinDbg -e
```

Puis :

1. Installer l'extension VSIX du WDK dans les Build Tools :
   `C:\Program Files (x86)\Windows Kits\10\Vsix\VS2022\10.0.26100.0\WDK.vsix`
   (double-clic ou `VSIXInstaller.exe`). Sans elle, `windows-drivers-rs` ne trouve pas les
   en-têtes.
2. `rustup toolchain install 1.96.1-x86_64-pc-windows-msvc --component rustfmt clippy`
   (version épinglée dans `packaging/windows/versions.json`).
3. `cargo install cargo-wdk` (outillage de build et d'empaquetage de
   `windows-drivers-rs` ; voir son README pour la toolchain exacte exigée par le crate
   du pilote, à épingler dans `drivers/windows/rust-toolchain.toml`).
4. Vérifier : `.\packaging\windows\setup-env.ps1 -Check` (à étendre pour le WDK, LLVM et
   `cargo-wdk` : tâche M1a-01).

Variables utiles : `LIBCLANG_PATH=C:\Program Files\LLVM\bin` pour `bindgen`.

## 2. VM de test (obligatoire pour charger le pilote)

Jamais de pilote de test sur la machine de développement (un bug = écran bleu).

- Hyper-V (Windows 11 Pro) : VM Windows 11, 4 Go, 2 vCPU, snapshot « propre ».
- Dans la VM : `bcdedit /set testsigning on`, redémarrer.
- Débogage noyau réseau : dans la VM `bcdedit /debug on` puis
  `bcdedit /dbgsettings net hostip:<ip hôte> port:50000 key:<clé>` ; sur l'hôte WinDbg
  « Attach to kernel » → Net, même port et clé.
- Collecte automatique des dumps : `HKLM\SYSTEM\CurrentControlSet\Control\CrashControl`
  `CrashDumpEnabled = 2` (kernel dump) dans la VM ; copier `C:\Windows\MEMORY.DMP` après
  chaque plantage.
- Installation du pilote de test : `pnputil /add-driver conduit.inf /install`, retrait :
  `pnputil /delete-driver oem<N>.inf /uninstall /force`.

## 3. Dépôt

Cloner dans `C:\dev\conduit` (éviter `\\wsl.localhost` et `/mnt/c`, lents ou refusés par
Cargo). Les hooks git-hooks.nix n'existent pas sous Windows : respecter à la main le
format des messages (`type(scope): description`, scopes de ROADMAP) et lancer
`cargo fmt --all` avant chaque commit.

Validation Windows : `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
--all-features -- -D warnings`, `cargo test --workspace --all-features`
(le crate `conduit-backend-pipewire` est vide hors Linux ; les tests de socket Unix
sont remplacés par le named pipe).

## 4. Tâches à enchaîner (ROADMAP)

1. **M1a-01** environnement épinglé : étendre `setup-env.ps1` (WDK, VSIX, LLVM,
   `cargo-wdk`), `drivers/windows/rust-toolchain.toml`, `docs/driver-dev.md`, job CI
   Windows compilant le crate vide du pilote.
2. **M1a-02** pilote WDM minimal (`DriverEntry`, `AddDevice`, `Unload`, INF, catalogue de
   test) chargé/déchargé 100 fois dans la VM.
3. **M1a-03** à **M1a-05** `portcls-sys` : bindgen sur `portcls.h`/`ksmedia.h`, puis
   vtables COM à la main (`IUnknown`, `IAdapterPowerManagement`, `IMiniportTopology`,
   `IMiniportWaveRT`, `IMiniportWaveRTStream`, `IPortWaveRT`) avec tests d'offsets.
4. **M1a-06** à **M1a-10** adaptateur, miniports rendu/capture en boucle locale, INF
   complet, outil de test de boucle WASAPI.
5. **M1a-11/12** Driver Verifier 1 h, ADR-009 (résultat du spike, porte de décision).

En parallèle côté utilisateur (hors code) : certificat EV et compte Hardware Dev Center
pour la signature d'attestation (délai de plusieurs semaines, SPEC §10).

## 5. Ce que la session Linux a validé pour Windows

- Tous les crates utilisateur compilent en croisé `x86_64-pc-windows-gnu` (check Nix
  `cross-windows`) ; les chemins spécifiques (`named pipe` dans `conduitd::ipc` et
  `conduit_protocol::client`, `SetThreadPriority` dans `conduit_backend::rt`) n'ont
  **jamais été exécutés** sous Windows : premiers points à tester
  (`cargo test -p conduitd -p conduitctl`).
- `packaging/windows/versions.json` fixe Rust 1.96.1, Build Tools 17, SDK/WDK 26100.
