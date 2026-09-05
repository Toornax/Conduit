# ADR-005 — GUI en `iced`, livrable de premier rang

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-05.

## Contexte

Le public cible n'utilisera pas une CLI. La GUI doit être portable sur les trois OS,
en Rust, et ne contenir aucune logique audio.

## Décision

La GUI est écrite avec **`iced`**, comme simple client IPC du démon. Elle fait partie
de la première version utilisable (M2).

## Conséquences

- Tout ce que fait la GUI passe par le protocole (ADR-010) : pas de chemin privilégié.
- Version d'`iced` épinglée, mise à jour par jalon.
