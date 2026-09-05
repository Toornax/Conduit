# Pilote Windows : conception (M1a, M1b.A)

Document d'architecture du pilote noyau `conduit-kmd` et de ses crates satellites.
Il précise SPEC §3.1, §5.3, §5.4 et ADR-003/004/007 au niveau nécessaire pour
implémenter les tâches M1a-01 à M1a-12 puis M1b.A sans redécider en cours de route.
Les sous-agents qui codent une tâche lisent ce document, la tâche dans `ROADMAP.md`
et [driver-dev.md](driver-dev.md) (installation, VM, débogage).

Statut : brouillon 0.1 (2026-09-05), à ajuster par ADR à mesure que le spike apprend.

## 1. Rappel des contraintes

- **Pilote bête** (ADR-007) : boucle locale rendu → capture par câble, aucune donnée
  audio vers le démon, une seule surface de contrôle (activer, désactiver, canaux).
- **Réserve fixe** (ADR-004) : 16 câbles enregistrés au démarrage, inactifs masqués par
  l'état de jack. Le spike M1a n'en enregistre qu'un ; la réserve arrive en M1b-02.
- **Rust d'abord** (ADR-003) : `windows-drivers-rs` (crates publiés `wdk-sys`/`wdk-build`
  0.5.1, `wdk`/`wdk-alloc` 0.4.1, `cargo-wdk` 0.1.1, **Rust stable** 1.96.1 MSVC comme le
  reste du dépôt, LLVM 17.0.6 pour bindgen : LLVM 22 casse les crates publiés), vtables
  COM générées par bindgen (§2.2), porte de décision M1a-12 avec repli C++ (SYSVAD).
- **Zéro panique en noyau** : une panique = `KeBugCheckEx` = écran bleu. Tout chemin
  faillible renvoie un `NTSTATUS`.
- **Nix ne couvre pas Windows** (ADR-008) : mais tout ce qui peut être testé sans le
  WDK doit l'être, depuis Linux, par `nix flake check`.

## 2. Organisation des crates

```
conduit/
├── Cargo.toml                       # workspace racine (utilisateur, tous OS)
│   exclude = ["drivers/windows", "crates/conduit-protocol/fuzz"]
├── crates/
│   ├── conduit-kmd-core/            # logique portable du pilote, no_std, membre racine
│   ├── conduit-com/                 # modèle objet COM générique, no_std, testé sous Miri (M1a-04)
│   └── conduit-helper/              # service d'assistance (M1b-20), membre racine
└── drivers/windows/
    ├── Cargo.toml                   # workspace NOYAU, indépendant, jamais ouvert par Nix
    ├── rust-toolchain.toml          # 1.96.1 MSVC (stable suffit à windows-drivers-rs)
    ├── .cargo/config.toml           # rustflags -C target-feature=+crt-static (exigé par wdk-build)
    ├── portcls-sys/                 # bindings PortCls/KS générés (structures, GUID, vtables)
    ├── portcls/                     # enveloppes sûres : traits Rust ↔ vtables PortCls, testées en mode utilisateur
    ├── conduit-kmd/                 # le pilote (.sys), cdylib no_std
    └── tools/                       # scripts PowerShell : build, install VM, verifier
```

Pourquoi deux workspaces (ADR-012) :

- le workspace noyau impose `panic = "abort"`, `#![no_std]`, un allocateur noyau, une
  toolchain propre et des flags d'édition de liens (`/DRIVER`, `/NODEFAULTLIB`,
  `/INTEGRITYCHECK`) incompatibles avec un workspace de binaires utilisateur ;
- `cargo check --workspace` à la racine doit rester vert sur Linux et macOS ;
- tout ce qui est utilisateur (helper, logique testable) reste dans le workspace racine
  pour bénéficier de clippy, Miri, couverture et vérification croisée `mingwW64`.

### 2.1 `conduit-kmd-core` (portable, `no_std`, workspace racine)

Contient tout ce qui se raisonne et se teste sans noyau :

| Module | Contenu | Tests |
|---|---|---|
| `position` | horloge virtuelle : `frames_at(qpc_now, qpc_start, qpc_freq, rate)` en arithmétique 128 bits sans débordement ; conversion trames ↔ octets ↔ position cyclique | proptest (monotonie, pas de débordement à 2⁶³ ticks, exactitude à ±1 trame) |
| `ring` | copie cyclique d'un tampon rendu vers un tampon capture entre deux positions, tailles différentes, avec conversion de format (M1a : float32 → float32, PCM16 → PCM16 ; M1b-05 : matrice complète) | proptest (aucune trame perdue ni dupliquée, wrap-around) |
| `format` | validation d'un `KSDATAFORMAT_WAVEFORMATEXTENSIBLE` demandé contre la liste supportée ; construction des `KSDATARANGE_AUDIO` déclarées | tables de cas, fuzz |
| `config` | structures `#[repr(C)]` de la propriété privée de configuration (M1b-04) et leur validation (`validate(&[u8]) -> Result<CableConfig, ConfigError>`) | fuzz (M1b-08), Miri |
| `params` | bornes des paramètres de registre (réserve, canaux, tampon) et repli par défaut (M1b-01) | tables de cas |

Règles : `#![no_std]`, `#![forbid(unsafe_code)]`, lints `clippy::panic`,
`clippy::unwrap_used`, `clippy::expect_used`, `clippy::indexing_slicing`,
`clippy::arithmetic_side_effects` en `deny`. Aucune dépendance hors `core`.
Le crate compile aussi avec `std` (feature `std`, tests et fuzz seulement).

### 2.2 `portcls-sys` (workspace noyau)

État (M1a-03, 2026-09-05) : **mode C confirmé** sur le WDK 10.0.26100.0, aucun repli.

- **Bindings par bindgen** (0.71, LLVM 17), générés **à la compilation** par `build.rs`
  sur `wrapper.h` (commité) : `ntddk.h`, `windef.h`, `mmreg.h` (`NOBITMAP`), `ks.h`,
  `ksmedia.h`, `punknown.h`, `drmk.h`, `#define INTERFACE void`, `portcls.h`, avec les
  chemins d'include et les défines du WDK fournis par `wdk-build`
  (`configure_wdk_library_build_and_then`, `bindgen::Builder::wdk_default`).
  `allowlist_file` limité aux cinq en-têtes PortCls/KS, récursivité active : les
  structures (`KSDATAFORMAT`, `KSDATARANGE_AUDIO`, `PCFILTER_DESCRIPTOR`,
  `PCPIN_DESCRIPTOR`, `PCNODE_DESCRIPTOR`, `PCCONNECTION_DESCRIPTOR`, `PCPROPERTY_ITEM`,
  `PCPROPERTY_REQUEST`, `PCAUTOMATION_TABLE`, `KSJACK_DESCRIPTION`, `KSRTAUDIO_*`…), les
  constantes, les 38 fonctions `Pc*` et les types de rappel ; et, par récursivité, les
  types NT qu'ils référencent (`DRIVER_OBJECT`, `DEVICE_OBJECT`, `IRP`,
  `UNICODE_STRING`, dont les alias `typedef` sont ajoutés explicitement). Ce sont
  d'**autres définitions** que celles de `wdk-sys`, de même disposition : le crate est
  autonome (ni `wdk-sys` ni feature `kernel`), donc testable en mode utilisateur, et
  `conduit-kmd` passe de l'une à l'autre par `cast()` de pointeur (commentaire en tête
  de `conduit-kmd/src/lib.rs`). Sortie : `OUT_DIR/portcls.rs` (~41 000 lignes,
  1,5 Mo) et `OUT_DIR/guids.rs` (639 GUID), inclus par `src/bindings.rs`.
- **Vtables générées, pas écrites** : compilé en mode C, `portcls.h` déclare chaque
  interface avec `DECLARE_INTERFACE_`/`STDMETHOD_` (`basetyps.h`) et recopie les
  méthodes héritées (`DEFINE_ABSTRACT_UNKNOWN`, `DEFINE_ABSTRACT_MINIPORT`,
  `DEFINE_ABSTRACT_MINIPORTWAVERTSTREAM`…) : bindgen produit
  `struct IMiniportWaveRT { lpVtbl: *mut IMiniportWaveRTVtbl }` et une vtable plate de
  pointeurs de fonction `extern "C"` dans l'ordre du header (54 vtables). Slots vérifiés
  : `IUnknown` 3, `IMiniport` 5, `IMiniportWaveRT` 8, `IMiniportWaveRTStream` 11,
  `IMiniportWaveRTStreamNotification` 15, `IMiniportTopology` 6,
  `IAdapterPowerManagement` 6, `IPort` 6, `IPortWaveRT` 6, `IPortTopology` 6,
  `IPortWaveRTStream` 10, `IResourceList` 11, `IRegistryKey` 11, `IPortClsVersion` 4.
  Pièges rencontrés et leur solution :
  - `THIS_` s'expanse en `INTERFACE *This` et `portcls.h` ne définit jamais `INTERFACE` :
    `#define INTERFACE void` juste avant `portcls.h` (les autres en-têtes le définissent
    puis l'annulent eux-mêmes) ;
  - `portcls.h` exige `KSDATAFORMAT_WAVEFORMATEX`, donc `WAVEFORMATEX` : `windef.h` puis
    `mmreg.h` avec `NOBITMAP` avant `ksmedia.h` ;
  - **GUID** : bindgen n'évalue pas les initialiseurs de structure, `PUT_GUIDS_HERE` /
    `initguid.h` ne sert à rien et chaque `DEFINE_GUID`/`DEFINE_GUIDSTRUCT` sortirait en
    `extern static` sans définition (symbole à lier). `build.rs` analyse donc lui-même
    les cinq en-têtes, émet chaque GUID en `pub const GUID` et bloque les `static`
    homonymes ; un `static … : GUID` restant fait échouer le build (liste complète par
    construction). Les valeurs sont contrôlées contre `cl.exe` par le golden ;
  - `DEVPKEY_*` (`DEFINE_DEVPROPKEY`, `ksmedia.h`) : mêmes `extern static`, bloqués ;
  - `IPortClsVersion` ne recopie pas `DEFINE_ABSTRACT_UNKNOWN()` : sa vtable C n'a qu'un
    slot au lieu de quatre. Bloquée et **redéfinie à la main dans `src/fixups.rs`**
    (`IUnknown` + `GetVersion`) ; c'est le seul rôle de ce module, avec la macro
    `PORT_CLASS_DEVICE_EXTENSION_SIZE` (`sizeof`, que bindgen n'évalue pas) ;
  - `IPortClsPower`, `IPortClsRuntimePower`, `IPortClsEtwHelper` : déclarées pour C++
    seulement (ni `THIS_` ni `IUnknown`), slots et signatures faux en C : exclues des
    bindings, à écrire à la main si M1b en a besoin ;
  - les assertions de disposition de bindgen (`layout_tests`) indexent un tableau dans un
    bloc `const` : `indexing_slicing` et `arithmetic_side_effects` sont autorisés dans
    `bindings.rs` seul.
  Le repli « vtables manuelles avec oracle bindgen C++ » n'a pas été nécessaire.
- Le `#[link(name = "portcls")]` n'est **pas** dans `portcls-sys` : c'est
  `conduit-kmd/build.rs` qui l'émet (`cargo::rustc-link-lib=portcls`).
- Tests (`cargo test -p portcls-sys` depuis `drivers/windows`, mode utilisateur, sans
  lier au noyau) : `tests/layout.rs` compare `size_of` de 27 structures et 14 vtables
  et la valeur de 11 GUID à `tests/layout.golden`, produit par `cl.exe` sur les mêmes
  en-têtes (`tools/sizeof-probe.c`, `drivers/windows/tools/regen-layout.ps1`, golden
  commité et contrôlé à jour par `tools/check.ps1`) ; `tests/vtables.rs` vérifie
  `lpVtbl` à l'offset 0, les trois slots `IUnknown` en tête, l'ordre des slots de
  `IMiniportWaveRT` et `IMiniportWaveRTStream[Notification]`, le nombre de slots de
  chaque vtable, `KSSTATE_RUN == 3` et la signature des rappels ; bindgen émet en outre
  ses propres assertions de disposition, vérifiées à la compilation.

### 2.3 `conduit-kmd` (workspace noyau)

`cdylib` `#![no_std]`, `panic = "abort"`, `[package.metadata.wdk.driver-model]
driver-type = "WDM"`, `build.rs` = `wdk_build::configure_wdk_binary_build()` plus
`cargo::rustc-link-lib=portcls`. `wdk-alloc` en allocateur global (pool non paginé, tag
`rust` imposé par le crate ; **n'honore pas les alignements supérieurs à 16**, donc les
tampons cycliques passent par `AllocatePagesForMdl`, jamais par `Box`). Gestionnaire de
panique **maison** (`panic.rs` : `KeBugCheckEx` avec un code privé, `DbgBreakPoint` avant
en debug) : `wdk-panic` 0.4.1 publié se contente d'un `loop {}` qui gèle la machine. Les
fonctions PortCls (`PcInitializeAdapterDriver`, `PcAddAdapterDevice`, `PcNewPort`,
`PcRegisterSubdevice`, `PcRegisterPhysicalConnection`, `PcNewResourceList`) sont déclarées
dans `portcls-sys` et liées ici.

Modules :

| Module | Rôle |
|---|---|
| `entry` | `DriverEntry`, `AddDevice`, `Unload` ; délègue à PortCls |
| `adapter` | `StartDevice` : lit les paramètres (M1b-01), crée la table des câbles, enregistre les sous-périphériques ; objet `IAdapterPowerManagement` |
| (hors crate) | le modèle objet COM générique est dans `crates/conduit-com`, les enveloppes PortCls dans `drivers/windows/portcls` (§3) |
| `cable` | état partagé par câble : flux rendu et capture courants, spin lock, timer/DPC de copie |
| `wave` | miniport `IMiniportWaveRT` (un par câble et par sens) et ses flux `IMiniportWaveRTStream` |
| `topo` | miniport `IMiniportTopology` (un par câble et par sens) : nœuds volume/mute factices, jack |
| `props` | gestionnaires de propriétés KS : `KSPROPERTY_JACK_DESCRIPTION`, `KSPROPERTY_AUDIO_*`, propriété privée de configuration (M1b-04) |
| `clock` | lecture QPC (`KeQueryPerformanceCounter`) et enveloppe des timers noyau |

## 3. Modèle objet COM en Rust

Répartition (révision 2026-09-05) : le **modèle générique** (`ComObject`, `ComRef`,
`IUnknown`, macros, comptage de références) est le crate portable `crates/conduit-com`
(`no_std` + `alloc`, aucune dépendance, `Guid` et vtable `IUnknown` définis localement avec
la même disposition que ceux du WDK) : il est testé, passé à Miri et couvert depuis Nix.
Les **enveloppes PortCls** (`drivers/windows/portcls`) relient un trait Rust sûr
(`MiniportTopology`, `AdapterPowerManagement`, puis `MiniportWaveRT`…) à la vtable générée
correspondante de `portcls-sys`, et enveloppent les interfaces reçues (`IPortWaveRT`,
`IResourceList`…) ; elles se testent en mode utilisateur avec de faux ports qui appellent
les vtables comme PortCls le ferait. `conduit-kmd` n'implémente que les traits.

PortCls dialogue avec le miniport par des interfaces COM (vtable C++ pure, convention
`__stdcall`, `IUnknown` en tête). Représentation :

```rust
#[repr(C)]
pub struct ComObject<V: 'static, T> {
    vtbl: &'static V,          // pointeur de vtable, offset 0 obligatoire
    refcount: AtomicU32,       // AddRef/Release
    inner: T,                  // état Rust de l'objet
}
```

- **Un objet Rust par interface** : pas d'héritage multiple. Le miniport WaveRT, le
  miniport topologie, chaque flux et l'objet d'alimentation sont des objets distincts.
  `QueryInterface` d'un objet ne répond qu'à `IID_IUnknown`, à son interface primaire et
  aux bases de celle-ci (`IMiniport` pour `IMiniportWaveRT`, préfixe de vtable
  compatible) ; tout autre IID → `STATUS_INVALID_PARAMETER` (comportement SYSVAD).
- **Allocation** : `Box::new` via l'allocateur global (pool non paginé). `Release`
  reconstruit le `Box` et le libère quand le compteur tombe à zéro. Aucun `Drop`
  n'appelle PortCls.
- **Références sortantes** : les pointeurs reçus de PortCls (`IPortWaveRT`,
  `IPortWaveRTStream`, `IResourceList`) sont enveloppés dans un `ComRef<V>` qui fait
  `AddRef` à la construction et `Release` au `Drop`, comme `_com_ptr_t`.
- **Macros** : `com_interface!` déclare une vtable et ses `IID` ; `impl_unknown!`
  fournit `QueryInterface`/`AddRef`/`Release` pour un `ComObject`. Les méthodes
  métier sont des `unsafe extern "system" fn(this: *mut ComObject<..>, ...)` qui
  récupèrent `&inner` et appellent une méthode Rust sûre renvoyant `Result<_, NTSTATUS>`.
- **IRQL** : chaque méthode porte un commentaire `// IRQL: PASSIVE_LEVEL` ou
  `// IRQL: <= DISPATCH_LEVEL`. Les méthodes appelables à `DISPATCH_LEVEL`
  (`GetPosition`, `GetPresentationPosition`, `SetState` partiellement, DPC) n'allouent
  pas, n'attendent pas et ne touchent que de la mémoire non paginée.

## 4. Sous-périphériques, topologie, INF

Par câble *n* (1 seul en M1a, 16 en M1b-02), quatre sous-périphériques PortCls :

| Nom | Port | Miniport | Rôle |
|---|---|---|---|
| `WaveRender<n>` | `IPortWaveRT` | `wave::Render` | le lecteur écrit ici |
| `TopoRender<n>` | `IPortTopology` | `topo::Render` | volume/mute factices, jack, bridge vers `WaveRender` |
| `WaveCapture<n>` | `IPortWaveRT` | `wave::Capture` | l'enregistreur lit ici |
| `TopoCapture<n>` | `IPortTopology` | `topo::Capture` | idem côté capture |

Connexions physiques (`PcRegisterPhysicalConnection`) : pin bridge de
`WaveRender<n>` → pin d'entrée de `TopoRender<n>` ; pin de sortie de `TopoCapture<n>` →
pin bridge de `WaveCapture<n>`. Sans elles, Windows ne construit pas d'endpoint.
Les nœuds de topologie sont réduits au minimum que le générateur d'endpoints accepte :
`KSNODETYPE_VOLUME`, `KSNODETYPE_MUTE`, `KSNODETYPE_SPEAKER` (rendu) /
`KSNODETYPE_LINE_CONNECTOR` (capture) et un jack. Référence de structure : SYSVAD
`SimpleAudioSample` (MIT), jamais copié.

INF : source `conduit.inx` (`cargo wdk build` le transforme en `.inf` par `stampinf`,
génère le `.cat` par `inf2cat`, le vérifie par `infverif` et signe `.sys` et `.cat` avec le
certificat de test `WDRLocalTestCert` qu'il crée au besoin). Classe `MEDIA`, périphérique
énuméré à la racine (`Root\ConduitCable`), `Include=ks.inf,wdmaudio.inf`,
`Needs=KS.Registration,WDMAUDIO.Registration`, interfaces `KSCATEGORY_AUDIO`,
`KSCATEGORY_RENDER`, `KSCATEGORY_CAPTURE`, `KSCATEGORY_REALTIME`, `FriendlyName`
« Conduit *n* » (M1a-09). Installation dans la VM : importer `WDRLocalTestCert.cer` dans
*Trusted Root* et *Trusted Publishers*, `pnputil /add-driver conduit.inf /install`, puis
créer le nœud racine avec `devgen /add /hardwareid "Root\ConduitCable"` (WDK 26100).
Retrait : `devgen /remove <id>` puis `pnputil /delete-driver oem<N>.inf /uninstall /force`.

## 5. Horloge, positions, boucle locale

### 5.1 Horloge

Un seul référentiel : le compteur de performance (`KeQueryPerformanceCounter`,
fréquence fixe). Au `SetState(KSSTATE_RUN)` d'un flux, `qpc_start` est mémorisé. La
position en trames est **calculée**, jamais comptée :

```
frames = ((qpc_now − qpc_start) × sample_rate) / qpc_freq      (u128, kmd-core::position)
bytes  = (frames × frame_size) mod buffer_size
```

`GetPosition` et `GetPresentationPosition` sont donc exacts à la trame près, sans
dépendre d'un timer. Pause/reprise : on accumule les trames jouées avant la pause.

### 5.2 Tampons cycliques

`AllocateAudioBuffer` : le miniport alloue un tampon **par flux** avec
`IPortWaveRTStream::AllocatePagesForMdl` (non paginé, contigu ou non, taille = multiple
de la trame, arrondie à la taille demandée par le moteur audio dans les bornes
[1 ms ; 100 ms]), et le mémorise dans l'état du câble. Le tampon est libéré à
`FreeAudioBuffer` (jamais avant l'arrêt du flux). Pas de partage de pages entre rendu
et capture : les tailles et les formats des deux flux peuvent différer.

### 5.3 Boucle locale

Un **timer noyau périodique par câble** (période 1 ms, `KeSetTimerEx` avec DPC ;
`ExSetTimerResolution` n'est pas touché), armé quand au moins un flux du câble est en
`RUN`, désarmé sinon. À chaque DPC (`DISPATCH_LEVEL`, sous le spin lock du câble) :

1. calculer la position courante du rendu et de la capture ;
2. copier du tampon rendu vers le tampon capture les trames comprises entre la
   dernière position copiée et la position de rendu courante moins une marge d'une
   période (`kmd-core::ring::copy`) ; convertir le format si les deux diffèrent ;
3. si le rendu n'est pas en `RUN`, écrire du silence dans la capture (SPEC §5.3 :
   « l'entrée sans producteur lit du silence ») ;
4. si aucune capture n'est en `RUN`, ne rien copier (« la sortie sans lecteur est
   jetée ») ;
5. signaler les événements de notification enregistrés par
   `IMiniportWaveRTStreamNotification::RegisterNotificationEvent` quand une période de
   notification est franchie.

Latence de traversée = période du moteur audio (10 ms en mode partagé) + marge de
copie (1 à 2 ms). Le tampon interne « 2 × 10 ms » de SPEC §5.3 est la taille par défaut
demandée au moteur, pas un tampon supplémentaire du pilote.

### 5.4 Formats

M1a : 48 kHz, 2 canaux, float32 et PCM16, un seul `KSDATARANGE_AUDIO` par format.
M1b-05 : 44,1/48/96 kHz, float32, PCM16, PCM24, canaux 1 à 8 selon la configuration du
câble. Le mode partagé ne voit que le format du moteur ; les autres servent au mode
exclusif.

## 6. Surface de contrôle (M1b-04, décision anticipée)

Pas d'objet de périphérique de contrôle séparé ni de dispatch d'IRP personnalisé :
la configuration passe par un **jeu de propriétés KS privé** (`KSPROPSETID_Conduit`,
GUID généré une fois et figé dans `conduit-kmd-core::config`) exposé par le filtre de
topologie de chaque câble. Le helper ouvre l'interface `KSCATEGORY_TOPOLOGY` du câble
et envoie `IOCTL_KS_PROPERTY` (`KSPROPERTY_CONDUIT_CABLE_STATE` get/set,
`KSPROPERTY_CONDUIT_VERSION` get). Avantages : PortCls fait tout le routage, l'accès
se fait par les handles standard, la validation est un simple parseur sur un tampon
borné (fuzzable en mode utilisateur, M1b-08).

Contrôle d'accès : le gestionnaire de propriété s'exécute dans le contexte du thread
appelant ; toute écriture exige `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`
(le helper tourne en `LocalSystem`), sinon `STATUS_PRIVILEGE_NOT_HELD`, sans effet.
Toute valeur invalide → `STATUS_INVALID_PARAMETER`, sans effet.

L'état « actif » d'un câble est **persisté par le pilote** dans le registre du
périphérique (`PcGet/SetDeviceProperty` ou clé `Parameters`) pour que la fonction
câble marche sans le démon (F-05) après redémarrage.

## 7. Gestion d'erreurs et robustesse

- Aucune panique atteignable : `unwrap`, `expect`, indexation non vérifiée et
  arithmétique débordante sont refusés par lint dans `conduit-kmd` et `kmd-core`.
  Le gestionnaire `wdk-panic` reste en filet de sécurité.
- Tout `NTSTATUS` d'échec de PortCls est propagé ; `StartDevice` échoue proprement
  (PortCls libère ce qui a été enregistré) plutôt que de charger un adaptateur
  incomplet.
- Journalisation : `WPP` n'est pas disponible côté Rust ; le spike utilise
  `wdk::println!` (`DbgPrint`, préfixe `conduit_kmd:`) derrière une macro `kmd_log!`
  (`conduit-kmd/src/log.rs`) vide en release ; `DbgPrintEx` avec `DPFLTR_IHVAUDIO_ID`
  reste une option si le filtrage devient nécessaire. M1b évalue `EtwWrite` via
  `wdk-sys`.
- Tags de pool distincts par famille d'objets (`CnKm`, `CnSt`, `CnBf`) pour suivre les
  fuites avec `!poolused` et Driver Verifier.

## 8. Plan de test (par niveau)

| Niveau | Où | Comment |
|---|---|---|
| Logique | `conduit-kmd-core` | `cargo test`, proptest, Miri, fuzz ; **tourne dans `nix flake check`** |
| Dispositions | `portcls-sys` | `cargo test` en mode utilisateur, Windows + WDK, golden `cl.exe` (`tests/layout.golden`, §2.2) |
| Chargement | VM Hyper-V | `tools/vm-cycle.ps1` : install/désinstall × 100 (M1a-02), dumps collectés |
| Fonctionnel | VM | outil `conduit-looptest` (WASAPI, workspace racine, `cfg(windows)`) : sinus → rendu → capture, vérifie fréquence, phase, trous (M1a-10) |
| Robustesse | VM | Driver Verifier (standard + special pool + IRQL) 1 h (M1a-11), 48 h (M1b-09) |
| Conformité | VM | HLK audio (M1b-10) |

Aucun pilote de test ne se charge sur la machine de développement.

## 9. Ce que le spike doit trancher (entrées d'ADR-009 bis / M1a-12)

1. `windows-drivers-rs` compile-t-il un WDM PortCls sans nightly et sans patch ?
2. Les vtables manuelles passent-elles les tests d'offsets et PortCls accepte-t-il les
   objets (`PcNewPort` + `RegisterSubdevice` réussissent, endpoints visibles) ?
3. Lecture et boucle locale stables 1 h sous Driver Verifier ?
4. Coût réel : jours passés par tâche, comparés au délai borné.

Succès sur les quatre → M1b en Rust. Échec sur 1 ou 2 dans le délai → ADR de repli
C++/SYSVAD ; `conduit-kmd-core` et le helper sont conservés tels quels.
