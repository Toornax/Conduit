# ADR-003 — Pilote Windows en Rust avec spike et porte de décision

**Statut** : acceptée (2026-09-05). Formalise SPEC §11 D-03.

## Contexte

Un pilote PortCls/WaveRT n'a pas de précédent public en Rust : les vtables COM
doivent être écrites à la main (`portcls-sys`). Le principe « tout en Rust » (SPEC
§1.4) s'oppose au risque calendaire.

## Décision

Le pilote est développé **en Rust** (`windows-drivers-rs`) dès le départ, dans un
spike borné (M1a) avec une porte de décision explicite (M1a-12). En cas d'échec dans
le délai, le repli est un pilote C++ dérivé de SYSVAD, sans changement du reste.

## Conséquences

- Le spike démarre en parallèle de M0.
- La décision de repli, si elle survient, fait l'objet d'une ADR (ADR-0xx) ; elle
  n'est jamais implicite.
