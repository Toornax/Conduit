# Décisions d'architecture (ADR)

Une décision structurante = un fichier numéroté, jamais modifié après acceptation
(une décision qui change donne une nouvelle ADR qui remplace l'ancienne).

Gabarit : contexte, décision, conséquences, statut.

| N° | Titre | Statut |
|---|---|---|
| [ADR-001](001-nom-conduit.md) | Nom : Conduit | acceptée |
| [ADR-002](002-ordre-des-plateformes.md) | Ordre des plateformes : Windows, Linux, macOS | acceptée |
| [ADR-003](003-pilote-windows-en-rust.md) | Pilote Windows en Rust avec spike et porte de décision | acceptée |
| [ADR-004](004-cables-variables-reserve-windows.md) | Nombre de câbles variable à chaud, réserve fixe sur Windows | acceptée |
| [ADR-005](005-gui-iced.md) | GUI en `iced`, livrable de premier rang | acceptée |
| [ADR-006](006-public-cible.md) | Public : utilisateur final un peu technique | acceptée |
| [ADR-007](007-pilote-bete-demon-intelligent.md) | Pilote bête, démon intelligent | acceptée |
| [ADR-008](008-nix-environnement-de-reference.md) | Nix comme environnement de référence (Linux, macOS) | acceptée |
| [ADR-009](009-dsp-dans-le-coeur-des-m0.md) | Briques DSP (EQ, filtres) dans le cœur dès M0 | acceptée |
| [ADR-010](010-protocole-source-de-verite-api.md) | `conduit-protocol` source de vérité de l'API, consommé par l'engine | acceptée |
| [ADR-011](011-noeud-de-peripherique-a-role.md) | Un nœud par périphérique, rôle changé à chaud sans recompiler | acceptée |
| [ADR-012](012-workspace-noyau-separe.md) | Workspace noyau séparé, logique du pilote dans un crate portable | acceptée |
