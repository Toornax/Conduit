# Mise en route du pilote dans la VM : plan de validation ordonné

Sept tâches du spike M1a ont leur code livré, testé hors noyau, mais **jamais chargé**
(M1a-02, 05, 06, 07, 08, 09, 10). Elles se valident toutes dans la même session de VM.
Ce document en donne l'ordre, le résultat attendu à chaque étape, et quoi faire quand
l'attendu n'arrive pas. Il complète [driver-dev.md](driver-dev.md), qui décrit chaque
outil en détail ; ici c'est l'enchaînement qui compte.

**Principe** : chaque étape ne dépend que des précédentes, et chaque échec a un
diagnostic borné. On ne passe pas à l'étape suivante avec une étape rouge. À la fin de
chaque étape verte, cocher la tâche dans `ROADMAP.md` et retirer sa ligne `*État*`.

Rédigé le 2026-09-06, avant toute exécution : les « attendus » sont des **prédictions**
issues de la conception et des rapports de tâches, pas des observations. Ce qui les
contredira est l'information la plus précieuse de la session : le noter tel quel.

## 0. Prérequis (hôte)

| Élément | Commande | Attendu |
|---|---|---|
| Accès à Hyper-V | `[Security.Principal.WindowsIdentity]::GetCurrent().Groups.Value -contains 'S-1-5-32-578'` | `True` — sinon lancer les scripts de VM en administrateur |
| Environnement pilote | `.\packaging\windows\setup-env.ps1 -Check -Scope Driver` | « environnement Windows conforme (périmètre pilote) » |
| Workspace noyau | `.\drivers\windows\tools\check.ps1` | « vérifications vertes » |
| Paquet signé | `.\drivers\windows\tools\build.ps1` | `target\debug\conduit_kmd_package\` avec `.sys`, `.inf`, `.cat`, `.cer` |
| ISO Windows 11 | — | image officielle Microsoft |

Les scripts de VM acceptent une session **administrateur** ou une session ordinaire dont
le compte est membre du groupe **Administrateurs Hyper-V**. Pour cette session de
validation, qui est longue et enchaîne des commandes Cargo, la seconde est préférable :
une fois, depuis un PowerShell administrateur, `Add-LocalGroupMember -SID S-1-5-32-578
-Member "$env:USERNAME"` — le groupe est désigné par son SID, son nom étant traduit —
**puis fermer et rouvrir la session Windows**, car le jeton n'intègre les appartenances
de groupe qu'à l'ouverture de session (rouvrir le terminal ne suffit pas). Détails dans
[driver-dev.md §3](driver-dev.md), « Deux façons de lancer les scripts ».

## 1. Créer et préparer la VM

```powershell
.\drivers\windows\tools\vm-new.ps1 -IsoPath <chemin de l'ISO>
```

Génération 2, Secure Boot **désactivé** (sinon `testsigning` est refusé), vTPM activé
(l'installeur Windows 11 l'exige). Installer Windows 11 Pro avec un compte **local**
`test` administrateur. **Microsoft a retiré `OOBE\BYPASSNRO` et `ms-cxh:localonly`** des
images récentes, 25H2 comprise. Si l'installeur impose un compte Microsoft : déconnecter
la carte réseau depuis l'hôte, puis dans la VM `Maj+F10` et
`reg add HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OOBE /v BypassNRO /t REG_DWORD /d 1 /f`
suivi de `shutdown /r /t 0` ; l'option « Je n'ai pas Internet » réapparaît. Reconnecter la
carte ensuite, le débogage noyau en dépend.

```powershell
Get-VMNetworkAdapter -VMName ConduitTest | Disconnect-VMNetworkAdapter   # avant
Get-VMNetworkAdapter -VMName ConduitTest | Connect-VMNetworkAdapter -SwitchName "Default Switch"
``` Puis :

```powershell
$cred = Get-Credential nathan
.\drivers\windows\tools\vm-prepare.ps1 -Name ConduitTest -Credential $cred
```

Active `testsigning`, le **débogage noyau par canal nommé série** (`\\.\pipe\conduitdbg`
sur le port COM 1 : le transport réseau, lui, ne s'est jamais connecté sur ce poste), le
**filtre de traces du noyau** (`Debug Print Filter\DEFAULT = 0xF`, qui ouvre les traces des
autres composants ; les `kmd_log!` du pilote, eux, sortent au niveau *erreur* et arrivent
sans configuration), la collecte des vidages, désactive la veille,
arrête l'invité le temps d'attacher le canal (`Set-VMComPort` exige la VM éteinte),
rallume et crée le point de contrôle « propre ». **Revenir à ce point de contrôle** avant
chaque reprise à froid.

Le compte à passer est celui **réellement créé** dans la VM : c'est `nathan` ici, quoi
qu'en dise l'habitude du `test` des premières versions de ce document.

*Si `testsigning` est refusé* : Secure Boot est resté actif. Le désactiver dans les
paramètres de la VM, redémarrer, relancer.

Pour déboguer : `.\drivers\windows\tools\vm-debug.ps1 -StartVM -Follow`. **Le débogueur
doit tenir le canal nommé avant que la machine démarre** — `-StartVM` impose cet ordre
(arrêt, débogueur, attente, démarrage) ; une VM démarrée en premier ne se rattrape pas
([driver-dev.md](driver-dev.md) §4).

## 2. M1a-02 — le pilote se charge et se décharge

```powershell
.\drivers\windows\tools\vm-cycle.ps1 -Name ConduitTest -Credential $cred -Count 100
```

**Attendu** : `1/100 OK` … `100/100 OK`, puis « 100 cycles sans erreur » et la durée
moyenne. Aucun vidage rapatrié dans `drivers\windows\target\dumps\`.

**Ce qui peut casser, par ordre de probabilité** :

1. *Le pilote ne se charge pas, code 39 ou 31 dans le gestionnaire de périphériques* :
   signature ou `testsigning`. Vérifier que `WDRLocalTestCert.cer` est bien dans
   *Trusted Root* **et** *Trusted Publishers* de la VM (le script le fait, le
   revérifier), et que `bcdedit` montre `testsigning Yes`.
2. *`ExAllocateTimer` échoue au démarrage* → `STATUS_INSUFFICIENT_RESOURCES`, le
   périphérique ne démarre pas (M1a-08, décision assumée : pas de repli). Regarder le
   journal du débogueur (`kmd_log!`).
3. *La course du retrait, `Kernel-PnP 411` avec l'état `0xC00000E5`* : **son signe
   distinctif est que l'événement désigne l'appareil du cycle PRÉCÉDENT**, à l'instant de
   son retrait, alors que le cycle en cours n'a jamais chargé le pilote. C'est PnP qui
   tente un dernier démarrage sur le devnode mourant pendant que `pnputil /delete-driver`
   lui retire son paquet sous les pieds — le pilote est hors de cause. Mesurée et corrigée
   le 2026-09-06 (attente de la disparition du devnode avant la suppression du paquet,
   `-RemoveTimeoutSeconds`, §3.3 de driver-dev.md) ; si elle réapparaît sur un invité lent
   ou ralenti par le débogueur, augmenter ce délai. Ne pas soupçonner le pilote avant
   d'avoir comparé l'identifiant de l'appareil à celui du cycle en cours.
4. *Écran bleu au chargement* : attacher le débogueur AVANT de démarrer la VM
   (`vm-debug.ps1 -StartVM`, §4 de driver-dev.md), puis `!analyze -v`.
   Un bug check `0xE0000001` est **notre** gestionnaire de panique : le message Rust est
   dans les paramètres, et c'est un bogue de logique, pas de noyau.
5. *Fuite entre les cycles* : `!poolused` sur les tags `Cndt`, ils doivent revenir à zéro.

En cas d'échec, regarder aussi `drivers\windows\target\dumps\<horodatage>_setupapi.dev.log` :
le script y rapatrie les 500 dernières lignes du journal d'installation PnP de l'invité,
qui dit ce que PnP a tenté et pourquoi il a échoué.

## 3. M1a-06 et M1a-09 — les endpoints apparaissent et portent le bon nom

Après une installation (sans la retirer), dans la VM :

```powershell
Get-PnpDevice -Class AudioEndpoint | Select-Object FriendlyName, Status
```

**Attendu** : deux endpoints, un rendu et un capture, nommés **« Conduit 1 (Conduit —
câbles audio virtuels) »**. Vérifier aussi dans `mmsys.cpl`.

**Diagnostic, du plus fréquent au moins** :

1. *Le périphérique s'installe mais **aucun** endpoint n'apparaît* : c'est le symptôme
   muet classique. Trois causes déjà écartées par construction mais à revérifier dans
   cet ordre : la section `.NT.HW` et son SDDL (sans elle le processus d'isolation audio,
   sous un compte restreint, ne peut pas ouvrir nos filtres) ; la correspondance entre
   les noms de l'INF et ceux passés à l'enregistrement des sous-périphériques (le test
   `portcls/tests/inf.rs` la garantit sur l'hôte, mais vérifier que l'INF installé est
   bien celui construit) ; les connexions physiques entre filtres wave et topologie,
   sans lesquelles Windows ne construit pas d'endpoint.
2. *Les endpoints apparaissent mais s'appellent « Haut-parleurs » et « Ligne »* : le nom
   de broche n'a pas été pris. C'est la **réserve documentée** de M1a-09 : deux pages de
   documentation Microsoft se contredisent sur la possibilité de renommer un endpoint de
   type haut-parleur. Vérifier d'abord que l'écriture de registre a eu lieu, sous la clé
   logicielle du périphérique, catégorie média, identifiant de broche du câble 0. Si
   elle est là et ignorée, appliquer le repli documenté : déclarer la broche endpoint de
   rendu autrement qu'en haut-parleur, au prix de l'icône et du rang de sélection par
   défaut.
3. *Le nom est bon mais trop long* : raccourcir la description du périphérique, c'est
   elle qui fournit la partie entre parenthèses.

## 3 bis. Où lancer les mesures audio : le piège qui a coûté une journée

**Toute mesure audio faite au mauvais endroit ne mesure rien**, et le symptôme est
exactement celui d'un pilote muet. Deux pièges, découverts le 2026-09-06 après des heures
de fausses pistes sur le pilote, qui lui n'avait rien.

1. **PowerShell Direct ouvre ses sessions dans la session 0**, celle des services. Elle
   est isolée de la session interactive et n'a pas l'audio de l'utilisateur, exactement ce
   que dit [ADR-013](adr/013-demarrage-du-demon-sous-windows.md) à propos du démon. Un
   test lancé par `Invoke-Command -VMName` énumère bien les endpoints, ouvre les flux et
   reçoit des trames **à la bonne cadence**, mais toutes silencieuses. Rien ne signale
   l'erreur.
2. **Le mode session étendue de `vmconnect` redirige l'audio vers l'hôte.** La session
   ouverte ainsi ne voit qu'un « périphérique audio distant » et **pas** les endpoints de
   la machine : le test ne trouve même plus le câble.

**La bonne configuration** : ouvrir `vmconnect` en **session de base** (désactiver le mode
session étendue), ouvrir une session Windows à la console, et lancer les mesures **dans
cette session**.

Depuis l'hôte, cette recette est maintenant un script — elle n'est plus à retenir :

```powershell
.\drivers\windows\tools\vm-run-console.ps1 -Credential $cred `
  -Path target\debug\conduit-looptest.exe -Arguments @("--repeat", "10")
```

`vm-run-console.ps1` **refuse de travailler** tant qu'une session console interactive n'est
pas ouverte, et le dit avec la marche à suivre ci-dessus : il ne rendra jamais un résultat
silencieusement faux. La vérification ne dépend pas de la langue
(`Win32_ComputerSystem.UserName` pour l'utilisateur ouvert à la console, et l'identifiant
de session de *son* `explorer`, qui doit différer de 0 — jamais `quser`, traduit). Il copie
l'exécutable au besoin, crée une tâche planifiée à **jeton interactif**
(`schtasks /create … /ru <compte du -Credential> /it /f`, sans mot de passe), la lance,
attend le **fichier de code de retour** que la commande écrit elle-même (le statut de
`schtasks /query` est traduit, donc inutilisable), relit la sortie, supprime la tâche et
propage le code de retour. `-TimeoutSeconds 4000` couvre la charge d'une heure de l'étape 7.

**Le réflexe, avant de suspecter le pilote** : vérifier la session du processus de test et
le volume des endpoints. `conduit-looptest` les affiche désormais lui-même et prévient
quand la mesure ne peut pas avoir de sens.

## 4. M1a-07 — une application peut jouer sur l'endpoint

Dans la VM, jouer un fichier sur « Conduit 1 » (l'application Musique, ou
`conduit-looptest --render "Conduit 1" --no-capture --seconds 5 --amplitude 0.05`).

**Attendu** : aucune erreur, la lecture dure le temps prévu (et non un temps aberrant),
le compteur de position progresse. **Rien n'est audible** : le câble n'a pas de sortie
matérielle, c'est normal.

**Le piège numéro un à ce stade** est la période réelle du timer. En debug, la ligne
« câble 0 : 1000 ticks » doit sortir **une fois par seconde** dans DebugView. Si elle
sort beaucoup plus rarement, la période d'`ExSetTimer` est mal interprétée — bien que la
documentation confirme les unités de 100 nanosecondes, c'est la première chose à
mesurer, parce que l'erreur serait silencieuse et fausserait tout le reste.

Si la lecture s'arrête ou bégaie : regarder les compteurs de débordement du câble.

## 5. M1a-08 — la capture reçoit ce que le rendu joue

Enregistrer sur « Conduit 1 » pendant qu'une application y joue (Enregistreur vocal, ou
l'étape suivante directement).

**Attendu** : le son enregistré est celui joué. Compteurs du câble : les trames copiées
croissent d'environ 48 000 par seconde, les débordements restent à zéro.

**Si la capture n'entend rien, deux réflexes d'abord — ils coûtent dix secondes et
expliquent la moitié des mesures silencieuses :**

1. **Le volume et la coupure des endpoints.** Un endpoint à zéro ou coupé rend toute
   la chaîne muette, et ce silence est indiscernable d'un pilote en panne. Le pilote
   Conduit crée ses endpoints avec une propriété de volume à zéro en registre : c'est
   *exactement* le piège.

   ```powershell
   conduit-looptest --list --show-volume            # relever, sans rien changer
   conduit-looptest --set-volume 1 --unmute         # corriger les deux côtés du câble
   ```

   L'outil relève de toute façon le volume avant chaque passe et avertit sur la sortie
   d'erreur si l'un des endpoints est coupé ou à zéro ; il n'y a donc rien à faire à la
   main si l'on lit ce qu'il imprime.

2. **La session Windows dans laquelle tourne le test.** Un test lancé depuis un
   service, une tâche planifiée « même si l'utilisateur n'est pas connecté » ou un
   agent d'exécution à distance tourne dans la **session 0**, celle des services : il
   n'y voit pas les périphériques audio de l'utilisateur et rien de ce qu'il joue ne
   sort. La mesure n'y a **aucun sens** — ce n'est pas une mesure ratée, c'est une
   mesure sans objet, et une journée peut y passer. `conduit-looptest` imprime la
   session avec tout diagnostic de silence ; `(Get-Process -Id $PID).SessionId` la
   donne aussi. Si elle vaut 0, relancer depuis une session ouverte à l'écran.

**Ces deux points écartés, la mesure suivante est la capture en écho**, avant toute
inspection du pilote :

```powershell
conduit-looptest --render "Conduit 1" --loopback --seconds 2 --amplitude 0.05
```

Elle ouvre l'endpoint de **rendu** en écho (WASAPI loopback) et prélève le mélange
**avant** qu'il n'atteigne le pilote. Elle coupe le problème en deux, et l'outil imprime
laquelle des deux conclusions s'applique :

- **le sinus est entendu en écho** → le moteur audio délivre bien vers le câble ; le
  défaut est dans l'échange de données du pilote (copie rendu → capture, avance de copie,
  positions du tampon cyclique). Continuer avec les points de doute ci-dessous ;
- **l'écho est silencieux** → rien n'arrive jusqu'au pilote, qui est donc hors de cause :
  chercher en amont (volume de l'endpoint, endpoint désactivé, format par défaut, un autre
  programme en mode exclusif). Inspecter le pilote à ce stade serait du temps perdu.

Deux réserves : l'écho prélève tout le mélange, donc aussi ce que jouent les autres
applications (les fermer), et le volume de l'endpoint s'y applique — une amplitude basse
n'accuse personne. La même commande sur une **vraie carte son** (`--render "Haut-parleurs"`)
sert de témoin : elle doit passer, sinon c'est l'hôte ou la VM qu'il faut regarder, pas
Conduit.

**Points de doute connus, dans l'ordre** :

1. *L'avance de copie de 2 ms* est le pari central de la conception. Trop courte, le
   moteur audio écrase les trames avant leur copie et on entend des ruptures ; trop
   longue, elle mord sur la moitié en cours de lecture. L'étape 6 la mesure
   objectivement ; ici on écoute.
2. *Débordements non nuls sous charge* : un tick manqué de plus d'un tampon fait un
   trou. Si ce n'est pas rare, agrandir le tampon ou rattraper partiellement au lieu de
   sauter.
3. *Ordres d'ouverture croisés* : ouvrir l'enregistreur avant le lecteur, puis
   l'inverse, puis fermer dans les deux ordres. Le lien entre les deux positions doit se
   rétablir sans rupture et la position de capture rester croissante.

## 6. M1a-10 — la boucle est mesurée, dix fois de suite

```powershell
conduit-looptest --repeat 10
```

**Attendu** : dix passes `OK`, code de retour 0. C'est le critère de sortie de M1a-10 et
la mesure objective de l'étape 5. L'outil détecte une seule trame perdue ou dupliquée
(seuil de phase adapté à la fréquence), les trous, l'écrêtage et la dérive de fréquence.

En cas d'échec, la sortie nomme la cause et la trame concernée : une rupture de phase
périodique pointe vers l'avance de copie, des trous vers la période du timer.

## 7. M1a-11 — Driver Verifier une heure

Activer Driver Verifier sur `conduit_kmd.sys` (paramètres standard, plus special pool et
contrôle d'IRQL), redémarrer, puis rejouer les étapes 4 à 6 pendant une heure avec des
ouvertures et fermetures répétées. Sans surveillance, et **dans la session console** (§3
bis), sinon l'heure entière ne mesure rien :

```powershell
.\drivers\windows\tools\vm-run-console.ps1 -Credential $cred `
  -RemoteExecutable conduit-looptest.exe -Arguments @("--repeat", "200") -TimeoutSeconds 4000
```

Le débogueur peut rester attaché pendant toute la charge (`vm-debug.ps1 -Follow`) : c'est
lui qui recueillera le bug check si Driver Verifier en déclenche un.

**Attendu** : aucun écran bleu, aucun vidage. C'est ce qui tranchera le dernier doute de
conception : le double verrouillage câble puis flux à un niveau d'interruption élevé, et
la libération du timer au déchargement.

## 8. M1a-12 — la porte de décision

Rédiger `docs/adr/014-resultat-du-spike-pilote-rust.md` (le numéro 009 annoncé par la
ROADMAP est déjà pris par une autre décision : le corriger dans la ROADMAP). Les quatre
questions posées au spike, et ce que la session aura répondu :

1. `windows-drivers-rs` compile-t-il un pilote PortCls sans nightly ni correctif ?
   **Déjà répondu oui** sur l'hôte, en Rust stable.
2. Les vtables COM tiennent-elles ? **Déjà répondu oui** pour les dispositions, vérifiées
   par un second compilateur ; l'exécution reste à confirmer aux étapes 3 et 4.
3. Lecture et boucle stables une heure sous Driver Verifier ? Étape 7.
4. Coût réel comparé au délai borné ? À chiffrer d'après l'historique Git.

Succès sur les quatre → M1b en Rust. Échec sur 3 dans le délai → décision de repli C++,
qui conserve `conduit-kmd-core`, `conduit-com`, `portcls-sys` et le helper.

## Après la porte

M1b.A reprend dans la VM (réserve de 16 câbles, jack, propriété de configuration,
formats, alimentation), M1b.B écrit le service d'assistance, et M1b-34 boucle enfin la
chaîne complète : `conduitctl cable add` de bout en bout.
