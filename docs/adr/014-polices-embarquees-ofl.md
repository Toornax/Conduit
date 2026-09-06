# ADR-014 — Polices embarquées sous OFL-1.1 dans le binaire de la GUI

**Statut** : acceptée (2026-09-06). Complète l'ADR-005 et le design system
[Sericæ](../design-system.md).

## Contexte

Le thème « Sericæ » repose sur trois familles — Fraunces, Spectral, Inter — et
ne peut pas s'en remettre aux polices du système : les trois OS n'en ont aucune
en commun, et la GUI doit rendre le même dessin partout (SPEC §5.8). `iced`
charge une police par `Application::font(&'static [u8])` : les fichiers doivent
donc être **embarqués dans le binaire** par `include_bytes!`.

Ces trois familles sont publiées sous **SIL Open Font License 1.1** (OFL-1.1).
Or SPEC.md:502 exige « MIT OR Apache-2.0 (pilotes inclus). Aucune dépendance
copyleft. » L'OFL comporte une clause de réciprocité — toute version modifiée
d'une police reste sous OFL — et une clause de nom réservé.

## Décision

Les quatre fichiers sont embarqués dans `conduit-gui`, sous
`crates/conduit-gui/assets/fonts/`, avec le texte de leur licence à côté :

| Fichier | Famille | Taille |
|---|---|---|
| `Fraunces[SOFT,WONK,opsz,wght].ttf` | Fraunces (variable) | 352 Kio |
| `Inter[opsz,wght].ttf` | Inter (variable) | 856 Kio |
| `Spectral-Regular.ttf` | Spectral 400 | 255 Kio |
| `Spectral-Light.ttf` | Spectral 300 | 263 Kio |

L'exigence de SPEC.md:502 porte sur les **dépendances de code** : ce qui est
lié dans le binaire et pourrait imposer sa licence à Conduit. L'OFL-1.1 porte
sur des **fichiers de données embarqués**, pas sur une dépendance Cargo :

- sa réciprocité ne s'étend qu'aux polices dérivées, jamais au logiciel qui les
  affiche ou les distribue (clause 5 : « This Font Software … does not affect
  … any document created using the Font Software ») ;
- elle n'atteint donc pas la licence du binaire, qui reste MIT OR Apache-2.0 ;
- elle n'est pas vue par `cargo-deny`, qui n'inspecte que le graphe de crates.

La règle de SPEC.md:502 est donc respectée en substance, et cette ADR en note
l'écart de forme.

## Conséquences

- **~1,8 Mo** ajoutés au binaire `conduit` (1 767 416 octets de `.ttf`). C'est
  le prix d'une interface identique sur les trois OS ; il est payé une fois, à
  la compilation, sans coût à l'exécution.
- Les fichiers `OFL-fraunces.txt`, `OFL-inter.txt` et `OFL-spectral.txt`
  **accompagnent la distribution** : ils sont livrés dans le MSI et dans les
  archives, et leur contenu est repris dans la mention des tiers.
- Le filtre de sources Nix (`nix/packages.nix`) inclut
  `crates/conduit-gui/assets/` : sans cela, `include_bytes!` échouerait dans le
  bac à sable. Un test (`typo::tests::les_quatre_polices_sont_embarquees`)
  garde cet accès.
- **Tout sous-ensemblage ultérieur** — ne garder que les glyphes latins pour
  réduire la taille — produit une police dérivée. Il devra donc rester sous
  OFL-1.1 et **respecter la clause de nom réservé** : le fichier produit ne
  pourra plus s'appeler « Fraunces », « Inter » ni « Spectral ». Cette
  optimisation n'est pas faite aujourd'hui.
- Ajouter une famille supplémentaire demande une décision : trois voix
  suffisent au design system.
