# ADR-004 — Nombre de câbles variable à chaud, réserve fixe sur Windows

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-04 et §5.4.

## Contexte

Les utilisateurs veulent créer et supprimer des câbles sans redémarrer. Sur macOS et
Linux, la création dynamique est naturelle. Sur Windows, la création dynamique
exigerait un pilote de bus et une danse PnP par câble.

## Décision

Les câbles sont **créés et supprimés à chaud** partout. Sur Windows, le pilote
enregistre une **réserve fixe de 16 câbles** dont les inactifs sont déclarés « jack
débranché » ; activer un câble = basculer le jack. Limite : 16 sur Windows, 32 sur
macOS et Linux.

## Conséquences

- L'interface `CableControl` expose une limite (`max_cables`) et une erreur
  `LimitReached` avec le remède.
- Le backend `null` simule le même modèle (numéro stable, recréation pour changer les
  canaux).
