# ADR-007 — Pilote bête, démon intelligent

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-07 et §3.1.

## Contexte

Deux architectures : (a) pilote minimal en boucle locale + démon qui s'attache aux
câbles via l'API audio standard ; (b) pilote mince relié au démon par mémoire
partagée (modèle PipeWire), plus performant mais liant la stabilité du son à celle
du démon.

## Décision

Architecture (a) : le pilote n'a aucune logique de routage ni de transport vers le
démon ; sa seule surface de contrôle est une configuration minimale validée. Le
démon fait tout le reste (graphe, mixage, rééchantillonnage, persistance, contrôle).

## Conséquences

- La fonction câble marche sans le démon (F-05).
- Un tampon de latence supplémentaire par traversée de câble ; l'architecture (b)
  est reportée en v2 si les mesures l'exigent.
- Le cœur (`conduit-core`, `conduit-engine`) est identique sur les trois OS ; seuls
  les backends changent.
