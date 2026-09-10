# Conduit — Feuille de route

Version 0.1 — 2026-09-05. Découpage de chaque jalon de `SPEC.md` §9 en tâches commitables.

## Conventions

- **`nix flake check` est la commande de validation** sur Linux et macOS, en local comme en
  CI. Windows utilise l'environnement scripté (SPEC §5.11). « CI verte » signifie les deux.
- **Une tâche = un commit** qui laisse la CI verte. Les tests d'une fonctionnalité sont dans
  le même commit que la fonctionnalité. Une tâche trop grosse pour un commit lisible est
  re-découpée, pas fusionnée.
- **Format des messages** : Conventional Commits, `type(scope): description à l'impératif`.
  Types : `feat`, `fix`, `test`, `docs`, `chore`, `ci`, `refactor`, `perf`, `bench`.
  Scopes : `core`, `engine`, `backend`, `null`, `wasapi`, `pipewire`, `coreaudio`,
  `protocol`, `daemon`, `cli`, `gui`, `driver`, `portcls`, `helper`, `hal`, `packaging`.
- **Une branche par groupe** (ex. `m0/core-graph`), une PR par groupe, revue avant fusion
  sur `main`. `main` est toujours livrable.
- **Chaque tâche a un critère « Fait quand »** vérifiable. Sans critère rempli, pas de fusion.
- **Toute décision qui dévie de `SPEC.md`** produit une ADR dans `docs/adr/` dans le même
  commit ou le commit suivant.
- Les identifiants `F-xx` renvoient aux exigences de `SPEC.md` §4.
- Cocher les cases au fil de l'eau ; ce fichier est le suivi officiel.

Ordre général : M0 et M1a en parallèle, puis M1b, M2, M3, M4, M5.

**État au 2026-09-06.** M0 clos (tag `v0.1.0-core`, `nix flake check` vert, 211 tests).
Le poste Windows est en place depuis le 2026-09-05 (WDK 26100, LLVM 17.0.6, `cargo-wdk`) :
M1a et la partie Windows de M1b ont avancé en parallèle.

- **M1a** : **close, et franchie.** Le pilote PortCls/WaveRT en Rust se charge cent fois
  sans erreur, transporte l'audio dix passes sur dix, et tient une heure sous Driver
  Verifier sans le moindre vidage. Les douze tâches sont cochées et
  [ADR-015](docs/adr/015-resultat-du-spike-pilote-rust.md) tranche : on reste en Rust, le
  repli C++ n'est pas exercé, en deux jours contre trois semaines proposées.
- **M1b.C** : M1b-30, 32, 33 et 35 cochées ; le démon charge WASAPI par défaut sous
  Windows et tourne sans xrun (1 h, deux cartes, 720 006 cycles). M1b-31 attend la boucle
  par câble, donc le pilote.
- **M1b.A, M1b.B, M1b.D** : débloquées par la VM, qui valide désormais la propriété de
  configuration du pilote supposée par le helper et le MSI ; pas encore reprises.
- **M2** : ouvert et mené en parallèle, puisqu'il ne dépend pas de la VM — la GUI se
  développe avec le backend `null`. Onze tâches cochées : les trois vues (Câbles,
  Patchbay, Diagnostic), le thème clair/sombre, le démarrage du démon et les messages
  d'erreur orientés action. La fenêtre suit la maquette Sericæ
  ([docs/design-system.md](docs/design-system.md), ADR-014 pour les polices embarquées).
  Restent ouverts : les VU-mètres (M2-07, le protocole ne pousse aucun niveau), la zone
  de notification (M2-09), les traductions (M2-11b), le repli logiciel et le paquet Nix
  (M2-12, M2-12b), les captures de référence (M2-14) et le packaging (M2-15).
- Conception et décisions de la reprise : [docs/driver-design.md](docs/driver-design.md),
  [docs/windows-drivers-rs.md](docs/windows-drivers-rs.md), ADR-012 (workspace noyau
  séparé) et ADR-013 (démon démarré à l'ouverture de session, pas un service).

**Prochaine action, hors code** : obtenir une ISO Windows 11 et dérouler
[docs/vm-bringup.md](docs/vm-bringup.md).

**Dette repérée pendant M2**, à traiter hors GUI : le protocole ne diffuse **aucune**
notification quand un gain ou une coupure change (`SetNodeGain`, `SetLinkGain` répondent
`Reply::Ok` et rien d'autre), ce qui oblige tout client à relire l'état derrière chaque
réglage. Rediffuser le descripteur du nœud ou du lien suffirait.

---

## M0 — Cœur portable

Objectif : moteur complet, démon et CLI fonctionnels avec le backend `null`, testés sans
matériel sur les trois OS. Critère de sortie : DLL validée avec horloges dérivantes, zéro
allocation dans le fil audio, CI verte partout, couverture ≥ 80 % sur `core` et `protocol`.

### M0.A — Fondations du dépôt

- [x] **M0-01** `docs: ajoute SPEC.md et ROADMAP.md`
  *Fait quand* : les deux fichiers sont sur `main`.
- [x] **M0-02** `chore: initialise le workspace Cargo`
  Workspace vide avec les crates listés dans SPEC §7 (chacun avec un `lib.rs` vide),
  `rust-toolchain.toml` (stable épinglé, source de vérité pour le flake), `rustfmt.toml`,
  `clippy.toml`, `.gitignore`, `.editorconfig`, `LICENSE-MIT`, `LICENSE-APACHE`,
  `Cargo.lock` versionné.
  *Fait quand* : `cargo build --workspace` et `cargo clippy --workspace -- -D warnings` passent.
- [x] **M0-03** `chore(nix): flake.nix avec flake-parts, rust-overlay et devshell`
  `flake.nix`, `flake.lock`, `nix/devshell.nix` : toolchain lue depuis `rust-toolchain.toml`,
  `cargo-nextest`, `cargo-deny`, `cargo-audit`, `cargo-llvm-cov`, `clang`/`bindgen`,
  `pkg-config`, PipeWire et ALSA sur Linux, `apple-sdk` sur macOS. `.envrc` pour direnv.
  *Fait quand* : `nix develop -c cargo build --workspace` passe sur Linux et macOS ;
  `nix flake show` liste le devshell.
- [x] **M0-04** `chore(nix): packages crane et checks (fmt, clippy, tests)`
  `nix/packages.nix` : vendoring des dépendances, un package par binaire ; `nix/checks.nix` :
  `fmt`, `clippy`, `nextest`.
  *Fait quand* : `nix flake check` et `nix build .#conduitd` passent sur Linux et macOS.
- [x] **M0-05** `chore: cargo-deny et cargo-audit intégrés aux checks`
  `deny.toml` (licences autorisées MIT/Apache/BSD/ISC/Zlib, refus copyleft, sources
  crates.io uniquement), check Nix `deny` et `audit`.
  *Fait quand* : `nix flake check` échoue si on ajoute une dépendance GPL en test local.
- [x] **M0-06** `chore(nix): hooks de pré-commit via git-hooks.nix`
  `rustfmt`, `nixfmt`, `cargo-deny`, format des messages de commit.
  *Fait quand* : un commit mal formaté est refusé dans le devshell.
- [x] **M0-07** `ci: nix flake check sur Linux et macOS avec cache binaire`
  Workflow GitHub Actions : installation de Nix, cache (Cachix ou équivalent), `nix flake check`,
  `nix build` des packages, publication vers le cache.
  *Fait quand* : CI verte sur `ubuntu-latest` et `macos-latest` ; second run nettement plus
  rapide grâce au cache.
- [x] **M0-08** `ci: workflow Windows sans Nix, environnement scripté et épinglé`
  `packaging/windows/setup-env.ps1` : versions exactes de Rust (via `rustup` et
  `rust-toolchain.toml`), Visual Studio Build Tools, Windows SDK ; le script échoue si une
  version installée diffère. Workflow : setup, `cargo fmt --check`, clippy, `cargo nextest`.
  *Fait quand* : CI verte sur `windows-latest` ; versions listées dans un fichier versionné.
- [x] **M0-09** `chore(nix): vérification croisée mingwW64 des crates utilisateur`
  Check Nix compilant `conduitd`, `conduitctl`, `conduit-protocol` pour `x86_64-pc-windows-gnu`.
  *Fait quand* : le check tourne dans `nix flake check` sur Linux et attrape une erreur
  `cfg(windows)` volontaire.
- [x] **M0-10** `docs: ADR-001 à ADR-008 (décisions de SPEC §11)`
  Un fichier par décision, gabarit ADR (contexte, décision, conséquences), index.
  *Fait quand* : les huit ADR sont relus et cohérents avec SPEC.
- [x] **M0-11** `docs: README développeur minimal`
  Objectif en trois lignes, `nix develop` puis `cargo build`, chemin sans Nix, lien vers
  SPEC et ROADMAP.
  *Fait quand* : un nouveau contributeur peut builder en suivant le README, avec et sans Nix.

### M0.B — `conduit-core` : types et tampons

- [x] **M0-15** `feat(core): types de base SampleRate, Frames, ChannelCount, Db, Gain`
  Newtypes, conversions dB ↔ linéaire, bornes, `Display`.
  *Fait quand* : tests unitaires des conversions, y compris −∞ dB et 0 dB.
- [x] **M0-16** `feat(core): tampon audio planaire pré-alloué AudioBuffer`
  Un `Vec<f32>` par canal, vues mutables par canal, `fill_silence`, `copy_from`, aucune
  allocation après construction.
  *Fait quand* : tests + test de non-allocation sur les opérations de vue.
- [x] **M0-17** `feat(core): tampon circulaire SPSC temps réel`
  Intégration de `rtrb` derrière une façade `RingBuffer` (producteur/consommateur séparés,
  lecture/écriture par blocs, niveau de remplissage atomique).
  *Fait quand* : tests de propriétés `proptest` (jamais de perte ni de duplication), test
  multi-fils.

### M0.C — `conduit-core` : graphe

- [x] **M0-20** `feat(core): modèle du graphe (NodeId, PortId, LinkId, Direction, PortSpec)`
  Structures descriptives uniquement, sans traitement. Identifiants générationnels.
  *Fait quand* : tests de sérialisation `serde` des identifiants.
- [x] **M0-21** `feat(core): trait Node et ProcessContext`
  `Node::process(&mut self, ctx, inputs, outputs)`, `ProcessContext { frames, sample_rate,
  position }`, `Node::ports()`.
  *Fait quand* : un nœud de test passe-plat compile et s'exécute.
- [x] **M0-22** `feat(core): GraphBuilder avec détection de cycles`
  Ajout/retrait de nœuds et liens, type d'erreur explicite (`LinkError::WouldCycle` avec
  le chemin), validation des directions et types de ports.
  *Fait quand* : tests couvrant cycle direct, cycle indirect, auto-lien, port inexistant (F-12).
- [x] **M0-23** `feat(core): compilation du graphe en plan d'exécution`
  Ordre topologique, attribution des tampons par port, liste d'exécution plate,
  `CompiledGraph` immuable.
  *Fait quand* : tests d'ordre sur graphes en diamant et en chaîne ; aucun tampon partagé
  entre ports actifs simultanément.
- [x] **M0-24** `feat(core): sommation des liens avec gain et rampe`
  Somme des liens entrants dans un port, gain par lien, rampe linéaire sur un quantum lors
  d'un changement.
  *Fait quand* : test vérifiant l'absence de discontinuité (dérivée bornée) lors d'un saut de gain (F-13).
- [x] **M0-25** `feat(core): Executor exécutant un cycle sur un CompiledGraph`
  Parcourt la liste d'exécution, remplit les entrées, appelle `process`, aucune allocation.
  *Fait quand* : test d'un graphe sinus → gain → moniteur produisant la sortie attendue.
- [x] **M0-26** `feat(core): échange atomique du graphe (GraphSlot)`
  Publication d'un nouveau `CompiledGraph` par échange de pointeur (`arc-swap`), l'ancien
  est renvoyé au fil de gestion pour libération hors temps réel.
  *Fait quand* : test de stress échange/exécution concurrents sans blocage ; test que le
  `drop` ne se produit jamais sur le fil d'exécution.
- [x] **M0-27** `test(core): allocateur de garde et test zéro allocation du cycle`
  Allocateur global de test qui panique quand un drapeau est armé ; armé pendant `Executor::run`.
  *Fait quand* : le test passe, et échoue si on introduit volontairement un `Vec::new()` dans un `process`.

### M0.D — `conduit-core` : nœuds utilitaires

- [x] **M0-30** `feat(core): nœuds Silence, Sine, PinkNoise`
  *Fait quand* : tests de fréquence (FFT ou comptage de passages par zéro) et d'amplitude.
- [x] **M0-31** `feat(core): nœuds Mixer N→M et Splitter`
  *Fait quand* : tests de somme et de duplication canal par canal.
- [x] **M0-32** `feat(core): nœud Meter (crête et RMS, export atomique)`
  Valeurs publiées par atomiques lisibles hors temps réel, décroissance configurable.
  *Fait quand* : test sur sinus d'amplitude connue.
- [x] **M0-33** `feat(core): adaptation de canaux mono↔stéréo et mapping explicite`
  Duplication mono→stéréo, somme pondérée stéréo→mono, table de mapping arbitraire.
  *Fait quand* : tests des trois cas (F-15).

### M0.E — `conduit-core` : horloges et rééchantillonnage

- [x] **M0-40** `feat(core): rééchantillonneur sinc à ratio variable`
  Sinc fenêtré avec interpolation du ratio par échantillon (`rubato` en mode asynchrone ou
  implémentation interne si l'API ne convient pas), qualité configurable.
  *Fait quand* : mesure de rapport signal/bruit ≥ 90 dB sur sinus à ratio fixe, pas de
  discontinuité lors d'un changement de ratio de 100 ppm.
- [x] **M0-41** `feat(core): boucle à verrouillage de délai (DLL)`
  Filtre de second ordre estimant le ratio d'horloge à partir du remplissage d'un tampon,
  paramètres de bande passante, anti-emballement.
  *Fait quand* : simulation avec horloges dérivantes de ± 1000 ppm et gigue : convergence
  en < 2 s, remplissage stable à ± un quantum, zéro xrun sur 1 h simulée.
- [x] **M0-42** `feat(core): AsyncPort (tampon + rééchantillonneur + DLL)`
  Composant reliant un flux à horloge étrangère au graphe, dans les deux sens, avec
  détection et comptage des xruns.
  *Fait quand* : test bout en bout avec deux horloges virtuelles dérivantes, continuité
  de phase d'un sinus vérifiée, xruns = 0.
- [x] **M0-43** `bench(core): criterion sur le cycle de traitement et le rééchantillonneur`
  *Fait quand* : bench exécuté en CI (sans seuil bloquant), résultats dans le rapport.
- [x] **M0-44** `chore(nix): check Miri sur conduit-core (toolchain nightly séparée)` — toolchain nightly dans `devShells.nightly` ; Miri tourne hors bac à sable (son sysroot télécharge des crates), 56 tests verts en local
  *Fait quand* : check Miri vert dans `nix flake check` sur les tests marqués compatibles.

### M0.F — `conduit-backend` : traits et backend `null`

- [x] **M0-50** `feat(backend): trait Backend, DeviceInfo, DeviceHandle, rappel audio`
  Énumération, ouverture avec format demandé, rappel `FnMut(&mut AudioBuffer, ClockInfo)`,
  position d'horloge, fermeture. Documentation des garanties temps réel du rappel.
  *Fait quand* : doc-tests compilent, trait objet-safe.
- [x] **M0-51** `feat(backend): trait CableControl`
  Lister, créer, supprimer, configurer canaux, renommer ; erreurs typées (limite atteinte,
  droits insuffisants, non supporté).
  *Fait quand* : doc + erreurs testées avec une implémentation factice.
- [x] **M0-52** `feat(backend): événements de périphériques via canal`
  `DeviceEvent::{Added, Removed, DefaultChanged}` émis par le backend.
  *Fait quand* : test de réception avec le backend null.
- [x] **M0-53** `feat(null): périphériques simulés à horloge virtuelle`
  Périphériques d'entrée/sortie avec horloge avançable manuellement ou par fil timer, dérive
  et gigue configurables, injection de signal, capture de sortie pour assertions.
  *Fait quand* : tests déterministes en mode manuel ; mode timer fonctionne.
- [x] **M0-54** `feat(null): câbles simulés et branchement à chaud scriptable`
  `CableControl` factice, méthodes de test pour faire apparaître/disparaître un périphérique.
  *Fait quand* : tests de scénario ajout/retrait.

### M0.G — `conduit-engine`

- [x] **M0-60** `feat(engine): registre des nœuds et clés stables NodeKey`
  Clé = identifiant OS + nom, indépendante de l'ordre d'énumération ; table clé → NodeId.
  *Fait quand* : test de stabilité après redémarrage simulé (F-30).
- [x] **M0-61** `feat(engine): fil audio et pilote de graphe`
  Le rappel du périphérique désigné pilote exécute `Executor::run` ; les autres nœuds
  matériels passent par `AsyncPort`.
  *Fait quand* : test null : sortie audible sur le pilote, entrée capturée depuis un second périphérique.
- [x] **M0-62** `feat(engine): priorité temps réel du fil audio`
  `audio_thread_priority` ou appels natifs, dégradation documentée si refusée.
  *Fait quand* : test que la promotion est tentée et l'échec journalisé, pas fatal.
- [x] **M0-63** `feat(engine): horloge interne comme pilote de secours`
  Timer haute résolution cadençant le graphe quand aucun matériel n'est disponible.
  *Fait quand* : test de gigue de période < 10 % du quantum sur 60 s (F-22).
- [x] **M0-64** `feat(engine): suspension et réactivation à chaud des nœuds`
  Périphérique retiré → nœud suspendu, liens conservés ; réapparu → réactivé.
  *Fait quand* : test null retrait/réapparition, graphe identique avant/après (F-20).
- [x] **M0-65** `feat(engine): changement du pilote de graphe à chaud`
  *Fait quand* : test null de basculement avec mesure du silence < 2 quanta (F-21).
- [x] **M0-66** `feat(engine): compteurs xrun, temps de cycle, file d'événements hors RT`
  File SPSC d'événements du fil audio vers le fil de gestion ; agrégation min/moy/max.
  *Fait quand* : test : xrun provoqué → compteur incrémenté et événement émis.
- [x] **M0-67** `feat(engine): API de commandes (enum Command, réponses typées)`
  Commandes : lier, délier, gain, pilote, câbles, requêtes d'état ; exécutées sur le fil de gestion.
  *Fait quand* : chaque commande a un test.
- [x] **M0-68** `test(engine): scénarios d'intégration engine + null`
  Scénarios F-11 à F-16, F-20 à F-22 enchaînés.
  *Fait quand* : tous les scénarios passent en CI sur les trois OS.

### M0.H — `conduit-protocol`

- [x] **M0-70** `feat(protocol): types Request, Response, Event et serde`
  Miroir des commandes de l'engine, erreurs avec code et message destiné à l'utilisateur.
  *Fait quand* : round-trip serde de chaque variante.
- [x] **M0-71** `feat(protocol): framing u32 + MessagePack avec limites de taille`
  Encodeur/décodeur incrémental, taille maximale de trame, erreurs non paniquantes.
  *Fait quand* : tests de trames tronquées, trop grandes, corrompues.
- [x] **M0-72** `feat(protocol): négociation Hello et version`
  *Fait quand* : test client trop ancien / trop récent → erreur claire.
- [x] **M0-73** `feat(protocol): export JSON Schema et docs/protocol.md généré`
  *Fait quand* : job CI vérifie que la doc est à jour avec les types.
- [x] **M0-74** `test(protocol): fuzzing du décodeur avec cargo-fuzz`
  Cible dans le devshell nightly ; check Nix court (corpus seulement) ; job long en nocturne.
  *Fait quand* : 1 h de fuzzing sans crash, corpus versionné, check court vert.

### M0.I — `conduitd`

- [x] **M0-80** `feat(daemon): squelette, configuration TOML, chemins par OS`
  Chargement/validation de la config de SPEC §5.7, valeurs par défaut, chemins via `directories`.
  *Fait quand* : tests de config valide/invalide avec messages exploitables.
- [x] **M0-81** `feat(daemon): journalisation tracing (fichier tournant + stderr)`
  *Fait quand* : niveau configurable, rotation testée.
- [x] **M0-82** `feat(daemon): serveur IPC multi-clients (socket Unix, named pipe)`
  `tokio`, permissions 0600 / ACL utilisateur, limite de clients.
  *Fait quand* : test de deux clients simultanés sur chaque OS en CI.
- [x] **M0-83** `feat(daemon): pont IPC ↔ engine`
  *Fait quand* : chaque requête du protocole a un test bout en bout avec le backend null.
- [x] **M0-84** `feat(daemon): abonnements et diffusion des événements`
  *Fait quand* : test : événement engine → reçu par les abonnés seulement (F-43).
- [x] **M0-85** `feat(daemon): persistance et restauration de l'état du graphe`
  Sauvegarde atomique (fichier temporaire + renommage) à chaque changement, restauration au démarrage par clés stables.
  *Fait quand* : test redémarrage simulé, graphe identique (F-30).
- [x] **M0-86** `feat(daemon): règles d'auto-connexion`
  *Fait quand* : test : nœud apparaissant correspondant à une règle → lié (F-32).
- [x] **M0-87** `feat(daemon): arrêt propre et watchdog du fil audio`
  Signaux/événements de contrôle, fermeture des périphériques, détection d'un fil audio
  bloqué > 1 s avec journalisation et redémarrage du moteur.
  *Fait quand* : test de blocage simulé → moteur redémarré, clients notifiés.
- [x] **M0-88** `feat(daemon): rapport de diagnostic (dump)`
  *Fait quand* : le rapport ne contient aucun chemin utilisateur ni nom de machine.

### M0.J — `conduitctl`

- [x] **M0-90** `feat(cli): squelette clap, client IPC, sortie table et JSON`
  *Fait quand* : `conduitctl --help` documente chaque commande ; `--json` sur toutes.
- [x] **M0-91** `feat(cli): status, nodes, ports, links`
- [x] **M0-92** `feat(cli): link, unlink, volume, driver`
- [x] **M0-93** `feat(cli): cable add, remove, rename, list`
- [x] **M0-94** `feat(cli): monitor, xruns, dump, load`
  *Fait quand (91 à 94)* : test bout en bout par commande contre un démon null (F-41).
- [x] **M0-95** `test: scénario bout en bout démon null + cli`
  Script de test : démarrer le démon, créer un câble, lier, retirer un périphérique, restaurer.
  *Fait quand* : passe en CI sur les trois OS.

### M0.K — Clôture

- [x] **M0-96** `chore(nix): check de couverture llvm-cov avec seuil 80 % sur core et protocol`
- [x] **M0-97** `chore(nix): apps (démon, CLI, bancs de test) et packages testés hors devshell`
  `nix run .#conduitd` fonctionne ; les binaires packagés démarrent sur une machine sans devshell.
  *Fait quand* : test CI lançant le package dans un conteneur nu.
- [x] **M0-98** `docs: guide développeur (architecture, tests, conventions, Nix)`
- [x] **M0-99** `chore: tag v0.1.0-core`
  *Fait quand* : tous les critères de sortie M0 sont cochés dans ce fichier.

---

## M1a — Spike pilote Windows en Rust (en parallèle de M0)

Objectif : prouver qu'un pilote PortCls/WaveRT en Rust est faisable. Délai borné (à fixer,
proposition : trois semaines de travail effectif). Porte de décision en M1a-12.

**Le spike est clos et sa porte est franchie.** Sur la machine virtuelle `ConduitTest`,
le pilote se charge et se décharge **cent fois sans erreur** (deux séries), l'audio
**traverse le câble** — dix passes sur dix à 440,00 Hz, amplitude 0,500, aucune rupture de
phase, aucun trou — et une **heure sous Driver Verifier** (pool spécial, IRQL forcée,
conformité DDI) passe sans écran bleu ni vidage, sur 360 cycles d'ouverture et fermeture
des deux flux. Les douze tâches sont cochées.
[ADR-015](docs/adr/015-resultat-du-spike-pilote-rust.md) répond aux quatre questions de la
porte : le pilote **reste en Rust**, le repli C++ n'est pas exercé, et le délai borné n'est
pas consommé — deux jours contre trois semaines proposées. La séquence de validation et
l'arbre de diagnostic restent dans [docs/vm-bringup.md](docs/vm-bringup.md).

- [x] **M1a-01** `chore(driver): environnement de build WDK et windows-drivers-rs`
  Hors Nix (SPEC §5.11). Extension de `packaging/windows/setup-env.ps1` (WDK 26100, LLVM
  17.0.6, `cargo-wdk` 0.1.1, versions dans `versions.json`) ; workspace noyau
  `drivers/windows` (ADR-012) avec `rust-toolchain.toml` **stable** 1.96.1 (nightly inutile),
  `.cargo/config.toml` (`crt-static`), crates vides `portcls-sys` et `conduit-kmd` ;
  `docs/driver-dev.md` (installation, VM de test, mode test signing, kernel debugger) qui
  remplace `docs/windows-setup.md`.
  *Fait quand* : un développeur reproduit le build à partir de la doc ; le job CI Windows
  compile le crate vide du pilote.
- [x] **M1a-01b** `feat(driver): conduit-kmd-core, horloge virtuelle et copie cyclique testées sous Nix`
  Crate `#![no_std]` sans `unsafe` du workspace racine (ADR-012, [driver-design.md](docs/driver-design.md) §2.1) :
  modules `position`, `ring`, `format` ; lints anti-panique en `deny`.
  *Fait quand* : proptest et Miri verts dans `nix flake check` et sous Windows.
- [x] **M1a-02** `feat(driver): pilote WDM minimal chargé et déchargé en mode test`
  `DriverEntry`, `AddDevice`, `Unload`, INF, catalogue de test, installation `pnputil`.
  *Fait quand* : chargement/déchargement 100 fois sans erreur dans la VM.
- [x] **M1a-03** `feat(portcls): bindings PortCls et KS générés en mode C (structures, GUID, vtables)`
  `bindgen` via `wdk_build::BuilderExt::wdk_default` sur `ks.h`, `ksmedia.h`, `punknown.h`,
  `drmk.h`, `portcls.h` avec `#define INTERFACE void` ; les vtables COM sortent plates du mode
  C ([driver-design.md](docs/driver-design.md) §2.2). Repli documenté : vtables manuelles.
  *Fait quand* : compile ; `size_of` des structures clés et nombre de slots de chaque vtable
  vérifiés par tests en mode utilisateur.
- [x] **M1a-04** `feat(portcls): objets COM sûrs, IUnknown, IAdapterPowerManagement, IMiniportTopology`
  `ComObject<V, T>`, `ComPtr`, `ComRef<I>` (crate portable `conduit-com`, Miri), comptage de
  références, `QueryInterface` par préfixe de vtable ; traits `AdapterPowerManagement` et
  `MiniportTopology` reliés aux vtables générées par une constante associée par type
  (`drivers/windows/portcls`, driver-design §3), enveloppes `ResourceList`/`PortTopology`.
  *Fait quand* : tests en mode utilisateur (AddRef/Release, QueryInterface, un faux « port »
  appelant la vtable) ; Miri sur le modèle objet.
- [x] **M1a-05** `feat(portcls): enveloppes IMiniportWaveRT, IMiniportWaveRTStream, IPortWaveRT`
  Traits `MiniportWaveRT`, `MiniportWaveRTStream`, `MiniportWaveRTStreamNotification` et
  leurs vtables (ordre du header 26100), `StreamObject` (flux à type effacé rendu par
  `NewStream`), enveloppes reçues `PortWaveRT`/`PortWaveRTStream` (`AllocatePagesForMdl`…),
  `adapter` (`PcNewPort`, `IPort::Init`, `PcRegisterSubdevice`, `PcRegisterPhysicalConnection`
  sous la feature `kernel`, noms UTF-16 des sous-périphériques).
  *Fait quand* : idem, plus les appels vers `IPortWaveRT` (`PcNewPort`, `RegisterSubdevice`) fonctionnent dans la VM.
- [x] **M1a-06** `feat(driver): adaptateur enregistrant une topologie rendu et capture`
  `StartDevice` (`conduit-kmd::adapter`) : `PcNewPort`, `IPort::Init`, `PcRegisterSubdevice`
  pour `WaveRender0`, `TopoRender0`, `WaveCapture0`, `TopoCapture0`, puis
  `PcRegisterPhysicalConnection` entre broches bridge ; tables KS `static` en `const`
  (`descriptors`), miniports `topo`/`wave` (`NewStream` valide le format puis refuse),
  état de câble `static` (`cable`), INF avec les `AddInterface` `KSCATEGORY_*`
  ([driver-design.md](docs/driver-design.md) §4.1).
  *Fait quand* : le gestionnaire de périphériques montre un endpoint rendu et un capture.
- [x] **M1a-07** `feat(driver): miniport WaveRT rendu avec tampon cyclique et horloge timer`
  Allocation du tampon cyclique, position via timer noyau, formats 48 kHz float32 et PCM16.
  *Fait quand* : une application lit un fichier sur l'endpoint sans erreur, position cohérente.
- [x] **M1a-08** `feat(driver): miniport WaveRT capture en boucle locale sur le rendu`
  *Fait quand* : un enregistreur capture ce que joue le lecteur.
- [x] **M1a-09** `feat(driver): INF complet, endpoints nommés Conduit 1`
  INF complet (`conduit_kmd.inx` en UTF-16 LE, `DeviceDesc`, `.NT.HW` DeviceType et SDDL,
  `FriendlyName` des quatre interfaces) et nom d'endpoint : les broches endpoint des
  filtres topologie portent un GUID `KsPinDescriptor.Name`
  (`portcls::adapter::pin_name_guid`) que l'INF associe à « Conduit 1 » sous
  `HKR\MediaCategories` — le seul levier documenté, l'INF seul ne suffit pas
  ([driver-design.md](docs/driver-design.md) §4.2).
  *Fait quand* : les réglages Son montrent « Conduit 1 » en rendu et en capture.
- [x] **M1a-10** `test(driver): script de test de boucle (sinus → capture, vérification)`
  Outil utilisateur (Rust, WASAPI) qui joue un sinus sur le rendu, capture, et vérifie
  fréquence, continuité de phase et absence de trous. `crates/conduit-looptest`
  (binaire du workspace racine, ADR-012 §4) : module `analysis` sans plateforme
  (moindres carrés `a·cos + b·sin` sans FFT, phase par blocs, trous, écrêtage,
  verdict) au-dessus de `conduit-backend-wasapi` ; `--repeat`, `--json`, `--list`,
  `--self-test` ([dev-guide.md](docs/dev-guide.md) §4 quinquies,
  [driver-dev.md](docs/driver-dev.md) §3.4).
  *Fait quand* : passe 10 fois de suite.
- [x] **M1a-11** `test(driver): 1 h Driver Verifier sans erreur, collecte automatique des dumps`
- [x] **M1a-12** `docs: ADR-015 résultat du spike`
  [ADR-015](docs/adr/015-resultat-du-spike-pilote-rust.md), acceptée le 2026-09-07 :
  **la porte est franchie sur les quatre questions**, le pilote reste en Rust et le repli
  C++ n'est pas exercé. Le délai borné n'est pas consommé — deux jours contre trois
  semaines proposées.

---

## M1b — Windows complet

Objectif : première plateforme livrable. Critère de sortie : F-01 à F-06 vérifiés, pilote
signé par attestation, HLK audio passé, latence conforme à SPEC §5.6, endurance 24 h.

### M1b.A — Pilote

- [x] **M1b-01** `feat(driver): paramètres de registre (taille de réserve, canaux) validés au démarrage`
  *Fait quand* : valeurs hors bornes → valeurs par défaut et journal d'événements, jamais d'échec de chargement.
  `conduit-kmd-core::params` est une fonction **totale** : elle écrête vers la borne
  franchie et rend un compte rendu, à l'inverse de `format::buffer_bytes` qui refuse. Un
  tampon trop grand est une demande d'application qu'il faut rejeter ; un paramètre de
  registre aberrant est une erreur d'administration, qu'il ne faut surtout pas transformer
  en pilote qui ne charge plus.
  *Mesuré le 2026-09-07*, dans la VM, débogueur série attaché — le pilote **charge dans
  les huit cas** (statut OK, code problème 0), et ses propres traces disent ce qu'il a
  décidé :

  | Valeur | Décision | Journal |
  |---|---|---|
  | 2 | 2 câbles | (rien : dans les bornes) |
  | 3 | 3 câbles | (rien) |
  | 16 (défaut de l'INF) | 16 câbles | (rien) |
  | 0 | « sous le plancher 1, repli sur 1 » | 1 entrée |
  | 99 | « au-dessus du plafond 16, repli sur 16 » | 1 entrée |
  | absente | 16 câbles | 1 entrée |
  | `"abc"` (`REG_SZ`) | 16 câbles | 1 entrée |
  | `BufferMs` = 999 | « au-dessus du plafond 500, repli sur 500 » | 1 entrée |

  Le texte de l'Observateur d'événements est **vide** : il faudrait une ressource de
  messages dans le binaire, que ni l'INF ni `HKR` ne peuvent fournir sans survivre à la
  désinstallation (F-52). Toute l'information est dans la chaîne d'insertion, propriété
  **1** sur trois, relevée telle quelle : `Conduit : tampon (ms) (BufferMs) = 999
  au-dessus du plafond 500, repli sur 500`.
  Les **canaux** sont lus, validés et journalisés mais pas appliqués : `CHANNELS` est
  scellée dans les tables KS et dans des assertions à la compilation, et la rendre
  dynamique serait faire M1b-05 par la bande.
- [x] **M1b-02** `feat(driver): réserve de N câbles (N × rendu + N × capture)`
  *Fait quand* : 16 câbles enregistrés, chargement < 2 s.
  *Mesuré le 2026-09-07*, dans la VM : **16 câbles, 64 sous-périphériques, 32 endpoints**,
  `CM_PROB_NONE`, démarrage en **0,86 s** — le budget des deux secondes n'est consommé qu'à
  moitié. L'INF (944 lignes) est engendré depuis les constantes du pilote par
  `portcls/tests/inf.rs`, dont un test vérifie la fraîcheur du fichier commité.
  Un plafond oublié a fait échouer le cinquième sous-périphérique avec
  `STATUS_ALLOTTED_SPACE_EXCEEDED` : `MaxSubdevices`, passé à `PcAddAdapterDevice`, borne
  `PcRegisterSubdevice` et valait encore la taille d'un seul câble.
- [x] **M1b-03** `feat(driver): état de jack par câble, inactifs masqués`
  `KSPROPERTY_JACK_DESCRIPTION` avec `IsConnected` ; câbles 1 et 2 actifs par défaut.
  *Fait quand* : les réglages Son ne montrent que Conduit 1 et 2 ; les autres apparaissent sous « déconnectés ».
  *Mesuré le 2026-09-07*, dans la VM — et **mesuré plutôt que constaté à l'œil** : ce que
  les réglages Son affichent est l'état MMDevice, qui se lit. Les seize câbles publient
  bien leurs trente-deux endpoints ; **seuls Conduit 1 et 2 sont à l'état 1 (actif)**, les
  quatorze autres à l'état **8 (débranché)**, et seuls les quatre endpoints des deux
  premiers câbles ont un devnode présent.
  Trois affirmations de la conception étaient fausses et ont été corrigées avant d'écrire
  le code : la description de prise est une propriété du **filtre**, pas de la broche (une
  broche bridge à `MaxInstanceCount = 0` ne s'instancie jamais, une table posée là ne
  serait jamais atteinte) ; `N` compte les **prises**, pas les canaux, donc un câble en
  déclare **une** à deux canaux ; et la broche décrite est celle qui porte la catégorie
  d'endpoint, pas celle que notre code appelle `bridge_pin` — collision de vocabulaire avec
  la documentation. Cette dernière erreur ne casse rien de visible : la propriété répond,
  toujours vide. Des assertions `const` la verrouillent désormais.
  *Dette assumée* : `KSEVENT_PINCAPS_JACKINFOCHANGE` n'est pas implémenté. Rien ne change
  encore l'état, mais sans lui M1b-04 donnerait « la propriété rend la bonne valeur et le
  panneau de son ne bouge pas ». Noté à trois endroits du code.
- [x] **M1b-03b** `feat(driver): nœud de volume, pour que le câble soit transparent`
  Nœuds `KSNODETYPE_VOLUME` et `KSNODETYPE_MUTE` sur les filtres de topologie. Sans eux,
  Windows insère son **APO logiciel** et applique au signal le volume par défaut qu'il
  donne à tout endpoint neuf : mesuré à 64 % le 2026-09-06, soit une amplitude de 0,229
  pour 0,500 demandée ([driver-design.md](docs/driver-design.md) §5.5).
  *Fait quand* : `conduit-looptest --repeat 10` rend l'amplitude demandée sans avoir à
  forcer le volume de l'endpoint, et la modifier dans les réglages Son ne change plus rien
  à ce qui traverse le câble.
  *Mesuré le 2026-09-07*, en session console : un endpoint neuf rend **100 %** au lieu de
  64 %, dix passes sur dix à **amplitude 0,500**, et le volume forcé à **30 % laisse
  l'amplitude à 0,500** — le curseur est décoratif, le câble est transparent. Au passage,
  deux hypothèses de conception vérifiées : l'échelle est bien 1/65536 dB (plage annoncée
  −96,0 / 0,0 dB au pas de 0,5), et le canal est bien le premier `LONG` de l'instance
  (Windows interroge canal 0 puis canal 1).
*Non-régression du lot M1b-01/02/03, mesurée le 2026-09-08* : `conduit-looptest --repeat 10`
sur Conduit 1, en session console, **deux séries de dix passes sur dix**, 440,000 Hz,
amplitude 0,500, aucun saut de phase, aucun trou — avec les seize câbles enregistrés, le
nœud de volume et l'état de jack en place.
Le **débogueur noyau attaché fausse cette mesure** : à version, machine et séquence
identiques, il donne 17 passes sur 20 (sauts de phase et un trou de 384 trames) contre
20 sur 20 une fois détaché. Chaque trace part par le canal série, et le moteur audio
interroge la broche de jack en boucle. Consigné dans `vm-debug.ps1`.

- [x] **M1b-04** `feat(driver): propriété KS privée de configuration (activer, désactiver, canaux)`
  Jeu de propriétés KS privé `KSPROPSETID_Conduit` exposé par le filtre de topologie de
  chaque câble, et non un objet de périphérique de contrôle séparé
  ([driver-design.md](docs/driver-design.md) §6, décision anticipée) : PortCls fait le
  routage, l'accès passe par les handles standard, et la validation reste un parseur sur un
  tampon borné, fuzzable en mode utilisateur (M1b-08). Écriture soumise à
  `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`, bascule du jack à chaud.
  *Fait quand* : activation → endpoint visible en < 1 s sans PnP (F-01) ; entrée invalide → `STATUS_INVALID_PARAMETER` sans effet.
  *Mesuré le 2026-09-08*, dans la VM, par `conduit-looptest --cable-*` (client livré dans
  `conduit-backend-wasapi`, que M1b-34 réutilisera) :

  | Mesure | Résultat |
  |---|---|
  | Activation du câble 3 → endpoint **rendu** | **77 ms** |
  | Activation du câble 3 → endpoint **capture** | **77 ms** |
  | Désactivation → disparition | 21 ms et 31 ms |
  | Six entrées invalides | **6 refus sur 6**, `ERROR_INVALID_PARAMETER`, état inchangé |
  | Persistance (`ActiveCables` 0x3 → 0xB, redémarrage du périphérique) | état conservé |
  | Les deux côtés du câble | s'accordent, seize filtres sur seize |

  Les entrées éprouvées : charge utile de 15 puis 17 octets, champ réservé non nul,
  `connected = 2`, index de câble d'un **autre** câble, index hors domaine.
  **Deux incertitudes levées par la mesure**, toutes deux écrites comme incertaines dans le
  code avant de l'être :
  1. *Le contexte de thread du gestionnaire de propriété.* La documentation n'en dit rien
     et le danger était asymétrique : sur un fil système, `ExGetPreviousMode()` rendrait
     `KernelMode` et le contrôle de privilège **laisserait tout passer**. Il refuse — il
     évalue donc le jeton de l'appelant en mode utilisateur.
  2. *Le routage d'un item d'événement déclaré au niveau filtre vers une requête visant une
     broche*, attesté par SYSVAD mais non spécifié. Les 77 ms le tranchent : l'événement
     part. L'expérience de repli (une seconde table sur la broche endpoint) n'a pas eu à
     être tentée.
  Au passage, une règle Windows qui coûte cher à ignorer : `SeLoadDriverPrivilege` est
  **présent mais désactivé** dans tout jeton neuf, **y compris celui de `LocalSystem`**.
  Le pilote l'exige actif ; c'est au client de l'armer, et `AdjustTokenPrivileges` rend
  `TRUE` même quand elle n'a rien armé — seul `ERROR_NOT_ALL_ASSIGNED` le dit.
- [x] **M1b-05** `feat(driver): formats 44,1/48/96 kHz, float32, PCM16 et PCM24`
  *Fait quand* : test de boucle pour chaque format (F-04).
  *Code livré le 2026-09-09, côté pilote et côté crate portable* : `SampleFormat::Pcm24`,
  matrice de formats, **24 variantes** de descripteurs par sens (3 fréquences × 8 canaux —
  pas 72, parce que `copy_frames` convertit déjà les profondeurs et que seuls la fréquence
  et les canaux doivent s'accorder entre les deux bouts), une valeur `CableFormat<n>` par
  câble, `CHANNELS` libérée (ses six assertions devenues des invariants de domaine),
  `BufferMs` enfin appliqué. Coût : **+20,3 Kio** de section de données.
  *La moitié espace utilisateur n'est pas faite* : ordre 8 du protocole, version 3,
  `conduitctl cable set-format`, et le format dans `CableSpec`/`CableInfo`.
  **Deux défauts trouvés en machine le 2026-09-09.**
  *Le premier, corrigé* : un câble configuré pour autre chose que la stéréo ne pouvait plus
  être **activé** (`Win32 87`). Le `SET` de `KSPROPERTY_CONDUIT_CABLE_STATE` comparait les
  canaux de la requête à ceux configurés pour le câble, alors que le service envoyait
  toujours 2 — défaut de frontière entre deux lots écrits en parallèle. Tranché en écrivant
  la sémantique du champ : `channels` est un **écho vérifié**, comme `cable`, et un client
  qui veut seulement brancher le jack renvoie ce que le `GET` vient de lui rendre. Le refus
  nomme désormais la valeur attendue au lieu de rendre « code 87 ».
  *Le second, ouvert* : **seule la variante par défaut produit un endpoint utilisable.**
  Quatre câbles neufs, quatre formats, activés par la propriété KS, une dimension à la fois :

  | Format du câble | Variante | Endpoint obtenu |
  |---|---|---|
  | 48 kHz, 2 canaux — **témoin** | 9 (défaut) | 2 canaux, 48000 Hz, 16 bits |
  | 48 kHz, 2 canaux, PCM24 préférée | 9 | 2 canaux, 48000 Hz, 16 bits |
  | **44,1 kHz**, 2 canaux | 1 | **2 canaux, 44100 Hz, 16 bits** |
  | **96 kHz**, 2 canaux | 17 | aucun format |
  | 48 kHz, **1 canal** | 8 | aucun format |
  | 48 kHz, **3 canaux** | 10 | aucun format |
  | 48 kHz, **6 canaux** | 13 | aucun format |

  `GetMixFormat` rendait **`0x88890008`, `AUDCLNT_E_UNSUPPORTED_FORMAT`**.
  Un confondant a été levé en chemin : les deux premiers cas sont la **même** variante, celle
  du défaut de compilation, et ne prouvaient donc rien sur les autres. Le 44,1 kHz l'a réglé —
  le pilote servait bien ses variantes.
  **Cause unique, trouvée et corrigée** : le pilote laissait `DataRangeIntersection` au
  défaut, donc PortCls appliquait ses *default data-intersection handlers*, qui ne traitent
  **que PCM, mono et stéréo, sans `WAVEFORMATEXTENSIBLE`**. Notre propre gestionnaire
  (négociation **pure** dans `conduit-kmd-core::wavefmt`, traduction dans
  `conduit-kmd/src/intersect.rs`) règle les trois symptômes d'un coup — y compris le mono et
  le 96 kHz, que la documentation n'expliquait pas :

  | Format du câble | Endpoint obtenu |
  |---|---|
  | 48 kHz, **1 canal** | 1 canal, 48000 Hz, **32 bits** |
  | 48 kHz, **3 canaux** | 3 canaux, 48000 Hz, 32 bits |
  | 48 kHz, **6 canaux** | 6 canaux, 48000 Hz, **24 bits** |
  | **96 kHz**, 2 canaux | 2 canaux, **96000 Hz**, 32 bits |
  | **44,1 kHz**, 2 canaux | 2 canaux, 44100 Hz, 24 bits |

  Les profondeurs retenues suivent la règle documentée — « *the highest value in each
  parameter's region of intersection* » — là où le gestionnaire par défaut ne rendait jamais
  que du 16 bits.
  *Critère F-04 tenu, mesuré le 2026-09-09* en session console, débogueur détaché —
  **douze passes, quatre formats, aucune faute** : 440,00 Hz, amplitude 0,500, zéro saut de
  phase, zéro trou, à 48 kHz stéréo, **44,1 kHz stéréo**, **96 kHz stéréo** et
  **48 kHz sur six canaux**. Le bloc suit la fréquence (441, 480, 960 trames).
  *Piège d'exploitation à connaître* : le format du moteur d'un endpoint est **mis en cache
  à sa création**. Changer `CableFormat<n>` d'un câble déjà actif ne le déplace pas — il faut
  un câble dont l'endpoint n'existait pas encore. C'est ce qui rend le redémarrage du
  périphérique nécessaire et non suffisant, et il faudra le dire à l'utilisateur quand
  `conduitctl cable set-format` existera.
  *Reste* : la moitié espace utilisateur (`conduitctl cable set-format`, ordre 8 du
  protocole, format dans `CableSpec`/`CableInfo`).

  « Aucun format » : l'endpoint existe et devient actif, mais `IAudioClient::GetMixFormat`
  échoue. Ce qui est **écarté** : les tables du binaire livré sont exactes entrée par entrée
  (démontage du PE, relocations comprises) ; le garde-fou `check_cable_pins`, qui traverse
  la chaîne **par les pointeurs que PortCls suivra**, ne signale rien ; le pilote lit la
  bonne valeur ; et le chemin d'activation est hors de cause, le témoin passant par le même.
  Le pilote est donc cohérent avec lui-même : ce qu'il déclare **ailleurs** que dans les
  plages de la broche wave dit encore « 48 kHz stéréo ».
  *Fausse piste, consignée pour ne pas y revenir* : j'ai d'abord cru que l'endpoint d'un
  câble activé au-delà des deux par défaut n'avait pas de format (`GetMixFormat` échouait).
  C'était **mon** chemin d'activation : écrire le masque `ActiveCables` en registre puis
  redémarrer le périphérique ne fait **pas** partir l'événement de jack, et Windows réutilise
  telle quelle la clé d'endpoint créée jadis « débranché, sans format ». Activé par la
  propriété KS — le seul chemin du produit — le câble 3 marche parfaitement. Un démontage du
  binaire livré avait d'ailleurs établi que les 24 rangées de plages, les 48 broches système
  et les 48 filtres wave sont exacts entrée par entrée, et qu'au format par défaut les seize
  câbles reçoivent des descripteurs **identiques**.
  *Instruction du 2026-09-09, à la table plutôt qu'en machine — trois suspects écartés,
  preuve à l'appui* : le pilote **livré** a été démonté (en-tête PE, table des symboles du
  `.map`, contenu de `.rdata`, table de relocation). Les quatre tables de descripteurs y
  sont **exactes, entrée par entrée** : les 24 rangées de `SYSTEM_RANGE_VALUES` portent bien
  (44,1 / 48 / 96 kHz) × (1 à 8 canaux) × (F32 32, PCM 24, I16 16), dans l'ordre annoncé ;
  les 24 rangées de `SYSTEM_RANGES_TABLE` visent bien leur propre triplet de plages ; les
  48 broches système visent bien leur propre rangée de pointeurs, avec `DataRangesCount = 3`
  et une instance ; les 48 descripteurs de filtre wave visent bien leur propre paire de
  broches, `Nodes` nul et une connexion ; les 32 filtres de topologie visent bien leur propre
  paire de broches et le GUID de nom de **leur** câble (`Data4[7]` = 0 à 15) ; et les
  **1 072** pointeurs logés dans `.rdata`/`.data` ont tous leur relocation, aucun n'en
  manque. Conséquences : (1) aucun décalage d'index dans `variant_of` ni dans l'indexation
  des tables ; (2) le repli silencieux ne peut **pas** produire le symptôme — il rend la
  variante 9, dont les octets sont ceux que Conduit 1 sert et qui marche ; (3) les deux bouts
  d'un même câble s'accordent, table pour table. Les seize câbles reçoivent des descripteurs
  **identiques** au format par défaut : le pilote ne peut donc pas être la source d'une
  différence *entre* câbles à format égal. Reste à instruire, dans l'ordre : ce que Windows
  fait de la **troisième** plage (PCM 24 bits en conteneur de trois octets, la seule chose
  que M1b-05 ajoute à ce que la broche déclarait) et le moment où l'endpoint d'un câble
  activé **après** le démarrage se voit attribuer son `PKEY_AudioEngine_DeviceFormat` —
  l'expérience qui tranche est de retirer la plage PCM24 de `SAMPLE_DEPTHS` et de rejouer la
  séquence, ce qui ramène la broche à ce qu'elle déclarait avant M1b-05.
  *Instruction documentaire du 2026-09-09, deuxième passe — la moitié « canaux » est
  tranchée, sur pièces.* La page « Default Data-Intersection Handlers » du WDK
  (`learn.microsoft.com/windows-hardware/drivers/audio/default-data-intersection-handlers`)
  décrit les limites du handler d'intersection **par défaut** de PortCls — celui auquel notre
  `DataRangeIntersection` renvoie en rendant `STATUS_NOT_IMPLEMENTED` : il ne traite que les
  formats **PCM**, ne connaît que « *Only mono and stereo audio streams* », et **ne sait
  produire aucun format contenant un `WAVEFORMATEXTENSIBLE`** — c'est-à-dire exactement la
  structure qu'il faut pour porter un `dwChannelMask` au-delà de deux canaux. La même page
  conclut qu'un pilote non-PCM ou multicanal doit écrire son propre handler ; « Extensible
  Wave-Format Descriptors » dit de son côté qu'un `WAVEFORMATEX` simple ne peut décrire
  correctement que le mono et le stéréo ; et « Data-Intersection Handlers » est le seul
  endroit de la documentation qui relie explicitement le **moteur audio** au handler
  d'intersection du filtre wave. SYSVAD confirme par l'abstention : son
  `CMiniportWaveRT::DataRangeIntersection` finit par rendre `STATUS_NOT_IMPLEMENTED`, et sa
  table de broche ne déclare que du stéréo (`SPEAKER_HOST_MAX_CHANNELS = 2`), de 24 à
  96 kHz. **Donc : six canaux ne peuvent pas marcher tant que le pilote n'implémente pas son
  propre `DataRangeIntersection`**, rendant un `KSDATAFORMAT_WAVEFORMATEX` étendu en
  `WAVEFORMATEXTENSIBLE` (`wFormatTag = WAVE_FORMAT_EXTENSIBLE`, `cbSize = 22`,
  `dwChannelMask`, `SubFormat`), avec la taille requise majorée de 22 octets et
  `OutputBufferLength == 0` traité en `STATUS_BUFFER_OVERFLOW`. Rien n'est *faux* dans le
  pilote : c'est une limite documentée du repli qu'on avait choisi, et le commentaire
  d'`audio_range` — « PortCls intersecte lui-même, une plage ponctuelle ne lui laisse rien à
  choisir » — est vrai jusqu'au stéréo et faux au-delà.
  *Ce que la documentation ne dit pas*, et il faut le dire aussi : **rien n'explique le
  96 kHz**. SYSVAD déclare 88,2 et 96 kHz en stéréo et une `KSDATARANGE_AUDIO` de 24 000 à
  96 000 Hz. Aucune borne dépendant de la fréquence n'est documentée pour
  `AllocateAudioBuffer`, `GetDeviceDescription` ni `GetHWLatency` (qui rend `VOID` et ne peut
  pas échouer) ; `KSPROPERTY_AUDIO_CHANNEL_CONFIG` est un vestige DirectSound, déprécié
  depuis Vista, et n'est requis nulle part pour le multicanal WASAPI ; aucune page ne donne
  l'ordre des requêtes du générateur d'endpoints ni son repli quand l'INF ne pose pas
  `PKEY_AudioEngine_OEMFormat` — le **seul** levier documenté pour fixer le format par défaut
  d'un endpoint. Enfin, `AUDCLNT_E_UNSUPPORTED_FORMAT` n'est pas documenté comme code de
  retour de `GetMixFormat` : le candidat cohérent est `AUDCLNT_E_DEVICE_INVALIDATED`.
  *Le confondant que le tableau ci-dessus ne sépare pas* : les deux formats qui marchent
  (`0x00020302` et `0x00020202`) sont **la même variante de descripteurs**, la variante 9,
  qui est aussi celle du **défaut de compilation** ; les deux qui échouent sont des variantes
  que le pilote n'avait jamais servies. Le tableau fait donc varier deux choses à la fois —
  ce que Windows voit, et le fait d'être ou non sur la rangée par défaut — et ne peut pas les
  séparer. Aucune mesure faite à ce jour ne le peut.
  *L'expérience qui les sépare*, une seule fournée de câbles neufs, activés par la propriété
  KS comme le témoin :

  | `CableFormat<n>` | Format | Variante | Attendu |
  |---|---|---|---|
  | `0x00020301` | 44,1 kHz, 2 canaux, F32 | 1 | **marche** si le pilote est hors de cause (44,1 kHz stéréo est le format le plus banal qui soit, et il est dans le domaine du handler par défaut) |
  | `0x00010302` | 48 kHz, **1 canal**, F32 | 8 | **marche** si le pilote est hors de cause (le mono est explicitement supporté par le handler par défaut) |
  | `0x00030302` | 48 kHz, **3 canaux**, F32 | 10 | **échoue** dans tous les cas : au-delà du stéréo, borne documentée — c'est le contrôle négatif |

  Lecture : *les deux premiers marchent* → le pilote sert bien autre chose que sa variante
  par défaut, la moitié « canaux » est expliquée, et il ne reste à instruire que le 96 kHz,
  seul. *Les deux premiers échouent* → l'axe n'est ni la fréquence ni les canaux mais
  « toute variante autre que celle du défaut », le défaut est dans le pilote et
  `check_cable_pins` ne le couvre pas ; l'étape suivante est alors de lire
  `KSPROPERTY_PIN_DATARANGES` depuis l'espace utilisateur sur le filtre wave fautif **et** sur
  le témoin, et de comparer les 88 octets. *Un des deux seulement échoue* → l'axe est celui
  qui reste, et le suspect se réduit à lui.
  *Deux garde-fous ajoutés en attendant.* `topo::check_cable_topology`, appelé par
  `StartDevice` à côté de `check_cable_pins`, confronte ce que la **topologie** déclarera — le
  nombre de canaux des nœuds volume et sourdine, la cartographie `KSAUDIO_SPEAKER_*` du
  jack — à ce que la broche wave déclare réellement, lu au bout des pointeurs que PortCls
  suivra ; il rend bruyants, au journal d'événements, deux replis muets en release
  (`mapping_rendu`, qui rend le masque stéréo hors domaine, et `description`, qui rend le
  filtre du câble 0 faute de rangée). Et l'énumération WASAPI porte désormais le **HRESULT**
  de `GetMixFormat` dans son message au lieu de « a échoué » : c'est l'information la plus
  discriminante qui manquait au tableau, et un `.ok()` l'avalait.
- [ ] **M1b-06** `feat(driver): gestion d'alimentation et arrêt propre`
  *Fait quand* : veille/reprise 50 fois avec flux ouvert, sans erreur ni fuite.
  *Code livré le 2026-09-09* : enveloppe de `PcRegisterAdapterPowerManagement`,
  implémenteur d'`AdapterPowerManagement`, `Cable::suspend` (D3) et `Cable::stop`.
  **Le critère n'est pas mesurable dans la VM** : `powercfg /a` dans l'invité rend
  « aucun état de veille disponible » — Hyper-V n'expose ni S1-S3, ni veille prolongée,
  ni S0 basse consommation. À reprendre sur une machine physique, ou à requalifier.
  *Ce qui a été mesuré à la place*, et c'est le vrai risque : la règle Driver Verifier du
  domaine audio dit que **deux `PcRegisterAdapterPowerManagement` sans désenregistrement
  intercalaire donnent un bugcheck `0xC4 / 0x00071006`**. Or le pilote ne désenregistre
  jamais et chaque cycle désactiver/réactiver rejoue `StartDevice`. **Cinq cycles** :
  périphérique OK, quatre endpoints à chaque fois, **zéro vidage, zéro bugcheck**,
  amorçage inchangé — le verrou d'unicité tient.
  *Deux points documentés comme incertains dans le code*, faute de source : l'IRQL des
  rappels d'alimentation (PortCls ne le documente pas ; l'affirmation `PASSIVE_LEVEL` qui
  traînait dans le trait a été corrigée), et la survie de l'enregistrement à un cycle PnP.
  D'où le choix d'`ExCancelTimer` (`<= DISPATCH_LEVEL`, n'attend rien) plutôt que
  d'`ExDeleteTimer(Wait = TRUE)` (`<= APC_LEVEL`, bloquant) en D3.
  *Suite décrite et non faite* : `IAdapterPnpManagement` est le vrai rappel d'arrêt, et
  c'est là que `Cable::stop` et `PcUnregisterAdapterPowerManagement` trouveraient leur
  appelant.
- [x] **M1b-07** `feat(driver): comportement à un seul côté ouvert`
  *Fait quand* : capture seule → silence ; rendu seul → pas d'accumulation.
  Le comportement était **déjà correct et testé** depuis M1a ; le travail a consisté à le
  rendre **mesurable**. Le plan dit désormais *pourquoi* il demande un silence
  (`SilenceCause::NoRender` — permanent — contre `BeforeRenderStart` — transitoire), et
  `Plan::discarded` marque le seul cas qui ne demande rien à écrire. Sans lui, « le rendu
  tournait seul » et « rien ne tournait » rendaient exactement le même plan vide.
  *Mesuré le 2026-09-09*, dans la VM, session 0 (on éprouve des **chemins de code**, pas du
  contenu sonore) — **la moitié « rendu seul » est démontrée** :
  ```
  câble 0 : 13000 ticks, 0 trames copiées, 0 silences sans rendu, 13000 ticks jetés
  câble 0 : 14000 ticks, 48775 trames copiées, …, 13319 ticks jetés
  ```
  Vingt secondes de rendu sans capture : le compteur de rejets suit les ticks, tout le
  reste reste à zéro. Puis la passe normale démarre et il se fige net.
  *Les deux moitiés mesurées le 2026-09-09*, **sans débogueur**, session console intacte,
  par `conduit-looptest --cable-compteurs` — les compteurs sont désormais exposés par le jeu
  de propriétés privé (`KSPROPERTY_CONDUIT_COUNTERS`, GET seul, sans privilège), ce qui les
  rend lisibles **en release** et sans l'outil qui fausse les mesures de transport :

  | | ticks | copiées | silences sans rendu | ticks jetés |
  |---|---|---|---|---|
  | après une passe normale de 5 s | 3 440 | 242 676 | **192** | 0 |
  | après 10 s de **rendu seul** | 10 058 | 242 676 *(inchangé)* | 192 *(inchangé)* | **6 618** |

  La première ligne prouve la moitié « capture seule » : les 192 silences viennent de la
  fenêtre où la capture tourne encore alors que le rendu s'est arrêté. La seconde prouve
  « rendu seul » : rien copié, aucun silence écrit, seuls les rejets montent.
  *Défaut mineur du relevé, à corriger* : la ligne de régime (« les deux côtés tournent »)
  est déduite des compteurs **cumulés**, donc elle décrit le passé et non l'instant — elle
  annonçait « le câble transporte » alors que plus rien ne tournait.
- [x] **M1b-08** `test(driver): harnais utilisateur et fuzzing du parseur de la propriété KS`
  Le code de validation compile aussi en mode utilisateur pour être fuzzé.
  *Fait quand* : 1 h de fuzzing sans panique.
  *Campagne du 2026-09-09*, nightly + AddressSanitizer, quatre cibles en parallèle,
  20 min chacune — **1 h 20 de fuzz cumulé, 506 556 115 exécutions, zéro panique, zéro
  artefact** :

  | Cible | Exécutions | exec/s | Corpus final |
  |---|---|---|---|
  | `etat-cable` (`CableState::from_bytes`) | 162 871 439 | 135 613 | 9 |
  | `parametres-registre` (`decode_dword` + `sanitize`) | 182 341 689 | 151 824 | 18 |
  | `formats` (`validate`, `buffer_bytes`) | 143 602 343 | 119 568 | 56 |
  | `protocole` (`Requete::from_bytes`, `decouper`) | 17 740 644 | 14 771 | **417** |

  Le protocole du service est dix fois plus lent — il alloue et boucle — et c'est lui qui a
  produit le corpus le plus riche : **c'est là qu'est le gisement**, comme prévu.
  `conduit-kmd-core` interdit déjà la panique par lint (`panic`, `unwrap`,
  `indexing_slicing`, `arithmetic_side_effects`) : le fuzz y a valeur de **preuve**, pas de
  découverte.
  *Défaut trouvé et corrigé* : `crates/conduit-protocol/fuzz` **ne construisait plus**, donc
  la commande documentée dans le guide était morte. `workspace.exclude` n'affranchit pas un
  crate logé **sous** un membre du workspace — il faut un `[workspace]` vide dans son propre
  manifeste. Un harnais de fuzzing cassé qu'on croit fonctionnel est pire que pas de harnais.
  *Le même piège s'est refermé une deuxième fois* : la cible `formats` a cessé de compiler
  quand M1b-05 a supprimé `M1A_FORMATS`, et **aucune vérification ne le voyait** — les
  crates `fuzz/` vivent dans leur propre workspace, précisément pour ne pas être tirés par
  `--workspace`. La parade est un test **du workspace** (`conduit-testing/tests/fuzz.rs`)
  qui fait `cargo check --manifest-path` sur chaque `crates/*/fuzz/Cargo.toml` : il part
  donc à chaque `cargo test`, sans réintégrer les crates. Vérifié dans les deux sens en
  réintroduisant le défaut. Le nightly n'est **pas** nécessaire — `libfuzzer-sys` ne l'exige
  que pour lier le binaire instrumenté, pas pour l'analyse.
  Une septième cible a été ajoutée pour `wavefmt::intersect`, la négociation de format dont
  la moitié des octets vient de Windows : **33 841 780** exécutions, zéro plantage.
  *Défaut trouvé et délibérément non corrigé par le fuzzer* : `SupportedFormat::accepts`
  compare le nombre de canaux demandé **sans vérifier que celui de l'entrée soit
  représentable** — un format accepté dont `layout()` vaut `None`. Inatteignable tant que la
  liste supportée était une `const` à deux canaux ; **M1b-05 la construit à l'exécution**.
  *Corrigé le 2026-09-09* : `accepts` commence par refuser une entrée sans trame, et le test
  `accepts_refuse_une_entree_sans_trame` rejoue l'entrée déclenchante du fuzzer (demande et
  entrée toutes deux à `channels: 0`). Il échoue sans la garde, passe avec.
- [ ] **M1b-09** `test(driver): 1000 cycles activation/désactivation et 48 h de stress`
  *Fait quand* : aucune fuite (pool tags stables), aucun BSOD, Driver Verifier actif.
  **Reporté (2026-09-08)** : la priorité va au fonctionnel. La campagne d'une heure sous
  Driver Verifier de M1a-11 (360 tours, zéro vidage, zéro bugcheck) tient lieu de garde-fou
  en attendant. C'est aussi cette tâche qui dira si les seize minuteurs à 1 ms doivent
  devenir un minuteur global — voir le principe « pilote le plus léger possible »
  ([driver-design.md](docs/driver-design.md) §1).
- [ ] **M1b-10** `test(driver): passage des tests HLK audio`
  *Fait quand* : rapport HLK sans échec bloquant.
- [x] **M1b-11** `feat(portcls): le flux devient composite, sans exposer une interface de plus`
  Lot 1 du mode paquets WaveRT (commit `b99366c`). Chaque flux est désormais un **objet
  composite à plusieurs têtes de vtable** (`PacketStream`, `portcls/src/packet.rs`),
  construit avec `PacketInterfaces::None` : la mécanique du mode paquets existe et le pilote
  n'expose **rien de plus qu'avant**.
  *Fait quand* : le transport est intact — la neutralité est l'objet même du lot.
  *Mesuré le 2026-09-10*, en session console, débogueur détaché (VM Hyper-V, Windows 11
  26200) : `conduit-looptest --repeat 20` rend **20 passes sur 20, deux fois** (00:00 puis
  01:18). Quinze tests de cycle de vie en mode utilisateur, dont un vérifié par mutation.
  *Une fausse alerte, consignée pour ne pas y revenir* : un premier **14 sur 20** le
  2026-09-09 à 20:46 était une charge transitoire de la VM — la relance identique rend 20 sur
  20. L'autre hypothèse, une fuite de référence, est écartée par la **forme** de la panne et
  non par la relance : elle se serait vue dès la passe n° 2, en `STATUS_DEVICE_BUSY`.
- [x] **M1b-12** `feat(pilote): ce que le moteur audio a demandé se lit sans débogueur`
  Lot 0 du mode paquets (commit `4b006cb`). Sélecteur privé
  `KSPROPERTY_CONDUIT_TRANSPORT = 3`, **GET seul**, rendu par
  `conduit-looptest --cable-transport` : par sens, le mode d'allocation
  (`AllocateAudioBuffer`, c'est-à-dire scrutation, contre `AllocateBufferWithNotification`),
  le `NotificationCount`, la taille du tampon, les événements enregistrés, l'état KS et un
  compteur cumulé d'allocations **refusées**. `CONFIG_VERSION` **3 → 4**.
  *Fait quand* : « le moteur scrute-t-il, ou avons-nous dit non ? » se tranche en session
  console, sans l'outil qui fausse les mesures de transport.
- [x] **M1b-13** `feat(pilote): exposer le mode paquets sans le servir, et le mesurer`
  Lot 2 (commit `b57cd11` ; `c0cbb47` pour `conduit-looptest --exclusif`). Paramètre de
  registre `PacketMode` (REG_DWORD, défaut **0**, **jamais livré à 1**) et sélecteur
  `KSPROPERTY_CONDUIT_PACKETS = 4` : compteurs par sens et par méthode, IRQL, QPC, et le
  comptage des `QueryInterface` reçus sur les deux IID de paquets. `CONFIG_VERSION` **4 → 5**.
  *Fait quand* : on sait si le moteur audio cherche les paquets, et ce qu'il fait quand il
  les trouve. *Mesuré le 2026-09-10 entre 01:15 et 01:19*, session console, débogueur
  détaché — la réponse est **non** en partagé et **oui** en exclusif :

  | Client | Allocation obtenue | Appels de paquets à `PacketMode = 1` |
  |---|---|---|
  | Partagé classique (`IAudioClient::Initialize`, 10 ms) | `AllocateAudioBuffer` (scrutation), 4096 trames, **0 notification** | **0** — interfaces exposées, rendues au moteur, **jamais appelées** |
  | Exclusif événementiel (`--exclusif`) | `AllocateBufferWithNotification`, `NotificationCount` **2**, 2 × 240 trames | **8 000 `GetReadPacket` en 20 s** (400/s), tous refusés (`STATUS_NOT_SUPPORTED`) |

  **La décision que la mesure impose** : exposer sans servir **casse** les clients exclusifs —
  la passe échoue en `0x80070032` au préremplissage —, donc `PacketMode` ne doit jamais être
  livré à 1. Réversibilité prouvée : retour à 0, 20 passes sur 20.
  *Un signal qui n'en est pas un* : les `QueryInterface` sur les IID de paquets sont une
  **sonde systématique de PortCls**, deux par flux créé quel que soit le client. Le compteur
  ne dit donc rien du moteur.
- [x] **M1b-14** `feat(pilote): contraintes de taille de paquet, et 4 ms de période en partagé`
  Commits `8d54aed`, `99dc6c3`, `b84f0d5`, `85b56ac`, plus `7fd05e1` pour l'outil
  (`conduit-looptest --faible-latence --periode-ms`). *Origine* : **décision de Nathan du
  2026-09-10**, « j'aimerais être compatible des normes modernes ». Le critère du plan — « le
  moteur emprunte les paquets » — s'est révélé **inatteignable en l'état** ; plutôt que de
  clôturer, on a cherché les conditions manquantes. Deux ont été nommées : le mode paquets est
  **défini** par l'allocation avec notifications (sans notification, pas de paquet), et
  `DEVPKEY_KsAudio_PacketSize_Constraints2` est classée **obligatoire** par « Low Latency
  Audio ».
  *Fait quand* : une application faible latence obtient mieux que les 10 ms historiques sur un
  câble Conduit, en partagé.
  *Où* : sur l'interface `KSCATEGORY_AUDIO` de chaque filtre wave (`WaveRender<n>` /
  `WaveCapture<n>`), **avant** `PcRegisterSubdevice` ; non persistante (elle dépend de
  `BufferMs`) ; échec journalisé et **non fatal** ; relue par `--cable-transport`.
  `CONFIG_VERSION` **5 → 6**.
  *Mesuré le 2026-09-10*, session console, débogueur détaché — **l'entrée de mode est la seule
  variable qui compte** :

  | Contraintes déclarées | Ce que le moteur en fait |
  |---|---|
  | sans entrée de mode (`NumProcessingModeConstraints = 0`) | **ignorées** : min = max = défaut = 480 trames, alors que la propriété est bien posée (16 octets vérifiés dans le registre de la VM) |
  | avec une entrée `AUDIO_SIGNALPROCESSINGMODE_DEFAULT` **strictement au-dessus** du minimum (la doc l'exige : « higher… otherwise ignored ») | **lues** : fondamentale 1, minimum **96 trames** (2 ms), et la période demandée servie par `IAudioClient3::InitializeSharedAudioStream` |

  *La borne est une mesure, pas un raisonnement.* Tenue de la boucle locale par période
  moteur : 2 ms → **0/1** (195 sauts), 3 ms → 1/2, **4 ms → 4/4 puis 5/5**, 5 ms → 4/4,
  8 ms → 2/2, 10 ms → 20/20. Le minimum annoncé a donc été **remonté à 4 ms**
  (`MIN_PACKET_PERIOD_HNS = 40 000`, commit `85b56ac`) : **le pilote n'annonce que ce qu'il
  tient.** Mécanisme probable, non instrumenté : l'avance fixe de 2 ms de la boucle locale sur
  la position de rendu cesse d'être sûre quand la période du moteur descend à 2 ms.
  **Résultat net** : une application faible latence obtient **4 ms** de période sur un câble
  Conduit au lieu de 10, en partagé, aujourd'hui.
- [x] **M1b-15** `feat(pilote): faire allouer le moteur partagé avec notifications`
  **Clos le 2026-09-10 sans que le critère soit atteint — et c'est la réponse.**
  *Fait quand* (le critère d'origine) : en partagé, le tampon du pilote est alloué par
  `AllocateBufferWithNotification` — c'est la définition même du mode paquets. **Il ne l'est
  pas, et il n'y a plus de variable à essayer.**
  *Ce qui a été livré* : les modes de traitement déclarés **sur les broches** de flux
  (attribut `KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE` sur les plages et propriété
  `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES`, fusion `ed39ea5`), qui étaient la dernière
  variable non testée — voir [driver-design.md](docs/driver-design.md) §4.4.
  *Mesuré après la fusion* : le moteur partagé **scrute toujours** (`AllocateAudioBuffer`),
  modes déclarés **et** contraintes lues — le minimum annoncé est passé à **240 trames /
  5 ms**, la contrainte de mode gouvernant le minimum. Toutes les conditions déclaratives de
  SYSVAD sont désormais remplies : plages avec attribut, propriété sur la broche de flux et
  `Count = 0` sur le pont, `DEVPKEY_KsAudio_PacketSize_Constraints2` avec entrée de mode.
  **La scrutation en partagé est le comportement du moteur sur cette machine, pas un manque
  du pilote** ; aucune documentation ne décrit cette décision, et l'hypothèse « mode aware »
  est écartée.
  *Conséquence* : **rien à faire de plus côté pilote**. Le mode paquets sert les clients qui
  le demandent — les exclusifs événementiels (M1b-16) —, et le partagé prend le chemin
  cyclique, qui fonctionne.
- [x] **M1b-16** `feat(pilote): servir réellement les paquets WaveRT (lot 3)`
  Lot 3 (commit `d3c45ac` ; correctif d'avance `7603b7b` ; fusion des modes `ed39ea5` /
  `8a83094` ; **défaut à 1** dans `e2c06a9`). Les quatre méthodes **servent** au lieu de
  refuser : `SetWritePacket` valide le numéro et borne la copie, `GetReadPacket` rend le
  dernier paquet complet de la capture, `GetOutputStreamPresentationPosition` des trames
  absolues, `GetPacketCount` un compte base 1.
  *Fait quand* : les méthodes du mode paquets servent au lieu de refuser, et une passe
  `--exclusif` réussit avec `PacketMode = 1`. **Les deux sont vérifiés.**
  **Prérequis de tout `PacketMode = 1` livré**, et M1b-13 dit pourquoi : les clients exclusifs
  sont mesurés demandeurs (400 appels par seconde) et cassés par le refus.
  *Mesuré le 2026-09-10*, session console, débogueur détaché :

  | Ce qui est mesuré | Résultat |
  |---|---|
  | Exclusif servi (`PacketMode = 1`) | la passe qui **mourait au préremplissage** avec les interfaces refusées passe : ~1600 `SetWritePacket` en 20 s, ~3160 `GetReadPacket`, plus `GetPacketCount` et `GetOutputStreamPresentationPosition`, **tous servis, IRQL max 0** |
  | Correctif d'avance (`7603b7b`) | la mise à zéro des deux côtés donnait **8/10** avec sauts sub-trame ; la dissymétrie (source bornée par `committed`, cible qui garde l'avance) donne **9/10** |
  | **Contrôle décisif** : exclusif à `PacketMode = 0` | **8/10 aussi**, mêmes sauts sub-trame (~0,02–0,03 rad, zéro trou). Servi et non servi font **jeu égal** : les échecs résiduels sont le bruit du mode exclusif à 5 ms sous Hyper-V, **pas un défaut du mode paquets** |
  | Partagé avec `PacketMode = 1` | **20/20** — le moteur, qui scrute (M1b-15), n'est pas affecté |
  | **Driver Verifier `/standard`, 1 h 00, `PacketMode = 1`** | **382 tours** (moitié exclusifs, moitié classiques), **0 incident, 0 redémarrage, 0 vidage**, Verifier actif au début et à la fin ; **936 296 appels de paquets servis à `PASSIVE_LEVEL`** pendant que la DPC du minuteur tenait les mêmes verrous (305 763 `SetWritePacket`, 614 962 `GetReadPacket`, 581 `GetPacketCount`, 14 990 `GetOutputStreamPresentationPosition`). Verifier désarmé ensuite |

  **`PacketMode` est donc à 1 par défaut** depuis `e2c06a9` : c'était la condition écrite
  partout, la campagne l'a levée. 0 reste, et n'est plus qu'un **repli de diagnostic**.
  *Ce que ce lot n'a pas fait* : la borne des **4 ms** de M1b-14 n'est pas redescendue
  (`MIN_PACKET_PERIOD_HNS` vaut toujours 40 000 ; le minimum **annoncé** est même passé à
  240 trames / 5 ms depuis que la contrainte de mode gouverne, voir M1b-15). La copie sait
  désormais où le rendu en est par les paquets servis, mais seuls les clients exclusifs les
  empruntent — le partagé, que cette borne concerne, scrute. Reste donc à calculer l'avance
  sur la période effective au lieu d'une constante.

### M1b.B — Service d'assistance

- [x] **M1b-20** `feat(helper): service Windows minimal exposant la propriété KS au démon`
  Named pipe à interface fixe (activer, désactiver, canaux, lister), validation, journal.
  *Fait quand* : tests unitaires de validation ; le démon non-admin active un câble via le helper.
  *Mesuré le 2026-09-08*, dans la VM : service `ConduitHelper` installé en `LocalSystem`,
  et **`conduit-helper activer 3` depuis la session console non élevée réussit** — le
  câble passe à connecté et son endpoint apparaît. Le service existe pour une raison
  mesurée et non théorique : le pilote exige `SeLoadDriverPrivilege` **actif**, que le
  démon n'a pas.
  Le canal est le vrai sujet de sécurité — un service `LocalSystem` qui accepte des ordres
  par canal nommé est une élévation de privilège s'il est mal fermé. Descripteur explicite,
  jamais nul, **relu** après pose (le service refuse de démarrer s'il ne peut pas constater
  que la DACL n'est ni absente ni nulle) :
  `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100083;;;IU)`.
  `INTERACTIVE` reçoit un masque **numérique** et non `GRGW` : `GENERIC_WRITE` s'étend en
  `FILE_GENERIC_WRITE`, qui contient `FILE_APPEND_DATA` — sur un canal nommé, c'est
  `FILE_CREATE_PIPE_INSTANCE`, et l'accorder laisserait n'importe quel utilisateur
  connecté **squatter le nom du canal**. Ce détail a été trouvé par un test de bout en
  bout, qui a fait tomber la première version : elle créait une instance par client et le
  second `CreateNamedPipeW` rendait `ERROR_ACCESS_DENIED` — le descripteur se refusait à
  lui-même le droit qu'il refuse aux autres. Le serveur sert désormais une seule instance
  en recouvrement.
  Chaque ordre modifiant l'état journalise **l'identité de l'appelant** (nom, SID, pid) :
  on doit pouvoir dire qui a activé un câble.
  *Conséquence à ne pas oublier* : le client doit porter le SID `INTERACTIVE`. Une tâche
  planifiée « exécuter même si l'utilisateur n'est pas connecté » ouvre une session `BATCH`
  et se verrait refuser le canal — ADR-013 choisit bien « à l'ouverture de session ».
- [x] **M1b-21** `feat(helper): renommage d'endpoint via le registre`
  *Fait quand* : renommage visible dans les réglages Son après rafraîchissement (F-02).
  Première application du principe « pilote le plus léger possible »
  ([driver-design.md](docs/driver-design.md) §1) : **pas une ligne de pilote**, tout se
  fait depuis le service.
  *Mesuré le 2026-09-08*, dans la VM : `conduitctl cable rename 1 Musique` et
  `cable add --name Studio` depuis une session **non élevée**, et le nom apparaît
  **immédiatement** — aucun rafraîchissement n'a été nécessaire, contrairement à ce que le
  critère laissait craindre. Retour au nom d'origine et **zéro marque résiduelle** : la
  désinstallation ne laisse rien (F-52).
  *Ce que la documentation a tranché* : il n'existe **pas** de valeur « nom personnalisé »
  séparée — renommer écrase `PKEY_Device_DeviceDesc` lui-même
  (`{a45c254e-df1c-4efd-8020-67d146a850e0},2`). `PKEY_Device_FriendlyName` est `,14` du
  **même** fmtid et n'est pas rangé mais composé à la volée ; le `{b3f8fa53-…},6` qu'on
  pourrait croire équivalent est un espace privé du moteur audio dont la valeur est
  **partagée par plusieurs endpoints** — l'écrire en renommerait plusieurs d'un coup.
  *Le défaut que ça crée, et son remède* : la description est **aussi** le pont
  câble ↔ endpoints. Après renommage, le dorsal ne rattachait plus rien — mesuré :
  `cable list` rendait `conduit:cable1:rendu` au lieu des identifiants MMDevice, et les
  règles d'auto-connexion filtrant sur `cable = N` auraient cessé de fonctionner dès qu'un
  utilisateur renomme. Le service écrit donc une **marque** à nous dans la même clé,
  `{3f1b27a4-…},1 = "Conduit N"` (le GUID de `KSPROPSETID_CONDUIT`), et
  `cable_id_from_endpoint` la lit **avant** la description. Ordre d'écriture pensé pour
  qu'une interruption laisse toujours un endpoint rattachable : marque puis description à
  l'aller, description puis suppression au retour.
  *Question ouverte* : deux câbles peuvent porter le même nom personnalisé (renommer vers
  le nom **canonique** d'un autre câble est refusé, mais pas vers un même nom libre). À
  trancher si l'interface le rend gênant.

### M1b.C — Backend WASAPI

- [x] **M1b-30** `feat(wasapi): énumération MMDevice et notifications`
  *Fait quand* : branchement d'un casque USB → `DeviceEvent::Added` (F-10, F-20).
  Fait : fil MMDevice dédié (COM MTA, `IMMDeviceEnumerator`, `IMMNotificationClient`
  Rust), `DeviceInfo` complet (nom, sens, format du moteur, fréquences acceptées en
  mode partagé, période, défaut `eConsole`, câble par le nom), `Added` / `Removed` /
  `DefaultChanged`, tests d'intégration sur les cartes son de la machine, exemple
  `list --watch`. Reste à faire à la main : le critère casque USB est le test
  `#[ignore]` `hotplug_produces_added_then_removed` (brancher puis débrancher pendant
  30 s), non exécuté par l'agent. Tranché en M1b-31 : `DeviceInfo` = format de
  mixage (`GetMixFormat`, repli `PKEY_AudioEngine_DeviceFormat`) et `sample_rates` =
  44,1/48/96 kHz, toutes acceptées en partagé par conversion automatique.
- [ ] **M1b-31** `feat(wasapi): lecture et capture en mode partagé, événementiel`
  *Fait quand* : test de boucle à travers Conduit 1 via l'engine, xruns = 0 sur 10 min.
  *État* : flux partagés livrés et testés sur cartes réelles ; la boucle via Conduit 1
  attend le pilote (M1a). Fait : format demandé honoré (`IAudioClient3` au format de
  mixage avec période choisie, sinon `Initialize` + `AUTOCONVERTPCM`), un fil temps
  réel par flux (rendu direct dans le tampon WASAPI, capture par paquets, sans
  allocation ni verrou — `tests/no_alloc.rs`), déconnexion propre sur
  `AUDCLNT_E_DEVICE_INVALIDATED`, `ClockInfo` par atomiques, `latency()` hors trait.
- [x] **M1b-32** `feat(wasapi): mode exclusif quand disponible`
  *Fait quand* : latence mesurée inférieure au mode partagé, repli automatique documenté.
  *Mesuré* le 2026-09-06 sur les 5 endpoints de rendu du poste (`tests/exclusive.rs`, 48 kHz
  stéréo) : tampon et période **144 trames = 3 ms** en exclusif contre **1 056 trames = 22 ms**
  de tampon (période 480 = 10 ms) en partagé, soit sept fois moins, sur chacune des cinq.
  Aucune n'accepte le float32 en exclusif : Realtek Digital Output (S/PDIF), G27QC A et E2351
  (HDMI NVIDIA) prennent du PCM 24 dans un conteneur 32, l'Audeze Maxwell (USB, Chat et Game)
  du PCM 24 compacté ; la conversion est donc à notre charge (module `convert`, sans allocation —
  `tests/no_alloc.rs` couvre le cas). Politique opt-in `ExclusivePolicy` sur le backend
  (`Never` par défaut, `Preferred` avec repli et raison conservée, `Required` avec
  `UnsupportedFormat` explicite), gestion d'`AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` (reprise unique
  avec recréation du client — **non déclenchée** par ce matériel, couverte par des tests
  unitaires seulement), tampon entier par réveil (`GetCurrentPadding` rend toujours le tampon
  plein en exclusif événementiel), `IAudioClock` en trames/s sur ce chemin. Reste à vérifier sur
  une carte **analogique** : le réalignement du tampon et le repli PCM 16.
- [x] **M1b-33** `feat(wasapi): position d'horloge IAudioClock et intégration DLL`
  *Fait quand* : deux cartes réelles en même temps, dérive absorbée, xruns = 0 sur 1 h.
  *Vérifié* le 2026-09-05 : 1 h avec le moteur, G27QC A pilote et Realtek asynchrone, 720 006 cycles,
  0 xrun, DLL verrouillée (dérive relative −18 ppm absorbée), `write_ahead` ≈ 20 ms.
  `ClockInfo` : position `IAudioClock::GetPosition` en trames du format livré
  (fréquence classée : octets/s du mixage sur les deux cartes du poste, octets/s du
  format client par le chemin conversion), horodatage QPC commun aux flux ; latence
  estimée `write_ahead_frames` ; `clock_now()` ; `tests/two_devices.rs` (60 s : base
  commune à 43 µs, dérive Realtek − G27QC = −18 ppm ; `#[ignore]` 1 h avec le moteur).
- [x] **M1b-34** `feat(wasapi): CableControl via le helper`
  *Fait quand* : `conduitctl cable add` fonctionne de bout en bout (F-01, F-03).
  *Mesuré le 2026-09-08*, dans la VM, démon et `conduitctl` en session **non élevée** :
  `cable add` rend `câble 5 « Conduit 5 » stéréo : rendu {0.0.0.…}, capture {0.0.1.…}` —
  de vrais identifiants MMDevice, donc l'endpoint existe vraiment ; `cable list` le montre
  actif ; `cable remove 5` le repasse inactif. Le démon journalise au démarrage
  « service d'assistance Conduit présent : les câbles sont pilotables ».
  `cable add --name Musique` **refuse** en nommant M1b-21, plutôt que d'ignorer le nom en
  silence ou de paniquer.
  *Décisions prises et écrites* : tout passe par le service, **y compris `list`** — une
  réponse porte les seize câbles d'un coup là où une lecture directe ouvrirait seize
  filtres, et une source unique empêche un `list` de contredire l'`add` qui précède ; sans
  service, `list` rend une erreur qui nomme `conduit-helper installer`, jamais une liste
  vide. `create` sur un câble déjà actif rend l'état existant sans erreur, parce que
  `ensure_cables` rejoue la configuration à chaque démarrage.
  *Contrainte de dépendances* : `conduit-helper` dépend du dorsal WASAPI pour le transport
  KS, donc le dorsal ne peut pas dépendre du helper. Le `CableControl` vit dans
  `conduit-helper::controle`, et `conduitd` — seul à voir les deux — l'injecte.
- [x] **M1b-35** `feat(daemon): démarrage à l'ouverture de session et instance unique`
  Tâche planifiée par utilisateur (ADR-013), détection d'instance unique par le named pipe,
  arrêt propre à la fermeture de session. **Pas** de service SCM pour le démon.
  *Fait quand* : le démon démarre à l'ouverture de session, le socket est accessible.
  **Instance unique** : `single_instance::check` interroge le point de contrôle avant
  d'ouvrir le backend (Unix : socket orphelin supprimé ; Windows : ouverture du pipe,
  `ERROR_PIPE_BUSY`/`ERROR_ACCESS_DENIED` = pris) ; un démon de trop sort en **3** avec un
  message qui dit quoi faire, la course entre deux démarrages simultanés étant rattrapée
  par `AddrInUse` sur `bind`. **Fin de session Windows** : fenêtre cachée de **premier
  niveau** sur un fil dédié (`session_end`) — ni `SetConsoleCtrlHandler`
  (`CTRL_LOGOFF_EVENT` « received only by services ») ni `HWND_MESSAGE` (hors des messages
  diffusés) ne conviennent ; `WM_ENDSESSION` attend l'arrêt propre, borné à 3 s. Vérifié
  bout en bout sur une tâche jetable, y compris sur le processus **sans console** lancé
  par le Planificateur. **Autodémarrage** : `conduitd autostart <enable|disable|status>`,
  `schtasks.exe` + XML UTF-16 (`autostart::xml`), `--task-name` caché pour les tests.
  *État* : `--replace` **non implémenté** — le protocole n'a pas de commande d'arrêt
  (`Command` s'arrête à `Load`), donc rien ne permet de demander au démon en place de
  partir ; l'ajouter est une version de protocole (ADR-010) avec sa question de sécurité,
  à trancher séparément. La suppression du **dossier** vide `Conduit` du Planificateur
  (que `schtasks` ne sait pas faire) reste à traiter par la désinstallation, M1b-40.

### M1b.D — Empaquetage et validation

- [ ] **M1b-40** `feat(packaging): MSI WiX (pilote, helper, démon, CLI)`
  WiX épinglé dans `setup-env.ps1`.
  *Fait quand* : installation et désinstallation propres sur VM neuve (F-50, F-52).
- [ ] **M1b-41** `ci: signature EV et attestation Hardware Dev Center, pipeline de release`
  *Fait quand* : le MSI s'installe sans mode test sur Windows 10 et 11.
- [ ] **M1b-42** `test: banc Windows auto-hébergé (boucle, latence)`
  *Fait quand* : mesures publiées et conformes à SPEC §5.6.
- [ ] **M1b-43** `test: endurance 24 h Windows`
  *Fait quand* : 0 xrun, 0 fuite, câbles créés/supprimés aléatoirement.
- [ ] **M1b-44** `docs: guide utilisateur Windows (installation, dépannage)`
  Ébauche commencée en M1b-35 : `docs/user/windows.md` (démarrage automatique, instance
  unique, fermeture de session). Restent l'installation par le MSI et le dépannage.
- [ ] **M1b-45** `chore: tag v0.2.0-windows`

---

## M2 — GUI

Objectif : un utilisateur cible fait un routage complet sans documentation. Développée et
testée d'abord avec le backend null, puis sur Windows.

- [x] **M2-01** `feat(gui): squelette iced, client IPC asynchrone, reconnexion`
  *Fait quand* : la fenêtre affiche l'état de connexion et se reconnecte après redémarrage du démon.
- [x] **M2-02** `feat(gui): état miroir du démon alimenté par les événements`
  *Fait quand* : tests de réduction d'état pour chaque événement.
- [x] **M2-03** `feat(gui): vue Câbles (liste, ajouter, supprimer, renommer, canaux)`
  *Fait quand* : chaque action passe par l'IPC et se reflète dans l'OS (F-01, F-02, F-03).
- [x] **M2-04** `feat(gui): patchbay, rendu des nœuds et ports`
  Canvas `iced`, disposition automatique, positions mémorisées.
- [x] **M2-05** `feat(gui): patchbay, liens par glisser-déposer et suppression`
  *Fait quand* : lien créé/supprimé via IPC ; cycle refusé avec message (F-11, F-12).
- [x] **M2-06** `feat(gui): gains par lien et par nœud, muet`
  Pied de carte (glissière −60 à +12 dB, libellé, bouton « M ») ; le gain de lien est dans
  l'en-tête, faute de place sur une courbe. Commande envoyée au relâchement, butée basse
  au silence (F-13).
- [ ] **M2-07** `feat(gui): VU-mètres temps réel`
  *Fait quand* : rafraîchissement 30 Hz sans charge CPU notable.
- [x] **M2-08** `feat(gui): vue Diagnostic (xruns, latence, pilote, export de rapport)`
  Quatre tuiles (xruns, latence estimée, temps de cycle, charge d'un cœur), tableau des
  périphériques et colonne « Moteur » où se choisit le pilote de graphe. Le quantum et la
  fréquence y sont en lecture seule, faute de commande au protocole ; la latence par nœud et
  la capacité de tampon n'existant pas non plus, la vue le dit plutôt que de les inventer.
  L'export reprend le rapport texte du démon (`Command::Dump`) et l'écrit dans Documents.
- [ ] **M2-09** `feat(gui): icône de zone de notification et menu rapide`
  *Fait quand* : état OK / xruns / pilote absent visible, ouverture de la fenêtre au clic.
- [x] **M2-10** `feat(gui): démarrage du démon si absent`
  Le binaire `conduitd` est cherché à côté de l'exécutable courant (disposition du MSI
  comme d'un `cargo build`), à défaut dans le `PATH` ; il est lancé détaché, sans console
  sous Windows, avec le `--socket` que la GUI surveille. La GUI ne l'attend pas : la boucle
  de reconnexion voit le démon apparaître. Le bouton « Démarrer le démon » passe en
  « Démarrage… » jusqu'à la connexion (F-51).
- [x] **M2-11a** `feat(gui): thème clair/sombre`
  Design system « Sericæ » : jetons, polices embarquées, styles de widgets, formatage français.
  *Fait quand* : la fenêtre suit le mode du système, contraste WCAG AA vérifié par test
  (`docs/design-system.md`, ADR-014).
- [ ] **M2-11b** `feat(gui): traductions fr et en (fluent)`
  La table `i18n.rs` devient un catalogue `fluent` ; la langue suit celle du système.
- [ ] **M2-12** `feat(gui): repli logiciel tiny-skia`
  *Fait quand* : fonctionne dans une VM sans GPU.
- [ ] **M2-12b** `chore(nix): package conduit-gui avec wrapper des bibliothèques graphiques`
  Wayland, X11, `libxkbcommon`, Vulkan, OpenGL fournis par `RPATH`/wrapper.
  *Fait quand* : `nix run .#conduit-gui` démarre sur NixOS et sur une distribution classique
  avec Nix installé.
- [x] **M2-13** `feat(gui): écrans d'état et messages d'erreur orientés action`
  Chacun des huit `ErrorCode` a son conseil, ajouté après le message du démon, qui reste
  affiché tel quel et en premier (ADR-006) ; les erreurs locales de la fenêtre — refus de
  boucle, démon non lancé, rapport non écrit — prennent le même format. Deux écrans d'état
  remplacent la vue : « démon absent » quand le démon n'a jamais répondu (le texte suit la
  plateforme : boucle locale sous Windows et macOS, câbles disparus sous Linux, où F-05
  n'est pas garanti), et l'accueil de premier lancement, affiché une seule fois.
- [ ] **M2-14** `test(gui): tests hors écran et captures de référence`
- [ ] **M2-15** `feat(packaging): GUI dans le MSI, raccourci, lancement à la session`
- [ ] **M2-16** `test: session de test utilisateur`
  Trois personnes du public cible réalisent : créer un câble, router un lecteur vers Discord
  et le casque, sans documentation.
  *Fait quand* : 3/3 réussissent ; les blocages observés deviennent des tâches `fix(gui)`.
- [ ] **M2-17** `chore: tag v0.3.0-gui`

---

## M3 — Linux

Objectif : câbles et routage fonctionnels sur PipeWire, endurance 24 h.

- [x] **M3-01** `feat(pipewire): connexion au démon PipeWire, suivi du registre`
  *Fait quand* : `DeviceEvent` émis à l'ajout/retrait d'un nœud PipeWire (F-10, F-20).
  Fait : fil de boucle dédié, `Added` / `Removed` / `DefaultChanged`, test d'intégration
  sur un démon headless. Réserve : le registre ne publie pas `audio.channels`, les
  canaux retombent sur `audio.position` puis sur 2 ; la fréquence (48 kHz) et le
  quantum (256) annoncés sont ceux du graphe, pas encore lus dans les métadonnées
  `settings`.
- [ ] **M3-02** `feat(pipewire): nœuds câbles (pw_stream Audio/Sink et Audio/Source)`
  *Fait quand* : Conduit 1 visible dans les réglages son ; test de boucle.
- [x] **M3-03** `feat(pipewire): entrée/sortie des périphériques réels via streams`
  Fait : `open()` crée un `pw_stream` f32 entrelacé ciblant le nœud, `start`/`stop`/
  `clock` et destruction propre, rappel `process` sans allocation ni verrou. Test :
  rendu sur le `Null-Sink` du banc headless (avec WirePlumber, sans lequel aucun flux
  client n'est relié). Reste hors périmètre : la latence n'est pas encore remontée à
  la DLL (M3-04).
- [ ] **M3-04** `feat(pipewire): horloge et intégration DLL`
  *Fait quand* : xruns = 0 sur 1 h avec deux cartes.
- [ ] **M3-05** `feat(pipewire): CableControl (création, destruction, noms)`
  *Fait quand* : `conduitctl cable add` → nœud PipeWire créé (F-01, F-06).
- [ ] **M3-06** `feat(daemon): unité systemd --user, socket dans XDG_RUNTIME_DIR`
- [ ] **M3-07** `feat(pipewire): configuration PipeWire persistante optionnelle pour les câbles`
  Génère un drop-in `context.objects` pour que les câbles survivent à l'arrêt du démon.
  *Fait quand* : câble présent après `systemctl --user stop conduitd` (F-05 sur Linux).
- [ ] **M3-08** `feat(nix): module NixOS et module Home Manager`
  Service utilisateur systemd, options (câbles par défaut, quantum), test NixOS en VM.
  *Fait quand* : test `nixosTest` démarrant Conduit sur PipeWire et vérifiant un câble.
- [ ] **M3-09** `feat(packaging): paquets .deb et .rpm générés depuis les sorties Nix`
  Via `nfpm` dans un check Nix.
- [ ] **M3-10** `test: banc Linux (boucle, latence) et endurance 24 h`
- [ ] **M3-11** `docs: guide utilisateur Linux (paquets, flake, NixOS)`
- [ ] **M3-12** `chore: tag v0.4.0-linux`

---

## M4 — macOS

Objectif : câbles visibles dans Réglages Son, plugin signé et notarisé, F-05 vérifié.

### M4.A — Plugin HAL

- [ ] **M4-01** `feat(hal): squelette AudioServerPlugIn (cdylib, factory, bundle)`
  Package Nix `conduit-hal` produisant le bundle `.driver` depuis le `apple-sdk` de nixpkgs.
  *Fait quand* : `coreaudiod` charge le plugin sans erreur dans le journal ; `nix build .#conduit-hal` passe.
- [ ] **M4-02** `feat(hal): objets Plugin, Device, Stream, Control et propriétés obligatoires`
  *Fait quand* : un périphérique Conduit 1 apparaît dans Réglages Son.
- [ ] **M4-03** `feat(hal): cycle d'entrée/sortie, tampon de boucle locale, horloge`
  *Fait quand* : test de boucle (sinus → capture) passe 10 fois.
- [ ] **M4-04** `feat(hal): création et destruction dynamiques de périphériques`
  *Fait quand* : ajout/retrait reflété dans Réglages Son en < 1 s (F-01).
- [ ] **M4-05** `feat(hal): propriétés personnalisées de configuration et persistance`
  *Fait quand* : les câbles sont recréés après redémarrage sans le démon (F-05).
- [ ] **M4-06** `feat(hal): formats 44,1/48/96 kHz float32, 1 à 8 canaux`
- [ ] **M4-07** `ci: signature et notarisation en aval de nix build, script d'installation avec redémarrage de coreaudiod`
  `codesign` et `notarytool` appelés hors Nix sur le bundle produit par Nix.
- [ ] **M4-08** `test(hal): 1000 chargements, leaks, stress 48 h`

### M4.B — Backend et empaquetage

- [ ] **M4-10** `feat(coreaudio): énumération et notifications`
- [ ] **M4-11** `feat(coreaudio): entrée/sortie, horloge et intégration DLL`
- [ ] **M4-12** `feat(coreaudio): CableControl via les propriétés du plugin`
- [ ] **M4-13** `feat(daemon): LaunchAgent`
- [ ] **M4-14** `feat(packaging): pkg (plugin, démon, CLI, GUI)`
- [ ] **M4-15** `test: banc macOS (Intel et Apple Silicon), endurance 24 h`
- [ ] **M4-16** `docs: guide utilisateur macOS`
- [ ] **M4-17** `chore: tag v0.5.0-macos`

---

## M5 — Produit

Objectif : installation en un clic sur les trois OS, documentation complète, v1.0.

- [ ] **M5-01** `feat(daemon): migration de configuration entre versions`
  *Fait quand* : test de migration depuis chaque tag précédent (F-53).
- [ ] **M5-02** `feat(gui): vérification de mise à jour (opt-in, sans télémétrie)`
- [ ] **M5-03** `docs: site de documentation utilisateur (mdBook), illustré`
- [ ] **M5-04** `docs: protocole v1 gelé et politique de compatibilité`
- [ ] **M5-05** `ci: pipeline de publication (tag → changelog → artefacts signés 3 OS)`
  Linux et macOS depuis `nix build` ; Windows depuis le workflow scripté ; hachages publiés.
- [ ] **M5-05b** `ci: job hebdomadaire de double build et comparaison des hachages Linux`
  *Fait quand* : les packages Linux sont reproductibles au bit près.
- [ ] **M5-06** `test: campagne de test finale sur matériel varié`
  Cartes USB, Bluetooth, HDMI, cartes ASIO/pro, casques avec micro.
- [ ] **M5-07** `chore: audit de sécurité et des dépendances avant release`
- [ ] **M5-08** `chore: tag v1.0.0`

---

## Après v1 (à étudier, non planifié)

- Pilote mince + mémoire partagée (latence).
- Pilote de bus Windows pour un nombre illimité de câbles.
- Filtres DSP, MIDI, audio réseau.
- Fallback ALSA `snd-aloop` pour Linux sans PipeWire.
- Export Prometheus.
