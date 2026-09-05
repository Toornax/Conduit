# ADR-001 — Nom : Conduit

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-01.

## Contexte

Le projet a besoin d'un nom court, prononçable en français et en anglais, évoquant
le transport de l'audio d'un point à un autre, et libre pour les binaires et les
crates.

## Décision

Le projet s'appelle **Conduit**. Binaires : `conduitd` (démon), `conduitctl` (CLI),
`conduit` (GUI). Crates préfixés `conduit-`. Les périphériques virtuels apparaissent
dans l'OS sous le nom `Conduit 1`, `Conduit 2`, …

## Conséquences

- La vérification de disponibilité du nom sur crates.io et parmi les marques audio
  reste à faire avant toute publication (SPEC §11, question ouverte 4).
- Le préfixe des sockets, pipes et répertoires de configuration est `conduit`.
