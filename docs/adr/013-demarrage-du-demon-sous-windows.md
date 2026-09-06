# ADR-013 — Le démon Windows démarre à l'ouverture de session, il n'est pas un service

**Statut** : acceptée (2026-09-06). Corrige une contradiction interne de `SPEC.md`
(F-51 contre §5.10) avant l'implémentation de M1b-35.

## Contexte

`SPEC.md` §4.6 (F-51) demande que le démon soit « enregistré comme service utilisateur :
**service Windows**, unité systemd `--user`, LaunchAgent ». La même spécification, en
§5.10, exige que « le démon tourne avec les droits de l'utilisateur » et réserve
l'élévation à un **service d'assistance** minimal.

Les deux ne peuvent pas être vraies en même temps sous Windows. Un service au sens du
gestionnaire de services (SCM) s'exécute dans la **session 0**, sous `LocalSystem`,
`NetworkService` ou un compte fixe, jamais « sous l'utilisateur connecté » au sens où
systemd `--user` et les LaunchAgent l'entendent. Trois conséquences rédhibitoires pour
Conduit :

1. **Pas d'audio.** Depuis Windows Vista, la session 0 est isolée de la session
   interactive. Le moteur audio et le mixage sont par session : un processus de la
   session 0 n'a pas les endpoints de l'utilisateur.
2. **Pas de périphérique par défaut.** Le choix de l'endpoint par défaut et les rôles
   (`eConsole`, `eMultimedia`, `eCommunications`) sont stockés **par utilisateur**
   (`HKCU`). Le pilote de graphe de Conduit (`DriverChoice::Auto`) s'appuie dessus.
3. **Élévation non désirée.** Un service SCM implique des droits administrateur
   permanents pour tout le démon, exactement ce que §5.10 interdit.

Le terme « service utilisateur » de F-51 décrit donc une **intention** (le démon démarre
tout seul, sans que l'utilisateur le lance) que Windows réalise autrement que Linux et
macOS.

## Décision

- **`conduitd` n'est pas un service Windows.** C'est un processus ordinaire, aux droits
  de l'utilisateur, dans sa session interactive.
- **Démarrage automatique à l'ouverture de session**, par une **tâche planifiée par
  utilisateur** (déclencheur « à l'ouverture de session de cet utilisateur »),
  enregistrée par le MSI dans le contexte de l'utilisateur qui installe et supprimée à
  la désinstallation (F-52). La tâche planifiée est préférée à une clé `Run` du registre
  parce qu'elle permet un redémarrage automatique en cas d'arrêt inopiné, un délai au
  démarrage, et qu'elle est visible et désactivable par l'utilisateur.
- **Une seule instance par session** : le démon s'appuie sur l'exclusivité du named pipe
  de contrôle (déjà en place, `first_pipe_instance`) ; une seconde instance sort avec un
  message clair au lieu d'échouer obscurément.
- **`conduit-helper` reste, lui, un vrai service Windows** (M1b-20) : il a besoin des
  droits administrateur pour la propriété privée du pilote et le renommage d'endpoint,
  il ne touche à aucun flux audio, et l'isolation de session ne le gêne donc pas. C'est
  la répartition voulue par §5.10.
- **La GUI démarre le démon s'il est absent** (F-51, dernière phrase) : ce chemin reste
  le filet de sécurité si la tâche planifiée est désactivée.

## Conséquences

- `SPEC.md` F-51 est amendé : « service Windows » devient « tâche planifiée à
  l'ouverture de session ».
- `ROADMAP.md` M1b-35 change d'intitulé : la tâche n'écrit pas de service SCM mais
  l'enregistrement de la tâche planifiée, la détection d'instance unique et l'arrêt
  propre à la fermeture de session.
- Le MSI (M1b-40) installe deux choses de natures différentes : un service (helper, par
  machine, élévation à l'installation) et une tâche planifiée (démon, par utilisateur).
  Une installation par machine devra donc créer la tâche pour chaque utilisateur, ou à
  la première ouverture de session ; à trancher en M1b-40.
- Sur un poste multi-utilisateur, un démon par session ouverte, chacun avec son pipe :
  cohérent avec le modèle Linux et macOS.
- Si un besoin de démon sans session ouverte apparaissait (rendu automatisé sur un
  serveur), il faudrait une ADR séparée : ce n'est pas un objectif de la v1.
