# ADR-012 — Workspace noyau séparé, logique du pilote dans un crate portable

**Statut** : acceptée (2026-09-05). Précise SPEC §7 pour `drivers/windows`.

## Contexte

SPEC §7 place `portcls-sys`, `conduit-kmd` et `conduit-helper` sous `drivers/windows/`
sans dire s'ils appartiennent au workspace Cargo racine. Or un pilote noyau impose
`panic = "abort"`, `#![no_std]`, un allocateur noyau, des flags d'édition de liens
propres au WDK et une toolchain potentiellement différente ; un tel crate dans le
workspace racine casserait `cargo check --workspace` sur Linux et macOS, donc
`nix flake check` (ADR-008). À l'inverse, le helper est un programme utilisateur que
la vérification croisée `mingwW64` doit couvrir (SPEC §5.11), et une bonne partie de
la logique du pilote (positions d'horloge, copie cyclique, validation des formats et
de la configuration) ne dépend pas du noyau et doit être testée, fuzzée et passée à
Miri depuis Nix (SPEC §8 : « le parseur de l'IOCTL compile aussi en mode
utilisateur »).

## Décision

1. `drivers/windows/` est un **workspace Cargo indépendant** (« workspace noyau »)
   contenant `portcls-sys` et `conduit-kmd`, avec son propre `rust-toolchain.toml`.
   Le workspace racine l'exclut explicitement. Il n'est construit que sur Windows
   avec le WDK, par `cargo wdk` ou les scripts de `drivers/windows/tools/`.
2. La logique portable du pilote vit dans **`crates/conduit-kmd-core`**, crate
   `#![no_std]` sans `unsafe`, membre du workspace racine, dépendance par chemin du
   workspace noyau. Il ne dépend d'aucun crate `wdk-*`.
3. Le service d'assistance vit dans **`crates/conduit-helper`**, membre du workspace
   racine (`cfg(windows)` pour le corps, vide ailleurs, comme `conduit-backend-wasapi`).
4. Les outils de test utilisateur du pilote (`conduit-looptest`, M1a-10) sont des
   binaires du workspace racine.

## Conséquences

- `nix flake check` couvre `conduit-kmd-core` (tests, proptest, Miri, couverture) et
  le helper (clippy, croisé) sans WDK ; seuls `portcls-sys` et `conduit-kmd` exigent
  un poste Windows.
- Le repli C++ (ADR-003) ne perd ni `conduit-kmd-core` ni le helper : la validation
  de configuration et les structures partagées restent la source de vérité, appelées
  depuis le C++ via une petite ABI `extern "C"` si nécessaire.
- Deux `Cargo.lock` à maintenir ; `cargo-deny` s'exécute sur les deux.
- SPEC §7 est amendé : `conduit-helper` passe de `drivers/windows/` à `crates/`.
