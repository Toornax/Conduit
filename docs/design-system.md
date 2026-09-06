# Sericæ — design system de la GUI Conduit

Ce document décrit les jetons, les règles et les écarts assumés du thème de
`conduit-gui`. Le code qui l'implémente est dans
[`crates/conduit-gui/src/theme.rs`](../crates/conduit-gui/src/theme.rs),
[`typo.rs`](../crates/conduit-gui/src/typo.rs),
[`style.rs`](../crates/conduit-gui/src/style.rs) et
[`format.rs`](../crates/conduit-gui/src/format.rs). Les polices embarquées et
leur licence font l'objet de l'[ADR-014](adr/014-polices-embarquees-ofl.md).

Le nom dit l'intention : une matière de soie, un métier d'atelier. L'interface
est un objet de papier et d'encre, filé d'or.

## 1. Matières

Elles ne changent jamais ; seuls leurs rôles changent d'un mode à l'autre.

| Matière | Valeur | Emploi |
|---|---|---|
| encre | `#14120F` | fond du mode sombre, titres du mode clair |
| encre-2 | `#1E1B16` | surfaces posées du mode sombre |
| grège | `#EDE6D6` | fond du mode clair, titres et texte du mode sombre |
| grège-2 | `#E4DBC7` | surfaces posées du mode clair |
| or | `#B08D3E` | le fil : filets, pastilles, courbes, barres |
| garance | `#7A2E2B` | liens, focus, alerte |
| céladon | `#3E4A41` | accent des informations |
| sépia | `#3A342C` | texte courant du mode clair |
| voile d'or | `rgba(176,141,62,.35)` | rails, sélections, aplats |

## 2. Jetons

| Jeton | Clair | Sombre |
|---|---|---|
| `surface` | grège | encre |
| `surface_2` | grège-2 | encre-2 |
| `titre` | encre | grège |
| `texte` | sépia | grège |
| `texte_2` | `#6B6353` | `#9A8F76` |
| `contour` | `rgba(58,52,44,.25)` | `rgba(176,141,62,.25)` |
| `accent_texte` | garance | or |
| `graisse_texte` | 400 | 300 |

Invariants entre les deux modes : **le fil d'or**, **la garance** des liens et
du focus.

`texte_2` est **réservé à `surface`**. Sur `surface_2` en mode clair il tombe à
4,3:1, sous le seuil WCAG AA ; les cartes portent donc `texte`, pas `texte_2`.
Un test verrouille cette raison
(`theme::tests::le_texte_secondaire_est_reserve_a_la_surface`).

## 3. Échelles

- **Espacement** : 4, 8, 12, 16, 24, 32, 48, 64. Aucune autre valeur.
- **Rayon** : 8 px, partout, sauf les filets et les champs (0).
- **Cible tactile** : 44 px de haut au minimum ; boutons `padding` horizontal 28.
- **Corps** : 12, 14, 17, 24, 40, 68.
- **Interligne** : 1,6.
- **Capitales espacées** : +0,08 em.
- **Filet** : 1 px, jamais autre chose. Focus : filet garance 1 px, offset 3 px.

## 4. Règles

1. **Aucune ombre, aucun dégradé, aucune transparence de flou.** Les bordures
   sont des filets de 1 px. Toutes les `Shadow` sont `Shadow::default()` et
   tous les fonds des `Background::Color` — un test le vérifie.
2. **L'or ne fait jamais du texte sur une surface claire** : il ne passe que
   **2,5:1** sur le grège. Il reste aux filets, pastilles, courbes et barres.
   La seule exception est le libellé du bouton primaire en mode clair, posé sur
   l'encre, où il passe **6,0:1**.
3. **Une seule couleur d'accent visible par surface.**
4. **Trois familles, trois voix** : *Fraunces* parle (display, titres),
   *Spectral* raconte (texte courant), *Inter* renseigne (interface,
   métadonnées, chiffres). Une sérif ne descend jamais sous 13 px en interface :
   `typo::texte_a` repasse à Inter en dessous.
5. **Survol** : glissement de couleur vers l'or ou la garance, jamais d'échelle
   ni d'ombre. **Pression** : opacité 0,85. **Désactivé** : opacité 0,45.
6. **Contraste** : ≥ 4,5:1 pour le texte courant, ≥ 3:1 au-dessus de 24 px
   (`theme::seuil_contraste`). Tous les couples produits par `style.rs` sont
   parcourus par un test.

## 5. Boutons

Base commune : Inter 13 px, capitales espacées de +0,08 em, hauteur minimale
44 px, `padding` horizontal 28 px, rayon 8 px.

| Rôle | Clair | Sombre |
|---|---|---|
| primaire | fond encre, texte **or**, filet encre ; survol : texte grège | fond grège, texte encre, filet grège ; survol : texte garance |
| secondaire | sans fond, texte et filet sépia ; survol : garance | sans fond, texte et filet grège ; survol : **or** |
| lien | sans fond ni filet ni padding, texte garance souligné ; survol : encre | idem en **or** ; survol : grège |

Le survol du secondaire passe à l'or en mode sombre, et non à la garance :
la garance ne passe que **2,0:1** sur l'encre. Même raison pour le lien.

`désactivé` = opacité 0,45 ; `pressé` = opacité 0,85, appliquées au texte, au
filet et au fond.

## 6. Trois écarts de fidélité assumés

`iced` 0.14 rend le texte avec `cosmic-text`. Trois choses que le design
demande n'y sont pas accessibles ; voici ce qui les remplace.

### 6.1 `opsz` n'est pas pilotable

`cosmic-text` applique l'axe `wght` des polices variables — `Weight::Semibold`
sur Fraunces donne bien wght 600 — mais **aucun autre axe**. `opsz` reste à sa
valeur par défaut du fichier, et `SOFT`/`WONK` de Fraunces restent à 0 (ce qui
est exactement le dialecte « Boutique » recherché, donc sans regret).

*Remplacement* : la compensation optique des grands corps passe par la graisse.
`typo::display` allège le dessin quand le corps grandit — Bold sous 24 px,
Semibold à 24, Medium à partir de 40.

### 6.2 `tnum` est inaccessible

Aucune fonctionnalité OpenType n'est exposée : pas de chiffres tabulaires, donc
pas d'alignement des colonnes de nombres par la police.

*Remplacement* : **fixer la largeur des cellules numériques**.

```rust
text(format::millisecondes(latence)).width(72).align_x(Right)
```

C'est la seule manière d'aligner une colonne de chiffres ; la règle vaut pour
toute la vue Diagnostic (xruns, latence) et pour les gains.

### 6.3 Pas de `letter-spacing`

`iced` n'a pas d'interlettrage.

*Remplacement* : `typo::petites_capitales` compose une `Row` d'un `text` par
caractère, espacée de `taille × 0,08`, les coupures entre mots étant des
`space::horizontal().width(taille × 0,35)`. À réserver aux libellés courts —
onglets, boutons, sur-titres : un paragraphe composé ainsi ne se coupe pas en
fin de ligne.

## 7. Mise en forme des nombres

`format.rs` tient trois conventions françaises : **virgule décimale**, **espace
insécable** (U+00A0) avant l'unité et **espace insécable fine** (U+202F) entre
les groupes de milliers, **signe moins typographique** (U+2212). Les unités
elles-mêmes viennent de `i18n.rs` : aucune chaîne affichée n'est écrite
ailleurs.

La latence estimée d'un nœud vaut deux quanta : `2 × quantum / fréquence`.

## 8. Thème du système

Au démarrage, `iced::system::theme()` donne le mode ; `theme_changes()` le
suit. `Mode::None` — aucune préférence connue — retombe sur le mode clair.
`App::theme` renvoie toujours un thème, jamais `None`, qui ferait retomber
`iced` sur ses thèmes intégrés.

`iced::Theme` ne transporte que six couleurs (`Palette`) : bien trop peu pour ce
vocabulaire. Les deux thèmes construits ne servent qu'à la plomberie d'`iced`,
et les jetons sont retrouvés dans les closures de style par `theme::jetons`,
qui lit `extended_palette().is_dark` — le seul discriminant disponible dans une
closure qui ne reçoit qu'un `&Theme`.
