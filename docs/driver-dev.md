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
.\drivers\windows\tools\check.ps1               # fmt, clippy -D warnings, test portcls-sys, build conduit-kmd, golden à jour
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
`wdk-sys` 0.5.1 lie `ntoskrnl.lib` même en test (issue #502). `portcls-sys` ne dépend
pas de `wdk-sys` (ses bindings régénèrent les types NT dont PortCls a besoin) :
`cargo test -p portcls-sys` reste en mode utilisateur.

**Golden de disposition** (`portcls-sys\tests\layout.golden`) : `tests\layout.rs`
compare les `size_of` et les GUID des bindings à ce fichier, produit par `cl.exe` sur
les mêmes en-têtes du WDK (`portcls-sys\tools\sizeof-probe.c`). Il est commité ;
`check.ps1` le régénère dans un dossier temporaire et échoue s'il diffère. À régénérer
(puis à commiter) après toute modification du probe, de `wrapper.h`, ou changement de
version du WDK :

```powershell
.\drivers\windows\tools\regen-layout.ps1     # cl.exe via vswhere, WDK de versions.json
```

Pour ajouter une mesure : une ligne `TAILLE(...)`/`GUID_*(...)` dans le probe **et** une
entrée dans les tables `tailles!`/`guids!` de `tests\layout.rs` (le test échoue si les
deux listes de noms diffèrent), puis régénération.

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
`pnputil`, `devgen` ni `bcdedit` **sur l'hôte** : `vm-prepare.ps1` et `vm-cycle.ps1` les
exécutent dans l'invité par PowerShell Direct (`Invoke-Command -VMName`).

Trois scripts dans `drivers\windows\tools\`, à lancer dans l'ordre depuis un PowerShell
**administrateur** de l'hôte (Hyper-V exige l'élévation ; chacun le vérifie et s'arrête
avec un message clair sinon). Prérequis : Hyper-V activé (Windows 11 Pro), le
commutateur `Default Switch` (créé par Hyper-V, NAT vers l'hôte), une **ISO officielle
de Windows 11** (microsoft.com, « Télécharger l'image de disque »), le WDK 26100 sur
l'hôte (pour `devgen.exe`) et le paquet produit par `build.ps1`.

| Script | Rôle | Une fois / à chaque fois |
|---|---|---|
| `vm-new.ps1 -IsoPath <iso>` | crée et démarre la VM | une fois |
| `vm-prepare.ps1 -Credential <cred>` | `testsigning`, débogueur réseau, vidages, point de contrôle `propre` | une fois (ou après réinstallation) |
| `vm-cycle.ps1 -Credential <cred> [-Count 100]` | copie le paquet, l'installe et le retire N fois | à chaque build à valider |

Les fonctions d'analyse partagées (`vm-common.psm1` : identifiant d'instance de `devgen`,
`oemN.inf` de `pnputil /enum-drivers`, clé kdnet, résumé des durées) ont des tests Pester
dans `tools\tests\` (`Invoke-Pester drivers\windows\tools\tests`, syntaxe Pester 3/4 livrée
avec Windows).

### 3.1 Création : `vm-new.ps1`

```powershell
.\drivers\windows\tools\vm-new.ps1 -IsoPath C:\iso\Win11_x64.iso   # [-Name ConduitTest] [-MemoryGB 4] [-Cpu 2] [-DiskGB 64] [-Path <dossier>]
```

VM de **génération 2**, 4 Go, 2 vCPU (minimums de Windows 11), VHDX dynamique de 64 Go,
`Default Switch`, ISO en premier périphérique de démarrage, vTPM (l'installeur de
Windows 11 l'exige), **Secure Boot désactivé** (sinon `bcdedit /set testsigning on` est
refusé), points de contrôle automatiques désactivés, interface de services invité
activée. Le script démarre la VM, ouvre `vmconnect` et affiche la marche à suivre :
appuyer sur une touche pour démarrer sur l'ISO, installer Windows 11 (Pro), créer un
**compte local** `test` administrateur sans compte Microsoft (Maj+F10 puis
`OOBE\BYPASSNRO` si l'installeur l'impose), puis passer à `vm-prepare.ps1`.

### 3.2 Préparation : `vm-prepare.ps1`

```powershell
$cred = Get-Credential test
.\drivers\windows\tools\vm-prepare.ps1 -Name ConduitTest -Credential $cred   # [-HostIp <ip>] [-DebugPort 50000] [-DebugKey <clé>]
```

Dans l'invité, par PowerShell Direct : `bcdedit /set testsigning on`, `bcdedit /debug on`,
`bcdedit /dbgsettings net hostip:<hôte> port:50000 key:<clé>` (hôte = adresse de
`vEthernet (Default Switch)` par défaut ; clé générée et **affichée à la fin**, à
conserver pour WinDbg), `CrashDumpEnabled = 2` (vidage noyau complet) et `AutoReboot = 1`
dans `HKLM\SYSTEM\CurrentControlSet\Control\CrashControl`, veille, écran et hibernation
désactivés. Puis redémarrage de l'invité, vérification que `testsigning` est bien actif
(`bcdedit /enum`) et point de contrôle **`propre`** (`Checkpoint-VM`). Après un plantage
inexpliqué : `Restore-VMCheckpoint -VMName ConduitTest -Name propre -Confirm:$false`.

### 3.3 Cycle de chargement (M1a-02) : `vm-cycle.ps1`

```powershell
.\drivers\windows\tools\build.ps1                                            # paquet target\debug\conduit_kmd_package
.\drivers\windows\tools\vm-cycle.ps1 -Name ConduitTest -Credential $cred -Count 100   # [-Package <dossier>] [-Profile dev|release]
```

Le script copie le paquet et `devgen.exe` (WDK de l'hôte, version de `versions.json`)
dans `C:\ConduitTest` de l'invité, importe `WDRLocalTestCert.cer` dans *Root* et
*TrustedPublisher* de la machine invitée, retire les restes d'une exécution interrompue
(périphériques `Root\ConduitCable`, paquets `oemN.inf` de `conduit_kmd.inf`), puis répète
`Count` fois, en affichant `i/Count OK` et la durée :

1. `pnputil /add-driver conduit_kmd.inf /install` ;
2. `devgen /add /hardwareid "Root\ConduitCable"` (identifiant d'instance lu dans la
   sortie) : PnP appelle `AddDevice` puis `IRP_MN_START_DEVICE` → `StartDevice` ;
3. attente de `Get-PnpDevice -InstanceId <id>` en état `OK` (30 s au plus, sinon échec
   avec le code de problème) ;
4. `devgen /remove <id>` : `IRP_MN_REMOVE_DEVICE`, l'objet de périphérique disparaît ;
5. `pnputil /delete-driver oemN.inf /uninstall /force` (`oemN.inf` retrouvé par
   `pnputil /enum-drivers`) : le service est supprimé et le `.sys` déchargé ;
6. vérification qu'aucun événement `Kernel-PnP` de niveau erreur ni `BugCheck`
   (1001, 6008) n'est apparu dans l'invité depuis le début.

Tout `devgen`/`pnputil` dont le code de retour n'est pas 0 arrête le script avec sa
sortie. À la fin (même après un plantage : l'invité redémarre seul et le script se
reconnecte), un `C:\Windows\MEMORY.DMP` ou un `C:\Windows\Minidump\*.dmp` plus récent que
le début est rapatrié dans `drivers\windows\target\dumps\` et le script **échoue**.
Sortie : cycles réussis et durée moyenne, minimale et maximale d'un cycle. Le critère de
M1a-02 est `vm-cycle : 100 cycles sans erreur.`

À la main, dans la VM, les mêmes commandes se tapent depuis `C:\ConduitTest` :

```powershell
pnputil /add-driver package\conduit_kmd.inf /install
.\devgen.exe /add /hardwareid "Root\ConduitCable"      # imprime l'identifiant d'instance
.\devgen.exe /remove <id>
pnputil /enum-drivers                                  # repérer oemN.inf du fournisseur Conduit
pnputil /delete-driver oemN.inf /uninstall /force
```

Le gestionnaire de périphériques doit montrer *Conduit Virtual Audio Cable* sous
*Contrôleurs audio, vidéo et jeu* entre `/add` et `/remove` (sans endpoint audio avant
M1a-06).

### 3.4 Driver Verifier et journal

`verifier /standard /driver conduit_kmd.sys` puis redémarrage (retour : `verifier /reset`).
Les messages `kmd_log!` (`conduit-kmd/src/log.rs`, `wdk::println!` → `DbgPrint`, profil
`dev` seulement) passent par le débogueur : fenêtre de commande WinDbg ou DebugView
(« Capture Kernel ») dans l'invité. Attendu à chaque cycle : `DriverEntry`, `AddDevice`,
`StartDevice`, `DriverUnload`.

## 4. Débogage noyau et plantages

Sur l'hôte, WinDbg → *Attach to kernel* → *Net*, même port et même clé qu'au §3.2 ;
la VM s'arrête au démarrage jusqu'à la connexion si `bcdedit /set {default} bootdebug on`.
Commandes utiles : `!analyze -v` après un bug check, `!verifier 3` pour l'état de Driver
Verifier, `lm m conduit*` pour vérifier que le module et son `.pdb` sont chargés
(`.sympath+ <dossier du package>`), `bp conduit_kmd!DriverEntry`.

Une panique Rust se traduit par le bug check **`0xE0000001`** (`conduit-kmd/src/panic.rs`,
`KeBugCheckEx`) : `!analyze -v` l'affiche avec ses quatre paramètres.

Après un plantage pendant `vm-cycle.ps1`, le script rapatrie `C:\Windows\MEMORY.DMP` (ou
`C:\Windows\Minidump\*.dmp`) dans `drivers\windows\target\dumps\` ; sinon, copier le
fichier de la VM vers l'hôte. L'ouvrir dans WinDbg (`.sympath` sur le dossier du package).

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
| `signtool verify` : « terminated in a root certificate which is not trusted » | Normal sur l'hôte : `WDRLocalTestCert` est auto-signé. `vm-cycle.ps1` importe le `.cer` dans la VM (§3.3). |
| Un script `tools\*.ps1` échoue avec « Jeton inattendu » ou « Le terminateur " est manquant » sur une ligne contenant `—`, `œ` ou `…` | Fichier UTF-8 **sans BOM** : Windows PowerShell 5.1 le lit en ANSI et prend des octets pour des guillemets typographiques. Tous les scripts du dépôt sont en UTF-8 avec BOM ; conserver ce BOM en éditant. |
| `vm-prepare.ps1` : « testsigning n'est pas actif après redémarrage » | Secure Boot encore actif sur la VM : `Set-VMFirmware -VMName ConduitTest -EnableSecureBoot Off` (VM arrêtée), puis relancer le script. |
| `vm-cycle.ps1` : « Le périphérique … n'est pas passé en état OK », problème 10 ou 28 | 10 : `StartDevice` ou PortCls a échoué (journal `kmd_log!`, événement Kernel-PnP 411). 28 : l'INF ne correspond pas (`pnputil /enum-drivers` dans la VM, certificat importé ?). |

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

1. **M1a-02** pilote minimal via PortCls (`DriverEntry`, `AddDevice`, `StartDevice` vide, INF,
   catalogue de test) : code et scripts de VM prêts, à charger et décharger 100 fois
   dans la VM dès qu'une ISO Windows 11 est disponible (§3.3).
2. **M1a-03** à **M1a-05** `portcls-sys` : bindgen en mode C sur `portcls.h`/`ks.h`/
   `ksmedia.h` (vtables plates, driver-design §2.2), objets COM sûrs, enveloppes
   `IMiniportWaveRT`/`IMiniportWaveRTStream`/`IPortWaveRT` avec tests en mode utilisateur.
3. **M1a-06** à **M1a-10** adaptateur, miniports rendu/capture en boucle locale, INF
   complet (KS, `wdmaudio.inf`, endpoints « Conduit 1 »), outil de test de boucle WASAPI.
4. **M1a-11/12** Driver Verifier 1 h, ADR sur le résultat du spike (porte de décision
   Rust / repli C++).

En parallèle côté utilisateur (hors code) : certificat EV et compte Hardware Dev Center
pour la signature d'attestation (délai de plusieurs semaines, SPEC §10).
