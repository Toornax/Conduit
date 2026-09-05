# Guide développeur

Ce guide complète [SPEC.md](../SPEC.md) (quoi) et [ROADMAP.md](../ROADMAP.md)
(quand) : il explique **comment** le code est organisé et comment contribuer.

## 1. Vue d'ensemble

```
 applications ──▶ OS (WASAPI / PipeWire / CoreAudio) ◀──▶ conduitd ◀──▶ conduitctl, GUI
                                   ▲                       │
                             pilote de câbles         (moteur, IPC,
                             (boucle locale)           persistance)
```

Chaîne de dépendances (ADR-010) :

```
conduit-core ← conduit-backend ← conduit-protocol ← conduit-engine ← conduitd
                                        ▲
                                  conduitctl, conduit-gui
```

| Crate | Rôle | Plateforme |
|---|---|---|
| `conduit-core` | graphe, exécution, tampons, DSP, nœuds, rééchantillonnage, DLL, ports asynchrones | aucune |
| `conduit-backend` | traits `Backend`, `DeviceHandle`, `CableControl`, événements, backend `null`, priorité RT | `rt` seulement |
| `conduit-backend-pipewire` | backend Linux : fil de boucle PipeWire, registre, `pw_stream` | Linux |
| `conduit-backend-wasapi` | backend Windows : fil MMDevice, énumération, notifications | Windows |
| `conduit-protocol` | API (`Command`, `Reply`, `Notification`), enveloppe, framing, client, schéma | aucune |
| `conduit-engine` | `Engine` : périphériques, rôles, pilote, horloge interne, commandes | aucune |
| `conduitd` | configuration, IPC, service, persistance, règles, watchdog | socket / pipe |
| `conduitctl` | CLI | — |
| `conduit-testing` | allocateur de garde | — |
| `conduit-kmd-core` | logique portable du pilote Windows : positions, copie cyclique, formats | aucune |
| `conduit-com` | modèle objet COM du pilote : `ComObject`, `ComPtr`, `ComRef` | aucune |
| `portcls-sys` | bindings PortCls/KS générés, testables en mode utilisateur (`drivers/windows`, workspace noyau, ADR-012) | Windows |
| `conduit-kmd` | le pilote noyau `.sys` : WDM, `no_std`, PortCls/WaveRT (`drivers/windows`, workspace noyau ; [driver-dev.md](driver-dev.md)) | Windows |

## 2. Le fil audio

Tout ce qui tourne dans un rappel audio ou dans `Node::process` respecte :
**pas d'allocation, pas de verrou, pas de syscall bloquant, pas de log.**

Mécanismes qui rendent cela possible :

- **`GraphSlot` / `Executor`** (`conduit-core::slot`, `executor`) : le fil de gestion
  compile un `CompiledGraph` (tampons pré-alloués, ordre topologique) et le pousse
  dans une file SPSC ; le fil audio l'adopte au début d'un cycle en **déplaçant**
  les nœuds de l'ancienne version, et renvoie l'ancienne version pour libération
  hors temps réel. Les nœuds gardent leur état entre versions.
- **`GainParam`** : cible atomique lue par le fil audio, rampe linéaire sur un cycle.
- **`Param` / `Flag`** : paramètres atomiques des nœuds internes (sinus, EQ…).
- **`DeviceNode`** (`conduit-engine::device_node`, ADR-011) : le rôle d'un
  périphérique change par boîte aux lettres SPSC, sans recompiler.
- **`Arc<Mutex<Executor>>`** partagé entre rappel pilote et horloge interne, avec
  `try_lock` uniquement : un cycle est sauté (silence) plutôt que d'attendre.
- **Test `no_alloc`** (`conduit-core/tests/no_alloc.rs`) : l'allocateur de garde fait
  échouer le test si un cycle alloue. Ajoutez vos nœuds à ce test.

Ce qui est autorisé sur le fil audio : `Instant::now()`, atomiques, files `rtrb`,
calcul flottant (y compris `sin`, `powf` pour les coefficients).

## 3. Horloges

- Le **pilote de graphe** cadence les cycles : rappel d'un périphérique (rendu ou
  capture) ou `InternalClock` (timer, F-22).
- Tout autre périphérique est **asynchrone** : `AsyncPort` = tampon circulaire +
  `Resampler` à ratio variable + `Dll` asservissant le ratio au remplissage.
  Préremplissage jusqu'à la cible, xruns comptés, resynchronisation après xrun.
- Les tests de `conduit-core::asyncport` simulent deux horloges dérivantes avec
  gigue ; le test d'une heure est `#[ignore]` (lancer en release).

## 4. Backend `null`

`NullBackend` simule périphériques et câbles. Deux modes :

- **manuel** : `advance(Duration)` exécute les rappels dus dans l'ordre chronologique
  (déterministe, utilisé par presque tous les tests) ;
- **timer** : `start_timer()` cadence en temps réel.

Une poignée clonée (`null.clone()`) partage les périphériques : le test scripte le
backend que le moteur possède (ajout/retrait à chaud, injection de signal avec
`set_signal`, lecture de ce qui a été rendu avec `take_recorded`).

## 4 bis. Backend PipeWire (Linux)

`conduit-backend-pipewire` connecte Conduit au démon PipeWire. Un **fil dédié**
(`loop_thread`) fait tourner la boucle : rien de `pipewire-rs` n'est `Send`, donc
contexte, registre, métadonnées et `pw_stream` y vivent tous. Le reste du programme
lui parle par `pw::channel` (ouvrir, activer, fermer, quitter) et lit l'état par
`Arc<Mutex<Shared>>`. Le rappel `process` d'un flux tourne, lui, sur le fil temps
réel de PipeWire : il respecte §2 et se coordonne avec `start`/`stop` par un
automate atomique (`arrêté` / `actif` / `en cours`), si bien que `stop` ne rend la
main qu'après le rappel en cours.

Les tests d'intégration lancent un **démon headless** (`tests/common/mod.rs`,
configuration `tests/pipewire-test.conf`) : `core.daemon = true` et
`libpipewire-module-access` sont indispensables. Un flux client n'est relié et
cadencé que par un gestionnaire de session : les tests qui veulent voir le rappel
`process` lancent aussi `wireplumber`. Sans ces binaires dans le `PATH`, les tests
se sautent avec un message.

## 4 ter. Logique du pilote Windows

`conduit-kmd-core` est un crate `#![no_std]` sans `unsafe` ni dépendance, membre du
workspace racine (ADR-012) : horloge virtuelle et positions (`position`), copie
cyclique rendu → capture avec conversion F32 ↔ I16 (`ring`), validation des formats
et taille de tampon (`format`). Son code tourne à `DISPATCH_LEVEL` dans le pilote :
les lints anti-panique (`unwrap`, indexation, arithmétique débordante) sont en `deny`.
Conception et invariants : [driver-design.md](driver-design.md) §2.1 et §5.

`conduit-com` est le modèle objet COM générique du pilote, lui aussi `#![no_std]`
(+ `alloc`) sans dépendance et membre du workspace racine, mais avec de l'`unsafe`
(chaque bloc porte un `// SAFETY:`) : `ComObject<V, T>` (vtable à l'offset 0, compteur
atomique, `QueryInterface`/`AddRef`/`Release` génériques), `ComPtr` (possession Rust) et
`ComRef` (interfaces reçues de PortCls), testés en mode utilisateur à travers les
pointeurs de vtable et sous Miri. Les vtables PortCls concrètes et les traits Rust qui
les implémentent sont dans `drivers/windows/portcls` ([driver-design.md](driver-design.md) §3).

Le pilote lui-même (`portcls-sys`, `conduit-kmd`) est dans le workspace noyau
`drivers/windows`, construit uniquement sous Windows avec le WDK : installation du
poste, build, VM de test et débogage dans [driver-dev.md](driver-dev.md), outillage
`windows-drivers-rs` dans [windows-drivers-rs.md](windows-drivers-rs.md).

## 4 quater. Backend WASAPI (Windows)

`conduit-backend-wasapi` parle à l'API MMDevice depuis un **fil dédié**
(`mmdevice_thread`, calqué sur `loop_thread`) : il initialise COM en MTA (garde RAII
`ComApartment`), crée l'`IMMDeviceEnumerator`, enregistre un `IMMNotificationClient`
écrit en Rust (`notify`, macro `#[implement]` du crate `windows`) et sert les
commandes du `WasapiBackend` par `std::sync::mpsc` (`Enumerate`, `DefaultDevice`,
`Open`, `Subscribe`, `Shutdown`). Le backend lui-même ne détient que l'émetteur du canal et
la poignée du fil : il est `Send`, et sa destruction envoie `Shutdown`, désenregistre
le client puis joint le fil.

Les **rappels COM** (`OnDeviceStateChanged`, `OnDeviceAdded`, `OnDeviceRemoved`,
`OnDefaultDeviceChanged`) arrivent sur un fil choisi par Windows, éventuellement
pendant qu'une commande est en cours : ils **ne font que copier leurs arguments et
poster** une `Notification` sur le même canal. C'est le fil MMDevice qui décide :
passage à `DEVICE_STATE_ACTIVE` ou `OnDeviceAdded` → ré-énumération de ce seul
endpoint (`GetDevice`) et `DeviceEvent::Added` s'il n'était pas connu ; autre état
ou `OnDeviceRemoved` → `Removed` s'il l'était ; `OnDefaultDeviceChanged` → `DefaultChanged`
pour le rôle `eConsole` seulement ; `OnPropertyValueChanged` ignoré. Une table des
endpoints connus évite les doublons, quel que soit l'ordre des rappels.

`devices` traduit un `IMMDevice` en `DeviceInfo` : identifiant d'endpoint (`GetId`),
`PKEY_Device_FriendlyName`, `IMMEndpoint::GetDataFlow`, canaux et fréquence du
**format de mixage** du moteur (`IAudioClient::GetMixFormat`, repli
`PKEY_AudioEngine_DeviceFormat`) — c'est ce qu'un flux partagé délivre sans
conversion —, `sample_rates` = 44,1/48/96 kHz dès que l'`IAudioClient` s'active
(le mode partagé convertit automatiquement ce qui diffère du mixage), bloc par
défaut = période du moteur (`GetDevicePeriod`) en trames, `is_default` =
`GetDefaultAudioEndpoint(flow, eConsole)`, `cable` si le nom est exactement
`Conduit <n>`. Seuls les endpoints `DEVICE_STATE_ACTIVE` sont énumérés. Tout ce que
COM alloue est rendu par une garde (`CoTaskString`, `CoTaskMem`, `PropVariant`) ;
chaque bloc `unsafe` porte son `SAFETY:`.

**Flux.** `open(id, format, rappel)` honore le format demandé : le rappel reçoit
exactement `format.channels` canaux `f32` entrelacés à `format.sample_rate`. Le fil
MMDevice (`open`) active l'`IAudioClient` et choisit un chemin : si (fréquence,
canaux) est le format de mixage et que celui-ci est float32,
`IAudioClient3::InitializeSharedAudioStream` avec la plus petite période prise en
charge ≥ `block_frames` (`choose_period` : multiple de la fondamentale, bornée à
`[min, max]` de `GetSharedModeEnginePeriod`) ; sinon `IAudioClient::Initialize`
en partagé avec `EVENTCALLBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, un
`WAVEFORMATEXTENSIBLE` float32 aux valeurs demandées et la période par défaut
(`block_frames` effectif = cette période en trames à la fréquence demandée). Un
refus du premier chemin retombe sur le second avec un client neuf. `format()` rend
le format effectif ; `WasapiHandle::latency()` (hors trait) expose `GetBufferSize`,
la période, `GetStreamLatency` (0 chez certains pilotes) et le chemin retenu.
Les interfaces du crate `windows` ne sont pas `Send` : `IAudioClient` et le
service de rendu ou de capture voyagent vers le fil du flux en `AgileReference`,
résolue là-bas (MTA des deux côtés, objets WASAPI libres de fil).

Chaque flux a **son fil** (`stream`), créé par `start()` et joint par `stop()` :
`ComApartment` MTA, `rt::promote_current_thread()` (résultat lisible par
`rt_outcome()`), puis boucle sur `WaitForMultipleObjects(arrêt, tampon, 2 s)`.
Rendu : `GetCurrentPadding` → `GetBuffer(libre)` → rappel écrivant directement
dans le tampon WASAPI vu comme `&mut [f32]` (`bytemuck::try_cast_slice_mut`,
tampon intermédiaire pré-alloué si l'alignement manquait) → `ReleaseBuffer` ; un
tampon de silence précède `Start`. Capture : tant que `GetNextPacketSize` > 0,
`GetBuffer` → rappel avec `input` (zéros pré-alloués si `SILENT`) →
`ReleaseBuffer`. Rien n'alloue ni ne verrouille dans la boucle (§2, prouvé par
`tests/no_alloc.rs` avec un allocateur comptant par fil natif). Une erreur
`AUDCLNT_E_DEVICE_INVALIDATED` (ou `RESOURCES_INVALIDATED`) fait sortir de la
boucle sans panique : `is_running()` devient faux, `stop()` rend `Disconnected`,
`start()` le refuse. `stop()` signale l'événement d'arrêt, joint le fil (qui a fait
`Stop` puis `Reset`) : aucun rappel n'est en cours au retour ; `Drop` appelle
`stop()` puis libère les objets COM sous un appartement MTA temporaire.

**Horloge** (`clock`, M1b-33). À l'ouverture, `IAudioClient::GetService(IAudioClock)`
et `GetFrequency`. Microsoft ne fixe pas l'unité de cette fréquence, seulement
qu'elle est celle de la position : `ClockScale::new` la classe (`ClockUnits`) —
`freq == mix_rate` ou `== sample_rate` → trames/s ; `freq == mix_rate × nBlockAlign`
du mixage → octets/s du mixage (**observé** sur les deux cartes du poste par le
chemin `IAudioClient3` : 384 000 pour 48 kHz stéréo float32) ; `freq == sample_rate
× channels × 4` → octets/s du format livré (**observé** par le chemin conversion :
176 400 en 44,1 kHz mono) ; sinon « autre » — et convertit toujours par le rapport
générique `position × sample_rate / freq` (128 bits), exact dans tous les cas :
`ClockInfo::position` est en **trames du format livré** au rappel. À chaque rappel,
**avant** de toucher au tampon, `IAudioClock::GetPosition(&pos, &qpc)` :
`position` = `pos` converti, jamais décroissante ; `timestamp_ns` = `qpc × 100`
(le compteur de performance en unités de 100 ns : base **QPC commune** à tous les
flux du processus, ce qui permet de comparer deux cartes — mesuré : 43 µs d'écart
entre deux lectures immédiates sur deux cartes) ; `frames` = trames du rappel. En
rendu, `pos` est la position de **lecture** du matériel, en retard sur ce qu'on
écrit : la latence estimée `write_ahead_frames` = trames écrites − position lue
(≈ 958 trames pour un tampon de 1 056 sur ce poste) est exposée par
`WasapiHandle::latency()` et `write_ahead_frames()` (atomique) ; en capture, c'est
position d'écriture − trames livrées. Microsoft ne documente pas `GetPosition`
comme sûr en temps réel ; en mode partagé il lit une section partagée avec le
moteur audio, et on le mesure sur le fil (`Instant`, autorisé §2) : ≈ 1 µs en
moyenne, 6 µs au pire sur 6 000 appels (`WasapiHandle::clock_stats()`). Si
`GetPosition` échoue ponctuellement, la position est extrapolée (dernière + trames
du rappel précédent), l'horodatage vient de `QueryPerformanceCounter`, et l'échec
est compté. Sans `IAudioClock` (`ClockSource::Counter`, visible par
`clock_source()`), `position` = trames livrées depuis `start()`, toujours
horodatées QPC. `clock()` (trait) rend la dernière `ClockInfo` publiée par le
rappel ; `clock_now()` (hors trait, alloue) interroge `IAudioClock` immédiatement
depuis le fil appelant. Le moteur, lui, n'exploite pas encore `ClockInfo` : sa DLL
est asservie au remplissage du port asynchrone (§3), et c'est ainsi qu'il absorbe
la dérive mesurée entre les deux cartes (−18 ppm, `tests/two_devices.rs`).

**Démon.** `conduitd` charge ce backend par défaut sous Windows depuis M1b-31 :
`--backend auto` (la valeur par défaut) appelle `WasapiBackend::new()` sous
`cfg(windows)` ; `--backend wasapi` le demande explicitement. Si le fil MMDevice ne
démarre pas (COM, service audio arrêté), l'erreur est journalisée et le démon se
replie sur le backend `null` plutôt que de refuser de démarrer (F-51) — `conduitctl
status` montre alors `backend null`. Le pilote de graphe suit la logique habituelle
(`engine.driver` de la configuration ; en `auto`, le périphérique de rendu par
défaut `eConsole`, sinon une capture, sinon l'horloge interne). Le
test `conduitd_binary_auto_backend_is_wasapi_on_windows` (`crates/conduitd/tests/binary.rs`)
lance le binaire et vérifie que le graphe contient chaque endpoint énuméré ; il se
saute si WASAPI est indisponible ou qu'aucune carte n'est active.

Ce qui manque encore : le mode exclusif (M1b-32), `CableControl` par le helper
(M1b-34 — d'ici là le démon ignore la section `[[cable]]` avec un avertissement).
Les tests d'intégration (`tests/wasapi.rs`, `tests/stream.rs`, `tests/no_alloc.rs`,
`tests/two_devices.rs` — 60 s sur deux cartes de rendu, dérive imprimée) tournent
sur les cartes son de la machine, en silence ; ceux qui demandent un périphérique
absent se sautent avec un message. À lancer à la main : le critère « casque USB
branché → `Added` »
(`cargo test -p conduit-backend-wasapi --test wasapi -- --ignored --nocapture`), le
sinus audible
(`cargo test -p conduit-backend-wasapi --test stream sine_audible -- --ignored --nocapture`)
et l'heure d'endurance avec le moteur (une carte pilote, l'autre asynchrone,
xruns = 0 ; `CONDUIT_ENDURANCE_SECS` pour raccourcir)
(`cargo test -p conduit-backend-wasapi --test two_devices -- --ignored --nocapture`).

## 5. Tests

```sh
cargo test --workspace --all-features           # tout (Linux : libpipewire-0.3-dev)
cargo test -p conduit-core --lib asyncport      # un module
cargo test --release -p conduit-core -- --ignored input_port_one_hour
cargo bench -p conduit-core                     # criterion
cargo +nightly miri test -p conduit-core --lib -- ring:: graph:: executor::
cargo +nightly miri test -p conduit-kmd-core --all-features --lib
cargo +nightly miri test -p conduit-com --all-features
cargo +nightly fuzz run decoder                 # depuis crates/conduit-protocol
cargo deny check
cargo run -p conduit-protocol --features schema --example gen-docs   # docs/protocol.md
```

Niveaux : unitaires par module ; intégration `conduit-engine/tests/scenarios.rs`
(F-11 à F-22 enchaînés) ; bout en bout `conduitd/tests` et `conduitctl/tests`
(démon en processus + client IPC, et vrais binaires).

## 6. Conventions

- Commits : Conventional Commits, scopes de ROADMAP (`core`, `engine`, `backend`,
  `null`, `protocol`, `daemon`, `cli`, `nix`, …). Une tâche = un commit, CI verte.
- `unsafe` : interdit (`#![forbid(unsafe_code)]`) sauf `conduit-testing` (allocateur),
  `conduit-backend::rt` (appels système) et `conduit-backend-wasapi` (appels COM),
  chaque bloc commenté `SAFETY:` (lint `undocumented_unsafe_blocks` du workspace).
- Messages d'erreur : dire quoi faire (ADR-006). Codes stables dans
  `ProtocolError`.
- Identifiants : `NodeId`/`LinkId` sont générationnels (jamais réutilisés) ; la
  persistance et les règles utilisent `NodeKey` (stable).
- Toute déviation de SPEC produit une ADR dans `docs/adr`.

## 7. Nix

`nix develop` donne l'environnement de référence ; `nix flake check` lance format,
clippy, tests, cargo-deny, doc, vérification de `docs/protocol.md` et compilation
croisée Windows. `nix build .#conduitd` produit le binaire Linux/macOS.
`nix develop .#nightly` fournit Miri et cargo-fuzz : `cargo miri test -p conduit-core --lib`.
Miri n'est pas un check du flake : la construction de son sysroot télécharge des
crates (impossible dans le bac à sable Nix) ; il tourne dans le devshell nightly et
en CI hors Nix.

Windows : `packaging/windows/setup-env.ps1` installe ou vérifie (`-Check`) les
versions de `versions.json`.

## 8. Ajouter un nœud interne

1. Implémenter `Node` dans `conduit-core::nodes` (sans allocation dans `process`,
   `prepare` pour allouer) et l'exposer.
2. Ajouter une variante à `InternalKind` (`conduit-protocol::api`) et son
   instanciation dans `Engine::add_internal` ; ses paramètres à chaud dans
   `Engine::set_param`.
3. Ajouter la sous-commande `add` correspondante dans `conduitctl::cli`.
4. Tests : unitaire, `no_alloc`, un scénario CLI. Régénérer `docs/protocol.md`.
