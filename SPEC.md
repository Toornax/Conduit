# Conduit — Spécification

Câble audio virtuel et graphe de routage multiplateforme, en Rust.

Version 0.4 — 2026-09-05 — document de cadrage, à faire évoluer (0.4 : périmètre DSP de M0, ADR-009).

---

## 1. Vision et périmètre

### 1.1 Objectif

Fournir sur Windows, Linux et macOS :

1. **Des câbles audio virtuels** : des périphériques audio (sortie + entrée appariées)
   visibles par toutes les applications du système. Tout ce qu'une application joue
   dans la sortie « Conduit 1 » est capturable par une autre application depuis l'entrée
   « Conduit 1 ». C'est la fonction VB-Cable, en nombre variable.
2. **Un graphe de routage** à la PipeWire : un démon en espace utilisateur qui voit les
   périphériques réels et virtuels comme des nœuds, permet de les relier librement
   (mixage, duplication, monitoring), avec une latence faible et maîtrisée.
3. **Une expérience identique sur les trois OS** : même GUI, même CLI, même fichier de
   configuration, même protocole de contrôle.

### 1.2 Public cible

Utilisateur final un peu technique : streamer, personne en visioconférence, musicien
amateur, qui sait ce qu'est un périphérique audio mais ne veut pas lire une doc de 50 pages.

Conséquences :

- La **GUI est un livrable de premier rang**, pas une option. Elle doit suffire à 100 % des
  usages courants.
- **Ça marche à l'installation** : deux câbles créés par défaut, réglages automatiques,
  aucun fichier à éditer.
- La CLI et le fichier de configuration existent pour les usages avancés et le support.
- Les messages d'erreur disent quoi faire, pas seulement ce qui a échoué.

### 1.3 Non-objectifs (v1)

- Remplacer PipeWire, PulseAudio ou JACK sur Linux. On s'appuie dessus.
- Vidéo, MIDI, audio réseau, Bluetooth, mobile (iOS/Android).
- Système de filtres extensible (plugins, effets temporels, dynamique). Le cœur inclut
  dès M0 le gain, le mixage, le rééchantillonnage, l'adaptation de canaux et une base DSP
  bornée (biquads, égaliseur paramétrique, générateurs, VU-mètre) — voir ADR-009. Un
  système de filtres ouvert viendra en v2 si le cœur est stable.
- Nombre de câbles illimité sur Windows : la v1 a une réserve fixe (voir §5.4).

### 1.4 Principes directeurs

1. **Fiabilité avant fonctionnalités.** Un pilote noyau qui plante = écran bleu. Un démon
   qui plante = silence. Chaque couche doit dégrader proprement.
2. **Le pilote est bête, le démon est intelligent.** Voir §3.
3. **Le cœur est 100 % portable et testable sans matériel.** Toute la logique de graphe
   s'exécute avec une horloge virtuelle en test.
4. **Temps réel strict** dans le fil audio : pas d'allocation, pas de verrou, pas de
   syscall bloquant, pas de log.
5. **`unsafe` confiné** aux crates de backend et de pilote, chaque bloc justifié.
6. **Tout en Rust**, pilotes compris. Si un composant s'avère impossible en Rust dans un
   délai borné, la décision de repli est explicite et documentée (ADR), jamais implicite.
7. **Builds reproductibles.** L'environnement de développement, les tests et les
   artefacts Linux et macOS sont définis par un flake Nix. Ce que Nix ne peut pas couvrir
   (Windows) est épinglé et scripté avec la même rigueur. Voir §5.11.

---

## 2. Vocabulaire (aligné sur PipeWire)

| Terme | Définition |
|---|---|
| **Nœud** (node) | Unité produisant et/ou consommant de l'audio : périphérique matériel, câble virtuel, flux applicatif, mixeur. |
| **Port** | Entrée ou sortie mono d'un nœud, typée (audio f32). |
| **Lien** (link) | Connexion d'un port de sortie vers un port d'entrée. Plusieurs liens vers un même port d'entrée = somme. |
| **Graphe** | Ensemble des nœuds et liens. Doit rester acyclique. |
| **Quantum** | Nombre d'échantillons traités par cycle (ex. 256). |
| **Pilote de graphe** (driver node) | Le nœud dont l'horloge cadence le cycle de traitement. Un seul à la fois. |
| **Nœud asynchrone** | Nœud possédant sa propre horloge (autre carte son), raccordé au graphe par un tampon et un rééchantillonneur adaptatif. |
| **Câble** | Paire (sortie virtuelle, entrée virtuelle) exposée à l'OS par le pilote de plateforme. |
| **Réserve de câbles** | Sur Windows, ensemble fixe d'emplacements créés à l'installation, dont N sont actifs. |
| **Xrun** | Sous-alimentation ou débordement d'un tampon ; toujours compté et rapporté, jamais silencieux. |

---

## 3. Architecture globale

### 3.1 Décision clé : pilote en boucle locale + démon en espace utilisateur

Chaque plateforme fournit un **pilote minimal** dont l'unique rôle est d'exposer des câbles
en boucle locale : ce qui entre dans la sortie « Conduit X » ressort dans l'entrée
« Conduit X », avec un tampon interne et l'horloge du pilote.

Le pilote **ne transporte aucune donnée audio vers le démon** et n'a aucune logique de
routage. Sa seule surface de contrôle est une **interface de configuration minimale et
strictement validée** : nombre de câbles actifs, nombre de canaux par câble, nom. Rien
d'autre.

Le **démon** s'attache à ces câbles comme à n'importe quel périphérique, via l'API audio
standard de l'OS (WASAPI, PipeWire, CoreAudio). Il fait tout le reste : graphe, mixage,
rééchantillonnage, persistance, contrôle.

Conséquences :

- La fonction « câble » de base marche **même si le démon est arrêté ou planté**.
- Le pilote est petit, sans état dynamique complexe, donc auditable et stable.
- Le cœur Rust est identique sur les trois OS ; seule la couche backend change.
- Coût : un tampon de latence supplémentaire par traversée de câble (voir §5.6).

L'alternative « pilote mince relié au démon par mémoire partagée » (modèle PipeWire) est
plus performante mais lie la stabilité du système audio à celle du démon, et requiert
une IPC noyau/`coreaudiod`. Elle est reportée en v2, uniquement si les mesures de
latence de la v1 s'avèrent insuffisantes.

### 3.2 Schéma

```
 Applications           OS (API audio)                Démon conduitd
 ┌──────────┐    ┌──────────────────────────┐    ┌──────────────────────────────┐
 │ Lecteur  │──▶ │ Sortie "Conduit 1" ─┐    │    │  Backend (WASAPI/PipeWire/   │
 └──────────┘    │                     │loop│    │           CoreAudio)         │
 ┌──────────┐    │ Entrée "Conduit 1" ◀┘ ───┼──▶ │        │                     │
 │ Discord  │◀── │                          │    │   ┌────▼──────────────┐      │
 └──────────┘    │ Carte son réelle   ◀─────┼──▶ │   │ Moteur temps réel │      │
                 │ Micro USB          ──────┼──▶ │   │ graphe / mixage / │      │
                 └──────────────────────────┘    │   │ rééchantillonnage │      │
                          ▲                      │   └────▲──────────────┘      │
            config (N câbles, canaux)            │        │ commandes / événements
                 ┌────────┴─────────┐            │   ┌────┴─────────┐           │
                 │ Pilote boucle    │◀───────────┼───│ Serveur IPC  │◀──▶ GUI / CLI
                 │ locale           │            │   └──────────────┘           │
                 └──────────────────┘            └──────────────────────────────┘
```

### 3.3 Composants

| Composant | Techno | Rôle |
|---|---|---|
| Pilote Windows | Rust, `windows-drivers-rs` (WDM + PortCls/WaveRT) | Réserve de câbles, boucle locale noyau, activation à chaud. |
| Plugin macOS | Rust, `cdylib` AudioServerPlugIn | Câbles dynamiques, boucle locale dans `coreaudiod`. |
| Linux | Aucun pilote : nœuds virtuels créés dans PipeWire par le démon | Câbles = paires sink/source PipeWire. |
| Démon `conduitd` | Rust | Moteur, backends, IPC, persistance, gestion du pilote. |
| GUI `conduit` | Rust, `iced` | Patchbay, gestion des câbles, volumes, diagnostic. |
| CLI `conduitctl` | Rust | Contrôle et diagnostic en ligne de commande. |
| Bibliothèque `conduit-core` | Rust | Moteur embarquable sans le démon. |

---

## 4. Spécifications fonctionnelles

Identifiants stables pour tracer les tests (F-xx).

### 4.1 Câbles

- **F-01** L'utilisateur peut créer et supprimer des câbles **à chaud**, sans redémarrage,
  depuis la GUI ou la CLI. Deux câbles existent après installation.
- **F-02** Chaque câble apparaît dans l'OS comme une sortie et une entrée nommées
  `Conduit 1`, `Conduit 2`, etc. L'utilisateur peut leur donner un alias dans Conduit
  (« Musique », « Micro traité ») ; le renommage côté OS est appliqué quand la plateforme
  le permet (voir §5.4).
- **F-03** Chaque câble a de 1 à 8 canaux. Le changement de canaux d'un câble existant
  peut nécessiter une réactivation de ce câble (court silence), jamais un redémarrage.
- **F-04** Formats supportés par le pilote : 44,1 / 48 / 96 kHz, float 32 bits (et PCM 16/24
  bits sur Windows pour compatibilité). Le mixeur de l'OS fait les conversions restantes.
- **F-05** Un câble fonctionne sans le démon (boucle locale pure). Nuance Linux en §5.3.
- **F-06** Limite v1 : 16 câbles sur Windows (réserve fixe), 32 sur macOS et Linux.

### 4.2 Graphe

- **F-10** Le démon énumère tous les périphériques audio de l'OS (réels et virtuels) et les
  expose comme nœuds avec leurs ports.
- **F-11** Création, suppression, listing de liens port à port. Un port d'entrée peut recevoir
  plusieurs liens (mixage par somme).
- **F-12** Refus de tout lien créant un cycle, avec message explicite.
- **F-13** Gain par lien et par nœud (dB, muet), appliqué avec rampe (pas de clic).
- **F-14** Nœuds utilitaires internes : mixeur N→M, duplicateur, silence, générateur de test
  (sinus, bruit rose), nœud « moniteur » (VU-mètre exportable).
- **F-15** Adaptation automatique du nombre de canaux (mono→stéréo par duplication,
  stéréo→mono par somme pondérée) quand des ports de comptes différents sont liés.
- **F-16** Rééchantillonnage automatique et transparent entre nœuds de fréquences
  différentes.

### 4.3 Périphériques

- **F-20** Branchement/débranchement à chaud : un nœud dont le périphérique disparaît passe
  en état « suspendu », ses liens sont conservés et réactivés à sa réapparition.
- **F-21** Changement du pilote de graphe à chaud (avec un court silence toléré).
- **F-22** Horloge interne (timer haute résolution) quand aucun périphérique matériel n'est
  disponible pour cadencer le graphe.

### 4.4 Persistance et configuration

- **F-30** L'état complet (câbles, liens, gains, pilote choisi, quantum) est sauvegardé et
  restauré au démarrage. Les nœuds sont identifiés par une clé stable (identifiant OS +
  nom), pas par un index.
- **F-31** Fichier de configuration lisible (TOML) : câbles, quantum, fréquence, règles
  d'auto-connexion, niveau de log. La GUI l'écrit ; l'utilisateur n'a jamais besoin de l'ouvrir.
- **F-32** Règles d'auto-connexion déclaratives : « tout nouveau nœud correspondant à ce
  motif est lié à tel port ».

### 4.5 Contrôle

- **F-40** GUI `iced` : patchbay (glisser-déposer pour lier), liste des câbles avec
  création/suppression/renommage, gains et VU-mètres, page de diagnostic (xruns, latence,
  export de rapport), icône de zone de notification.
- **F-41** CLI complète : `status`, `nodes`, `ports`, `links`, `link`, `unlink`, `volume`,
  `driver`, `cable add|remove|rename|list`, `monitor`, `dump`, `load`, `xruns`.
- **F-42** Protocole IPC documenté et versionné, utilisable depuis n'importe quel langage.
- **F-43** Abonnement aux événements (nœud ajouté/retiré, lien modifié, câble créé, xrun,
  changement d'horloge).

### 4.6 Installation et cycle de vie

- **F-50** Installeurs natifs : MSI (Windows), pkg (macOS), paquet ou script (Linux).
  Installation en un clic, sans étape manuelle.
- **F-51** Démon enregistré comme service utilisateur : service Windows, unité systemd
  `--user`, LaunchAgent. La GUI démarre le démon si absent.
- **F-52** Désinstallation propre : suppression du pilote, des périphériques et de la config.
- **F-53** Mise à jour : la nouvelle version migre la configuration précédente.

---

## 5. Spécifications techniques

### 5.1 Moteur (crate `conduit-core`)

**Représentation interne.** Audio f32 planaire (un tampon par canal), fréquence unique par
graphe (défaut 48 kHz), quantum fixe par graphe (défaut 256, plage 32 à 8192, puissance de 2).

**Graphe.** Nœuds et liens stockés dans une structure immuable côté temps réel : toute
modification construit une nouvelle version du graphe (ordre topologique précalculé,
tampons pré-alloués) qui est échangée atomiquement avec la version en cours
(pointeur `Arc` échangé, ancienne version libérée hors du fil audio). Le fil audio ne voit
jamais un graphe partiellement modifié.

**Cycle de traitement.** Déclenché par le pilote de graphe. Pour chaque nœud dans l'ordre
topologique : lire les entrées (somme des liens avec gain), appeler `process`, écrire
les sorties. Budget : le cycle doit tenir dans le quantum ; dépassement = xrun compté.

**Contraintes du fil temps réel.**
- Aucune allocation (vérifié en test par un allocateur de garde qui panique).
- Aucun verrou : communication par files SPSC sans verrou (`rtrb` ou équivalent) pour les
  données, et par échange atomique pour le graphe.
- Priorité temps réel demandée à l'OS (`audio_thread_priority` ou appels natifs).
- Pas de `log!`/`tracing!` : les événements sont poussés dans une file et loggés par un autre fil.
- Le code `core` compile sans dépendance plateforme et s'exécute sous Miri pour les parties
  `unsafe`.

**Horloges multiples.** Chaque périphérique matériel a sa propre horloge, jamais parfaitement
synchrone. Tout nœud qui n'est pas le pilote de graphe est **asynchrone** : ses données
transitent par un tampon circulaire et un rééchantillonneur adaptatif dont le ratio est
piloté par une boucle à verrouillage de délai (DLL) mesurant le remplissage du tampon.
Objectif : dérive absorbée sans xrun, latence stable à ± un quantum. C'est le point le plus
délicat du moteur et il doit être validé avec des horloges simulées dérivantes.

**Rééchantillonnage.** Sinc fenêtré (qualité configurable) via `rubato` ou implémentation
interne. Le rééchantillonneur adaptatif doit accepter un ratio variant continûment.

**Gains.** Rampe linéaire sur un quantum lors de tout changement. Sommation en f32, pas
d'écrêtage dans le graphe ; écrêtage doux optionnel en sortie matérielle.

### 5.2 Backends (trait `Backend`)

Un backend fournit : énumération des périphériques et notifications de changement,
ouverture d'un périphérique en entrée/sortie avec format demandé, rappel audio temps réel,
position d'horloge, fermeture. Chaque périphérique ouvert est un nœud asynchrone, sauf
celui désigné pilote de graphe, dont le rappel exécute le cycle.

Un backend fournit aussi le **contrôle des câbles** de sa plateforme (trait `CableControl`) :
lister, activer, désactiver, configurer les canaux, renommer.

| OS | Backend v1 | Notes |
|---|---|---|
| Windows | WASAPI mode exclusif si disponible, sinon partagé (crate `windows`) | Mode partagé impose la fréquence et le tampon du moteur audio Windows ; documenter l'impact. Contrôle des câbles par IOCTL vers le pilote. |
| Linux | PipeWire (crate `pipewire`) | Câbles = `pw_stream` avec `media.class = Audio/Sink` / `Audio/Source`. Les périphériques réels sont vus à travers PipeWire, pas ALSA directement. Fallback ALSA + `snd-aloop` en v1.x pour les systèmes sans PipeWire. |
| macOS | CoreAudio (bindings via `coreaudio-sys` / `objc2`) | Utilise les AudioDevice ordinaires, y compris ceux du plugin. Contrôle des câbles par propriétés personnalisées du plugin. |
| Tous | `null` | Périphériques et câbles simulés avec horloge virtuelle, pour les tests. |

`cpal` n'est pas utilisé : il masque les horloges et les tailles de tampon, ce qui est
incompatible avec §5.1.

### 5.3 Pilotes de plateforme

**Exigences communes.**
- Boucle locale par câble avec tampon interne de taille fixe (défaut 2 × 10 ms), horloge
  du pilote (timer ou position calculée).
- Interface de configuration minimale : nombre de câbles actifs, canaux, nom. Toute valeur
  est validée (bornes, longueur, caractères) avant usage ; toute valeur invalide est refusée
  sans effet de bord.
- Aucun transport de données audio vers le démon.
- Comportement défini quand un seul côté est ouvert : la sortie sans lecteur est jetée ;
  l'entrée sans producteur lit du silence.

**Windows — pilote noyau en Rust.**
- Base : `windows-drivers-rs` (`wdk-sys`, `wdk`) de Microsoft pour le WDM. PortCls et ses
  interfaces WaveRT (`IMiniportWaveRT`, `IMiniportWaveRTStream`, `IMiniportTopology`,
  `IAdapterPowerManagement`) sont des interfaces COM C++ : elles n'ont pas de bindings
  Rust prêts à l'emploi. Le pilote définit ces vtables à la main dans un crate
  `portcls-sys`, en s'appuyant sur les en-têtes du WDK via bindgen pour les structures et
  constantes.
- Modèle : un adaptateur PortCls enregistrant, au démarrage, **une réserve fixe de 16 câbles**
  (16 sous-périphériques rendu + 16 capture, chacun avec sa topologie). Voir §5.4 pour
  l'activation à chaud.
- Référence de comportement : l'exemple SYSVAD de Microsoft (MIT). On en reprend la
  structure, pas le code.
- Signature obligatoire : certificat EV + signature d'attestation via le Hardware Dev
  Center. Mode test acceptable en développement.
- **Ce pilote est le risque n°1 du projet** (voir §10) et démarre en premier.

**macOS — plugin AudioServerPlugIn en Rust.**
- Bundle `.driver` dans `/Library/Audio/Plug-Ins/HAL/`, `cdylib` implémentant la vtable
  `AudioServerPlugInDriverInterface`.
- Les périphériques sont créés et détruits dynamiquement (le plugin notifie `coreaudiod`
  d'un changement de liste d'objets).
- La configuration est reçue par **propriétés personnalisées** (`kAudioObjectPropertyCustomPropertyInfoList`)
  écrites par le démon. Le plugin persiste sa liste dans son propre stockage
  (`WriteToStorage`) pour recréer les câbles au redémarrage sans le démon.
- Signature et notarisation requises. BlackHole (GPL-3) sert de référence de comportement,
  **pas** de source copiée.

**Linux — pas de pilote.**
- Les câbles sont des nœuds PipeWire créés par le démon. Ils disparaissent si le démon
  s'arrête : F-05 n'est pas garanti sur Linux. Option v1.x : générer une configuration
  PipeWire (`context.objects`) pour des câbles persistants indépendants du démon.

### 5.4 Nombre variable de câbles : réponse par plateforme

C'est faisable sur les trois OS, avec des mécanismes différents.

| OS | Mécanisme | Limite | Délai d'apparition |
|---|---|---|---|
| Windows | **Réserve fixe + état de jack.** Le pilote enregistre 16 câbles au démarrage. Chaque câble inactif est déclaré « jack débranché » (`KSPROPERTY_JACK_DESCRIPTION`, `IsConnected = false`) : Windows le masque des applications et des réglages, comme un casque non branché. Activer un câble = un IOCTL qui bascule le jack en « branché » ; Windows publie l'endpoint immédiatement. | 16 | < 1 s, sans PnP |
| macOS | Création/destruction dynamique d'objets AudioDevice par le plugin. | 32 (arbitraire) | < 1 s |
| Linux | Création/destruction de nœuds PipeWire. | 32 (arbitraire) | immédiat |

Pourquoi pas une création vraiment dynamique sur Windows ? L'alternative est un pilote de
bus qui crée un périphérique enfant (PDO) par câble, avec un second pilote fonction PortCls
chargé sur chacun. Ça lève la limite mais double le nombre de pilotes, ajoute une danse
PnP à chaque création et augmente la surface de plantage noyau. Contraire au principe 1.
Reporté en v2 si la limite de 16 se révèle gênante. La taille de la réserve est un
paramètre de registre lu au démarrage du pilote ; la changer demande une réactivation du
périphérique (sans redémarrage de Windows).

**Canaux par câble sur Windows.** Les formats d'un endpoint sont déclarés à l'enregistrement.
Changer le nombre de canaux d'un câble = le désactiver (jack débranché), mettre à jour la
configuration en registre, le réactiver. Court silence, pas de redémarrage. Sur macOS et
Linux, le câble est simplement recréé.

**Noms.** Windows : le nom d'endpoint vient du registre ; le démon peut le réécrire (droits
administrateur requis, donc le renommage OS est proposé mais pas obligatoire ; l'alias
Conduit est toujours disponible). macOS et Linux : nom libre à la création.

### 5.5 Protocole de contrôle (crate `conduit-protocol`)

- Transport : socket Unix (Linux, macOS, permissions 0600 dans le répertoire runtime de
  l'utilisateur), named pipe (Windows, ACL utilisateur courant). Jamais de réseau par défaut.
- Encodage : trames `u32 longueur` + charge MessagePack (`rmp-serde`). Types définis en Rust
  avec `serde`, schéma exporté en JSON Schema pour les clients tiers.
- Modèle : `Request { id, body }` → `Response { id, result | error }` ; `Event { body }`
  pour les abonnés. Version de protocole négociée au `Hello`.
- Compatibilité : ajout de champs toujours optionnel ; changement incompatible = version
  majeure.
- Le parseur est fuzzé (`cargo-fuzz`). Un client malformé est déconnecté, jamais un
  démon qui panique.

### 5.6 Latence et performance

Objectifs mesurables à 48 kHz, quantum 256 :

| Chemin | Objectif |
|---|---|
| Application → câble → application (sans démon) | ≤ 2 tampons pilote (~20 ms par défaut, réglable à 10 ms) |
| Application → câble → démon → carte son | ≤ tampon pilote + 2 quanta (~20 ms) |
| Carte son → démon → carte son (pilote de graphe) | ≤ 2 quanta (~11 ms) |
| Charge CPU, graphe de 10 nœuds stéréo | < 3 % d'un cœur |
| Xruns en fonctionnement nominal sur 24 h | 0 |

Le démon expose en continu : latence estimée par nœud, remplissage des tampons, ratio du
rééchantillonneur, xruns, temps de cycle (min/moy/max).

### 5.7 Configuration (TOML)

Écrit par la GUI, lisible à la main.

```toml
[engine]
sample_rate = 48000
quantum     = 256
driver      = "auto"          # ou clé stable d'un nœud

[[cable]]
id       = 1
alias    = "Musique"
channels = 2

[[cable]]
id       = 2
alias    = "Micro traité"
channels = 1

[[autoconnect]]
match  = { cable = 1, direction = "capture" }
target = { node = "Haut-parleurs", ports = ["FL", "FR"] }

[log]
level = "info"
```

### 5.8 GUI (`iced`)

- Une fenêtre principale avec trois vues : **Câbles** (liste, créer, supprimer, renommer,
  canaux), **Patchbay** (graphe, liens par glisser-déposer, gains, VU-mètres), **Diagnostic**
  (xruns, latence, état du pilote, export de rapport).
- Icône de zone de notification avec état (OK / xruns / pilote absent) et menu rapide.
- La GUI est un simple client IPC : elle ne contient aucune logique audio et peut être
  fermée sans effet sur le son.
- Rendu `wgpu` par défaut, repli `tiny-skia` pour les machines sans GPU utilisable.
- Thème clair/sombre suivant le système ; textes en français et anglais (`fluent`).

### 5.9 Observabilité

- Logs structurés `tracing`, sortie fichier tournant + journal du système.
- Compteurs et jauges exposés par IPC (`status`) ; export Prometheus optionnel en v1.x.
- Rapport de diagnostic exportable (`dump`) : version, OS, périphériques, graphe, config,
  derniers xruns, sans données personnelles.

### 5.10 Sécurité

- Le démon tourne avec les droits de l'utilisateur. Les opérations nécessitant des droits
  administrateur (configuration du pilote Windows via IOCTL, renommage d'endpoint) passent
  par un **service d'assistance** minimal installé avec le pilote, à interface fixe et
  validée, jamais par élévation du démon entier.
- Socket de contrôle accessible uniquement à l'utilisateur.
- Pilotes : aucune surface exposée aux applications hors les interfaces audio standard de
  l'OS et l'IOCTL de configuration, réservé aux administrateurs.
- Dépendances auditées (`cargo audit`, `cargo deny`), licences vérifiées.

### 5.11 Environnement de build et reproductibilité (Nix)

**Objectif.** Un `git clone` puis `nix develop` donne un environnement identique à tout
contributeur et à la CI, sur Linux et macOS. `nix build` produit les binaires Linux et le
plugin macOS de façon hermétique. Les versions de tout (toolchain Rust, bibliothèques
système, outils) sont épinglées par `flake.lock` et `Cargo.lock`.

**Outillage.**

| Rôle | Choix |
|---|---|
| Structure du flake | `flake-parts` (un module Nix par préoccupation : devshell, packages, checks, CI). |
| Toolchain Rust | `rust-overlay`, version lue depuis `rust-toolchain.toml` pour rester cohérent avec un build sans Nix. Toolchain nightly séparée pour Miri et `cargo-fuzz`. |
| Build des crates | `crane` : dépendances vendorisées et mises en cache séparément des sources, un dérivé par binaire, tests et clippy comme `checks`. |
| Hooks de pré-commit | `git-hooks.nix` : `rustfmt`, `nixfmt`, `cargo-deny`, vérification des messages de commit. |
| Activation | `direnv` avec `.envrc` (`use flake`). |
| Cache binaire | Cachix (ou équivalent) alimenté par la CI, pour éviter de recompiler le monde. |

**Sorties du flake.**

- `devShells.default` : toolchain, `cargo-nextest`, `cargo-deny`, `cargo-audit`,
  `cargo-llvm-cov`, `bindgen`/`clang`, `pkg-config`, PipeWire et ALSA (Linux), SDK Apple
  (macOS, via `apple-sdk` de nixpkgs), bibliothèques `iced`/`wgpu` (Wayland, X11,
  `libxkbcommon`, Vulkan, OpenGL).
- `packages.{conduitd, conduitctl, conduit-gui}` sur Linux ; `packages.conduit-hal` en plus
  sur macOS.
- `checks` : format, clippy, tests, `cargo-deny`, `cargo-audit`, doc du protocole à jour.
  `nix flake check` est **la** commande de validation, identique en local et en CI.
- `nixosModules.default` et `homeManagerModules.default` : installation de Conduit avec
  son service utilisateur, livrable Linux de premier rang au même titre que les paquets.
- `apps` : lancer le démon, la CLI, la GUI, les bancs de test.

**Limites, assumées.**

- **Windows n'est pas couvert par Nix.** Il n'existe pas de Nix natif Windows, et le
  pilote noyau exige le WDK et MSVC. Les crates utilisateur Windows (`conduitd`,
  `conduitctl`, `conduit-gui`, `conduit-helper`) sont **compilés en vérification croisée**
  depuis Nix (cible `x86_64-pc-windows-gnu` via `pkgsCross.mingwW64`) pour détecter tôt les
  erreurs de compilation, mais les artefacts livrés sont produits sur un runner Windows avec
  la cible MSVC. Cet environnement Windows est défini par un script d'installation épinglé
  (versions exactes de Rust, WDK, SDK, WiX, signtool) et vérifié par la CI, dans le même
  esprit que le flake.
- **Signature et notarisation macOS** (`codesign`, `notarytool`) et **signature Windows**
  sortent de Nix : elles appellent les outils propriétaires depuis un job dédié, en aval
  d'un `nix build`.
- **HLK et Driver Verifier** tournent sur des VM Windows, hors Nix.
- Un développeur sans Nix peut toujours builder avec `cargo` et `rust-toolchain.toml` ; Nix
  est l'environnement de référence, pas une obligation pour contribuer.

**Reproductibilité visée.** Binaires Linux reproductibles au bit près à partir du même
`flake.lock` (vérifié périodiquement par un double build en CI). Sur macOS, reproductibilité
de l'environnement, pas garantie au bit près.

---

## 6. Exigences non fonctionnelles

| Domaine | Exigence |
|---|---|
| Fiabilité | Aucune panique dans le démon en usage normal ; toute erreur de périphérique est convertie en état de nœud. Un test d'endurance 24 h avec branchements/débranchements aléatoires fait partie de la CI nocturne. |
| Robustesse du pilote | Tests Driver Verifier (Windows), 48 h de stress sans fuite mémoire, 1000 cycles activation/désactivation de câbles. |
| Portabilité | Windows 10 21H2+ / 11 x86_64 et arm64, Linux x86_64/aarch64, macOS 13+ (Intel et Apple Silicon). |
| Qualité de code | `clippy -D warnings`, `rustfmt`, `#![deny(unsafe_op_in_unsafe_fn)]`, chaque `unsafe` commenté `SAFETY:`. Couverture ≥ 80 % sur `core` et `protocol`. |
| Documentation | Doc utilisateur courte et illustrée, doc protocole, doc d'architecture (ADR pour chaque décision structurante). |
| Licence | MIT OR Apache-2.0 (pilotes inclus). Aucune dépendance copyleft. |
| Support Rust | Stable pour le démon et la GUI, version épinglée dans `rust-toolchain.toml` et fournie par le flake. Le pilote Windows suit la toolchain requise par `windows-drivers-rs` (nightly toléré, épinglé). |
| Reproductibilité | `nix develop` et `nix flake check` identiques en local et en CI sur Linux et macOS ; environnement Windows scripté et épinglé. |

---

## 7. Structure du workspace

```
conduit/
├── flake.nix / flake.lock     # environnement, packages, checks, modules
├── nix/                       # modules flake-parts : devshell, packages, checks, nixos, hm
├── .envrc                     # direnv
├── rust-toolchain.toml        # source de vérité de la version Rust (lue par le flake)
├── Cargo.toml                 # workspace
├── crates/
│   ├── conduit-core/          # graphe, planification, tampons, gains, rééchantillonnage
│   ├── conduit-engine/        # fil temps réel, horloges, DLL, gestion des périphériques
│   ├── conduit-backend/       # traits Backend + CableControl, backend null
│   ├── conduit-backend-wasapi/
│   ├── conduit-backend-pipewire/
│   ├── conduit-backend-coreaudio/
│   ├── conduit-protocol/      # messages IPC, serde, versionnage
│   ├── conduitd/              # binaire du démon
│   ├── conduitctl/            # binaire CLI
│   ├── conduit-gui/           # application iced
│   ├── conduit-kmd-core/      # logique portable du pilote Windows (no_std, testée sous Nix)
│   └── conduit-helper/        # service d'assistance Windows (IOCTL, registre), ADR-012
├── drivers/
│   ├── windows/               # workspace Cargo NOYAU, indépendant (ADR-012)
│   │   ├── portcls-sys/       # vtables et types PortCls/WaveRT
│   │   └── conduit-kmd/       # pilote noyau
│   └── macos-hal/             # plugin AudioServerPlugIn (Rust cdylib + bundle)
├── packaging/
│   ├── windows/               # MSI (WiX), script d'environnement épinglé
│   ├── macos/                 # pkg, LaunchAgent, scripts de signature
│   └── linux/                 # deb/rpm générés depuis les sorties Nix
├── docs/
│   ├── adr/                   # décisions d'architecture numérotées
│   ├── protocol.md
│   └── user/
└── tests/                     # tests d'intégration et d'endurance
```

Règle de dépendance : `conduit-core` ne dépend de rien de plateforme ; `conduit-engine`
dépend de `core` et du trait `backend` ; seuls les `backend-*` et `drivers/` contiennent
du code spécifique à un OS.

---

## 8. Stratégie de tests et CI

| Niveau | Contenu |
|---|---|
| Unitaire | Graphe (ordre topologique, détection de cycles, échange atomique), mixage, rampes, tampons circulaires (tests de propriétés `proptest`), DLL avec horloges simulées dérivantes de ± 1000 ppm. |
| Temps réel | Allocateur de garde : tout `process` qui alloue fait échouer le test. Bench `criterion` du cycle de traitement. |
| Intégration | Démon + backend `null` + CLI : scénarios complets (créer des câbles, créer des liens, débrancher un nœud, changer de pilote, recharger config). |
| Fuzzing | Parseur du protocole, parseur de configuration, parseur de l'IOCTL côté pilote (harnais en mode utilisateur). |
| Miri | `conduit-core` et `conduit-protocol`. |
| Matériel | Bancs Windows/Linux/macOS auto-hébergés : test de boucle (générer un sinus, le lire à travers un câble, vérifier continuité de phase et absence de xrun), mesure de latence réelle. |
| Endurance | 24 h nocturne : branchements aléatoires, création/suppression de câbles, changements de pilote, charge CPU concurrente. |
| Pilotes | Windows : Driver Verifier, HLK audio tests, VM de test avec crash dumps automatiquement collectés. macOS : chargement/déchargement 1000 fois, `leaks`. |
| GUI | Tests des vues `iced` hors écran ; captures de référence sur les trois OS. |

CI GitHub Actions. Linux et macOS : `nix flake check` puis `nix build` des packages, cache
binaire partagé, vérification croisée `mingwW64` des crates utilisateur Windows. Windows :
workflow séparé sans Nix, environnement installé par le script épinglé, build MSVC, tests,
build du pilote. Les tests matériels et pilotes tournent sur runners auto-hébergés.
Un job hebdomadaire reconstruit les packages Linux deux fois et compare les hachages.

---

## 9. Jalons

Ordre choisi : Windows, puis Linux, puis macOS. Le pilote Windows en Rust est le poste de
risque principal ; il démarre en parallèle du cœur pour lever l'incertitude le plus tôt
possible.

| Jalon | Livrable | Critère de sortie |
|---|---|---|
| **M0 — Cœur** | `conduit-core`, `conduit-engine`, `conduit-backend` null, `conduit-protocol`, `conduitd`, `conduitctl` | Graphe complet testé avec horloge virtuelle ; DLL validée avec horloges dérivantes ; 0 allocation dans le fil audio ; CI verte sur les trois OS. |
| **M1a — Spike pilote Windows** (en parallèle de M0) | `portcls-sys` + pilote minimal : **un** câble stéréo en boucle locale, chargé en mode test | Audio audible d'une application à une autre à travers le câble. Aucun BSOD sur 1 h de Driver Verifier. **Porte de décision** : si le spike échoue dans le délai fixé, ADR de repli (voir §10). |
| **M1b — Windows complet** | `conduit-kmd` avec réserve de 16 câbles, jacks, IOCTL ; `conduit-helper` ; `conduit-backend-wasapi` ; MSI | F-01 à F-06 vérifiés ; pilote signé (attestation) ; HLK audio passé ; latence conforme à §5.6 ; endurance 24 h. |
| **M2 — GUI** | `conduit-gui` (Câbles, Patchbay, Diagnostic, zone de notification) | Un utilisateur cible fait un routage complet sans documentation. |
| **M3 — Linux** | `conduit-backend-pipewire`, paquets | Câbles et routage fonctionnels sur PipeWire ; endurance 24 h. |
| **M4 — macOS** | `drivers/macos-hal`, `conduit-backend-coreaudio`, pkg | Câbles visibles dans Réglages Son, signés et notarisés ; F-05 vérifié. |
| **M5 — Produit** | Services, mise à jour, docs utilisateur, site | Installation en un clic sur les trois OS ; documentation complète. |
| **v2 (étude)** | Pilote mince + mémoire partagée, pilote de bus pour câbles illimités, filtres DSP, MIDI, réseau | Décidé sur mesures et retours de la v1. |

La GUI arrive en M2, juste après Windows, parce que le public cible en a besoin pour
utiliser le produit : Windows + GUI forme la première version utilisable.

---

## 10. Risques

| Risque | Impact | Mitigation |
|---|---|---|
| **PortCls en Rust n'a pas de précédent public** : vtables COM à écrire à la main, comportement des interfaces peu documenté hors des exemples C++ | Bloque M1 | Spike M1a borné et isolé ; porte de décision explicite. Repli documenté : pilote en C++ dérivé de SYSVAD, tout le reste inchangé. |
| Toolchain `windows-drivers-rs` encore jeune (nightly, changements d'API) | Ruptures de build | Toolchain épinglée, dépendances vendorisées, CI dédiée. |
| Signature du pilote Windows (certificat EV, processus Microsoft) | Bloque la distribution M1b | Démarrer les démarches administratives dès M0 ; mode test pour le développement. |
| Instabilité de la DLL / rééchantillonnage adaptatif | Xruns, dérive de latence | Simulation exhaustive dès M0 ; s'inspirer de l'implémentation PipeWire (MIT). |
| Mode partagé WASAPI imposant ses paramètres | Latence plus élevée sur Windows | Mode exclusif quand disponible ; documenter. |
| Bac à sable de `coreaudiod` limitant le plugin macOS | Fonctionnalités du plugin | Le plugin ne fait que sa boucle locale ; configuration par propriétés personnalisées, persistance par `WriteToStorage`. |
| `iced` évolue vite entre versions | Coût de maintenance GUI | Version épinglée, mise à jour par jalon. |
| Nom « Conduit » | Collision de marque ou de crate | Vérifier crates.io et les marques audio avant publication. |
| Nix absent sur Windows, la plateforme prioritaire | Deux environnements à maintenir, dérive possible | Script Windows épinglé versionné avec le flake ; la CI Windows échoue si une version diffère de la liste ; vérification croisée `mingwW64` pour attraper les régressions depuis Nix. |
| Bibliothèques graphiques (`wgpu`, Wayland, Vulkan) et Nix : chemins d'exécution | GUI qui ne démarre pas hors `nix run` | Wrapper avec `RPATH`/`LD_LIBRARY_PATH` dans le package, testé hors devshell. |

---

## 11. Décisions prises et ouvertes

### Prises (2026-09-05)

| # | Décision | À formaliser en |
|---|---|---|
| D-01 | Nom : **Conduit**. Binaires `conduitd`, `conduitctl`, `conduit` (GUI). | ADR-001 |
| D-02 | Ordre des plateformes : **Windows, Linux, macOS**. | ADR-002 |
| D-03 | Pilote Windows **en Rust** dès le départ, avec spike et porte de décision. | ADR-003 |
| D-04 | Nombre de câbles **variable à chaud**, réserve fixe de 16 sur Windows. | ADR-004 |
| D-05 | GUI en **`iced`**, livrable de premier rang. | ADR-005 |
| D-06 | Public : **utilisateur final un peu technique**. | ADR-006 |
| D-07 | Architecture pilote bête / démon intelligent. | ADR-007 |
| D-08 | **Nix** (flake, `crane`, `flake-parts`) comme environnement de référence pour Linux et macOS ; Windows scripté et épinglé hors Nix. | ADR-008 |

### Ouvertes

1. **Taille de la réserve Windows** : 16 proposé. Chaque emplacement coûte deux endpoints
   visibles dans le gestionnaire de périphériques (masqués des applications quand inactifs).
2. **Tampon interne du pilote** : 10 ms par défaut, réglable ? Compromis latence / robustesse.
3. **Délai du spike M1a** avant la porte de décision.
4. **Vérification du nom** sur crates.io et dans les marques audio (à faire avant tout
   dépôt public).
