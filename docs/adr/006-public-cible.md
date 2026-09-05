# ADR-006 — Public : utilisateur final un peu technique

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-06 et §1.2.

## Contexte

Le projet pourrait viser les développeurs audio (API, flexibilité maximale) ou les
utilisateurs finaux (simplicité). Les deux publics imposent des choix opposés.

## Décision

Le public est l'**utilisateur final un peu technique** : streamer, personne en
visioconférence, musicien amateur.

## Conséquences

- Ça marche à l'installation : deux câbles par défaut, réglages automatiques.
- Les messages d'erreur disent quoi faire, pas seulement ce qui a échoué (règle
  appliquée dans `CableError`, `EngineError`, `ConfigError`, la CLI).
- La CLI et le fichier de configuration sont des outils avancés, jamais obligatoires.
