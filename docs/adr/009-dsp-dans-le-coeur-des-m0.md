# ADR-009 — Briques DSP (EQ, filtres) dans le cœur dès M0

**Statut** : acceptée (2026-09-05). **Dévie de SPEC §1.3** (« traitements DSP
avancés en v2 »).

## Contexte

SPEC §1.3 réserve l'égalisation et les effets à une v2. Le projet a demandé un cœur
« adapté au calcul » capable de traitement de données (mixage, égalisation) dès le
départ, pour valider tôt que l'architecture de nœuds supporte du DSP sans
allocation ni verrou.

## Décision

`conduit-core` inclut dès M0 un module `dsp` (biquad RBJ, rééchantillonneur sinc,
DLL, RNG) et des nœuds utilitaires de traitement (`EqualizerNode` multi-bandes,
`MixerNode`, `ChannelAdapter`, `MeterNode`, générateurs) avec paramètres réglables
à chaud par atomiques. Ces nœuds sont disponibles via le protocole
(`add_internal`, `set_param`) et la CLI.

Le périmètre reste borné : pas de plugins tiers, pas de chargement dynamique, pas
d'effets temporels (réverbération, délai) ni de dynamique (compresseur) en v1.
Un système de filtres extensible reste un sujet v2.

## Conséquences

- Le contrat `Node` (pas d'allocation dans `process`, état persistant entre versions
  du graphe) est validé par des nœuds réels et le test `no_alloc`.
- SPEC §1.3 doit être amendé à la prochaine révision pour refléter ce périmètre.
