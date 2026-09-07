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
- [ ] **M1b-04** `feat(driver): propriété KS privée de configuration (activer, désactiver, canaux)`
  Jeu de propriétés KS privé `KSPROPSETID_Conduit` exposé par le filtre de topologie de
  chaque câble, et non un objet de périphérique de contrôle séparé
  ([driver-design.md](docs/driver-design.md) §6, décision anticipée) : PortCls fait le
  routage, l'accès passe par les handles standard, et la validation reste un parseur sur un
  tampon borné, fuzzable en mode utilisateur (M1b-08). Écriture soumise à
  `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)`, bascule du jack à chaud.
  *Fait quand* : activation → endpoint visible en < 1 s sans PnP (F-01) ; entrée invalide → `STATUS_INVALID_PARAMETER` sans effet.
- [ ] **M1b-05** `feat(driver): formats 44,1/48/96 kHz, float32, PCM16 et PCM24`
  *Fait quand* : test de boucle pour chaque format (F-04).
- [ ] **M1b-06** `feat(driver): gestion d'alimentation et arrêt propre`
  *Fait quand* : veille/reprise 50 fois avec flux ouvert, sans erreur ni fuite.
- [ ] **M1b-07** `feat(driver): comportement à un seul côté ouvert`
  *Fait quand* : capture seule → silence ; rendu seul → pas d'accumulation.
- [ ] **M1b-08** `test(driver): harnais utilisateur et fuzzing du parseur de la propriété KS`
  Le code de validation compile aussi en mode utilisateur pour être fuzzé.
  *Fait quand* : 1 h de fuzzing sans panique.
- [ ] **M1b-09** `test(driver): 1000 cycles activation/désactivation et 48 h de stress`
  *Fait quand* : aucune fuite (pool tags stables), aucun BSOD, Driver Verifier actif.
- [ ] **M1b-10** `test(driver): passage des tests HLK audio`
  *Fait quand* : rapport HLK sans échec bloquant.

### M1b.B — Service d'assistance

- [ ] **M1b-20** `feat(helper): service Windows minimal exposant la propriété KS au démon`
  Named pipe à interface fixe (activer, désactiver, canaux, lister), validation, journal.
  *Fait quand* : tests unitaires de validation ; le démon non-admin active un câble via le helper.
- [ ] **M1b-21** `feat(helper): renommage d'endpoint via le registre`
  *Fait quand* : renommage visible dans les réglages Son après rafraîchissement (F-02).

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
- [ ] **M1b-34** `feat(wasapi): CableControl via le helper`
  *Fait quand* : `conduitctl cable add` fonctionne de bout en bout (F-01, F-03).
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
