# Conduit

Câble audio virtuel et graphe de routage multiplateforme, en Rust : des périphériques
virtuels « Conduit 1, 2, … » visibles par toutes les applications, et un démon qui
relie librement périphériques réels et virtuels (mixage, duplication, égalisation),
avec une latence faible et maîtrisée.

- [SPEC.md](SPEC.md) : spécification.
- [ROADMAP.md](ROADMAP.md) : découpage en tâches et suivi.
- [docs/adr](docs/adr/README.md) : décisions d'architecture.
- [docs/protocol.md](docs/protocol.md) : protocole de contrôle (généré).
- [docs/driver-dev.md](docs/driver-dev.md) : pilote Windows (installation, build, VM de test).

## État

Jalon M0 (cœur portable) : moteur, backend simulé `null`, protocole, démon
`conduitd` et CLI `conduitctl` fonctionnent et sont testés sans matériel. Sous
Windows, `conduitd` charge par défaut le backend WASAPI (mode partagé) et expose
les cartes son réelles ; les backends PipeWire et CoreAudio ne sont pas encore
branchés au démon, qui y tourne sur le backend `null`.

## Construire et tester

Sans Nix, avec la version Rust de `rust-toolchain.toml` (installée automatiquement
par `rustup`). Sur Linux, le backend PipeWire a besoin des en-têtes de la
bibliothèque et de `libclang` (bindgen) — par exemple
`apt install libpipewire-0.3-dev libclang-dev` ; ses tests d'intégration lancent un
démon headless et se sautent si `pipewire` et `wireplumber` ne sont pas dans le
`PATH` :

```sh
cargo build --workspace
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

Avec Nix (Linux, macOS) :

```sh
nix develop            # environnement identique à la CI
nix flake check        # format, clippy, tests, deny
```

Vérification croisée Windows depuis Linux :

```sh
rustup target add x86_64-pc-windows-gnu
cargo check --workspace --target x86_64-pc-windows-gnu
```

## Essayer

```sh
cargo run -p conduitd -- --backend null --root /tmp/conduit &
cargo run -p conduitctl -- --socket /tmp/conduit/conduitd.sock status
cargo run -p conduitctl -- --socket /tmp/conduit/conduitd.sock add sine gen --frequency 440
cargo run -p conduitctl -- --socket /tmp/conduit/conduitd.sock nodes
```

`--backend` vaut `auto` par défaut : WASAPI sous Windows (`--backend wasapi` le
force ; en cas d'échec le démon se replie sur `null` et le signale), `null` ailleurs
(simulé, sans matériel).

## Structure

```
crates/
  conduit-core       graphe, tampons, DSP, nœuds, rééchantillonnage, DLL (sans plateforme)
  conduit-backend    traits Backend / CableControl, backend null simulé, priorité RT
  conduit-backend-pipewire  backend Linux (PipeWire) : registre, périphériques, flux
  conduit-protocol   API (commandes, réponses, événements), framing, client
  conduit-engine     fil audio, périphériques, pilote, horloge interne, commandes
  conduitd           démon : configuration, IPC, persistance, règles, watchdog
  conduitctl         CLI
  conduit-testing    allocateur de garde pour les tests temps réel
```

Licence : MIT OR Apache-2.0.
