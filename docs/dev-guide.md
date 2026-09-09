# Guide développeur

Ce guide complète [SPEC.md](../SPEC.md) (quoi) et [ROADMAP.md](../ROADMAP.md)
(quand) : il explique **comment** le code est organisé et comment contribuer.

## 1. Vue d'ensemble

```
 applications ──▶ OS (WASAPI / PipeWire / CoreAudio) ◀──▶ conduitd ◀──▶ conduitctl, GUI
                                   ▲                       │
                             pilote de câbles         (moteur, IPC,
                             (boucle locale)           persistance)
```

Chaîne de dépendances (ADR-010) :

```
conduit-core ← conduit-backend ← conduit-protocol ← conduit-engine ← conduitd
                                        ▲
                                  conduitctl, conduit-gui
```

| Crate | Rôle | Plateforme |
|---|---|---|
| `conduit-core` | graphe, exécution, tampons, DSP, nœuds, rééchantillonnage, DLL, ports asynchrones | aucune |
| `conduit-backend` | traits `Backend`, `DeviceHandle`, `CableControl`, événements, backend `null`, priorité RT | `rt` seulement |
| `conduit-backend-pipewire` | backend Linux : fil de boucle PipeWire, registre, `pw_stream` | Linux |
| `conduit-backend-wasapi` | backend Windows : fil MMDevice, énumération, notifications | Windows |
| `conduit-protocol` | API (`Command`, `Reply`, `Notification`), enveloppe, framing, client, schéma | aucune |
| `conduit-engine` | `Engine` : périphériques, rôles, pilote, horloge interne, commandes | aucune |
| `conduitd` | configuration, IPC, service, persistance, règles, watchdog | socket / pipe |
| `conduitctl` | CLI | — |
| `conduit-testing` | allocateur de garde | — |
| `conduit-kmd-core` | logique portable du pilote Windows : positions, copie cyclique, formats | aucune |
| `conduit-com` | modèle objet COM du pilote : `ComObject`, `ComPtr`, `ComRef` | aucune |
| `portcls-sys` | bindings PortCls/KS générés, testables en mode utilisateur (`drivers/windows`, workspace noyau, ADR-012) | Windows |
| `conduit-kmd` | le pilote noyau `.sys` : WDM, `no_std`, PortCls/WaveRT (`drivers/windows`, workspace noyau ; [driver-dev.md](driver-dev.md)) | Windows |
| `conduit-looptest` | outil de test de boucle du pilote (§4 quinquies) : sinus → rendu → capture → vérification | Windows (le crate compile partout) |

## 2. Le fil audio

Tout ce qui tourne dans un rappel audio ou dans `Node::process` respecte :
**pas d'allocation, pas de verrou, pas de syscall bloquant, pas de log.**

Mécanismes qui rendent cela possible :

- **`GraphSlot` / `Executor`** (`conduit-core::slot`, `executor`) : le fil de gestion
  compile un `CompiledGraph` (tampons pré-alloués, ordre topologique) et le pousse
  dans une file SPSC ; le fil audio l'adopte au début d'un cycle en **déplaçant**
  les nœuds de l'ancienne version, et renvoie l'ancienne version pour libération
  hors temps réel. Les nœuds gardent leur état entre versions.
- **`GainParam`** : cible atomique lue par le fil audio, rampe linéaire sur un cycle.
- **`Param` / `Flag`** : paramètres atomiques des nœuds internes (sinus, EQ…).
- **`DeviceNode`** (`conduit-engine::device_node`, ADR-011) : le rôle d'un
  périphérique change par boîte aux lettres SPSC, sans recompiler.
- **`Arc<Mutex<Executor>>`** partagé entre rappel pilote et horloge interne, avec
  `try_lock` uniquement : un cycle est sauté (silence) plutôt que d'attendre.
- **Test `no_alloc`** (`conduit-core/tests/no_alloc.rs`) : l'allocateur de garde fait
  échouer le test si un cycle alloue. Ajoutez vos nœuds à ce test.

Ce qui est autorisé sur le fil audio : `Instant::now()`, atomiques, files `rtrb`,
calcul flottant (y compris `sin`, `powf` pour les coefficients).

## 3. Horloges

- Le **pilote de graphe** cadence les cycles : rappel d'un périphérique (rendu ou
  capture) ou `InternalClock` (timer, F-22).
- Tout autre périphérique est **asynchrone** : `AsyncPort` = tampon circulaire +
  `Resampler` à ratio variable + `Dll` asservissant le ratio au remplissage.
  Préremplissage jusqu'à la cible, xruns comptés, resynchronisation après xrun.
- Les tests de `conduit-core::asyncport` simulent deux horloges dérivantes avec
  gigue ; le test d'une heure est `#[ignore]` (lancer en release).

## 4. Backend `null`

`NullBackend` simule périphériques et câbles. Deux modes :

- **manuel** : `advance(Duration)` exécute les rappels dus dans l'ordre chronologique
  (déterministe, utilisé par presque tous les tests) ;
- **timer** : `start_timer()` cadence en temps réel.

Une poignée clonée (`null.clone()`) partage les périphériques : le test scripte le
backend que le moteur possède (ajout/retrait à chaud, injection de signal avec
`set_signal`, lecture de ce qui a été rendu avec `take_recorded`).

## 4 bis. Backend PipeWire (Linux)

`conduit-backend-pipewire` connecte Conduit au démon PipeWire. Un **fil dédié**
(`loop_thread`) fait tourner la boucle : rien de `pipewire-rs` n'est `Send`, donc
contexte, registre, métadonnées et `pw_stream` y vivent tous. Le reste du programme
lui parle par `pw::channel` (ouvrir, activer, fermer, quitter) et lit l'état par
`Arc<Mutex<Shared>>`. Le rappel `process` d'un flux tourne, lui, sur le fil temps
réel de PipeWire : il respecte §2 et se coordonne avec `start`/`stop` par un
automate atomique (`arrêté` / `actif` / `en cours`), si bien que `stop` ne rend la
main qu'après le rappel en cours.

Les tests d'intégration lancent un **démon headless** (`tests/common/mod.rs`,
configuration `tests/pipewire-test.conf`) : `core.daemon = true` et
`libpipewire-module-access` sont indispensables. Un flux client n'est relié et
cadencé que par un gestionnaire de session : les tests qui veulent voir le rappel
`process` lancent aussi `wireplumber`. Sans ces binaires dans le `PATH`, les tests
se sautent avec un message.

## 4 ter. Logique du pilote Windows

`conduit-kmd-core` est un crate `#![no_std]` sans `unsafe` ni dépendance, membre du
workspace racine (ADR-012) : horloge virtuelle et positions (`position`), copie
cyclique rendu → capture avec conversion F32 ↔ I16 (`ring`), validation des formats
et taille de tampon (`format`). Son code tourne à `DISPATCH_LEVEL` dans le pilote :
les lints anti-panique (`unwrap`, indexation, arithmétique débordante) sont en `deny`.
Conception et invariants : [driver-design.md](driver-design.md) §2.1 et §5.

`conduit-com` est le modèle objet COM générique du pilote, lui aussi `#![no_std]`
(+ `alloc`) sans dépendance et membre du workspace racine, mais avec de l'`unsafe`
(chaque bloc porte un `// SAFETY:`) : `ComObject<V, T>` (vtable à l'offset 0, compteur
atomique, `QueryInterface`/`AddRef`/`Release` génériques), `ComPtr` (possession Rust) et
`ComRef` (interfaces reçues de PortCls), testés en mode utilisateur à travers les
pointeurs de vtable et sous Miri. Les vtables PortCls concrètes et les traits Rust qui
les implémentent sont dans `drivers/windows/portcls` ([driver-design.md](driver-design.md) §3).

Le pilote lui-même (`portcls-sys`, `conduit-kmd`) est dans le workspace noyau
`drivers/windows`, construit uniquement sous Windows avec le WDK : installation du
poste, build, VM de test et débogage dans [driver-dev.md](driver-dev.md), outillage
`windows-drivers-rs` dans [windows-drivers-rs.md](windows-drivers-rs.md).

## 4 quater. Backend WASAPI (Windows)

`conduit-backend-wasapi` parle à l'API MMDevice depuis un **fil dédié**
(`mmdevice_thread`, calqué sur `loop_thread`) : il initialise COM en MTA (garde RAII
`ComApartment`), crée l'`IMMDeviceEnumerator`, enregistre un `IMMNotificationClient`
écrit en Rust (`notify`, macro `#[implement]` du crate `windows`) et sert les
commandes du `WasapiBackend` par `std::sync::mpsc` (`Enumerate`, `DefaultDevice`,
`Open`, `Subscribe`, `Shutdown`). Le backend lui-même ne détient que l'émetteur du canal et
la poignée du fil : il est `Send`, et sa destruction envoie `Shutdown`, désenregistre
le client puis joint le fil.

Les **rappels COM** (`OnDeviceStateChanged`, `OnDeviceAdded`, `OnDeviceRemoved`,
`OnDefaultDeviceChanged`) arrivent sur un fil choisi par Windows, éventuellement
pendant qu'une commande est en cours : ils **ne font que copier leurs arguments et
poster** une `Notification` sur le même canal. C'est le fil MMDevice qui décide :
passage à `DEVICE_STATE_ACTIVE` ou `OnDeviceAdded` → ré-énumération de ce seul
endpoint (`GetDevice`) et `DeviceEvent::Added` s'il n'était pas connu ; autre état
ou `OnDeviceRemoved` → `Removed` s'il l'était ; `OnDefaultDeviceChanged` → `DefaultChanged`
pour le rôle `eConsole` seulement ; `OnPropertyValueChanged` ignoré. Une table des
endpoints connus évite les doublons, quel que soit l'ordre des rappels.

`devices` traduit un `IMMDevice` en `DeviceInfo` : identifiant d'endpoint (`GetId`),
`PKEY_Device_FriendlyName`, `IMMEndpoint::GetDataFlow`, canaux et fréquence du
**format de mixage** du moteur (`IAudioClient::GetMixFormat`, repli
`PKEY_AudioEngine_DeviceFormat`) — c'est ce qu'un flux partagé délivre sans
conversion —, `sample_rates` = 44,1/48/96 kHz dès que l'`IAudioClient` s'active
(le mode partagé convertit automatiquement ce qui diffère du mixage), bloc par
défaut = période du moteur (`GetDevicePeriod`) en trames, `is_default` =
`GetDefaultAudioEndpoint(flow, eConsole)`, `cable` si le nom est exactement
`Conduit <n>`. Seuls les endpoints `DEVICE_STATE_ACTIVE` sont énumérés. Tout ce que
COM alloue est rendu par une garde (`CoTaskString`, `CoTaskMem`, `PropVariant`) ;
chaque bloc `unsafe` porte son `SAFETY:`.

**Flux.** `open(id, format, rappel, politique)` honore le format demandé : le rappel
reçoit exactement `format.channels` canaux `f32` entrelacés à `format.sample_rate`. Le fil
MMDevice (`open`) active l'`IAudioClient` et choisit un chemin : si la politique
demande le mode exclusif et que le matériel l'accorde, voir plus bas ; sinon, si
(fréquence, canaux) est le format de mixage et que celui-ci est float32,
`IAudioClient3::InitializeSharedAudioStream` avec la plus petite période prise en
charge ≥ `block_frames` (`choose_period` : multiple de la fondamentale, bornée à
`[min, max]` de `GetSharedModeEnginePeriod`) ; sinon `IAudioClient::Initialize`
en partagé avec `EVENTCALLBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, un
`WAVEFORMATEXTENSIBLE` float32 aux valeurs demandées et la période par défaut
(`block_frames` effectif = cette période en trames à la fréquence demandée). Un
refus du premier chemin retombe sur le second avec un client neuf. `format()` rend
le format effectif — **toujours** `f32` aux fréquence et canaux demandés, quel que
soit le mode ; `WasapiHandle::latency()` (hors trait) expose `GetBufferSize`,
la période (en trames et en durée), `GetStreamLatency` (0 chez certains pilotes en
partagé) et le chemin retenu ; `share_mode()` et `sample_type()` (hors trait aussi)
disent le mode obtenu et le format du tampon matériel.
Les interfaces du crate `windows` ne sont pas `Send` : `IAudioClient` et le
service de rendu ou de capture voyagent vers le fil du flux en `AgileReference`,
résolue là-bas (MTA des deux côtés, objets WASAPI libres de fil).

Chaque flux a **son fil** (`stream`), créé par `start()` et joint par `stop()` :
`ComApartment` MTA, `rt::promote_current_thread()` (résultat lisible par
`rt_outcome()`), puis boucle sur `WaitForMultipleObjects(arrêt, tampon, 2 s)`.
Rendu : `GetCurrentPadding` → `GetBuffer(libre)` → rappel écrivant directement
dans le tampon WASAPI vu comme `&mut [f32]` (`bytemuck::try_cast_slice_mut`,
tampon intermédiaire pré-alloué si l'alignement manquait) → `ReleaseBuffer` ; un
tampon de silence précède `Start`. Capture : tant que `GetNextPacketSize` > 0,
`GetBuffer` → rappel avec `input` (zéros pré-alloués si `SILENT`) →
`ReleaseBuffer`. Rien n'alloue ni ne verrouille dans la boucle (§2, prouvé par
`tests/no_alloc.rs` avec un allocateur comptant par fil natif). Une erreur
`AUDCLNT_E_DEVICE_INVALIDATED` (ou `RESOURCES_INVALIDATED`) fait sortir de la
boucle sans panique : `is_running()` devient faux, `stop()` rend `Disconnected`,
`start()` le refuse. `stop()` signale l'événement d'arrêt, joint le fil (qui a fait
`Stop` puis `Reset`) : aucun rappel n'est en cours au retour ; `Drop` appelle
`stop()` puis libère les objets COM sous un appartement MTA temporaire.

**Capture en écho** (`loopback`). `WasapiBackend::open_loopback(id, format, rappel)`
ouvre un endpoint de **rendu** avec `AUDCLNT_STREAMFLAGS_LOOPBACK` : le flux ne
l'alimente pas, il **prélève le mélange** que le moteur audio vient d'y écrire,
avant que le pilote du périphérique ne le consomme. C'est un `IAudioCaptureClient`
que `GetService` rend, et le rappel reçoit des trames d'entrée ; la poignée garde le
`DeviceInfo` de l'endpoint de rendu, et `latency().path` est un `InitPath::Loopback`.
Méthode **hors du trait `Backend`**, comme `set_exclusive_policy` : le trait est
portable et ne doit pas gagner une notion Windows.

L'écho n'existe **qu'en mode partagé** — `Initialize` refuse l'indicateur en
`AUDCLNT_SHAREMODE_EXCLUSIVE`, et c'est cohérent : un flux exclusif court-circuite
précisément le moteur dont l'écho prélève le mélange, il n'y aurait rien à prendre.
La combinaison est refusée **avant** tout appel COM (`check_shared`), tout comme un
écho demandé sur un endpoint de capture (`check_render`). Deux chemins de format,
calqués sur le partagé ordinaire : format demandé = format de mixage (float32, mêmes
canaux, même fréquence) → `Initialize` avec le bloc de `GetMixFormat` et les seuls
`EVENTCALLBACK | LOOPBACK`, aucune conversion ; sinon `AUTOCONVERTPCM |
SRC_DEFAULT_QUALITY` en plus, et un refus nomme le format de mixage à demander.
Période par défaut du moteur : `InitializeSharedAudioStream` ne prend pas
d'indicateur d'écho, la basse latence n'est pas disponible ici.

*À quoi cela sert.* C'est l'outil qui **coupe en deux une chaîne muette**, et il vaut
d'abord pour le pilote : sur un câble virtuel dont la capture n'entend pas le rendu,
il tranche ce qu'aucune inspection du pilote ne tranche. Signal entendu en écho → le
moteur délivre, le défaut est en aval, dans l'échange de données du pilote. Écho
silencieux → rien n'arrive jusqu'au pilote, qui est hors de cause, et le défaut est
en amont (volume de l'endpoint, coupure, format par défaut, mode exclusif tenu par un
autre programme). Deux réserves à garder en tête : l'écho prélève **tout** le
mélange, donc aussi ce que jouent les autres applications, et le **volume de
l'endpoint s'y applique** (mesuré sur le poste : un sinus demandé à 0,05 ressort à
0,034 sur la sortie HDMI G27QC) — une amplitude basse en écho ne condamne personne.
`tests/loopback.rs` exerce les trois cas sur les cartes de la machine ; le premier
**émet un son** (1,5 s à 2 % d'amplitude sur le rendu par défaut) parce qu'on ne peut
pas prouver qu'un écho prélève un mélange sans rien mélanger.

**Volume d'un endpoint** (`volume`). `EndpointVolumeControl::{new, read, set_scalar,
set_mute}` lit et écrit le **volume maître scalaire** (0 à 1, la position du curseur
du mélangeur — pas des décibels) et la **coupure** d'un endpoint désigné par son
identifiant. L'interface s'obtient par `IMMDevice::Activate::<IAudioEndpointVolume>`,
comme l'`IAudioClient`. Hors du trait `Backend`, pour la même raison que l'écho et le
mode exclusif. L'objet possède **son** appartement COM (MTA) et **son**
`IMMDeviceEnumerator` : il ne passe pas par le fil MMDevice, sert donc sans backend
ouvert, n'est ni `Send` ni `Sync`, et l'ordre de ses champs garantit que l'énumérateur
meurt avant le `CoUninitialize`. Un endpoint **sans mélangeur** refuse l'activation
par `E_NOINTERFACE` ou répond `E_NOTIMPL` : ces deux `HRESULT` deviennent `Ok(None)`,
et non une erreur — c'est une réponse (« pas de contrôle ici »), pas une panne, et la
distinction évite d'accuser un volume qu'on n'a pas vu. `EndpointVolume::clamp_scalar`
borne l'écriture à `[0, 1]` (`NaN` → 0) plutôt que de laisser `E_INVALIDARG`
remonter : un curseur ne va pas au-delà de ses butées. Les setters **relisent** après
écriture, parce que Windows range la valeur sur les crans du périphérique.
`tests/volume.rs` l'éprouve sur la carte réelle de la machine et **restaure la valeur
initiale par une garde `Drop`** ; la coupure n'y est jamais changée, seulement
réécrite à sa propre valeur. Un volume à zéro ou une coupure explique à elle seule
toute chaîne muette, écho compris : c'est la première hypothèse à écarter avant
d'accuser le pilote, et `conduit-looptest` s'en sert (§ 4 quinquies).

**Session Windows** (`session`). `current_session_id()` rend la session du processus
(`ProcessIdToSessionId`), `SERVICES_SESSION` vaut 0. Ce n'est pas de l'audio, c'en est
la condition : le moteur audio appartient à une session ouverte par un utilisateur, et
un processus de la session 0 (service, tâche planifiée « même si l'utilisateur n'est
pas connecté », agent d'exécution à distance) n'y voit pas les périphériques et ne
joue nulle part. Le renseignement vit ici plutôt que dans l'outil, qui interdit
l'`unsafe` et n'a pas à ouvrir Win32 pour lui seul.

**Mode exclusif** (`exclusive` + `convert`, M1b-32). En exclusif, le flux **prend le
périphérique pour lui seul** : le moteur audio de Windows est court-circuité, plus
aucune autre application n'y joue. C'est pourquoi c'est un **opt-in par backend**,
jamais imposé : `WasapiBackend::set_exclusive_policy(ExclusivePolicy)` / `exclusive_policy()`,
avec `Never` (**défaut**), `Preferred` (tenter, retomber en partagé sinon) et
`Required` (échouer plutôt que rendre un flux partagé). Le réglage est délibérément
**hors du trait `Backend`**, qui est portable et ne doit pas gagner une notion
Windows. Le défaut reste le partagé parce qu'un câble Conduit doit coexister avec le
reste du système : SPEC §5.6 fait du mode exclusif un bonus de latence, pas la norme.
Un repli garde sa raison sur la poignée (`WasapiHandle::exclusive_refusal()`) — ce
crate n'a pas de dépendance de traçage, c'est à l'appelant de la journaliser ; un
refus en `Required` devient une `BackendError::UnsupportedFormat` qui nomme la cause
(« un autre programme utilise déjà ce périphérique en mode exclusif », « le mode
exclusif est désactivé pour ce périphérique : Paramètres > Son > Propriétés > Avancé »,
« format refusé »), son `HRESULT`, et dit quoi faire.

*Négociation.* Pas d'`AUTOCONVERTPCM` en exclusif : le format passé à `Initialize`
est celui que le convertisseur reçoit. `exclusive::try_exclusive` propose donc à
`IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, …)`, aux **fréquence et canaux
demandés**, un `WAVEFORMATEXTENSIBLE` float32, puis PCM 24 dans un conteneur 32
(`wBitsPerSample = 32`, `wValidBitsPerSample = 24`), puis PCM 24 compacté, puis
PCM 16 — le matériel exclusif n'accepte souvent que l'entier. Une proposition du
pilote (`S_FALSE` + format suggéré ; Microsoft documente `*ppClosestMatch` toujours
nul en exclusif, on la gère quand même) n'est retenue que si elle garde la fréquence
et le nombre de canaux : les changer reviendrait à ne pas honorer le format.

*Période et alignement.* `GetDevicePeriod` donne la période **minimale**, passée en
`hnsBufferDuration` **et** `hnsPeriodicity` (l'événementiel exclusif exige les deux
égales). Si le pilote répond `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` — le rite de passage
du mode exclusif —, on lit la taille alignée qu'il vient de fixer (`GetBufferSize`),
on en déduit la période (`aligned_period_hns`, la formule de Microsoft calculée en
entiers ; une propriété vérifie qu'elle redonne exactement le nombre de trames par
`frames_from_period`), on **libère et recrée** l'`IAudioClient` — obligatoire, un
client dont l'initialisation a échoué n'est pas réutilisable — et on réinitialise.
Une seule reprise, puis abandon. *Aucune des cartes du poste n'a eu besoin de cette
reprise* : ce chemin n'est couvert que par ses tests unitaires.

*Boucle.* Deux différences dans le fil du flux (`stream`, `Worker::whole_buffer`).
Un réveil donne accès au **tampon entier** : en exclusif événementiel
`GetCurrentPadding` rend toujours la taille du tampon (donc « zéro trame libre »,
ce qui rendrait le flux muet — c'est le piège) et `GetNextPacketSize` ne s'applique
pas ; on fait donc un `GetBuffer(buffer_frames)` par réveil, et le préremplissage
initial est le tampon complet. Et la **conversion est à notre charge** : le rappel
écrit dans le tampon intermédiaire `f32` déjà alloué à l'ouverture, que
`convert::write_f32` verse dans le tampon WASAPI (`convert::read_f32` en capture).
Échelle `v × 2^(n−1)`, arrondi au plus proche, écrêtage à `[−2^(n−1), 2^(n−1) − 1]`,
`NaN` → 0 ; l'aller-retour tient dans un pas de quantification et le retour ne sort
jamais de `[−1, 1]`. PCM 24 dans 32 : les 24 bits significatifs sont **alignés à
gauche**, l'octet de poids faible nul. C'est une convention **différente** de celle
de `conduit_kmd_core::ring` (±32 767) et ce crate n'en dépend pas : `ring::copy_frames`
est une copie *cyclique* du pilote, limitée à F32/I16 et 8 canaux, sans PCM 24 ; et
faire dépendre un backend utilisateur de la logique du pilote noyau pour deux
fonctions scalaires inverserait les couches. Rien n'alloue : `tests/no_alloc.rs`
couvre le cas exclusif, conversion comprise.

*Ce que le matériel de test accepte.* Les cinq endpoints de rendu du poste
(2026-09-06, `tests/exclusive.rs`) accordent tous l'exclusif en 48 kHz stéréo, et
**aucun** n'accepte le float32 : Realtek Digital Output (S/PDIF), G27QC A et E2351
(HDMI NVIDIA) prennent du **PCM 24 dans un conteneur 32**, l'Audeze Maxwell (USB,
sorties Chat et Game) du **PCM 24 compacté**. Tous donnent la même géométrie :
tampon et période de **144 trames (3 ms, 30 000 × 100 ns)** contre **1 056 trames
(22 ms)** de tampon et 480 trames (10 ms) de période en partagé — soit **3 ms au
lieu de 22 ms**, sept fois moins, sans réalignement du tampon. `GetStreamLatency`,
qui rend 0 en partagé sur ces pilotes, rend 3 ms en exclusif. Tout ce matériel est
**numérique** (S/PDIF, HDMI, USB) : le comportement d'une carte analogique reste à
vérifier, en particulier le chemin de réalignement et le PCM 16.

**Horloge** (`clock`, M1b-33). À l'ouverture, `IAudioClient::GetService(IAudioClock)`
et `GetFrequency`. Microsoft ne fixe pas l'unité de cette fréquence, seulement
qu'elle est celle de la position : `ClockScale::new` la classe (`ClockUnits`) —
`freq == mix_rate` ou `== sample_rate` → trames/s ; `freq == mix_rate × nBlockAlign`
du mixage → octets/s du mixage (**observé** sur les deux cartes du poste par le
chemin `IAudioClient3` : 384 000 pour 48 kHz stéréo float32) ; `freq == sample_rate
× channels × 4` → octets/s du format livré (**observé** par le chemin conversion :
176 400 en 44,1 kHz mono) ; `freq == sample_rate × nBlockAlign` du **format
matériel** → octets/s de celui-ci (cas ajouté en M1b-32 pour un tampon exclusif
entier ; non rencontré) ; sinon « autre » — et convertit toujours par le rapport
générique `position × sample_rate / freq` (128 bits), exact dans tous les cas :
`ClockInfo::position` est en **trames du format livré** au rappel. En **mode
exclusif** les cinq cartes du poste comptent en trames/s (48 000) et non plus en
octets : sans moteur audio entre le pilote et nous, l'unité change — la conversion
générique absorbe la différence sans rien changer d'autre. À chaque rappel,
**avant** de toucher au tampon, `IAudioClock::GetPosition(&pos, &qpc)` :
`position` = `pos` converti, jamais décroissante ; `timestamp_ns` = `qpc × 100`
(le compteur de performance en unités de 100 ns : base **QPC commune** à tous les
flux du processus, ce qui permet de comparer deux cartes — mesuré : 43 µs d'écart
entre deux lectures immédiates sur deux cartes) ; `frames` = trames du rappel. En
rendu, `pos` est la position de **lecture** du matériel, en retard sur ce qu'on
écrit : la latence estimée `write_ahead_frames` = trames écrites − position lue
(≈ 958 trames pour un tampon de 1 056 sur ce poste) est exposée par
`WasapiHandle::latency()` et `write_ahead_frames()` (atomique) ; en capture, c'est
position d'écriture − trames livrées. Microsoft ne documente pas `GetPosition`
comme sûr en temps réel ; en mode partagé il lit une section partagée avec le
moteur audio, et on le mesure sur le fil (`Instant`, autorisé §2) : ≈ 1 µs en
moyenne, 6 µs au pire sur 6 000 appels (`WasapiHandle::clock_stats()`). Si
`GetPosition` échoue ponctuellement, la position est extrapolée (dernière + trames
du rappel précédent), l'horodatage vient de `QueryPerformanceCounter`, et l'échec
est compté. Sans `IAudioClock` (`ClockSource::Counter`, visible par
`clock_source()`), `position` = trames livrées depuis `start()`, toujours
horodatées QPC. `clock()` (trait) rend la dernière `ClockInfo` publiée par le
rappel ; `clock_now()` (hors trait, alloue) interroge `IAudioClock` immédiatement
depuis le fil appelant. Le moteur, lui, n'exploite pas encore `ClockInfo` : sa DLL
est asservie au remplissage du port asynchrone (§3), et c'est ainsi qu'il absorbe
la dérive mesurée entre les deux cartes (−18 ppm, `tests/two_devices.rs`).

**Démon.** `conduitd` charge ce backend par défaut sous Windows depuis M1b-31 :
`--backend auto` (la valeur par défaut) appelle `WasapiBackend::new()` sous
`cfg(windows)` ; `--backend wasapi` le demande explicitement. Si le fil MMDevice ne
démarre pas (COM, service audio arrêté), l'erreur est journalisée et le démon se
replie sur le backend `null` plutôt que de refuser de démarrer (F-51) — `conduitctl
status` montre alors `backend null`. Le pilote de graphe suit la logique habituelle
(`engine.driver` de la configuration ; en `auto`, le périphérique de rendu par
défaut `eConsole`, sinon une capture, sinon l'horloge interne). Le
test `conduitd_binary_auto_backend_is_wasapi_on_windows` (`crates/conduitd/tests/binary.rs`)
lance le binaire et vérifie que le graphe contient chaque endpoint énuméré ; il se
saute si WASAPI est indisponible ou qu'aucune carte n'est active.

**Câbles.** Depuis M1b-34, `conduitd` relie le dorsal au service d'assistance : au
démarrage il appelle `WasapiBackend::set_cable_control` avec
`conduit_helper::controle::ControleCables`, puis interroge le canal une fois et
journalise « service d'assistance absent : les câbles ne pourront pas être activés »
plutôt que de laisser l'erreur surgir au premier ordre. L'injection vient du démon et
non du dorsal parce que `conduit-helper` dépend déjà de `conduit-backend-wasapi` pour
le transport KS : l'inverse ferait un cycle entre paquets. `create` **active** le
premier câble libre de la réserve fixe de seize (SPEC §5.4) et rend l'état existant si
le câble visé est déjà actif ; `remove` **désactive** ; `rename` écrit le nom de
l'endpoint dans le registre (M1b-21, voir ci-dessous) ; `set_channels` propage le refus
du pilote, qui n'applique que deux canaux tant que M1b-05 n'est pas faite. Le bout en
bout se vérifie en machine virtuelle, service installé et pilote chargé :

```powershell
conduitctl cable add          # -> câble 1 « Conduit 1 » 2 : rendu …, capture …
conduitctl cable list
conduitctl cable remove 1
```

### Renommer un endpoint (M1b-21)

Le nom affiché d'un endpoint audio ne vient pas du pilote : il vit dans
`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\{Render|Capture}\{id}\Properties`,
dans la valeur `{a45c254e-df1c-4efd-8020-67d146a850e0},2` — `PKEY_Device_DeviceDesc`,
en `REG_SZ`. Écrire sous `HKLM` demande d'être `LocalSystem`, donc c'est le service qui
le fait ; **le pilote n'est pas touché**, ce qui est l'application directe du principe
« tout ce qui peut être fait hors du noyau y est fait ». Le détail de ce qui l'établit,
la façon de retrouver un câble déjà renommé et le retour au nom d'origine sont dans
l'en-tête de module de `crates/conduit-helper/src/registre.rs`.

Le câble doit être **connecté** : Windows ne publie ses endpoints — et donc leurs clés —
qu'à ce moment-là.

```powershell
conduit-helper activer 1
conduit-helper renommer 1 Musique
conduit-helper nom-defaut 1     # rend « Conduit 1 » et efface le nom personnalisé
```

et par le démon, depuis une session ordinaire :

```powershell
conduitctl cable rename 1 Musique
conduitctl cable rename 1 "Conduit 1"   # le retour en arrière (F-52)
conduitctl cable add --name Musique     # active le premier câble libre, puis le renomme
```

Pour voir ce que le service voit dans le registre — le premier renseignement à demander
quand un renommage n'a pas l'effet attendu :

```powershell
cargo test -p conduit-helper --test registre -- --ignored --nocapture
```

Ce test est en **lecture seule** : il n'écrit rien et ne renomme aucun endpoint.

Ce qui manque encore : l'exposition du mode exclusif dans la configuration du démon
(il ouvre tout en partagé pour l'instant).
Les tests d'intégration (`tests/wasapi.rs`, `tests/stream.rs`, `tests/no_alloc.rs`,
`tests/exclusive.rs` — chaque carte de rendu en partagé puis en exclusif, résultat
imprimé, latences comparées —, `tests/two_devices.rs` — 60 s sur deux cartes de
rendu, dérive imprimée) tournent sur les cartes son de la machine, en silence ;
ceux qui demandent un périphérique absent se sautent avec un message. À lancer à la main : le critère « casque USB
branché → `Added` »
(`cargo test -p conduit-backend-wasapi --test wasapi -- --ignored --nocapture`), le
sinus audible
(`cargo test -p conduit-backend-wasapi --test stream sine_audible -- --ignored --nocapture`)
et l'heure d'endurance avec le moteur (une carte pilote, l'autre asynchrone,
xruns = 0 ; `CONDUIT_ENDURANCE_SECS` pour raccourcir)
(`cargo test -p conduit-backend-wasapi --test two_devices -- --ignored --nocapture`).

## 4 quinquies. Outil de test de boucle

`conduit-looptest` (M1a-10) répond à une seule question : **ce que le pilote rend
ressort-il intact de la capture ?** Il joue un sinus sur un endpoint de rendu,
enregistre l'endpoint de capture, et mesure. Par défaut les deux endpoints sont les
deux côtés du câble `Conduit 1` : c'est le pilote qui est testé, pas la carte son.
Il ne réécrit pas WASAPI, il appelle `conduit-backend-wasapi`
(`WasapiBackend::{new, devices, open, open_loopback}`, `DeviceHandle::{start, stop}`).

```sh
cargo run -p conduit-looptest -- --list                    # les endpoints vus par WASAPI
cargo run -p conduit-looptest -- --repeat 10               # le critère de la ROADMAP
cargo run -p conduit-looptest -- --json --repeat 10        # sortie machine
cargo run -p conduit-looptest -- --self-test               # test de l'outil, sans périphérique
cargo run -p conduit-looptest -- --loopback                # écho : le moteur délivre-t-il ?
cargo run -p conduit-looptest -- --list --show-volume      # les endpoints, volume et coupure compris
cargo run -p conduit-looptest -- --set-volume 0.5 --unmute # règle --render/--capture, affiche, sort
```

**Le volume est vérifié avant chaque mesure.** Un endpoint coupé ou à zéro rend la
boucle muette, et ce silence-là est indiscernable d'un pilote en panne : c'est une
panne banale, invisible depuis le pilote, et elle a déjà coûté une journée. L'outil
relève donc le volume et la coupure des endpoints qu'il va utiliser (§ 4 quater,
`EndpointVolumeControl`), et **prévient si l'un est à zéro ou coupé** en nommant
l'option qui corrige — un avertissement qui laisse chercher ne vaut pas mieux que pas
d'avertissement. L'avertissement part sur la **sortie d'erreur**, pour rester visible
en `--json` ; il n'interrompt rien (l'utilisateur peut vouloir mesurer quand même) et
une lecture qui échoue devient un texte, jamais une erreur : un diagnostic ne doit pas
empêcher la mesure qu'il commente. `--show-volume` affiche le relevé même quand tout
va bien, et détaille chaque endpoint de `--list`. `--set-volume <0..1>` et `--unmute`
règlent les endpoints de `--render` et `--capture`, affichent l'état **avant et
après**, puis sortent : régler un volume ne doit pas avoir pour effet de bord
d'émettre du son. Les deux sont refusés avec `--list` (qui n'énumère) et `--self-test`
(qui n'ouvre aucun endpoint).

Le diagnostic imprimé quand **aucun signal** n'est trouvé (`pass::Unusable::NoSignal`,
typé pour cette raison : un enregistrement trop court est un réglage à corriger, pas
un silence à expliquer) ajoute la **session Windows** du processus. Un test lancé dans
la session des services (session 0) n'a pas l'audio de l'utilisateur : il ne mesure
rien, et tout ce qu'il conclurait du pilote serait faux. C'est un piège qui ne se voit
nulle part ailleurs dans la sortie, et dans lequel le projet est tombé une journée
entière.

`--loopback` répond à **l'autre moitié** de la question, celle qu'on oublie de poser
quand une boucle est muette : *le moteur audio délivre-t-il seulement quelque chose
vers l'endpoint ?* Au lieu d'ouvrir l'endpoint de capture, l'outil ouvre celui de
`--render` en **écho** (§ 4 quater) et prélève le mélange **avant** le pilote. Le
sinus retrouvé met le moteur hors de cause et laisse le pilote seul suspect ; un écho
silencieux fait exactement l'inverse. Le message de fin dit laquelle des deux
conclusions s'applique — c'est là toute la valeur du mode, un chiffre seul ne
diagnostiquerait rien. Trois conséquences pratiques : le flux prend le **format de
mixage** de l'endpoint (ni `--rate` ni `--channels` ne s'appliquent, l'outil le dit),
l'écho s'arrête **avant** le rendu (sans quoi la queue de silence de l'arrêt
compterait comme un trou), et le mélange contient ce que jouent les autres
applications — à fermer pour une mesure propre. `--loopback` est incompatible avec
`--capture`, `--no-capture` et `--self-test`.

Options utiles : `--render`/`--capture` (identifiant exact ou fragment de nom, ou
`none`), `--freq`, `--rate`, `--channels`, `--seconds`, `--block`, `--amplitude`,
`--skip-ms` (marge jetée après la détection du signal), `--phase-tolerance`,
`--no-capture` (joue seulement), `--loopback` (capture en écho), `--show-volume`,
`--set-volume`, `--unmute`. **Codes de retour** : `0` toutes les passes
passent, `1` au moins une échoue (le pilote est en cause), `2` l'environnement ne
permet pas le test (endpoint absent, backend indisponible, options incohérentes,
système autre que Windows) — c'est la distinction qui compte en CI.

Le module `analysis` est le cœur, et il ne dépend d'aucune plateforme :

- **fréquence** : ajustement au sens des moindres carrés de `a·cos(ωn) + b·sin(ωn)`,
  sans FFT ni nouvelle dépendance — corrélations en un passage, sommes `Σcos²` et
  compagnie en forme close, maximum cherché par raffinements successifs autour de
  la fréquence attendue (±5 %) puis section dorée. Sur signaux synthétiques l'écart
  mesuré est inférieur à 0,001 ppm ; la tolérance du verdict est 200 ppm ;
- **continuité de phase** : le même ajustement par blocs de 128 trames, avec une
  référence de temps globale et la dérive lente retirée (médiane des écarts). Une
  trame perdue ou dupliquée décale tout ce qui suit de `ω` radians — 0,058 rad
  seulement à 440 Hz et 48 kHz : le seuil effectif est donc `min(--phase-tolerance,
  max(0,005, 0,3·ω))`, pas la tolérance brute. C'est **le** détecteur de
  discontinuité de la boucle ;
- **trous** : suites d'au moins 8 trames sous −80 dBFS ;
- **amplitude**, **écrêtage** (`|x| ≥ 0,999`) et **THD+N**.

Verdict : un refus par critère, avec le chiffre mesuré, le seuil et ce qu'il faut
regarder (« trames perdues ou dupliquées dans la boucle », « sous-alimentation du
tampon »…). Le rappel de capture n'alloue ni ne verrouille : l'enregistrement vit
dans un tableau d'`AtomicU32` réservé à l'ouverture, écrit par index, avec un
compteur atomique de trames — la seule façon d'écrire depuis un fil temps réel sans
`unsafe`.

`--self-test` (caché) teste l'outil lui-même : il fabrique en mémoire ce qu'une
boucle parfaite rendrait (préambule silencieux puis sinus) et le fait passer par la
même analyse ; `--inject-glitch [trame]` en retire une trame et doit faire sortir
en 1. Les tests d'intégration `tests/binary.rs` lancent ces trois cas sur la machine
de développement, sans pilote. Un seul y **émet un son** : celui qui valide
`--loopback` sur le rendu par défaut (1,5 s à 2 % d'amplitude) — il vérifie que le
sinus est bien retrouvé dans l'écho et que l'outil en tire la bonne conclusion, pas
le verdict de la passe, puisque le mélange peut contenir autre chose.

## 4 sexies. Cycle de vie du démon

Trois questions, une réponse chacune : *qui tient le point de contrôle ?*, *comment le
démon apprend-il que la session se ferme ?*, *qui le démarre ?* (ADR-013, M1b-35).

### Instance unique

Le point de contrôle **est** le verrou : pas de fichier `.pid`, qui peut mentir (PID
recyclé, fichier oublié après un arrêt brutal). `conduitd::single_instance::check` pose
toujours la même question — « quelqu'un répond-il là où j'allais écouter ? » — avant
d'ouvrir le backend audio :

- **Unix** : le fichier de socket existe et une connexion aboutit → un démon vit derrière ;
  la connexion échoue → le fichier est **orphelin**, on le supprime et on continue.
- **Windows** : un named pipe n'existe que tant qu'un processus le détient, il n'y a donc
  pas d'orphelin. L'ouverture réussit (quelqu'un écoute) ou échoue avec
  `ERROR_FILE_NOT_FOUND`. `ERROR_PIPE_BUSY` (231) et `ERROR_ACCESS_DENIED` (5) valent
  « nom déjà tenu » : ce sont exactement les erreurs que `first_pipe_instance` rendrait.

Un démon de trop sort en **3** avec « un démon Conduit est déjà en cours pour cette
session… ». La course des deux démons lancés en même temps est rattrapée par
`ipc::Listener::bind`, qui rend `ErrorKind::AddrInUse` dans ce cas — même verdict, même
code de retour.

Il n'y a **pas** de `--replace` : le protocole n'a aucune commande d'arrêt
(`conduit_protocol::Command` va de `Status` à `Load`, sans `Shutdown`), donc rien ne
permet de demander poliment au démon en place de partir. L'ajouter n'est pas une ligne de
code mais une version de protocole (ADR-010 : le protocole est la source de vérité de
l'API), avec sa question de sécurité — n'importe quel client du socket pourrait couper
l'audio. À trancher ailleurs ; d'ici là, on arrête le démon existant par le système.

### Fin de session Windows

Le réflexe — `SetConsoleCtrlHandler` et `CTRL_LOGOFF_EVENT` — ne marche pas ici, et la
documentation le dit : « *this signal is received only by services. Interactive
applications are terminated at logoff* »
([HandlerRoutine](https://learn.microsoft.com/en-us/windows/console/handlerroutine)).
`conduitd` est justement une application interactive, et lancé par une tâche planifiée il
n'a de toute façon pas de console.

Le second réflexe — une fenêtre `HWND_MESSAGE` — ne marche pas non plus : « *A
message-only window […] does not receive broadcast messages* »
([Window Features](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features)),
et `WM_QUERYENDSESSION` est diffusé aux fenêtres **de premier niveau**.

`conduitd::session_end` crée donc, sur un fil dédié, une fenêtre de **premier niveau
jamais montrée** (`WS_OVERLAPPED` sans `WS_VISIBLE`, `WS_EX_TOOLWINDOW` pour rester hors
d'Alt+Tab) et pompe ses messages : `WM_QUERYENDSESSION` → `TRUE` immédiatement (on ne
bloque jamais une fermeture de session), `WM_ENDSESSION` → réveil du démon puis **attente
bornée** (3 s, `session_end::GRACE`) de l'accusé de réception, parce que le système peut
tuer le processus dès le retour de la procédure de fenêtre. `ShutdownBlockReasonCreate`
n'est pas utilisé : il sert à *retarder* l'arrêt en affichant une raison, ce que Microsoft
réserve aux travaux ininterruptibles, alors que notre sauvegarde est l'écriture atomique
d'un petit JSON (déjà refaite toutes les 500 ms par le service).

C'est le seul `unsafe` du démon (§6). Sous Unix, `SIGTERM` et Ctrl-C restent le chemin
d'arrêt, inchangés.

### Autodémarrage

`conduitd autostart <enable|disable|status>` gère la **tâche planifiée par utilisateur**
`Conduit\conduitd` : déclencheur « à l'ouverture de session de cet utilisateur » avec 30 s
de délai, `InteractiveToken` / `LeastPrivilege` (aucune élévation, SPEC §5.10),
`StopExisting`, pas d'arrêt sur batterie, pas de limite de durée, trois redémarrages
espacés d'une minute, `Priority` **5** — le défaut du Planificateur (7) donnerait
`BELOW_NORMAL_PRIORITY_CLASS` à un démon audio.

L'implémentation appelle `%SystemRoot%\System32\schtasks.exe` (chemin absolu, jamais le
`PATH`) avec un fichier XML produit par `autostart::xml` et encodé en UTF-16LE avec BOM.
Le XML plutôt que l'API COM du Planificateur : c'est le **format natif** de l'outil, celui
que son interface exporte et réimporte, donc un artefact lisible par l'utilisateur et
testable sans Windows — là où la même définition construite en COM n'existerait nulle
part sous forme inspectable, et coûterait une centaine de lignes d'`unsafe` de plus. Le
prix payé est la lecture d'une sortie **traduite** pour `status` : on la restitue telle
quelle plutôt que de l'interpréter.

Codes de retour : `0` fait (`status` : présente), `1` `schtasks` a échoué, `2` plateforme
sans tâche planifiée (Linux/macOS : le message renvoie vers systemd `--user` ou un
LaunchAgent, M4/M5), `4` `status` : absente. `disable` sur une tâche absente rend `0`
(la désinstallation doit pouvoir l'appeler deux fois). `--task-name` (caché) vise une
tâche jetable : c'est ce dont se servent les tests, qui n'approchent jamais la vraie tâche
de l'utilisateur. Le guide utilisateur est [user/windows.md](user/windows.md).

## 5. Tests

```sh
cargo test --workspace --all-features           # tout (Linux : libpipewire-0.3-dev)
cargo test -p conduit-core --lib asyncport      # un module
cargo test --release -p conduit-core -- --ignored input_port_one_hour
cargo bench -p conduit-core                     # criterion
cargo +nightly miri test -p conduit-core --lib -- ring:: graph:: executor::
cargo +nightly miri test -p conduit-kmd-core --all-features --lib
cargo +nightly miri test -p conduit-com --all-features
cargo +nightly fuzz run decoder                 # depuis crates/conduit-protocol
cargo deny check
cargo run -p conduit-protocol --features schema --example gen-docs   # docs/protocol.md
```

Niveaux : unitaires par module ; intégration `conduit-engine/tests/scenarios.rs`
(F-11 à F-22 enchaînés) ; bout en bout `conduitd/tests` et `conduitctl/tests`
(démon en processus + client IPC, et vrais binaires).

### Fuzzing (M0-74, M1b-08)

Six cibles `cargo-fuzz`, réparties en trois crates `fuzz/`, chacun **à côté du crate
qu'il éprouve** — c'est la convention de `cargo-fuzz`, qui se lance depuis le répertoire
du crate cible, et ce que le dépôt faisait déjà pour `conduit-protocol` :

| Crate cible | Cible | Ce qu'elle éprouve |
|---|---|---|
| `conduit-protocol` | `decoder` | le décodeur de trames du protocole utilisateur |
| `conduit-protocol` | `config` | l'analyseur de configuration TOML de `conduitd` |
| `conduit-kmd-core` | `etat-cable` | `CableState::from_bytes`, le parseur de la propriété KS |
| `conduit-kmd-core` | `parametres-registre` | `decode_dword` puis `sanitize`, la lecture des paramètres |
| `conduit-kmd-core` | `formats` | `validate`, `buffer_bytes`, `buffer_bytes_for_notifications` |
| `conduit-helper` | `protocole` | `decouper`, `Requete::from_bytes`, `Reponse::from_bytes` |

```sh
cargo install cargo-fuzz            # ou : nix develop .#nightly
cd crates/conduit-kmd-core && cargo +nightly fuzz run etat-cable
cd crates/conduit-kmd-core && cargo +nightly fuzz run parametres-registre
cd crates/conduit-kmd-core && cargo +nightly fuzz run formats
cd crates/conduit-helper   && cargo +nightly fuzz run protocole
cargo +nightly fuzz run <cible> -- -max_total_time=3600      # campagne bornée
cargo +nightly fuzz run <cible> fuzz/artifacts/<cible>/<fichier>   # rejouer un plantage
```

**Sous Windows**, la bibliothèque d'exécution d'AddressSanitizer est une DLL du MSVC qui
n'est pas dans le `PATH` par défaut : sans elle le binaire s'arrête sur
`STATUS_DLL_NOT_FOUND` (`0xc0000135`) avant d'exécuter la moindre entrée.

```powershell
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\<version>\bin\Hostx64\x64;$env:PATH"
```

**Corpus.** Chaque cible a un corpus de départ versionné sous `fuzz/corpus/<cible>/`,
semé depuis les `to_bytes` des cas valides et des refus « à un octet près » (bornes
franchies, mauvais type de valeur de registre, 24 bits sur 32, nom de câble maximal). Ces
graines sont curées et nommées : pour ne pas les noyer sous ce qu'une campagne découvre,
donner à libFuzzer un répertoire de sortie jetable **avant** le corpus versionné, qu'il
ne lira alors qu'en entrée.

```sh
cargo +nightly fuzz run protocole /tmp/campagne-protocole fuzz/corpus/protocole -- -max_total_time=1200
```

**Ce que les cibles vérifient, au-delà de l'absence de panique** : l'aller-retour (ce qui
est accepté se resérialise à l'octet près en l'entrée reçue — c'est ce qui interdit
d'accepter un préfixe valide suivi d'octets en trop), les domaines annoncés, et pour
`sanitize` l'idempotence. Aucune borne n'est écrite en dur dans les cibles : elles lisent
les constantes du crate, et survivent donc à un déplacement de domaine.

**Où l'effort compte le plus** : dans `conduit-kmd-core`, la panique est interdite *par
construction* (`clippy::panic`, `unwrap_used`, `indexing_slicing`,
`arithmetic_side_effects`, `#![no_std]` sans `alloc`), et le fuzz y a donc valeur de
**preuve** plutôt que de découverte. La cible `protocole` est l'inverse : elle **alloue**,
a une longueur variable depuis la version 2 du protocole, et son parseur tourne dans un
processus `LocalSystem` dont le canal nommé est ouvert au groupe `INTERACTIVE`.

Les crates `fuzz/` sont **hors du workspace racine** : `libfuzzer-sys` n'est
constructible qu'en nightly. Le `exclude` du `Cargo.toml` racine ne suffit pas pour un
crate logé **sous** un membre — cargo ne l'y honore pas, et `cargo fuzz` échoue alors sur
« current package believes it's in a workspace when it's not » ; c'est le `[workspace]`
vide de chaque `fuzz/Cargo.toml` qui les affranchit. Comme Miri, le fuzz n'est pas un
check du flake : il exige le shell nightly (§7).

## 6. Conventions

- Commits : Conventional Commits, scopes de ROADMAP (`core`, `engine`, `backend`,
  `null`, `protocol`, `daemon`, `cli`, `nix`, …). Une tâche = un commit, CI verte.
- `unsafe` : interdit (`#![forbid(unsafe_code)]`) sauf `conduit-testing` (allocateur),
  `conduit-backend::rt` (appels système), `conduit-backend-wasapi` (appels COM) et le seul
  module `conduitd::session_end` (fenêtre Win32 de fin de session, `cfg(windows)` — le
  crate est donc en `deny` plutôt qu'en `forbid`, avec un `#![allow]` local et motivé dans
  ce module ; le binaire, lui, reste en `forbid`). Chaque bloc est commenté `SAFETY:`
  (lint `undocumented_unsafe_blocks` du workspace).
- Messages d'erreur : dire quoi faire (ADR-006). Codes stables dans
  `ProtocolError`.
- Identifiants : `NodeId`/`LinkId` sont générationnels (jamais réutilisés) ; la
  persistance et les règles utilisent `NodeKey` (stable).
- Toute déviation de SPEC produit une ADR dans `docs/adr`.

## 7. Nix

`nix develop` donne l'environnement de référence ; `nix flake check` lance format,
clippy, tests, cargo-deny, doc, vérification de `docs/protocol.md` et compilation
croisée Windows. `nix build .#conduitd` produit le binaire Linux/macOS.
`nix develop .#nightly` fournit Miri et cargo-fuzz : `cargo miri test -p conduit-core --lib`.
Ni Miri ni le fuzz ne sont des checks du flake : la construction du sysroot de Miri
télécharge des crates (impossible dans le bac à sable Nix), et une campagne de fuzz n'a
pas de fin. Les deux se lancent à la main depuis le devshell nightly (§5).

Windows : `packaging/windows/setup-env.ps1` installe ou vérifie (`-Check`) les
versions de `versions.json`. `-Scope User` se limite à Rust et aux Build Tools, ce qu'il
faut pour les crates utilisateur ; `-Scope Driver` (défaut) y ajoute le WDK, LLVM et
`cargo-wdk`, nécessaires au seul `drivers/windows` (docs/driver-dev.md).

## 8. Ajouter un nœud interne

1. Implémenter `Node` dans `conduit-core::nodes` (sans allocation dans `process`,
   `prepare` pour allouer) et l'exposer.
2. Ajouter une variante à `InternalKind` (`conduit-protocol::api`) et son
   instanciation dans `Engine::add_internal` ; ses paramètres à chaud dans
   `Engine::set_param`.
3. Ajouter la sous-commande `add` correspondante dans `conduitctl::cli`.
4. Tests : unitaire, `no_alloc`, un scénario CLI. Régénérer `docs/protocol.md`.
