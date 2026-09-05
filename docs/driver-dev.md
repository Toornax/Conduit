# Pilote Windows : guide développeur

Comment installer le poste, construire `conduit-kmd`, le charger dans une VM et le
déboguer. Ce guide ne redit ni la conception ([driver-design.md](driver-design.md)) ni
les versions et pièges de `windows-drivers-rs` ([windows-drivers-rs.md](windows-drivers-rs.md)) :
il les cite. Les versions épinglées sont dans `packaging/windows/versions.json`, seule
source de vérité pour `setup-env.ps1` et la CI.

## 1. Installation du poste

### 1.1 En administrateur (une fois)

Dans un PowerShell **administrateur** :

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --add Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre --includeRecommended"
winget install --id Microsoft.WindowsWDK.10.0.26100 -e
winget install --id LLVM.LLVM -e --version 17.0.6 --force
winget install --id Rustlang.Rustup -e
winget install --id Git.Git -e
winget install --id Microsoft.WinDbg -e
```

- Les bibliothèques Spectre sont exigées par l'installeur du WDK, pas par le build.
- **LLVM 17.0.6 exactement** : `bindgen` (via `wdk-sys` 0.5.1) génère des bindings
  invalides avec LLVM 22 (`windows-drivers-rs` issue #659, corrigé sur `main` seulement).
  `--force` autorise winget à installer cette version à la place d'une plus récente.
- **Pas d'extension VSIX WDK** : elle ne sert qu'aux projets MSBuild C++ ; `wdk-build`
  trouve le WDK par le registre (`HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed
  Roots\KitsRoot10`).

### 1.2 En utilisateur : `setup-env.ps1`

```powershell
.\packaging\windows\setup-env.ps1          # installe ce qui ne demande pas d'élévation
.\packaging\windows\setup-env.ps1 -Check   # ne touche à rien, échoue si une version diffère
```

Le script installe (ou vérifie, avec `-Check`) : rustup et la toolchain
`1.96.1-x86_64-pc-windows-msvc` avec `rustfmt` et `clippy` ; `cargo-wdk` 0.1.1
(`cargo install cargo-wdk --version 0.1.1 --locked`, quelques minutes de compilation).
Il vérifie seulement, et affiche la commande winget à lancer en administrateur s'il
manque : les Build Tools 17 avec `link.exe` MSVC x64 (obligatoire : `rust-lld` produit
un `.sys` inutilisable) et le SDK 26100 ; le WDK 26100 (`KitsRoot10`, `km\portcls.h`,
`km\x64\portcls.lib`, `stampinf`, `inf2cat`, `devgen`) ; LLVM 17.0.6 (`libclang.dll`
dans `LIBCLANG_PATH` puis `C:\Program Files\LLVM\bin`, version lue sur `clang.exe`).
En cas de succès il liste toutes les versions vérifiées.

`LIBCLANG_PATH` n'est nécessaire que si LLVM n'est pas dans `C:\Program Files\LLVM`.

## 2. Construire le pilote

Le pilote vit dans le **workspace noyau** `drivers/windows` (ADR-012), indépendant du
workspace racine : `portcls-sys` (bindings, testable en mode utilisateur) et
`conduit-kmd` (le `.sys`, `cdylib` `no_std`). Sa toolchain (`rust-toolchain.toml`,
stable 1.96.1 MSVC), ses lints anti-panique (`Cargo.toml`) et `rustflags =
["-C", "target-feature=+crt-static"]` (`.cargo/config.toml`, exigé par `wdk-build`) lui
sont propres.

```powershell
.\drivers\windows\tools\check.ps1               # fmt, clippy -D warnings, test portcls-sys, build conduit-kmd
.\drivers\windows\tools\build.ps1               # cargo wdk build --profile dev
.\drivers\windows\tools\build.ps1 -Profile release
```

ou, depuis `drivers/windows` :

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p portcls-sys
cargo build -p conduit-kmd
cargo wdk build [--profile release]
```

Le tout fonctionne depuis un PowerShell ordinaire : `rustc` localise `link.exe` par le
registre de Visual Studio (comme `vswhere`), aucun `vcvars` n'est nécessaire. Ne lancez **pas** `cargo test -p conduit-kmd` :
`wdk-sys` 0.5.1 lie `ntoskrnl.lib` même en test (issue #502).

`cargo wdk build` enchaîne `cargo build -p conduit-kmd`, puis l'empaquetage :
copie de l'INF depuis le `.inx`, `stampinf` (`DriverVer`, `$ARCH$`), `inf2cat`,
création du certificat de test `WDRLocalTestCert` dans le magasin `WDRTestCertStore`
de l'utilisateur (`makecert`, une seule fois), signature du `.sys` et du `.cat`
(`signtool`), puis `infverif /w`. Résultat dans
`drivers\windows\target\<debug|release>\conduit_kmd_package\` :

```
conduit_kmd.sys  conduit_kmd.inf  conduit_kmd.cat  conduit_kmd.pdb  conduit_kmd.map  WDRLocalTestCert.cer
```

Conventions imposées par `cargo-wdk` 0.1.1 (le nom du crate y est pris avec des `_`) :

- la source de l'INF est **`conduit-kmd/conduit_kmd.inx`**, à côté du `Cargo.toml` du
  crate ; le binaire et le service s'appellent `conduit_kmd` ;
- `infverif /w` (exigences « Windows Driver ») refuse `%12%` : les fichiers vont dans le
  magasin des pilotes (`DefaultDestDir = 13`, `ServiceBinary = %13%\conduit_kmd.sys`),
  d'où le modèle `NT$ARCH$.10.0...16299` ;
- le dossier `target` doit rester dans `drivers/windows` : `wdk-build` remonte depuis
  `OUT_DIR` jusqu'au `Cargo.lock` du workspace (pas de `CARGO_TARGET_DIR` externe).

Le build de `wdk-sys` régénère les bindings de `ntddk.h` avec bindgen (`LLVM 17`), une
minute environ la première fois ; ensuite le cache Cargo suffit.

## 3. VM de test

Jamais de pilote de test sur la machine de développement : un bug = écran bleu, et le
mode `testsigning` affaiblit la machine. Les scripts du dépôt ne lancent jamais
`pnputil`, `devgen` ni `bcdedit` : ces commandes se tapent dans la VM.

### 3.1 Création (Hyper-V, Windows 11 Pro sur l'hôte)

VM de génération 2, Windows 11, 4 Go, 2 vCPU, **Secure Boot désactivé** (sinon
`testsigning` est refusé), un commutateur virtuel avec accès à l'hôte. Prendre un
point de contrôle « propre » après l'installation et y revenir après chaque plantage
inexpliqué. Partager le dossier de package par un partage SMB de l'hôte ou par
`Copy-VMFile`.

### 3.2 Préparation (dans la VM, PowerShell administrateur)

```powershell
bcdedit /set testsigning on
bcdedit /debug on
bcdedit /dbgsettings net hostip:<ip de l'hôte> port:50000 key:<clé>   # débogage noyau réseau
reg add HKLM\SYSTEM\CurrentControlSet\Control\CrashControl /v CrashDumpEnabled /t REG_DWORD /d 2 /f   # kernel dump
Restart-Computer
```

Puis importer le certificat de test dans *Trusted Root* et *Trusted Publishers* :

```powershell
certutil -addstore Root WDRLocalTestCert.cer
certutil -addstore TrustedPublisher WDRLocalTestCert.cer
```

### 3.3 Installation, création du nœud, retrait

```powershell
pnputil /add-driver conduit_kmd.inf /install
& "C:\Program Files (x86)\Windows Kits\10\Tools\10.0.26100.0\x64\devgen.exe" /add /hardwareid "Root\ConduitCable"
```

`devgen` (WDK 26100, à copier dans la VM avec le package) crée le périphérique racine
que l'INF cible ; le gestionnaire de périphériques doit montrer *Conduit Virtual Audio
Cable* sous *Contrôleurs audio, vidéo et jeu*. Retrait :

```powershell
devgen /remove <id renvoyé par /add>
pnputil /enum-drivers                      # repérer oemN.inf du fournisseur Conduit
pnputil /delete-driver oemN.inf /uninstall /force
```

Un cycle de charge/décharge se boucle avec `devgen /add` puis `devgen /remove`
(critère de M1a-02 : 100 cycles sans erreur).

### 3.4 Driver Verifier et journal

`verifier /standard /driver conduit_kmd.sys` puis redémarrage (retour : `verifier /reset`).
Les messages `wdk::println!` passent par `DbgPrint` : DebugView (« Capture Kernel ») ou la
fenêtre de commande WinDbg.

## 4. Débogage noyau et plantages

Sur l'hôte, WinDbg → *Attach to kernel* → *Net*, même port et même clé qu'au §3.2 ;
la VM s'arrête au démarrage jusqu'à la connexion si `bcdedit /set {default} bootdebug on`.
Commandes utiles : `!analyze -v` après un bug check, `!verifier 3` pour l'état de Driver
Verifier, `lm m conduit*` pour vérifier que le module et son `.pdb` sont chargés
(`.sympath+ <dossier du package>`), `bp conduit_kmd!DriverEntry`.

Une panique Rust se traduit par le bug check **`0xE0000001`** (`conduit-kmd/src/panic.rs`,
`KeBugCheckEx`) : `!analyze -v` l'affiche avec ses quatre paramètres.

Après chaque plantage, copier `C:\Windows\MEMORY.DMP` (ou `C:\Windows\Minidump\*.dmp`)
de la VM vers l'hôte et l'ouvrir dans WinDbg (`.sympath` sur le dossier du package).

## 5. Dépannage

| Symptôme | Cause et remède |
|---|---|
| `setup-env.ps1` : « LLVM 22.x, attendu 17.0.6 » | winget a installé le dernier LLVM. `winget install --id LLVM.LLVM -e --version 17.0.6 --force` (admin). Les bindings de `wdk-sys` 0.5.1 sont cassés avec LLVM 22. |
| bindgen : `Unable to find libclang` | `libclang.dll` hors de `C:\Program Files\LLVM\bin` : définir `LIBCLANG_PATH`. |
| `wdk-build` : `StaticCrtNotEnabled` | Build lancé hors de `drivers/windows`, ou `.cargo/config.toml` ignoré (`RUSTFLAGS` défini dans l'environnement remplace `rustflags`). Lancer depuis le workspace noyau, sans `RUSTFLAGS`. |
| `wdk-build` : « WDKContentRoot should be able to be detected » | Clé `KitsRoot10` absente (WDK installé autrement que par winget). Définir `$env:WDKContentRoot = "C:\Program Files (x86)\Windows Kits\10\"` (barre finale comprise). |
| `wdk-build` : « a Cargo.lock file should exist in the same directory as the top-level Cargo.toml » | `CARGO_TARGET_DIR` pointe hors du workspace : le retirer. |
| `cargo wdk build` : « Missing .inx file » | Le fichier doit s'appeler `conduit_kmd.inx` (underscore) dans `conduit-kmd/`. |
| `infverif` : ERROR 1322 « not isolated to DIRID 13 » | L'INF copie vers `%12%` : rester sur `%13%` (§2). |
| `Une stratégie de contrôle d'application a bloqué ce fichier (os error 4551)` à l'exécution d'un `build-script-build` ou d'un test | **Smart App Control** (Windows 11) bloque les binaires fraîchement compilés qu'il ne connaît pas. Journal : *Microsoft-Windows-CodeIntegrity/Operational*, événements 3077/3118. Aucune exclusion possible : le désactiver (*Sécurité Windows → Contrôle des applications et du navigateur → Smart App Control*, irréversible) ou développer dans une VM. |
| Windows PowerShell 5.1 affiche `NativeCommandError` sur des lignes `Finished …` | Bruit : cargo écrit sa progression sur stderr et `2>&1` la transforme en erreur. Ne pas rediriger, ou utiliser `pwsh` 7. |
| `signtool verify` : « terminated in a root certificate which is not trusted » | Normal sur l'hôte : `WDRLocalTestCert` est auto-signé. Importer le `.cer` dans la VM (§3.2). |

## 6. Dépôt

Cloner dans un chemin local court (`C:\dev\conduit` ; éviter `\\wsl.localhost` et
`/mnt/c`, lents ou refusés par Cargo). Les hooks git-hooks.nix n'existent pas sous
Windows : respecter à la main le format des messages (`type(scope): description`,
scopes de ROADMAP) et lancer `cargo fmt --all` dans les deux workspaces avant chaque
commit.

Validation du workspace racine sous Windows : `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo test --workspace --all-features` (le crate `conduit-backend-pipewire` est vide
hors Linux ; les tests de socket Unix sont remplacés par le named pipe). Le workspace
noyau se valide par `drivers\windows\tools\check.ps1` (§2).

## 7. Historique

### 7.1 Passation Linux → Windows (2026-09-05)

Ce que la session Linux avait validé pour Windows avant M1a-01 :

- tous les crates utilisateur compilent en croisé `x86_64-pc-windows-gnu` (check Nix
  `cross-windows`) ; les chemins spécifiques (`named pipe` dans `conduitd::ipc` et
  `conduit_protocol::client`, `SetThreadPriority` dans `conduit_backend::rt`) n'avaient
  **jamais été exécutés** sous Windows : premiers points à tester
  (`cargo test -p conduitd -p conduitctl`) ;
- `packaging/windows/versions.json` fixait Rust 1.96.1, Build Tools 17, SDK/WDK 26100 ;
  M1a-01 y a ajouté LLVM 17.0.6, `cargo-wdk` 0.1.1 et les crates `wdk-*`.

La note de passation prévoyait l'extension VSIX du WDK et `cargo install cargo-wdk`
sans version : abandonnés (VSIX inutile, version épinglée).

### 7.2 Tâches à enchaîner (ROADMAP, M1a)

1. **M1a-02** pilote WDM minimal (`AddDevice`, INF, catalogue de test) chargé et
   déchargé 100 fois dans la VM (§3.3).
2. **M1a-03** à **M1a-05** `portcls-sys` : bindgen en mode C sur `portcls.h`/`ks.h`/
   `ksmedia.h` (vtables plates, driver-design §2.2), objets COM sûrs, enveloppes
   `IMiniportWaveRT`/`IMiniportWaveRTStream`/`IPortWaveRT` avec tests en mode utilisateur.
3. **M1a-06** à **M1a-10** adaptateur, miniports rendu/capture en boucle locale, INF
   complet (KS, `wdmaudio.inf`, endpoints « Conduit 1 »), outil de test de boucle WASAPI.
4. **M1a-11/12** Driver Verifier 1 h, ADR sur le résultat du spike (porte de décision
   Rust / repli C++).

En parallèle côté utilisateur (hors code) : certificat EV et compte Hardware Dev Center
pour la signature d'attestation (délai de plusieurs semaines, SPEC §10).
