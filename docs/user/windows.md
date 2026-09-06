# Conduit sous Windows — guide utilisateur

> **Ébauche.** Ce guide est complété au fil de M1b ; la partie « Démarrage automatique »
> est à jour (M1b-35). L'installation par le MSI (M1b-40) et le dépannage (M1b-44)
> viendront s'y ajouter.

Conduit se compose de trois morceaux qui n'ont pas le même rôle :

| Morceau | Ce que c'est | Droits |
|---|---|---|
| **`conduitd`** | le démon : il tient le graphe audio, parle aux cartes son, exécute vos routages | les vôtres, dans votre session |
| **`conduit-helper`** | un petit service Windows, uniquement pour ce qui exige l'administrateur (pilote, renommage d'un périphérique) | administrateur |
| **`conduitctl`**, la GUI | ce avec quoi vous parlez au démon | les vôtres |

## Démarrage automatique

### Ce qui se passe

Le démon doit être là quand vous ouvrez votre session, sans que vous ayez à le lancer.
Conduit s'en occupe avec une **tâche planifiée** nommée `Conduit\conduitd`, déclenchée
**à l'ouverture de votre session**, 30 secondes après (le temps que le bureau finisse de
se charger). La tâche est enregistrée **pour vous seul** : sur un poste partagé, chaque
utilisateur a la sienne, et son propre démon.

```
conduitd autostart enable     enregistre la tâche
conduitd autostart status     dit si elle existe, son état, sa prochaine exécution
conduitd autostart disable    la supprime
```

`autostart status` répond par exemple :

```
tâche planifiée « Conduit\conduitd » : présente.
  nom complet          : Conduit\conduitd
  état                 : Activée
  statut               : Prêt
  prochaine exécution  : N/A
  ...
```

Codes de retour, si vous scriptez : `0` c'est fait (ou, pour `status`, la tâche existe),
`1` Windows a refusé, `4` la tâche n'est pas enregistrée.

### Pourquoi ce n'est pas un service Windows

C'est la question qu'on pose toujours, et la réponse tient au fonctionnement de l'audio
sous Windows. Un service, au sens du gestionnaire de services, tourne dans la **session 0**,
isolée de la vôtre depuis Windows Vista. Or :

- le moteur audio de Windows est **par session** : un processus de la session 0 ne voit
  aucun de vos périphériques ;
- votre **périphérique par défaut** (et les rôles « Communications », « Multimédia »…)
  est un réglage **par utilisateur** ; la session 0 n'en a pas ;
- un service imposerait des droits administrateur permanents à un programme qui n'en a
  aucun besoin.

Un service ne pourrait donc tout simplement pas faire le travail. Conduit fait comme
`systemd --user` sous Linux et les LaunchAgent sous macOS : un programme ordinaire, à vos
droits, démarré à l'ouverture de session. Le détail de la décision est dans
[ADR-013](../adr/013-demarrage-du-demon-sous-windows.md).

Le seul vrai service de Conduit est `conduit-helper`, qui ne touche à aucun son : il
n'existe que pour les opérations réservées à l'administrateur.

### La voir dans le Planificateur de tâches

1. Touche Windows, tapez **Planificateur de tâches**, ouvrez-le.
2. Dans l'arborescence de gauche : **Bibliothèque du Planificateur de tâches** →
   **Conduit**.
3. La tâche **conduitd** s'y trouve. Ses onglets vous montrent tout ce que Conduit a
   demandé : *Déclencheurs* (« À l'ouverture de session de … », différé de 30 secondes),
   *Actions* (le chemin de `conduitd.exe`), *Conditions* (pas d'arrêt sur batterie),
   *Paramètres* (redémarrage en cas d'échec, aucune limite de durée, « Arrêter l'instance
   existante » si la tâche est relancée).

Vous pouvez y cliquer **Exécuter** pour démarrer le démon tout de suite sans rouvrir de
session, et **Fin** pour l'arrêter.

### La désactiver

Trois façons, de la plus douce à la plus définitive :

- **temporairement** : dans le Planificateur, clic droit sur la tâche → **Désactiver**.
  Elle reste visible et vous la réactivez d'un clic.
- **définitivement** : `conduitd autostart disable` dans un terminal. La tâche est
  supprimée ; le démon lancé à la main ou par la GUI continue de marcher.
- **avec le reste** : désinstaller Conduit supprime la tâche.

Sans démarrage automatique, Conduit n'est pas cassé : la GUI démarre le démon si elle ne
le trouve pas, et `conduitd` se lance depuis un terminal.

### Un seul démon à la fois

Un seul démon peut tenir votre point de contrôle. Si vous en lancez un second, il refuse
poliment plutôt que d'échouer avec un message obscur :

```
un démon Conduit est déjà en cours pour cette session ; utilisez `conduitctl` pour lui
parler, ou arrêtez-le avant d'en lancer un autre
```

(code de retour 3). C'est normal, et c'est même le cas courant : la tâche planifiée en a
déjà démarré un.

### À la fermeture de session

Quand vous fermez votre session ou éteignez le PC, le démon est prévenu : il ferme ses
flux audio et **sauvegarde votre routage** avant de partir. Vous le retrouvez tel quel à
la prochaine ouverture. Rien à faire, et rien à craindre pour un arrêt un peu brutal :
l'état est de toute façon sauvegardé en continu pendant que vous travaillez.

## À suivre

- Installation par le MSI et désinstallation (M1b-40).
- Dépannage : le démon ne démarre pas, aucun périphérique, latence (M1b-44).
