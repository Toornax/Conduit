# ADR-008 — Nix comme environnement de référence (Linux, macOS)

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-08 et §5.11.

## Contexte

Les builds doivent être reproductibles et identiques entre contributeurs et CI.
Nix n'existe pas nativement sur Windows, plateforme prioritaire.

## Décision

Un flake Nix (`flake-parts`, `rust-overlay`, `crane`) définit l'environnement, les
packages et les checks sur Linux et macOS ; `nix flake check` est **la** commande de
validation. Windows est couvert par un script d'installation épinglé et un workflow
CI séparé ; les crates utilisateur Windows sont vérifiés en compilation croisée
depuis Nix (`x86_64-pc-windows-gnu`).

## Conséquences

- `rust-toolchain.toml` est la source de vérité de la version Rust, lue par le
  flake ; un contributeur sans Nix builde avec `cargo`.
- Les artefacts Linux visent la reproductibilité au bit près.
