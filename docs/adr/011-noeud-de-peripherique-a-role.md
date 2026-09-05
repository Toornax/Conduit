# ADR-011 — Un nœud par périphérique, rôle changé à chaud sans recompiler

**Statut** : acceptée (2026-09-05).

## Contexte

Un périphérique peut être absent (F-20), asynchrone ou pilote du graphe (F-21), et
passer d'un état à l'autre sans que l'utilisateur perde ses liens ni que les
identifiants changent. Recréer le nœud à chaque changement casserait les
identifiants et imposerait de re-tisser les liens.

## Décision

Chaque périphérique a **un seul `DeviceNode`** dans le graphe, dont le **rôle**
(`Suspended`, `AsyncCapture/Render`, `DriverCapture/Render`) est remplacé par une
boîte aux lettres SPSC lue au début de `process`. Le graphe n'est pas recompilé ;
l'ancien rôle (port asynchrone, tampons) est renvoyé au fil de gestion pour
libération hors temps réel. L'exécuteur est partagé (`Arc<Mutex<Executor>>`) entre
le rappel du pilote et l'horloge interne, avec `try_lock` uniquement (jamais
bloquant sur le fil audio).

## Conséquences

- `NodeId` et liens stables à travers débranchement, réapparition et changement de
  pilote (vérifié par les scénarios engine).
- Un nœud dont la disposition change (nombre de canaux) est recréé : ses liens sont
  perdus, ce qui est documenté.
- Un périphérique ne peut être retiré du graphe que s'il est absent
  (`EngineError::DevicePresent`).
