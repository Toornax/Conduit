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
.\packaging\windows\setup-env.ps1 -Scope Driver          # installe ce qui ne demande pas d'élévation
.\packaging\windows\setup-env.ps1 -Check -Scope Driver   # ne touche à rien, échoue si une version diffère
```

`-Scope Driver` est le défaut : il couvre tout l'outillage du pilote. Pour ne construire
que les crates utilisateur (`conduitd`, `conduitctl`, la GUI), `-Scope User` suffit — il
ne vérifie que Rust et les Build Tools, sans les plusieurs gigaoctets du WDK ni LLVM ni
`cargo-wdk` (SPEC §5.11) ; c'est ce qu'utilise le job `build-test` de la CI Windows.

En périmètre pilote, le script installe (ou vérifie, avec `-Check`) : rustup et la toolchain
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
workspace racine : `portcls-sys` (bindings, testable en mode utilisateur), `portcls`
(enveloppes sûres traits ↔ vtables, testées en mode utilisateur avec un faux PortCls) et
`conduit-kmd` (le `.sys`, `cdylib` `no_std`). Sa toolchain (`rust-toolchain.toml`,
stable 1.96.1 MSVC), ses lints anti-panique (`Cargo.toml`) et `rustflags =
["-C", "target-feature=+crt-static"]` (`.cargo/config.toml`, exigé par `wdk-build`) lui
sont propres.

```powershell
.\drivers\windows\tools\check.ps1               # fmt, clippy -D warnings, tests portcls-sys (± com) et portcls (dont cohérence INF ↔ Rust), build conduit-kmd, golden à jour
.\drivers\windows\tools\build.ps1               # cargo wdk build --profile dev
.\drivers\windows\tools\build.ps1 -Profile release
```

ou, depuis `drivers/windows` :

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p portcls-sys
cargo test -p portcls-sys --features com
cargo test -p portcls
cargo build -p conduit-kmd
cargo wdk build [--profile release]
```

Le tout fonctionne depuis un PowerShell ordinaire : `rustc` localise `link.exe` par le
registre de Visual Studio (comme `vswhere`), aucun `vcvars` n'est nécessaire. Ne lancez **pas** `cargo test -p conduit-kmd` :
`wdk-sys` 0.5.1 lie `ntoskrnl.lib` même en test (issue #502). `portcls-sys` ne dépend
pas de `wdk-sys` (ses bindings régénèrent les types NT dont PortCls a besoin) :
`cargo test -p portcls-sys` reste en mode utilisateur, comme `cargo test -p portcls`
(dont les seules dépendances sont `portcls-sys` et `conduit-com`).

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

**Le `.inx` est en UTF-16 LE dans la copie de travail** (M1a-09) : un INF contenant des
caractères non ASCII doit l'être, et `stampinf` recopie l'encodage tel quel. Le dépôt le
garde en UTF-8 — les diffs restent lisibles — grâce à `.gitattributes`
(`*.inx text working-tree-encoding=UTF-16LE-BOM eol=crlf`). Un `git` antérieur à 2.21 ou
un éditeur qui réenregistre le fichier en UTF-8 casserait silencieusement les libellés
accentués : `check.ps1` échoue alors, et `git checkout -- drivers/windows/conduit-kmd/conduit_kmd.inx`
rétablit l'encodage.

**Cohérence INF ↔ Rust** (M1a-09) : `portcls\tests\inf.rs`, lancé par `cargo test -p portcls`
donc par `check.ps1` et la CI, relit le `.inx` et le compare aux constantes du pilote —
noms de référence des `AddInterface` contre `WAVE_RENDER_0`… (`portcls::adapter`), GUID de
nom de broche contre `pin_name_guid(0)`, GUID `KSCATEGORY_*` contre ceux des en-têtes du
WDK, jetons `%…%` tous définis. Ces divergences ne se voient sinon que dans la VM, et mal :
périphérique installé sans le moindre endpoint, ou endpoint mal nommé
([driver-design.md](driver-design.md) §4.2).

Le build de `wdk-sys` régénère les bindings de `ntddk.h` avec bindgen (`LLVM 17`), une
minute environ la première fois ; ensuite le cache Cargo suffit.

## 3. VM de test

La **séquence complète de validation** des sept tâches du pilote livrées mais jamais
chargées est dans [vm-bringup.md](vm-bringup.md) : ordre, résultat attendu à chaque
étape et arbre de diagnostic. Ce chapitre-ci décrit chaque outil isolément.

Jamais de pilote de test sur la machine de développement : un bug = écran bleu, et le
mode `testsigning` affaiblit la machine. Les scripts du dépôt ne lancent jamais
`pnputil`, `devgen`, `bcdedit`, `verifier` ni `schtasks` **sur l'hôte** : `vm-prepare.ps1`,
`vm-cycle.ps1` et `vm-run-console.ps1` les exécutent dans l'invité par PowerShell Direct
(`Invoke-Command -VMName`). Seul `vm-debug.ps1` lance un exécutable sur l'hôte — `kd.exe`,
un débogueur, qui ne charge rien.

Trois scripts de mise en place dans `drivers\windows\tools\`, à lancer dans l'ordre sur
l'hôte, plus deux outils de séance. Prérequis : Hyper-V activé (Windows 11 Pro), le
commutateur `Default Switch` (créé par Hyper-V, NAT vers l'hôte), une **ISO officielle de
Windows 11** (microsoft.com, « Télécharger l'image de disque »), le WDK 26100 sur l'hôte
(pour `devgen.exe` et `kd.exe`) et le paquet produit par `build.ps1`.

| Script | Rôle | Une fois / à chaque fois |
|---|---|---|
| `vm-new.ps1 -IsoPath <iso>` | crée et démarre la VM | une fois |
| `vm-prepare.ps1 -Credential <cred>` | `testsigning`, débogueur **série**, filtre de traces, vidages, point de contrôle `propre` | une fois (ou après réinstallation) |
| `vm-cycle.ps1 -Credential <cred> [-Count 100]` | copie le paquet, l'installe et le retire N fois | à chaque build à valider |
| `vm-debug.ps1 [-StartVM] [-Follow]` | attache `kd.exe` au canal nommé **avant** de démarrer la VM | à chaque séance de débogage |
| `vm-run-console.ps1 -Credential <cred> -Path <exe>` | exécute une commande **dans la session console** de l'invité | à chaque mesure audio |

**Deux façons de lancer les scripts.** Piloter Hyper-V n'exige **pas** l'élévation :
appartenir au groupe **Administrateurs Hyper-V** suffit. Tous ces scripts acceptent donc
l'une **ou** l'autre de ces deux situations, et s'arrêtent sinon avec un message qui
rappelle les deux remèdes (`vm-debug.ps1` ne le vérifie qu'avec `-StartVM` : sans lui, il
ne touche pas à Hyper-V) :

1. **PowerShell lancé en tant qu'administrateur.** Rien à préparer, mais une invite UAC à
   chaque fois, et un terminal élevé pour toute la session.
2. **Session ordinaire, compte membre du groupe Administrateurs Hyper-V.** À privilégier
   pour les **sessions de validation longues** (`vm-bringup.md`), où l'on enchaîne
   `build.ps1`, `check.ps1` et `vm-cycle.ps1` : le terminal reste non élevé, donc les
   outils Rust travaillent avec les droits habituels et les fichiers produits gardent le
   bon propriétaire. Une fois, depuis un PowerShell administrateur :

   ```powershell
   Add-LocalGroupMember -SID S-1-5-32-578 -Member "$env:USERNAME"
   ```

> **Après cet ajout, fermer et rouvrir la session Windows.** Le jeton d'accès n'intègre
> les appartenances de groupe qu'à l'**ouverture** de session : rouvrir le terminal, ou
> même relancer l'explorateur, ne suffit pas. Tant que ce n'est pas fait, les scripts
> refuseront de démarrer exactement comme avant.

Le groupe est désigné par son **SID** `S-1-5-32-578` et non par son nom, qui est traduit
(« Administrateurs Hyper-V » en français) : `Add-LocalGroupMember -Group "Hyper-V
Administrators"` échoue sur un Windows français. Le code du dépôt applique la même règle
(`vm-common.psm1`), après que le nom traduit d'un composant d'intégration eut cassé la
création de VM (corrigé au commit `25b3557`).

Les fonctions d'analyse partagées (`vm-common.psm1` : identifiant d'instance de `devgen`,
`oemN.inf` de `pnputil /enum-drivers`, clé kdnet, résumé des durées, décision d'accès
Hyper-V, ligne de commande de `kd.exe`, décision de session console, fichier de commandes
de la tâche planifiée, reconnaissance des endpoints fantômes) ont des tests Pester dans
`tools\tests\` (`Invoke-Pester drivers\windows\tools\tests`, syntaxe Pester 3/4 livrée
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
**compte local** `test` administrateur sans compte Microsoft, puis passer à
`vm-prepare.ps1`. Microsoft a retiré `OOBE\BYPASSNRO` et `ms-cxh:localonly` des images
récentes : la marche à suivre qui fonctionne encore est détaillée dans
[vm-bringup.md](vm-bringup.md) §1 (déconnecter la carte réseau, poser la valeur de
registre `BypassNRO`, redémarrer).

### 3.2 Préparation : `vm-prepare.ps1`

```powershell
$cred = Get-Credential nathan
.\drivers\windows\tools\vm-prepare.ps1 -Name ConduitTest -Credential $cred   # [-DebugTransport serial|net] [-DebugPipe \\.\pipe\conduitdbg] [-DebugComPort 1]
```

Dans l'invité, par PowerShell Direct : `bcdedit /set testsigning on`, `bcdedit /debug on`,
le transport du débogueur, le **filtre de traces du noyau**, `CrashDumpEnabled = 2`
(vidage noyau complet) et `AutoReboot = 1` dans
`HKLM\SYSTEM\CurrentControlSet\Control\CrashControl`, veille, écran et hibernation
désactivés. Puis redémarrage de l'invité, vérification que `testsigning` est bien actif
(`bcdedit /enum`) et point de contrôle **`propre`** (`Checkpoint-VM`). Après un plantage
inexpliqué : `Restore-VMCheckpoint -VMName ConduitTest -Name propre -Confirm:$false`.

**Transport `serial` par défaut** : `bcdedit /dbgsettings serial debugport:1
baudrate:115200` dans l'invité, et `Set-VMComPort -VMName ConduitTest -Number 1 -Path
\\.\pipe\conduitdbg` sur l'hôte. Cette commande **exige la VM arrêtée** : le script
termine donc par un arrêt complet, attache le canal, puis rallume — d'où un cycle un peu
plus long qu'un simple redémarrage. Le transport réseau (`-DebugTransport net`, `bcdedit
/dbgsettings net` et une clé kdnet) reste disponible et se comporte comme avant, mais
**ne s'est jamais connecté sur ce poste** : ne le reprendre que pour l'y faire marcher.

**Filtre de traces** : `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print
Filter`, valeur `DEFAULT` (DWord) à `0xF`, **créée si la clé n'existe pas** — c'est le cas
sur une installation neuve. `DEFAULT` et non `IHVDRIVER` : `kmd_log!` passe par `DbgPrint`,
donc par le composant `DPFLTR_DEFAULT_ID` (§4). Sans ce filtre, seules les lignes de niveau
*erreur* traversent : les traces `kmd_log!` restent invisibles, et ce silence ressemble
trait pour trait à un pilote qui ne démarre pas. Posée en registre, la valeur vaut dès
l'amorçage, donc pour `DriverEntry`, avant même que le débogueur puisse agir.

Elle **ne suffit pourtant pas** : mesuré le 2026-09-06, une séance est restée muette alors
que la valeur valait déjà `0xFFFFFFFF` dans l'invité, et seul le `ed nt!Kd_DEFAULT_Mask 0xf`
joué à la connexion a débloqué les traces. Les deux sont nécessaires, aucun ne remplace
l'autre — le détail de la mesure est en §4.

### 3.3 Cycle de chargement (M1a-02) : `vm-cycle.ps1`

```powershell
.\drivers\windows\tools\build.ps1                                            # paquet target\debug\conduit_kmd_package
.\drivers\windows\tools\vm-cycle.ps1 -Name ConduitTest -Credential $cred -Count 100   # [-Package <dossier>] [-Profile dev|release] [-StartTimeoutSeconds 30] [-RemoveTimeoutSeconds 30]
```

Le script copie le paquet et `devgen.exe` (WDK de l'hôte, version de `versions.json`)
dans `C:\ConduitTest` de l'invité, importe `WDRLocalTestCert.cer` dans *Root* et
*TrustedPublisher* de la machine invitée, retire les restes d'une exécution interrompue
(périphériques `Root\ConduitCable`, paquets `oemN.inf` de `conduit_kmd.inf`, et **endpoints
fantômes** — périphériques non présents dont le nom contient « Conduit » — qui fausseraient
les mesures en dédoublant les instances ; rien d'autre n'est touché, cette VM servant aussi
à comparer avec de vraies cartes son), puis répète
`Count` fois, en affichant `i/Count OK` et la durée :

1. `pnputil /add-driver conduit_kmd.inf /install` ;
2. `devgen /add /hardwareid "Root\ConduitCable"` (identifiant d'instance lu dans la
   sortie) : PnP appelle `AddDevice` puis `IRP_MN_START_DEVICE` → `StartDevice` ;
3. attente de `Get-PnpDevice -InstanceId <id>` en état `OK` (30 s au plus, sinon échec
   avec le code de problème) ;
4. `devgen /remove <id>` : `IRP_MN_REMOVE_DEVICE`, l'objet de périphérique disparaît ;
5. **attente que le devnode ait vraiment disparu** (`-RemoveTimeoutSeconds`, 30 s par
   défaut) — voir « la course du `0xC00000E5` » ci-dessous ;
6. `pnputil /delete-driver oemN.inf /uninstall /force` (`oemN.inf` retrouvé par
   `pnputil /enum-drivers`) : le service est supprimé et le `.sys` déchargé ;
7. vérification qu'aucun événement `Kernel-PnP` de niveau erreur ni `BugCheck`
   (1001, 6008) n'est apparu dans l'invité depuis le début.

Tout `devgen`/`pnputil` dont le code de retour n'est pas 0 arrête le script avec sa
sortie. À la fin (même après un plantage : l'invité redémarre seul et le script se
reconnecte), un `C:\Windows\MEMORY.DMP` ou un `C:\Windows\Minidump\*.dmp` plus récent que
le début est rapatrié dans `drivers\windows\target\dumps\` et le script **échoue**.
Sortie : cycles réussis et durée moyenne, minimale et maximale d'un cycle. Le critère de
M1a-02 est `vm-cycle : 100 cycles sans erreur.`

**En cas d'échec, le journal d'installation PnP est rapatrié aussi** : les 500 dernières
lignes de `C:\Windows\INF\setupapi.dev.log` de l'invité, dans
`drivers\windows\target\dumps\<horodatage>_setupapi.dev.log`. C'est lui qui dit *pourquoi*
une installation ou un démarrage PnP a échoué (chaque étape et son code de fin), là où le
journal d'événements ne donne qu'un code ; il était perdu à chaque échec jusqu'ici.

#### La course du `0xC00000E5` (mesurée, corrigée le 2026-09-06)

Sur une série de deux cycles, le second a échoué :

```
Kernel-PnP 411 : L'appareil SWD\DEVGEN\{0c9acf52-…} a eu un problème de démarrage.
Problème : 0x0   État du problème : 0xC00000E5
```

**Le signe distinctif** : `{0c9acf52-…}` est l'appareil du cycle **1**, pas celui du cycle
2, et l'horodatage tombe à l'instant de son retrait. Les traces du pilote montraient un
cycle 1 complet et propre, `DriverUnload` compris ; le cycle 2 n'avait jamais chargé le
pilote, et l'invité ne gardait après coup ni périphérique résiduel ni paquet publié.

Le pilote était donc hors de cause. Le script enchaînait `devgen /remove` puis
immédiatement `pnputil /enum-drivers` et `/delete-driver … /uninstall /force`, sans
attendre que le retrait soit effectif : PnP tentait un dernier démarrage sur le devnode
mourant pendant qu'on lui retirait son paquet sous les pieds, et le journalisait en
erreur. C'est ce qui a produit l'échec au 33ᵉ cycle sur 100 la veille et au 2ᵉ le
lendemain — une course, dont la probabilité monte quand le débogueur ralentit l'invité.

L'étape 5 attend donc la disparition du devnode avant l'étape 6. Le critère est
**structurel** — l'instance n'est plus rendue par `Get-PnpDevice -InstanceId <id>`, ou
n'y figure plus qu'en périphérique non présent (`Present`, un **booléen**) — jamais le
libellé `Status`, qui est traduit.

À la main, dans la VM, les mêmes commandes se tapent depuis `C:\ConduitTest` :

```powershell
pnputil /add-driver package\conduit_kmd.inf /install
.\devgen.exe /add /hardwareid "Root\ConduitCable"      # imprime l'identifiant d'instance
.\devgen.exe /remove <id>
pnputil /enum-drivers                                  # repérer oemN.inf du fournisseur Conduit
pnputil /delete-driver oemN.inf /uninstall /force
```

Le gestionnaire de périphériques doit montrer *Conduit — câbles audio virtuels* sous
*Contrôleurs audio, vidéo et jeu* entre `/add` et `/remove` (sans endpoint audio avant
M1a-06).

### 3.3 bis Vérifier le nom des endpoints (M1a-09)

Le nom affiché se compose du nom de la **broche bridge** et de celui de l'adaptateur
([driver-design.md](driver-design.md) §4.2) : attendu « Conduit 1 (Conduit — câbles audio
virtuels) », un endpoint de rendu et un de capture. Dans l'invité, périphérique créé :

```powershell
# 1. Les endpoints tels que l'utilisateur les voit (nom composé, sens, état).
Get-PnpDevice -Class AudioEndpoint | Format-Table FriendlyName, Status, InstanceId -Auto

# 2. Les quatre filtres KS de l'adaptateur (les FriendlyName de l'INF, diagnostic).
Get-PnpDevice -Class MEDIA | Where-Object InstanceId -like 'ROOT\MEDIA*' |
  Format-Table FriendlyName, Status, InstanceId -Auto

# 3. La chaîne que KS résout pour le GUID de nom de broche : doit valoir « Conduit 1 ».
#    <NNNN> est l'instance de classe du périphérique (0000, 0001…).
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e96c-e325-11ce-bfc1-08002be10318}' |
  ForEach-Object { Get-ItemProperty "$($_.PSPath)\MediaCategories\{CAA74E3D-9BD5-4F78-8EAC-26A5A1AE0F00}" -ErrorAction SilentlyContinue }

# 4. Le panneau historique, qui montre le nom et permet de le renommer à la main.
mmsys.cpl
```

Les réglages Son de Windows 11 (*Paramètres → Système → Son*) affichent la même chaîne que
la colonne `FriendlyName` du point 1 (`PKEY_Device_FriendlyName`). Le module
`AudioDeviceCmdlets` (`Install-Module AudioDeviceCmdlets`), s'il est installé dans
l'invité, donne la même liste avec `Get-AudioDevice -List` ; il n'est pas requis.
`.\conduit-looptest.exe --list` (§3.4) sert aussi de vérification : il cherche les
endpoints nommés `Conduit 1`.

Diagnostic si le nom n'est pas le bon :

- **« Haut-parleurs » / « Ligne »** : KS n'a pas trouvé de chaîne pour le GUID
  `KsPinDescriptor.Name` et est retombé sur la catégorie de la broche. Vérifier le
  point 3 ; si la clé est absente, l'`AddReg` de l'INF n'a pas été appliqué.
- **aucun endpoint** alors que le périphérique est en état `OK` : les noms de référence des
  `AddInterface` ne correspondent pas à ceux passés à `PcRegisterSubdevice`, ou une
  connexion physique manque. `cargo test -p portcls` (test `inf.rs`) couvre le premier cas
  sur l'hôte, avant même de démarrer la VM.
- **texte accentué abîmé** (« câbles ») : la copie de travail du `.inx` a perdu son
  encodage UTF-16 LE. `git checkout -- drivers/windows/conduit-kmd/conduit_kmd.inx` puis
  reconstruire ; `tools/check.ps1` le détecte aussi.

### 3.4 Test de boucle (M1a-10) : `conduit-looptest`

Une fois le pilote installé et les endpoints `Conduit 1` visibles dans l'invité, la
boucle se vérifie avec l'outil du workspace racine. Copiez le binaire dans la VM
(`cargo build -p conduit-looptest` sur l'hôte, puis `target\debug\conduit-looptest.exe`)
ou construisez-le dans l'invité, et lancez **la commande du critère** :

```powershell
.\conduit-looptest.exe --list          # les deux côtés de « Conduit 1 » doivent apparaître
.\conduit-looptest.exe --repeat 10     # le critère M1a-10 : 10 passes de suite
```

Sans argument de périphérique, l'outil prend les deux endpoints nommés `Conduit 1` :
il joue un sinus de 440 Hz sur le rendu pendant 2 s, enregistre la capture, jette le
préambule, puis vérifie la fréquence (200 ppm), l'amplitude, la continuité de phase
(aucune trame perdue ni dupliquée) et l'absence de trous. Une ligne par passe, puis
un résumé :

```text
passe 3/10 : 440,00 Hz, amplitude 0,500, 0 saut(s), 0 trou(s) → OK
résumé : 10/10 passe(s) OK — fréquence de 440,000 à 440,000 Hz, 0 saut(s), 0 trame(s) de trou
```

**Codes de retour** : `0` les dix passes passent (critère atteint), `1` au moins une
échoue — le message dit quoi regarder (« trames perdues ou dupliquées dans la boucle
(position du tampon cyclique du pilote) », « sous-alimentation du tampon ») —, `2`
l'environnement ne permet pas le test : c'est le code que rend `--list` sans câble,
c'est-à-dire pilote non chargé. `--json` donne la même chose pour un script.

Utile au diagnostic : `--seconds`, `--rate 44100` et `--channels` pour couvrir
d'autres formats, `--block` pour changer la taille de rappel demandée,
`--freq 997` pour un signal non harmonique de la taille de bloc,
`--no-capture` pour n'exercer que le rendu. `--self-test` (sans périphérique)
vérifie l'outil lui-même. Détails dans [dev-guide.md](dev-guide.md) §4 quinquies.

### 3.4 bis Mesurer dans la session console : `vm-run-console.ps1`

**Une mesure audio lancée par `Invoke-Command -VMName` ne mesure rien.** PowerShell Direct
ouvre ses sessions dans la **session 0**, celle des services, qui n'a aucun audio
utilisateur : le test énumère les endpoints, ouvre les flux et reçoit des trames *à la
bonne cadence*, toutes silencieuses, **sans le moindre signal d'erreur**. Le résultat a
l'air d'un pilote en panne alors qu'il n'a aucun objet. Ce piège, cumulé au mode session
étendue de `vmconnect` qui redirige l'audio vers l'hôte, a invalidé une journée entière de
mesures ([vm-bringup.md](vm-bringup.md) §3 bis).

```powershell
.\drivers\windows\tools\vm-run-console.ps1 -Credential $cred `
  -Path target\debug\conduit-looptest.exe -Arguments @("--repeat", "10")
```

Le script **refuse de travailler** tant qu'une session console interactive n'est pas
ouverte dans l'invité, et le dit avec la marche à suivre : ouvrir `vmconnect` **en mode
session de base** (pas étendue), ouvrir une session Windows à l'écran, relancer. La
vérification ne dépend pas de la langue — `Win32_ComputerSystem.UserName` donne
l'utilisateur ouvert à la console, et l'identifiant de session de *son* processus
`explorer` doit différer de 0 ; jamais `quser`, dont toute la sortie est traduite.

Ensuite : copie facultative de l'exécutable (`-Path`, sinon `-RemoteExecutable` pour un
binaire déjà présent), création d'une tâche planifiée à **jeton interactif**
(`schtasks /create … /ru <compte> /it /f`, aucun mot de passe nécessaire), `schtasks /run`,
attente, relecture, suppression de la tâche (même en cas d'échec), et propagation du code
de retour. Le compte vient toujours du `-Credential` : le compte réel de cette VM est
`nathan`, pas le `test` de la documentation d'origine ; un compte qui ne serait pas celui
ouvert à la console est refusé d'emblée, car la tâche `/it` ne partirait jamais.

La fin d'exécution se constate par un **fichier de code de retour** écrit par la commande
elle-même, jamais par `schtasks /query`, dont le statut est traduit. `-TimeoutSeconds`
borne l'attente (prévoir large pour la charge d'une heure : `-TimeoutSeconds 4000`) ; la
session PowerShell Direct est rouverte si elle tombe, et l'avancement s'imprime chaque
minute. La sortie complète reste dans l'invité, sous `C:\ConduitTest\console-<horodatage>.out.txt`.

### 3.5 Driver Verifier et journal

`verifier /standard /driver conduit_kmd.sys` puis redémarrage (retour : `verifier /reset`).
Les messages `kmd_log!` (`conduit-kmd/src/log.rs`, `wdk::println!` → `DbgPrint`, profil
`dev` seulement) passent par le débogueur : fenêtre de `kd.exe` ouverte par `vm-debug.ps1`
(§4) ou DebugView (« Capture Kernel ») dans l'invité — à condition que le filtre de traces
soit posé (§3.2). Attendu à chaque cycle : `DriverEntry`, `AddDevice`, `StartDevice`,
`DriverUnload`.

La charge d'une heure sous Driver Verifier (M1a-11) tourne sans surveillance avec
`vm-run-console.ps1 -TimeoutSeconds 4000` : elle doit s'exécuter dans la session console,
comme toute mesure audio.

## 4. Débogage noyau et plantages

Le transport qui **marche** ici est le **canal nommé série**, pas le réseau. La VM est
préparée par `vm-prepare.ps1` (§3.2, transport `serial` par défaut) : son port COM 1 est
attaché à `\\.\pipe\conduitdbg`.

```powershell
.\drivers\windows\tools\vm-debug.ps1 -StartVM -Follow    # [-Name ConduitTest] [-Pipe \\.\pipe\conduitdbg] [-LogPath <fichier>]
```

> **La règle qui débloque tout : le débogueur doit tenir le canal nommé AVANT que la
> machine démarre.** Une VM démarrée en premier ne se laisse pas rattraper — la connexion
> n'arrive jamais, et l'on passe la séance à chercher pourquoi. `-StartVM` encapsule
> l'ordre : le script arrête la VM si nécessaire, lance `kd.exe`, **attend qu'il tienne le
> canal**, puis seulement `Start-VM`. Ce n'est plus une consigne à retenir, c'est un
> comportement.

**La règle vaut aussi pour un rattachement** (mesuré le 2026-09-06). `vm-debug.ps1` *sans*
`-StartVM` sur une VM déjà en marche écrit `Opened \\.\pipe\conduitdbg` puis reste
**indéfiniment** sur `Waiting to reconnect...` : la connexion ne s'établit jamais, et les
commandes initiales (`-c`) ne sont donc jamais jouées non plus. `resets=0,reconnect` fait
patienter le débogueur *avant* un démarrage, il ne rattrape pas un invité déjà parti.
L'aide du script annonçait le contraire (« VM déjà démarrée : kd se raccroche ») : c'était
faux, et corrigé. Le script avertit maintenant avant de lancer `kd` quand la VM tourne et
que `-StartVM` n'a pas été passé — sans interdire le cas, tenir le canal face à un invité
figé gardant son intérêt.

À la main, dans cet ordre et pas un autre :

```powershell
Stop-VM -Name ConduitTest
kd.exe -k com:pipe,port=\\.\pipe\conduitdbg,resets=0,reconnect    # <KitsRoot10>\Debuggers\x64\
Start-VM -Name ConduitTest
```

`resets=0,reconnect` est ce qui rend cet ordre praticable : le débogueur tient le canal
sans lâcher tant que la VM n'a pas démarré, et se raccroche aux redémarrages **suivants**
de l'invité — mais pas à une machine partie avant lui. `vm-debug.ps1` journalise tout (`-logo`, sous
`drivers\windows\target\debug-logs\`), joue `ed nt!Kd_DEFAULT_Mask 0xf`,
`ed nt!Kd_IHVDRIVER_Mask 0xf` puis `g` à la connexion, et avec `-Follow` suit le journal
en mettant en évidence les lignes du pilote.

#### Le masque de traces : `DEFAULT`, et il en faut **deux** (mesuré le 2026-09-06)

Avec les seules commandes d'origine (`ed nt!Kd_IHVDRIVER_Mask 0xf`), une séance complète —
débogueur connecté dès l'amorçage, 100 cycles de chargement — n'a rendu **aucune** trace
du pilote, alors que les `DbgPrint` d'autres composants passaient. En ajoutant
`ed nt!Kd_DEFAULT_Mask 0xf`, la séance suivante a immédiatement rendu les dix traces
attendues (`DriverEntry`, `AddDevice`, `StartDevice`, les quatre `Init`, `DriverUnload`…).

La raison est dans le code : `kmd_log!` ([`conduit-kmd/src/log.rs`](../drivers/windows/conduit-kmd/src/log.rs))
passe par `wdk::println!`, qui appelle **`DbgPrint`** — donc le composant
`DPFLTR_DEFAULT_ID`, jamais `DPFLTR_IHVDRIVER_ID`, qui exigerait un `DbgPrintEx` explicite.
Le masque à ouvrir est `Kd_DEFAULT_Mask` ; `Kd_IHVDRIVER_Mask` est conservé derrière, sans
coût, pour un éventuel `DbgPrintEx` futur.

Second constat de la même séance : la valeur de registre `Debug Print Filter\DEFAULT`
valait **déjà `0xFFFFFFFF`** dans l'invité pendant que la séance restait muette. Elle n'a
donc **pas suffi à elle seule** ; c'est le `ed` joué au moment de la connexion qui a
débloqué les traces. **Les deux sont nécessaires et aucun ne remplace l'autre** : la valeur
de registre (§3.2) vaut dès l'amorçage, avant que le débogueur puisse agir ; le `ed` vaut
pour la séance en cours. C'est un constat de mesure, pas une théorie : ne retirer ni l'un
ni l'autre sans le remesurer.

Commandes utiles dans la fenêtre de `kd.exe` : `!analyze -v` après un bug check,
`!verifier 3` pour l'état de Driver Verifier, `lm m conduit*` pour vérifier que le module
et son `.pdb` sont chargés (`.sympath+ <dossier du package>` puis `.reload`),
`bp conduit_kmd!DriverEntry`, Ctrl+Inter pour interrompre l'invité et `g` pour le relâcher.
La VM s'arrête au démarrage jusqu'à la connexion si `bcdedit /set {default} bootdebug on`
(dans l'invité).

Le transport **réseau** (kdnet, WinDbg → *Attach to kernel* → *Net*, port et clé de
`vm-prepare.ps1 -DebugTransport net`) est resté disponible mais **ne s'est jamais connecté
sur ce poste** : ne pas y passer de temps sans raison précise.

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
| `vm-run-console.ps1` : « Aucune session console interactive » | Voulu. Ouvrir `vmconnect` **en mode session de base** (pas étendue, qui redirige l'audio vers l'hôte), ouvrir une session Windows à l'écran avec le compte du `-Credential`, attendre le bureau, relancer. Sans cela la mesure tournerait dans la session 0 et serait silencieuse sans erreur (§3.4 bis). |
| `vm-run-console.ps1` : « Le compte ouvert à la console est … mais -Credential désigne … » | La tâche `/it` ne se déclenche que pour l'utilisateur ouvert à la console. Le compte réel de la VM est `nathan`, pas `test` : passer le bon `-Credential`. |
| `kd` reste sur `Waiting to reconnect...` après `Opened \\.\pipe\…` | La VM a démarré **avant** le débogueur — y compris quand `vm-debug.ps1` a été lancé sans `-StartVM` sur une VM déjà en marche, cas mesuré comme sans issue (§4). Arrêter la VM, relancer `vm-debug.ps1 -StartVM`, qui impose l'ordre. Vérifier aussi le port COM : `Get-VMComPort -VMName ConduitTest` doit montrer `\\.\pipe\conduitdbg` (attaché par `vm-prepare.ps1`, VM arrêtée). |
| Aucune trace `kmd_log!` alors que le pilote démarre | Masque de traces **`DEFAULT`**, pas `IHVDRIVER` (§4) : `ed nt!Kd_DEFAULT_Mask 0xf` dans le débogueur, **et** `Debug Print Filter\DEFAULT = 0xF` en registre (§3.2) — les deux, l'un ne remplace pas l'autre. Vérifier aussi le profil : `kmd_log!` n'existe qu'en `dev`. |
| `vm-cycle.ps1` : `Kernel-PnP 411`, état `0xC00000E5` sur l'appareil du cycle **précédent** | La course du retrait (§3.3), pas le pilote. Attendue et corrigée depuis ; si elle réapparaît, augmenter `-RemoveTimeoutSeconds`. Le journal `<horodatage>_setupapi.dev.log` rapatrié avec les vidages dit ce que PnP a tenté. |
| `vm-prepare.ps1` : « Set-VMComPort exige une VM ARRÊTÉE » | L'invité ne s'est pas éteint (mise à jour en cours, invité figé). `Stop-VM -Name ConduitTest`, puis relancer le script. |
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
  jamais été exécutés sous Windows. **Première exécution du named pipe le 2026-09-05** :
  `cargo test -p conduitd -p conduitctl -p conduit-protocol --all-features` passe
  (46 tests : 23 conduitd, 8 conduitctl, 15 conduit-protocol), après trois correctifs :
  1. `Paths::under(racine)` ignorait la racine sous Windows : tous les démons de test
     se disputaient `\\.\pipe\conduit-<utilisateur>` (`ERROR_ACCESS_DENIED` sur
     `first_pipe_instance`). Désormais le socket est `<racine>\conduitd.sock` partout et
     `conduit_protocol::client::pipe_name` projette tout chemin qui n'est pas déjà un
     nom de pipe sur `\\.\pipe\conduit-<empreinte FNV du chemin absolu>`, côté démon
     (`Listener::bind`) comme côté client ; `--socket <fichier>` est donc portable.
  2. `Listener::accept` (pipe) retirait l'instance en attente avant `connect()` : annulé
     par le `select!` de `serve`, l'appel suivant paniquait.
  3. Bug commun révélé par le timing Windows : `Service::start` construisait `Manager`
     (restauration, règles, premier drain des notifications de démarrage) dans le fil de
     gestion, en course avec les premiers clients IPC ; un abonné précoce recevait
     `NodeStateChanged`/`NodeAdded` des nœuds initiaux. `Manager` est maintenant
     construit avant le lancement du fil ;
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
