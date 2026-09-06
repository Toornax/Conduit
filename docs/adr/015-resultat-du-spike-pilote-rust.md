# ADR-015 — Résultat du spike : le pilote Windows reste en Rust

**Statut** : acceptée (2026-09-07). Clôt le spike ouvert par
[ADR-003](003-pilote-windows-en-rust.md) et répond à sa porte de décision (M1a-12).

## Contexte

[ADR-003](003-pilote-windows-en-rust.md) a engagé le pilote Windows en Rust malgré
l'absence de précédent public pour un pilote PortCls/WaveRT dans ce langage, et a borné
le risque par un spike avec porte de décision explicite : quatre questions, un délai, et
un repli C++ dérivé de SYSVAD si les réponses étaient mauvaises.

Cette ADR donne les réponses, mesurées sur la machine virtuelle `ConduitTest` les 6 et
7 septembre 2026, et tranche.

## Les quatre questions

### 1. `windows-drivers-rs` compile-t-il un WDM PortCls sans nightly ni correctif ?

**Oui.** Rust **stable** 1.96.1 MSVC, édition 2024, `wdk-sys` 0.5.1 et `wdk` 0.4.1, WDK
26100, aucun correctif appliqué à l'écosystème. La compilation passe sur le poste de
développement comme en intégration continue, où le job `pilote noyau (WDK)` installe le
kit puis enchaîne `check.ps1` et `build.ps1` à chaque commit.

Deux contraintes, ni l'une ni l'autre bloquante : LLVM doit être **épinglé à 17.0.6**
(les bindings de `wdk-sys` 0.5.1 ne se génèrent pas avec LLVM 22), et le CRT statique est
exigé par `wdk-build`. Les deux sont figées dans `packaging/windows/versions.json` et
vérifiées par `setup-env.ps1 -Check`.

### 2. Les vtables COM tiennent-elles, et PortCls accepte-t-il nos objets ?

**Oui, et pas seulement sur le papier.** La question portait sur deux risques distincts.

*La disposition mémoire* est vérifiée par un second compilateur : `regen-layout.ps1`
mesure avec `cl.exe` les tailles et décalages des structures du kit, et `check.ps1`
compare le résultat à un fichier de référence versionné (52 mesures). Les vtables sont
engendrées par bindgen en **mode C** — `DECLARE_INTERFACE_` avec `INTERFACE` défini à
`void` — ce qui produit des structures plates dont la disposition est celle de COM.

*L'exécution* est vérifiée dans la VM : `PcNewPort`, `IPort::Init` et
`PcRegisterSubdevice` réussissent pour les quatre sous-périphériques, les connexions
physiques entre broches bridge s'établissent, Windows construit deux endpoints qui portent
le nom « Conduit 1 », et **l'audio traverse le câble** — dix passes sur dix à 440,00 Hz,
amplitude 0,500 exactement, aucune rupture de phase, aucun trou.

Le pilote se charge et se décharge **cent fois de suite sans erreur**, deux séries
consécutives, sans aucun vidage.

### 3. Lecture et boucle locale stables une heure sous Driver Verifier ?

La campagne porte sur le pilote de `5edacc4`, c'est-à-dire **exactement le code du
pilote au moment de cette décision** : aucun commit n'a touché `drivers/windows` ni
`conduit-kmd-core` depuis la construction du paquet mis à l'épreuve. Valider une version
et en livrer une autre n'aurait rien prouvé.

Réglages : `verifier /standard`, qui comprend le **pool spécial**, la **vérification
d'IRQL forcée**, le suivi de pool, la vérification des E/S, la détection de blocage et la
conformité DDI. Le débogueur n'est **pas** attaché, délibérément : un écran bleu doit
produire un vidage exploitable, pas figer la machine dans `kd` sans surveillance.

Charge : **360 tours** en une heure, chacun ouvrant les deux flux du câble, les faisant
tourner huit secondes, puis les fermant — soit environ 48 minutes de transport effectif et
360 cycles d'ouverture et de fermeture, sous les deux verrous (câble puis flux) et le
minuteur haute résolution à 1 ms.

**Résultat : aucun incident.** Zéro écran bleu, zéro vidage — ni `MEMORY.DMP` ni
minidump —, **zéro événement `BugCheck` (identifiant 1001) dans tout le journal système**,
et Driver Verifier toujours actif à la fin de la campagne, ce qui atteste qu'elle a bien
été menée sous surveillance du début à la fin.

Deux événements « arrêt inattendu » (6008) figurent au journal, à 00:37:55 et 00:49:36 :
ce sont mes propres réinitialisations brutales de la machine pendant la mise au point,
antérieures au début de la charge à 00:52:53. Ils ne concernent pas la campagne.

**Réserve, à ne pas surinterpréter.** La campagne s'est déroulée dans la **session 0**, où
l'audio est muet : elle valide les *chemins de code* — allocation, marche, copie,
notification, libération, ordonnancement des verrous à `DISPATCH_LEVEL`, destruction du
minuteur — et non le *contenu sonore*, qui est validé séparément par les dix passes de
M1a-10 en session interactive. C'est bien ce que Driver Verifier surveille, mais la
distinction mérite d'être écrite.

### 4. Coût réel comparé au délai borné

Le délai proposé par la feuille de route était de **trois semaines de travail effectif**.

| Mesure | Valeur |
|---|---|
| Travail effectif | **2 jours** (2026-09-05 au 07) |
| Commits de périmètre pilote | 29 |
| Code noyau livré | ~9 200 lignes de Rust |
| Tests de ce code | ~4 200 lignes |

Le coût est donc très inférieur au délai borné, d'un ordre de grandeur. Deux réserves
d'honnêteté : ce chiffre porte sur du travail assisté et concentré, et une part
substantielle du temps réel n'a pas été passée à écrire le pilote mais à **se rendre
capable de le mesurer** — voir plus bas.

## Décision

**Le pilote Windows de Conduit reste en Rust.** Les quatre questions de la porte ouverte
par ADR-003 reçoivent une réponse favorable, mesurée et non supposée. Le repli C++ dérivé
de SYSVAD n'est pas exercé, et l'ADR de repli prévue par ADR-003 n'a pas lieu d'être.

M1b se poursuit sur la base livrée : `portcls-sys`, `portcls`, `conduit-kmd`,
`conduit-kmd-core` et `conduit-com`, sans réécriture.

## Ce que le spike a appris, au-delà de sa question

Trois enseignements qui valent d'être consignés, parce qu'ils ont coûté plus cher que le
code lui-même et qu'ils resserviront en M1b.

**L'environnement de mesure ment plus souvent que le pilote.** Une journée entière a été
passée à chercher un défaut de transport WaveRT qui n'existait pas : les mesures étaient
faites depuis la **session 0**, celle des services, qui n'a aucun audio utilisateur. Un
test y énumère les endpoints, ouvre les flux et reçoit des trames **à la bonne cadence**,
toutes silencieuses, sans le moindre signal d'erreur. Le mode session étendue de
`vmconnect` ajoutait un second piège en redirigeant l'audio vers l'hôte. Le pilote,
pendant ce temps, fonctionnait. La réponse est outillée : `vm-run-console.ps1` refuse de
mesurer s'il n'y a pas de session console interactive, plutôt que de rendre un résultat
silencieusement faux.

**Une observation unique n'est pas une mesure.** Deux conclusions de cette session ont été
tirées d'un seul essai réussi, écrites comme des faits, puis contredites par l'essai
suivant. Ce qui les a rattrapées à chaque fois est la comparaison contrôlée : même
machine, même séquence, une seule variable. C'est aussi ainsi qu'on a établi que le masque
de traces du composant `DPFLTR_DEFAULT_ID` est le seul armé au démarrage, contre ce que
laisse croire la documentation courante.

**Un échec intermittent n'est pas forcément dans le code qu'on suspecte.** Le fameux
`0xC00000E5` d'un chargement sur cent venait du **harnais de test**, qui supprimait le
paquet du pilote avant que le retrait du périphérique ne soit effectif ; PnP tentait alors
un dernier démarrage sur un devnode mourant. Le signe distinctif était dans le message
depuis le début : l'événement désigne l'appareil du cycle **précédent**.

## Conséquences

- **M1a est close.** Ses douze tâches sont cochées.
- **Le premier chantier de M1b.A est la transparence du câble** (M1b-03b) : faute de nœud
  `KSNODETYPE_VOLUME` dans notre topologie, Windows insère son APO logiciel et applique au
  signal le volume par défaut qu'il donne à tout endpoint neuf. Un câble virtuel qui
  atténue ce qu'il transporte manque sa raison d'être
  ([driver-design.md](../driver-design.md) §5.5).
- **La branche du mode paquets WaveRT** (`IMiniportWaveRTInputStream` /
  `OutputStream`, écrite pendant le spike mais délibérément non fusionnée pour que la
  campagne porte sur la version conservée) est fusionnée à l'ouverture de M1b, avec son
  propre passage en machine virtuelle.
- **L'outillage de validation fait désormais partie du produit** au même titre que le
  pilote : `vm-debug.ps1` (débogueur série, ordre d'allumage appliqué et non plus
  rappelé), `vm-run-console.ps1` (exécution en session console, qui **refuse** de mesurer
  hors d'elle), le nettoyage des périphériques fantômes et l'attente du retrait effectif
  dans `vm-cycle.ps1`. Sans eux, aucune des mesures de cette ADR n'aurait été fiable.
- **Une question reste ouverte et est assumée** : le débogueur noyau cesse de recevoir
  toute sortie après le premier chargement du pilote qui suit l'amorçage, sans que le
  pilote s'arrête ni que `kd` meure. Cause non identifiée. Conséquence pratique inscrite
  dans le code et la documentation : le critère d'une campagne longue est l'absence de
  vidage, pas ce qu'on lit dans le débogueur.
- Le délai borné n'est pas consommé : le budget restant bénéficie à M1b, dont les vingt
  tâches ouvertes n'ont, elles, aucune garantie d'être aussi rapides.
