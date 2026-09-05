# ADR-002 — Ordre des plateformes : Windows, Linux, macOS

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-02.

## Contexte

Le public cible (streamers, visioconférence) est majoritairement sous Windows, où
l'absence d'un routeur audio libre et fiable est la plus criante. Linux dispose déjà
de PipeWire ; macOS a BlackHole/Loopback. Le pilote Windows est aussi le risque
technique principal.

## Décision

Livrer dans l'ordre **Windows** (M1), **Linux** (M3), **macOS** (M4), la GUI (M2)
s'intercalant après Windows pour former la première version utilisable.

## Conséquences

- Le cœur portable (M0) est validé sans matériel sur les trois OS dès le départ.
- Si le spike du pilote Windows bloque (ADR-003), le travail bascule sur les parties
  Linux et macOS sans attendre.
