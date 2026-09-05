# ADR-010 — `conduit-protocol` source de vérité de l'API, consommé par l'engine

**Statut** : acceptée (2026-09-05). Précise ROADMAP M0-70 (« miroir des commandes
de l'engine »).

## Contexte

La roadmap prévoyait des types de protocole « miroir » de ceux de l'engine, donc
deux définitions à maintenir en parallèle et des conversions. La CLI et la GUI ne
doivent pas dépendre de l'engine (lourd, avec backends).

## Décision

Les types de l'API (`Command`, `Reply`, `Notification`, descripteurs, `NodeKey`,
codes d'erreur) vivent dans `conduit-protocol`, qui ne dépend que de `conduit-core`
et `conduit-backend` (types sérialisables). `conduit-engine` **dépend du protocole**
et implémente `Command` directement. Chaîne : core ← backend ← protocol ← engine ←
démon ; CLI et GUI ← protocol.

## Conséquences

- Une seule définition de chaque commande ; le schéma JSON et `docs/protocol.md`
  sont générés depuis ces types et vérifiés en CI.
- Les erreurs de l'engine sont converties en `ProtocolError` (code stable + message
  destiné à l'utilisateur) dans l'engine lui-même.
- La règle de dépendance de SPEC §7 (« engine dépend de core et backend ») est
  étendue à protocol, qui ne contient aucun code de plateforme.
