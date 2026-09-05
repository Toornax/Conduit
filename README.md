# Conduit

Câble audio virtuel et graphe de routage multiplateforme, en Rust : des périphériques
virtuels « Conduit 1, 2, … » visibles par toutes les applications, et un démon qui
relie librement périphériques réels et virtuels (mixage, duplication, égalisation),
avec une latence faible et maîtrisée.

- [SPEC.md](SPEC.md) : spécification.
- [ROADMAP.md](ROADMAP.md) : découpage en tâches et suivi.
- [docs/adr](docs/adr/README.md) : décisions d'architecture.
- [docs/protocol.md](docs/protocol.md) : protocole de contrôle (généré).

## État

Jalon M0 (cœur portable) : moteur, backend simulé `null`, protocole, démon
`conduitd` et CLI `conduitctl` fonctionnent et sont testés sans matériel. Aucun
backend natif (WASAPI, PipeWire, CoreAudio) n'est encore livré : `conduitd` tourne
sur le backend `null`.

## Construire et tester

Sans Nix, avec la version Rust de `rust-toolchain.toml` (installée automatiquement
par `rustup`) :

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

## Structure

```
crates/
  conduit-core       graphe, tampons, DSP, nœuds, rééchantillonnage, DLL (sans plateforme)
  conduit-backend    traits Backend / CableControl, backend null simulé, priorité RT
  conduit-protocol   API (commandes, réponses, événements), framing, client
  conduit-engine     fil audio, périphériques, pilote, horloge interne, commandes
  conduitd           démon : configuration, IPC, persistance, règles, watchdog
  conduitctl         CLI
  conduit-testing    allocateur de garde pour les tests temps réel
```

Licence : MIT OR Apache-2.0.
