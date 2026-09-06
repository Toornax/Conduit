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
| `format` | validation d'un `KSDATAFORMAT_WAVEFORMATEXTENSIBLE` demandé contre la liste supportée ; taille de tampon bornée, alignée sur la période de notification | tables de cas, proptest |
| `loopback` | plan de copie d'un tick (M1a-08) : `Loopback::plan(rendu, capture, avance)` → copie, silence, rien, ou débordement ; curseur et lien « même instant virtuel » entre les deux flux (§5.3) | tables de cas, proptest (blocs contigus sans trou ni recouvrement, décalage constant, bornes des tampons, jamais de panique) |
| `notify` | périodes de notification (M1a-07) : `Notifier::advance(frames)` dit si une frontière de `buffer_frames / count` a été franchie depuis le dernier signal, sur la position absolue | tables de cas, proptest (cohérence avec la formulation cyclique en octets) |
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
| `descriptors` | tables KS `static` des quatre filtres (§4.1), construites en `const`, invariants en assertions `const` |
| `cable` | état partagé par câble : emplacements des flux rendu et capture courants sous le spin lock du câble, `StreamState` (état verrouillé d'un flux tel que le tick le voit), plan `Loopback` et compteurs ; `Cable::on_tick` fait la copie et les notifications (M1a-08, §5.3) |
| `wave` | miniport `IMiniportWaveRT` (un par câble et par sens) : `NewStream` valide le format et crée le flux |
| `stream` | flux `WaveStream` (`IMiniportWaveRTStreamNotification`), un seul type pour les deux sens (`cable::Direction`) : tampon MDL, position QPC, notifications ; sans timer propre depuis M1a-08 |
| `topo` | miniport `IMiniportTopology` (un par câble et par sens) : nœuds volume/mute factices, jack |
| `props` | gestionnaires de propriétés KS : `KSPROPERTY_JACK_DESCRIPTION`, `KSPROPERTY_AUDIO_*`, propriété privée de configuration (M1b-04) |
| `clock` | lecture QPC (`KeQueryPerformanceCounter`), construction de la `VirtualClock` de `kmd-core` |
| `sync` | `SpinLock<T>` sur `KSPIN_LOCK` (`KeAcquireSpinLockRaiseToDpc`/`KeReleaseSpinLock`), garde RAII qui restaure l'IRQL |
| `timer` | `ExTimer` : timer haute résolution par câble (`ExAllocateTimer(EX_TIMER_HIGH_RESOLUTION)`, `ExSetTimer`, `ExCancelTimer`, `ExDeleteTimer`), rappel `EXT_CALLBACK` à `DISPATCH_LEVEL` |

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
La vtable de chaque type implémenteur est une **constante associée** d'un trait
compagnon (`impl<T: MiniportTopology> TopologyVtbl for T { const VTBL: IMiniportTopologyVtbl
= … }`, slots = thunks génériques instanciés pour `T`), dont `&T::VTBL` est promu en
`&'static` par le compilateur : ni `static` par type, ni macro. La feature **`com`** de
`portcls-sys` (module `com`, désactivée par défaut) déclare les vtables et interfaces
générées `ComVtable`/`ComInterface` avec leurs tables d'IID (bases par préfixe de
vtable) ; c'est la seule dépendance de `portcls-sys` hors scripts de build, et elle reste
optionnelle.
M1a-05 (2026-09-06) : `MiniportWaveRT`, `MiniportWaveRTStream`,
`MiniportWaveRTStreamNotification` et les enveloppes reçues `PortWaveRT`,
`PortWaveRTStream` (`AllocatePagesForMdl`, `MapAllocatedPages`, `FreePagesFromMdl`…)
suivent le même schéma ; les deux slots hérités de `IMiniport` ont des thunks communs
(`portcls::miniport`, trait `MiniportSlots` implémenté par vtable). `NewStream` rend un
**`StreamObject`**, pointeur COM possédé à type effacé (`ComRef<IMiniportWaveRTStream>`),
construit par `From` depuis l'objet typé (`StreamPtr<T>` ou `StreamNotificationPtr<T>`,
dont la vtable a celle du flux simple pour préfixe) : le thunk le transfère tel quel dans
`*Stream`, nul en cas d'erreur, et PortCls découvre la notification par `QueryInterface`.
Les enveloppes des fonctions `Pc*` (`portcls::adapter` : `new_port`,
`register_subdevice`, `register_physical_connection`) appellent des symboles de
`portcls.sys` que seul le pilote lie : elles n'existent que sous la feature **`kernel`**
de `portcls`, activée par `conduit-kmd` ; `port_init` (`IPort::Init`), `as_unknown` et les
noms UTF-16 des sous-périphériques (`WAVE_RENDER_0`…) restent testés en mode utilisateur.

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

INF : source `conduit-kmd/conduit_kmd.inx` — le nom est imposé par cargo-wdk, qui le
transforme en `.inf` par `stampinf`, génère le `.cat` par `inf2cat`, le vérifie par
`infverif /w` et signe `.sys` et `.cat` avec le certificat de test `WDRLocalTestCert`
qu'il crée au besoin. Classe `MEDIA`, périphérique énuméré à la racine
(`Root\ConduitCable`), `Include=ks.inf,wdmaudio.inf`,
`Needs=KS.Registration,WDMAUDIO.Registration`, interfaces `KSCATEGORY_AUDIO`,
`KSCATEGORY_RENDER`, `KSCATEGORY_CAPTURE`, `KSCATEGORY_REALTIME`, `KSCATEGORY_TOPOLOGY`
par sous-périphérique, `DeviceType` `FILE_DEVICE_SOUND` et descripteur de sécurité de
l'objet de périphérique dans `.NT.HW`, et l'enregistrement du **nom de broche** qui donne
leur nom aux endpoints (§4.2). Installation dans la VM : importer `WDRLocalTestCert.cer`
dans *Trusted Root* et *Trusted Publishers*,
`pnputil /add-driver conduit_kmd.inf /install`, puis créer le nœud racine avec
`devgen /add /hardwareid "Root\ConduitCable"` (WDK 26100). Retrait : `devgen /remove <id>`
puis `pnputil /delete-driver oem<N>.inf /uninstall /force`.

Le fichier `.inx` est encodé en **UTF-16 LE** dans la copie de travail : un INF qui
contient des caractères non ASCII doit l'être (*General Guidelines for INF Files* :
« if your INF contains non-ASCII characters, you must save the file as a Unicode
(UTF-16 LE) file »), sans quoi SetupAPI le relit dans la page de codes ANSI et
« câbles » devient « câbles ». `.gitattributes` (`*.inx text
working-tree-encoding=UTF-16LE-BOM eol=crlf`) garde le dépôt en UTF-8 — les diffs restent
lisibles — et restitue l'UTF-16 à l'extraction ; `stampinf` recopie l'encodage tel quel
dans le `.inf` produit et normalise les fins de ligne en CRLF. Le test
`portcls/tests/inf.rs` échoue si la copie de travail perd le BOM.

Rien dans l'INF n'est **supprimé** explicitement à la désinstallation (F-52) : toutes ses
écritures sont des `HKR` — clé logicielle ou matérielle du périphérique — que PnP retire
avec lui, et les fichiers vivent dans le magasin des pilotes (DIRID 13, `PnpLockdown=1`),
effacés par `pnputil /delete-driver`. Ni `DelReg` ni `DelFiles` ne sont donc nécessaires,
et une écriture hors des clés du périphérique serait de toute façon refusée par
`infverif /w`.

Ce que l'INF **ne fait pas**, et pourquoi :

Les valeurs héritées `HKR,Drivers,SubClasses` et `HKR,Drivers\wave|midi|mixer\wdmaud.drv`
que SYSVAD écrit ne sont **pas** reprises : elles décrivent des sous-classes wave, MIDI et
mixer que ce pilote n'expose pas (aucun nœud MIDI, aucun nœud de mixage avant M1b-03) et
le chemin qui construit les endpoints ne les lit pas. `AssociatedFilters` suffit à déclarer
les filtres attendus par wdmaud. À revoir en M1b-03 si une application MME s'avère les
exiger.

Pas de `[SignatureAttributes]` `DRMLevel` non plus : le pilote n'implémente aucun contenu
protégé (`IDrmPort`, `PcAddContentHandlers`) et le déclarer serait faux.

### 4.1 Descripteurs de filtres (M1a-06, minimum accepté par le générateur d'endpoints)

Le spike enregistre le strict nécessaire pour qu'un endpoint rendu et un endpoint capture
apparaissent ; volume, mute et jack (M1b-03) viendront après. Toutes les tables sont des
`static` `#[repr(C)]` de `portcls-sys` construites en `const` dans `conduit-kmd::descriptors`.

**Plages de formats** (`KSDATARANGE_AUDIO`, communes aux broches système) : type
`KSDATAFORMAT_TYPE_AUDIO`, spécificateur `KSDATAFORMAT_SPECIFIER_WAVEFORMATEX`, 2 canaux,
48 000 Hz min et max, une entrée par sous-type : `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` (32 bits)
et `KSDATAFORMAT_SUBTYPE_PCM` (16 bits). Plage « analogique » des broches bridge : type audio,
sous-type `KSDATAFORMAT_SUBTYPE_ANALOG`, spécificateur `KSDATAFORMAT_SPECIFIER_NONE`
(`KSDATARANGE` simple).

| Filtre | Broche | Flux | Communication | Catégorie / rôle |
|---|---|---|---|---|
| `WaveRender<n>` | 0 | `KSPIN_DATAFLOW_IN` | `KSPIN_COMMUNICATION_SINK` | `KSCATEGORY_AUDIO`, plages système : le lecteur écrit ici |
| | 1 | `KSPIN_DATAFLOW_OUT` | `KSPIN_COMMUNICATION_NONE` | bridge, plage analogique |
| `TopoRender<n>` | 0 | `KSPIN_DATAFLOW_IN` | `KSPIN_COMMUNICATION_NONE` | bridge, plage analogique |
| | 1 | `KSPIN_DATAFLOW_OUT` | `KSPIN_COMMUNICATION_NONE` | catégorie `KSNODETYPE_SPEAKER` : c'est l'endpoint |
| `WaveCapture<n>` | 0 | `KSPIN_DATAFLOW_IN` | `KSPIN_COMMUNICATION_NONE` | bridge, plage analogique |
| | 1 | `KSPIN_DATAFLOW_OUT` | `KSPIN_COMMUNICATION_SINK` | `KSCATEGORY_AUDIO`, plages système : l'enregistreur lit ici |
| `TopoCapture<n>` | 0 | `KSPIN_DATAFLOW_IN` | `KSPIN_COMMUNICATION_NONE` | catégorie `KSNODETYPE_LINE_CONNECTOR` : c'est l'endpoint |
| | 1 | `KSPIN_DATAFLOW_OUT` | `KSPIN_COMMUNICATION_NONE` | bridge, plage analogique |

Nœuds : aucun en M1a (connexion directe broche 0 → broche 1 via `PCFILTER_NODE` dans
`PCCONNECTION_DESCRIPTOR`). Connexions physiques après enregistrement des quatre
sous-périphériques : `WaveRender<n>` broche 1 → `TopoRender<n>` broche 0 ;
`TopoCapture<n>` broche 1 → `WaveCapture<n>` broche 0. Chaque broche système déclare
`KSPIN_DATAFLOW` correct et 1 instance possible (`MaxGlobalInstanceCount = 1`,
`MaxFilterInstanceCount = 1`) ; broches bridge et endpoint : 0 instance, catégorie
`KSCATEGORY_AUDIO` pour les bridges (comme SYSVAD). Tables d'automatisation : vides pour
le spike (`PCAUTOMATION_TABLE` sans propriétés ; PortCls gère `KSPROPSETID_Pin` et
`KSPROPSETID_Topology`). Catégories du filtre : `CategoryCount = 0`, PortCls fournit les
siennes (`KSCATEGORY_AUDIO`, `RENDER`/`CAPTURE`, `REALTIME` pour WaveRT ; `AUDIO`,
`TOPOLOGY` pour la topologie) ; l'INF les publie par `AddInterface` (M1a-06).

Réalité M1a-09 : les deux broches endpoint (`TopoRender<n>` broche 1, `TopoCapture<n>`
broche 0) portent en plus un GUID `KsPinDescriptor.Name` — les six autres broches laissent
le champ nul. C'est lui qui donne son nom à l'endpoint (§4.2).

Réalité des bindings (M1a-06) : `PCFILTER_NODE` (`((ULONG)-1)`) et les `WAVE_FORMAT_*`
de `mmreg.h` ne sortent pas de bindgen, ils sont recopiés dans `portcls-sys::fixups` ;
`PCFILTER_DESCRIPTOR_VERSION` n'existe pas dans `portcls.h` 26100, `Version` vaut 0
comme dans SYSVAD. Les numéros de broche sont nommés **par filtre**
(`WAVE_RENDER_PIN_SYSTEM = 0`, `WAVE_RENDER_PIN_BRIDGE = 1`, `TOPO_RENDER_PIN_BRIDGE = 0`,
`TOPO_RENDER_PIN_ENDPOINT = 1`, `WAVE_CAPTURE_PIN_BRIDGE = 0`, `WAVE_CAPTURE_PIN_SYSTEM = 1`,
`TOPO_CAPTURE_PIN_ENDPOINT = 0`, `TOPO_CAPTURE_PIN_BRIDGE = 1`) : la broche 0 est toujours
l'entrée, ce qui interdit une constante « bridge » commune aux deux sens.

**Séquence `StartDevice`** (adaptateur, IRQL `PASSIVE_LEVEL`), pour chaque câble *n* :

1. `port = PcNewPort(CLSID_PortWaveRT)` ; `mini = new_wavert_object(WaveRender { n, cable })` ;
   `port.Init(device, irp, mini.as_unknown(), None, resources)` ;
   `PcRegisterSubdevice(device, "WaveRender<n>", port.as_unknown())`.
2. Idem avec `CLSID_PortTopology` et `new_topology_object(TopoRender { n })`, nom `TopoRender<n>`.
3. Idem pour `WaveCapture<n>` et `TopoCapture<n>`.
4. `PcRegisterPhysicalConnection(device, wave_render_port, 1, topo_render_port, 0)` et
   `PcRegisterPhysicalConnection(device, topo_capture_port, 1, wave_capture_port, 0)`.

Tout `NTSTATUS` d'échec interrompt la séquence et est renvoyé : PortCls détruit ce qui a été
enregistré. Les objets miniport sont possédés par leur port (PortCls prend ses références) ;
l'adaptateur ne conserve que l'état partagé des câbles (`cable::Cable`, §5.3), qui est une
**`static`** du pilote (construction `const`, section de données non paginée, vit jusqu'au
déchargement : ni allocation, ni fuite de pool, ni `Drop` ; les cycles start/stop la
réutilisent). Ses deux `Slot` (flux rendu et capture courants) sont des `AtomicPtr` ;
M1a-08 fixera comment le tick garantit la survie de l'état pointé.
`PcRegisterAdapterPowerManagement` n'est pas appelé avant M1b-06. `NewStream` valide le
format (`kmd-core::format::validate`) puis crée le flux, dans les deux sens (§5).

Réalité M1a-08 : les emplacements du câble ne sont plus des `AtomicPtr` mais deux pointeurs
sous un **spin lock du câble** (`Cable::attach`/`detach`/`state`), car l'atomique seul ne
garantit pas la survie de l'état pointé. Contrat : le pointeur désigne le
`SpinLock<StreamState>` logé dans l'objet COM du flux ; le flux se retire de l'emplacement
avant sa destruction (dans son `Drop`) en prenant ce même verrou, donc tout lecteur qui
tient la garde des emplacements a la garantie que le flux qu'elle désigne vit encore.
Ordre de verrouillage fixe : **câble puis flux** — et, entre les deux flux, rendu puis
capture, seul `Cable::on_tick` prenant les deux. `StartDevice` appelle `Cable::start`, qui
oublie les flux d'un cycle précédent et crée le timer haute résolution du câble (§5.3) ;
`DriverUnload` appelle `cable::shutdown`, qui le supprime.

### 4.2 Noms d'endpoint (M1a-09)

**D'où vient le nom qu'un utilisateur voit.** Le service *AudioEndpointBuilder* surveille
la classe d'interface `KSCATEGORY_AUDIO` ; à l'arrivée d'une interface il cherche « any
unconnected bridge pins », crée un endpoint pour chacune, puis « sets the default
properties for the endpoint. For example, AudioEndpointBuilder sets the name, icon, and
the form factor » (*Audio Endpoint Builder Algorithm*). La règle de nommage y est
explicite : « The naming convention that is used for the endpoints is based on the
friendly names of the bridge pins. »

Le nom d'une broche vient de `KSPROPERTY_PIN_NAME`, que KS traite lui-même — « the client
uses KSPROPERTY_PIN_NAME to retrieve the **Registry name** of a pin factory » — en
cherchant, dans l'ordre (*Friendly Names for Audio Endpoint Devices*) :

1. une chaîne pour le GUID `KsPinDescriptor.Name` de la broche ;
2. à défaut, une chaîne pour son GUID `KsPinDescriptor.Category`.

Et, depuis Windows 10 1809, la recherche commence par la **clé logicielle du
périphérique** — « KS first looks for an entry in the device's software key. This is
created by the INF through an AddReg section referenced by the [Models] section […] using
the HKR\MediaCategories key » — avant de retomber sur l'espace de noms global
`HKLM\SYSTEM\CurrentControlSet\Control\MediaCategories`, « reserved for global definitions
and should not be modified by new drivers ».

Côté application, MMDevice compose trois propriétés (*Device Properties*, Core Audio) :

| Propriété | Contenu | Origine |
|---|---|---|
| `PKEY_Device_DeviceDesc` | « Speakers » | le nom de la broche bridge ci-dessus |
| `PKEY_DeviceInterface_FriendlyName` | « XYZ Audio Adapter » | le nom de l'adaptateur |
| `PKEY_Device_FriendlyName` | « Speakers (XYZ Audio Adapter) » | les deux, entre parenthèses |

C'est `PKEY_Device_FriendlyName` que les réglages Son de Windows 11 affichent. La
documentation le confirme côté endpoint : « the property store copies its initial value
for the PKEY_Device_DeviceDesc property key from the friendly name string that is
associated with the KS pin category GUID in the registry ».

**Conséquence pour Conduit.** Nos deux broches endpoint portaient les catégories
`KSNODETYPE_SPEAKER` et `KSNODETYPE_LINE_CONNECTOR` et **aucun** GUID `Name` : KS serait
donc retombé sur la catégorie, résolue par les entrées que `ks.inf` (`KS.Registration` →
`PinNameRegistration`) écrit dans l'espace global. Vérifié sur le poste de développement
(Windows 11 fr-FR) :

```text
HKLM\SYSTEM\CurrentControlSet\Control\MediaCategories\{DFF21CE1-F70F-11D0-B917-00A0C9223196}  Name = Haut-parleurs
HKLM\SYSTEM\CurrentControlSet\Control\MediaCategories\{DFF21FE3-F70F-11D0-B917-00A0C9223196}  Name = Ligne
```

Les endpoints se seraient donc appelés « Haut-parleurs (…) » et « Ligne (…) ».

**L'INF seul ne suffit pas.** Le `FriendlyName` que les sections `.Interfaces` posent sur
chaque interface KS (`HKR,,FriendlyName`) nomme le **filtre** dans le registre des
interfaces de périphérique, pas l'endpoint : aucun texte de la documentation ne le relie
au nom composé ci-dessus, et SYSVAD y met des libellés (« Simple Audio Sample Topology
Speaker ») qui n'apparaissent nulle part dans les réglages Son. Il reste utile au
diagnostic, et c'est à ce titre que Conduit le renseigne sur les quatre interfaces.

Le seul levier documenté est donc le couple **GUID de nom de broche + chaîne dans
`HKR\MediaCategories`** :

- côté pilote, les deux broches endpoint (`TopoRender<n>` broche 1, `TopoCapture<n>`
  broche 0) déclarent `KsPinDescriptor.Name = portcls::pin_name_guid(n)` — un GUID propre
  à Conduit dont le dernier octet porte le numéro de câble, ce qui donnera les seize de
  M1b-02 ;
- côté INF, `[ConduitCable_PinNames_AddReg]` écrit
  `HKR,%MediaCategories%\%GUID.PinName.Cable0%,Name,,"Conduit 1"`.

Il n'y a **rien à implémenter** de `KSPROPERTY_PIN_NAME` : KS y répond seul à partir du
registre. `KSJACK_DESCRIPTION` ne joue aucun rôle dans le nom — il décrit le connecteur
physique (type, couleur, emplacement, présence) et servira en M1b-03.

Les deux valeurs se trouvent de part et d'autre d'une frontière qu'aucun compilateur ne
franchit ; `portcls/tests/inf.rs` (lancé par `tools/check.ps1` et la CI) relit l'INX et
compare, de même que pour les noms de sous-périphériques des `AddInterface` et pour les
GUID `KSCATEGORY_*`.

**À constater dans la VM.** Deux points restent à observer, faute de machine de test :

1. Le nom composé attendu est « Conduit 1 (Conduit — câbles audio virtuels) », le second
   membre venant du `DeviceDesc` de l'adaptateur. Si les parenthèses gênent, c'est le
   `DeviceDesc` qu'il faudra raccourcir.
2. La page *Audio Endpoint Builder Algorithm* — antérieure au mécanisme de 1809 — affirme
   que « in the case of speaker endpoints, the name has been hardcoded to "Speakers" and
   cannot be altered by your driver or a third-party application ». Elle décrit le nom tiré
   de la **catégorie** ; la page *Friendly Names* (2022) donne la priorité au GUID `Name`.
   Si malgré tout l'endpoint de rendu s'affiche « Haut-parleurs », le repli est de changer
   la catégorie de sa broche endpoint (`KSNODETYPE_LINE_CONNECTOR` au lieu de
   `KSNODETYPE_SPEAKER`), au prix de l'icône et du rang de sélection par défaut (rendu :
   Speakers > Line-out > SPDIF).

Sources : *Audio Endpoint Builder Algorithm*, *Friendly Names for Audio Endpoint Devices*,
*Pin Category Property*, *KSPROPERTY_PIN_NAME* (WDK), *Device Properties* et
*PKEY_DeviceInterface_FriendlyName* (Core Audio), *General Guidelines for INF Files*, plus
`%SystemRoot%\INF\ks.inf` et le registre du poste pour les noms de catégorie constatés.

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

Réalité M1a-08 (`stream::WaveStream`, les deux sens) : la fréquence QPC est lue une fois à
`NewStream` (`clock::virtual_clock`, `VirtualClock` de `kmd-core`) ; `SetState(RUN)` fait
`StreamPosition::run(qpc_now)`, `PAUSE`/`ACQUIRE` depuis `RUN` `pause(clock, qpc_now)`
(accumulation), `STOP` `reset()` — le tampon est **conservé** à `STOP`, PortCls le libère
par `FreeAudioBuffer`. Les transitions non voisines sont tolérées (SYSVAD) et journalisées.
`GetPosition` (`<= DISPATCH_LEVEL`) prend le spin lock du flux, calcule `frames_at` puis
`byte_offset` ; 0 sans tampon. `GetHWLatency` répond trois zéros.

### 5.2 Tampons cycliques

`AllocateAudioBuffer` : le miniport alloue un tampon **par flux** avec
`IPortWaveRTStream::AllocatePagesForMdl` (non paginé, contigu ou non, taille = multiple
de la trame, arrondie à la taille demandée par le moteur audio dans les bornes
[1 ms ; 100 ms]), et le mémorise dans l'état du câble. Le tampon est libéré à
`FreeAudioBuffer` (jamais avant l'arrêt du flux). Pas de partage de pages entre rendu
et capture : les tailles et les formats des deux flux peuvent différer.

Réalité M1a-08 : taille par `kmd-core::format::buffer_bytes` (sans notification) ou
`buffer_bytes_for_notifications` (arrondie en plus à un multiple de `count` trames, pour
que chaque période de notification soit entière et que la fin du tampon soit une
frontière) ; `AllocatePagesForMdl(high = i64::MAX, bytes)` — pas `0xFFFF_FFFF_FFFF_FFFF`,
qui serait −1 dans le `QuadPart` signé —, contrôle de `GetPhysicalPagesCount` (allocation
partielle → `STATUS_INSUFFICIENT_RESOURCES`), `MapAllocatedPages(MmCached)` pour obtenir la
base noyau, tampon mis à zéro, `OffsetFromFirstPage = 0`. Un seul tampon par flux (second
appel → `STATUS_INVALID_DEVICE_REQUEST`). `FreeAudioBuffer` démappe puis
`FreePagesFromMdl`, **uniquement à `KSSTATE_STOP`** ; sinon le tampon est conservé et
libéré au `Drop` du flux (journalisé : ne doit pas arriver).

### 5.3 Boucle locale

Un **timer noyau périodique par câble** (période 1 ms, timer `Ex*` haute résolution :
`ExAllocateTimer(…, EX_TIMER_HIGH_RESOLUTION)` + `ExSetTimer(timer, −10 000, 10 000, NULL)`
— les deux valeurs en unités de 100 ns, contrairement à `KeSetTimerEx` dont la période est
en millisecondes ; **confirmé sur la documentation Microsoft d'`ExSetTimer` le 2026-09-06**,
inutile de le revérifier —, rappel `EXT_CALLBACK` à `DISPATCH_LEVEL` ;
`ExSetTimerResolution` n'est jamais touché, son effet est global), armé quand au moins un
flux du câble est en `RUN` avec un tampon, désarmé sinon. Deux contraintes de la même
documentation : l'échéance d'un timer haute résolution **doit** être relative (négative),
sous peine de bug check ; et une période plus courte que le tic d'horloge système par
défaut (15,6 ms) maintient l'horloge système au taux maximal tant que le timer tourne,
donc une consommation accrue. C'est assumé (le moteur audio de Windows fait de même quand
un flux joue, et notre timer ne tourne que câble actif), à revoir si l'autonomie sur
batterie devient un sujet (SPEC §6). À chaque tick (`DISPATCH_LEVEL`, sous le spin lock du câble puis celui de chaque
flux, ordre fixe câble → rendu → capture) :

1. calculer la position absolue en trames du rendu (`R`) et de la capture (`C`) ;
2. écrire dans le tampon de capture les trames **`[curseur, C + avance)`**, prises aux
   trames de rendu correspondantes (`kmd-core::ring::copy_frames`, qui convertit le
   format si les deux diffèrent) ; le curseur devient `C + avance` ;
3. si le rendu n'est pas en `RUN`, écrire du silence dans la capture au lieu de la copie
   (SPEC §5.3 : « l'entrée sans producteur lit du silence ») ;
4. si aucune capture n'est en `RUN`, ne rien écrire (« la sortie sans lecteur est
   jetée ») ;
5. signaler les événements de notification enregistrés par
   `IMiniportWaveRTStreamNotification::RegisterNotificationEvent`, pour les **deux** flux,
   quand une période de notification est franchie.

**Écrire et lire en avance, pas en retrait.** Le moteur audio écrit le tampon de rendu
*devant* la position de lecture `R` et, réveillé par la notification d'une frontière de
période `R_b`, réécrit aussitôt les emplacements des trames `[R_b − période, R_b)` qu'il
vient de jouer. Copier *derrière* `R` avec une marge (`R − 1 ms`) laisserait donc écraser
chaque dernière milliseconde jouée avant sa copie. Symétriquement, une trame écrite dans
la capture sous la position `C` arrive trop tard : le moteur lit *derrière* `C`. La copie
travaille donc **en avance des deux positions** — `avance` = 2 ms
(`kmd-core::loopback::LEAD_MS`, la période du tick plus sa gigue). Ces trames de rendu ont
été écrites par le lecteur au moins une période plus tôt et ne seront réécrites qu'une
fois jouées ; côté capture elles sont en place avant que `C` ne les atteigne, et leur
emplacement a été lu une période plus tôt (tampon d'au moins deux périodes : c'est ce que
le moteur demande). Tant que `avance` reste inférieure à une période de notification, elle
ne touche jamais la moitié en cours d'écriture ou de lecture par le moteur.

**Décalage fixe.** Au premier tick où les deux flux sont en `RUN`, le plan mémorise
`(R0, C0)` : la trame de capture `j` reçoit la trame de rendu `k = j − C0 + R0`, « même
instant virtuel ». Les deux positions avancent au même rythme (même compteur de
performance, même fréquence d'échantillonnage) : la latence en positions est constante et
nulle. Le lien est oublié dès que l'un des deux flux quitte `RUN` et rétabli au retour ; il
**survit** en revanche à un débordement.

**Débordement.** Si un tick a pris tant de retard que le bloc à écrire dépasse le plus
petit des deux tampons, les trames sont irrécupérables (déjà réécrites d'un côté, déjà lues
de l'autre) : rien n'est écrit, le curseur est resynchronisé sur `C + avance` et le
compteur `overruns` est incrémenté. C'est un trou dans la capture, pas un décalage.

Latence de traversée = période du moteur audio (10 ms en mode partagé) + avance de copie
(2 ms). Le tampon interne « 2 × 10 ms » de SPEC §5.3 est la taille par défaut demandée au
moteur, pas un tampon supplémentaire du pilote.

Réalité M1a-08 : toute l'arithmétique est dans `kmd-core::loopback` (`Loopback::plan` :
curseur, lien, `CopyOp`/`SilenceOp`/`overrun`), testée et vérifiée par proptest sans
noyau — blocs contigus sans trou ni recouvrement, décalage constant, `count` borné par le
plus petit tampon, aucune panique. `conduit-kmd` ne fait que lire des positions et déplacer
des octets : `cable::Cable::on_tick` construit deux `StreamView`, applique le plan avec
`ring::copy_frames` / `ring::silence` sur des tranches obtenues par
`slice::from_raw_parts[_mut]` des mappages noyau des deux MDL (mémoire partagée avec le
client : octets lus et écrits sans hypothèse d'atomicité, comme tout pilote WaveRT), puis
appelle `KeSetEvent` pour les événements des flux dont une frontière de période est
franchie (`kmd-core::notify::Notifier`, sur la position absolue en trames : le bouclage
n'est pas un cas particulier puisque le tampon est un multiple de la période). Quatre
compteurs atomiques par câble (ticks, trames copiées, silences, débordements) sont
journalisés toutes les 1000 ticks — **en debug seulement** (`kmd_log!` est vide en
release). Le flux (`stream::WaveStream`, un seul type pour les deux sens, distingués par
`cable::Direction`) n'a plus de timer : après chaque transition il appelle
`Cable::refresh_timer` **hors de son propre verrou** (ordre câble → flux), et son `Drop`
se contente de `Cable::detach` — le spin lock du câble garantit qu'au retour aucun tick ne
détient plus le pointeur, ce qui remplace le `KeFlushQueuedDpcs` de M1a-07.

Décision (2026-09-06, révisée à la livraison de M1a-08, sans attendre la VM) : un `KTIMER`
de 1 ms n'a que la résolution de l'horloge système (15,6 ms par défaut) tant que personne
ne l'élève, ce qui condamnerait à la fois les notifications d'un tampon de 10 ms et la
boucle locale (une avance de 2 ms serait dépassée à chaque tick, donc un débordement à
chaque tick). Le câble utilise donc les timers `Ex*` haute résolution
(`EX_TIMER_HIGH_RESOLUTION`, Windows 8.1+, ce que fait SYSVAD), créés au premier
`StartDevice` et supprimés au `DriverUnload` par
`ExDeleteTimer(timer, Cancel = TRUE, Wait = TRUE, NULL)`, qui attend la fin du tick en
cours. **Pas de repli sur `KeSetTimerEx`** (le brief le laissait ouvert) : un échec
d'`ExAllocateTimer` (pool épuisé) fait échouer `StartDevice` avec
`STATUS_INSUFFICIENT_RESOURCES`, journalisé. Un câble qui s'énumère et ne délivre que des
trous serait bien plus difficile à diagnostiquer qu'un adaptateur qui ne démarre pas
(§7).

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
