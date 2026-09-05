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

## 4 quater. Backend WASAPI (Windows)

`conduit-backend-wasapi` parle à l'API MMDevice depuis un **fil dédié**
(`mmdevice_thread`, calqué sur `loop_thread`) : il initialise COM en MTA (garde RAII
`ComApartment`), crée l'`IMMDeviceEnumerator`, enregistre un `IMMNotificationClient`
écrit en Rust (`notify`, macro `#[implement]` du crate `windows`) et sert les
commandes du `WasapiBackend` par `std::sync::mpsc` (`Enumerate`, `DefaultDevice`,
`Subscribe`, `Shutdown`). Le backend lui-même ne détient que l'émetteur du canal et
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
`PKEY_Device_FriendlyName`, `IMMEndpoint::GetDataFlow`, canaux et fréquence lus dans
`PKEY_AudioEngine_DeviceFormat` (repli `IAudioClient::GetMixFormat`), fréquences de
44,1/48/96 kHz acceptées telles quelles en mode partagé (`IsFormatSupported`,
float32), bloc par défaut = période du moteur (`GetDevicePeriod`) en trames,
`is_default` = `GetDefaultAudioEndpoint(flow, eConsole)`, `cable` si le nom est
exactement `Conduit <n>`. Seuls les endpoints `DEVICE_STATE_ACTIVE` sont énumérés.
Tout ce que COM alloue est rendu par une garde (`CoTaskString`, `CoTaskMem`,
`PropVariant`) ; chaque bloc `unsafe` porte son `SAFETY:`.

Ce qui manque encore : les flux (`open` renvoie `Platform("flux WASAPI : M1b-31")`),
le mode exclusif (M1b-32), l'horloge (M1b-33), `CableControl` par le helper
(M1b-34) ; le démon ne charge pas ce backend avant M1b-31. Les tests d'intégration
(`tests/wasapi.rs`) tournent sur les cartes son de la machine ; le critère « casque
USB branché → `Added` » est un test `#[ignore]` à lancer à la main :
`cargo test -p conduit-backend-wasapi --test wasapi -- --ignored --nocapture`.

## 5. Tests

```sh
cargo test --workspace --all-features           # tout (Linux : libpipewire-0.3-dev)
cargo test -p conduit-core --lib asyncport      # un module
cargo test --release -p conduit-core -- --ignored input_port_one_hour
cargo bench -p conduit-core                     # criterion
cargo +nightly miri test -p conduit-core --lib -- ring:: graph:: executor::
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
