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
| `conduit-protocol` | API (`Command`, `Reply`, `Notification`), enveloppe, framing, client, schéma | aucune |
| `conduit-engine` | `Engine` : périphériques, rôles, pilote, horloge interne, commandes | aucune |
| `conduitd` | configuration, IPC, service, persistance, règles, watchdog | socket / pipe |
| `conduitctl` | CLI | — |
| `conduit-testing` | allocateur de garde | — |
| `conduit-kmd-core` | logique portable du pilote Windows : positions, copie cyclique, formats | aucune |
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

Le pilote lui-même (`portcls-sys`, `conduit-kmd`) est dans le workspace noyau
`drivers/windows`, construit uniquement sous Windows avec le WDK : installation du
poste, build, VM de test et débogage dans [driver-dev.md](driver-dev.md), outillage
`windows-drivers-rs` dans [windows-drivers-rs.md](windows-drivers-rs.md).

## 5. Tests

```sh
cargo test --workspace --all-features           # tout (Linux : libpipewire-0.3-dev)
cargo test -p conduit-core --lib asyncport      # un module
cargo test --release -p conduit-core -- --ignored input_port_one_hour
cargo bench -p conduit-core                     # criterion
cargo +nightly miri test -p conduit-core --lib -- ring:: graph:: executor::
cargo +nightly miri test -p conduit-kmd-core --all-features --lib
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
- `unsafe` : interdit (`#![forbid(unsafe_code)]`) sauf `conduit-testing` (allocateur)
  et `conduit-backend::rt` (appels système), chaque bloc commenté `SAFETY:`.
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
