<#
.SYNOPSIS
  Banc rejouable du critère M1b-31 : boucle à travers « Conduit 1 » par le moteur du
  démon, et verdict xruns = 0 sur la durée demandée. S'exécute DANS L'INVITÉ, session
  console.
.DESCRIPTION
  Monte un graphe minimal et le laisse tourner, puis rend un verdict qui ne peut pas
  être vrai par accident :

      sine 440 Hz 0,25  ──FL/FR──▶  rendu de « Conduit 1 »
                                          │ (pilote noyau, boucle)
      mesure (nœud meter) ◀──FL/FR── capture de « Conduit 1 »

  Ce que ce script refuse de faire, et pourquoi :

  1. TOURNER EN SESSION 0. PowerShell Direct ouvre ses sessions dans la session des
     services, qui n'a aucun audio utilisateur : les flux s'ouvrent, les trames arrivent
     à la bonne cadence, toutes silencieuses, et « 0 xrun » y est acquis d'avance. Ce
     n'est pas une mesure ratée, c'est une mesure sans objet (docs/vm-bringup.md §3 bis).
     Le lancement passe donc par vm-run-console.ps1, qui garantit la session console ;
     ce script vérifie quand même, parce qu'il se lance aussi à la main.
  2. MESURER SUR DU SILENCE. Avant de démarrer le chronomètre, la crête du VU doit
     dépasser −40 dBFS. Sans ce témoin, un câble qui ne transporte rien rendrait
     exactement le même « 0 xrun » qu'une boucle parfaite.
  3. MESURER SUR LE DORSAL SIMULÉ. « conduitd --backend wasapi » se replie sur le dorsal
     « null » — un simple minuteur, sans périphérique réel — quand WASAPI ne démarre pas
     (F-51, crates/conduitd/src/main.rs), et ne le signale que par une ligne de journal.
     Le banc relit donc « backend » dans l'état et s'arrête si ce n'est pas « wasapi ».
  4. ROUTER PAR LE NOM. Les DEUX nœuds du câble s'appellent « Conduit 1 » : le rendu et
     la capture. Un nom est donc ambigu par construction, et conduitctl le refuse. Le
     routage se fait par les DeviceId relevés dans « cable list --json » (champs render
     et capture), qui sont aussi la clé de rapprochement avec status.devices[].id.

  Isolation de la mesure. Sur une racine vierge, conduitd applique la configuration par
  DÉFAUT de la SPEC — deux câbles — et active donc « Conduit 2 » en plus du câble mesuré.
  Ses deux endpoints tournent à vide dans le graphe et alimentent le compteur GLOBAL de
  xruns du moteur, celui-là même que lit le verdict : mesuré, 32 à 42 des xruns d'une passe
  venaient de là. Avant de démarrer le démon, le banc écrit donc dans
  <Racine>\config\conduit.toml — le nom exact vient de Paths::in_dirs,
  crates/conduitd/src/paths.rs — une configuration qui ne contient QUE le câble mesuré. Le
  fichier n'est écrit que s'il est ABSENT : une configuration déposée exprès pour une
  campagne (quantum, pilote, format) n'est jamais écrasée. La configuration par défaut du
  PRODUIT reste à deux câbles ; c'est le banc qui isole, pas le produit qui change.

  Série temporelle. Toutes les -Intervalle secondes, l'état complet est ajouté à
  serie.jsonl. Elle date un xrun, attrape un démon mort en cours de route et montre la
  dérive du ratio de rééchantillonnage — ce qu'un unique relevé final ne dit pas. Le
  premier xrun n'interrompt PAS la mesure : la suite de la série est ce qui distingue un
  accident isolé d'une dérive.

  Bornes de remplissage et de ratio. Chaque relevé et le récapitulatif affichent, pour les
  deux bouts du câble, « remplissage min–max » et « ratio min–max » (status.devices[] :
  fill_min, fill_max, ratio_min_millionths, ratio_max_millionths). Le champ « fill » seul
  ne sert à rien ici : le remplissage mesuré par un port asynchrone est le niveau
  INSTANTANÉ de son anneau, donc une dent de scie d'amplitude un paquet (480 trames à
  10 ms), et un relevé périodique l'échantillonne au lieu de la mesurer. Les bornes, tenues
  à jour à chaque cycle du graphe, montrent la hauteur de la dent de scie, le creux
  réellement atteint — celui qui s'annule en sous-alimentation — et tout écrêtage de la
  DLL. Elles sont cumulées depuis la remise à zéro des xruns que le banc fait juste avant
  la mesure : elles couvrent la campagne, pas l'intervalle.

  Effets de bord et restauration. Le banc arrête les conduitd déjà lancés (deux démons se
  disputeraient les périphériques) et supprime la tâche d'autodémarrage le temps de la
  mesure. Le bloc finally réenregistre la tâche si elle existait et relance les démons
  arrêtés avec leur ligne de commande d'origine, relevée avant de les tuer.

  Codes de retour : 0 verdict favorable, 1 verdict défavorable ou banc interrompu.
.PARAMETER Duree
  Durée de la mesure, en secondes (600 = les 10 min du critère).
.PARAMETER Cable
  Nom OS du câble à boucler (« Conduit 1 »). Créé s'il est absent ou inactif.
.PARAMETER Racine
  Dossier de travail du démon (conduitd --root) : configuration, données, socket, et les
  artefacts du banc (serie.jsonl, conduitd.log, recapitulatif.txt).
.PARAMETER Pilote
  Pilote de graphe : « auto », « internal », ou un périphérique (nom ou DeviceId).
  « auto » laisse le démon choisir et le banc journalise le pilote EFFECTIF.
.PARAMETER Intervalle
  Période d'échantillonnage de la série temporelle, en secondes.
.PARAMETER Dossier
  Dossier des trois exécutables (conduitd.exe, conduitctl.exe, conduit-helper.exe).
  Défaut : le dossier de ce script, où vm-bench-boucle.ps1 les dépose.
.EXAMPLE
  .\bench-boucle.ps1 -Duree 600
.EXAMPLE
  # Répétition courte, pilote forcé sur le câble lui-même :
  .\bench-boucle.ps1 -Duree 60 -Intervalle 10 -Pilote "Conduit 1"
#>
[CmdletBinding()]
param(
  [ValidateRange(10, 86400)][int]$Duree = 600,
  [string]$Cable = "Conduit 1",
  [string]$Racine = "C:\ConduitTest\bench",
  [string]$Pilote = "auto",
  [ValidateRange(1, 3600)][int]$Intervalle = 30,
  [string]$Dossier
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Les trois exécutables écrivent en UTF-8 (Rust). Sans cette ligne, Windows PowerShell
# 5.1 décoderait leur sortie — et le JSON du protocole — dans la page de codes OEM, et
# les noms de périphériques accentués reviendraient abîmés. C'est aussi ce qui rend
# lisible la sortie de ce script une fois redirigée par vm-run-console.ps1, qui la relit
# en UTF-8.
$encodagePrecedent = $null
try {
  $encodagePrecedent = [Console]::OutputEncoding
  [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
} catch {
  Write-Warning "encodage de sortie non modifiable ($($_.Exception.Message)) : les accents peuvent être abîmés."
  $encodagePrecedent = $null
}

# --- Fonctions ------------------------------------------------------------------------

function Format-Argument {
  <#
  .SYNOPSIS
    Cite un argument s'il en a besoin. Windows PowerShell 5.1 ne cite RIEN quand on passe
    un tableau à Start-Process -ArgumentList : la citation est à notre charge.
  #>
  param([Parameter(Mandatory)][AllowEmptyString()][string]$Valeur)
  if ($Valeur -eq "") { return '""' }
  if ($Valeur -match '[\s"]') { return '"' + ($Valeur -replace '"', '\"') + '"' }
  return $Valeur
}

function Invoke-Natif {
  <#
  .SYNOPSIS
    Lance un exécutable natif en capturant sa sortie, et rend { Lignes, Texte, Code }
    sans jamais lever d'exception sur le code de retour.
  .DESCRIPTION
    Même garde que Invoke-NativeChecked de vm-common.psm1, recopiée ici parce que ce
    script tourne SEUL dans l'invité, sans le module : Windows PowerShell 5.1 transforme
    les lignes stderr d'un exécutable en erreurs TERMINANTES quand $ErrorActionPreference
    vaut Stop et que « 2>&1 » est utilisé. La préférence est relâchée le temps de l'appel.
  #>
  param(
    [Parameter(Mandatory)][string]$Exe,
    [AllowEmptyCollection()][string[]]$Arguments = @()
  )
  $precedent = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $global:LASTEXITCODE = 0
    $lignes = @(& $Exe @Arguments 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $precedent
  }
  return [pscustomobject]@{
    Lignes = $lignes
    Texte  = ($lignes -join "`n")
    Code   = [int]$code
  }
}

function Invoke-Ctl {
  <#
  .SYNOPSIS
    « conduitctl --socket <socket> <arguments> » ; lève une exception avec la sortie si le
    code de retour n'est pas 0. Rend la sortie texte.
  .DESCRIPTION
    --socket et --json sont des options GLOBALES de conduitctl (clap global = true,
    crates/conduitctl/src/cli.rs) : elles se placent aussi bien avant qu'après la
    sous-commande. Elles sont passées ici avant, une fois pour toutes.
  #>
  param([Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Arguments)
  $r = Invoke-Natif -Exe $script:Conduitctl -Arguments (@("--socket", $script:Socket) + $Arguments)
  if ($r.Code -ne 0) {
    throw "« conduitctl $($Arguments -join ' ') » a échoué (code $($r.Code)) :`n$($r.Texte)"
  }
  return $r.Texte
}

function Invoke-CtlJson {
  <#
  .SYNOPSIS
    Comme Invoke-Ctl, en ajoutant --json et en rendant l'objet analysé.
  #>
  param([Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Arguments)
  $texte = Invoke-Ctl (@("--json") + $Arguments)
  if ([string]::IsNullOrWhiteSpace($texte)) {
    throw "« conduitctl $($Arguments -join ' ') --json » n'a rien rendu."
  }
  return ($texte | ConvertFrom-Json)
}

function Get-Champ {
  <#
  .SYNOPSIS
    Lit un champ d'un objet JSON en disant lequel manque, plutôt que de laisser
    Set-StrictMode échouer sur « propriété inexistante » à un endroit quelconque.
  .DESCRIPTION
    C'est la seule protection contre une dérive du protocole : un champ renommé dans
    conduit-protocol doit se voir ici, nommément, et pas se déguiser en panne du banc.
  #>
  param(
    [Parameter(Mandatory)][AllowNull()]$Objet,
    [Parameter(Mandatory)][string]$Nom,
    [Parameter(Mandatory)][string]$Source
  )
  if ($null -eq $Objet) { throw "réponse vide là où « $Source » devait rendre un objet contenant « $Nom »." }
  $propriete = $Objet.PSObject.Properties[$Nom]
  if ($null -eq $propriete) {
    throw ("champ « $Nom » absent de la réponse de « $Source » (champs présents : " +
      (($Objet.PSObject.Properties | ForEach-Object { $_.Name }) -join ", ") +
      "). Le contrat JSON de conduit-protocol a changé : mettre ce banc à jour.")
  }
  return $propriete.Value
}

function ConvertTo-Dbfs {
  <#
  .SYNOPSIS
    Niveau linéaire (≥ 0) en dBFS ; −160 tient lieu de −∞ pour rester comparable.
  #>
  param([Parameter(Mandatory)][double]$Lineaire)
  if ($Lineaire -le 0) { return [double](-160) }
  return [Math]::Round(20 * [Math]::Log10($Lineaire), 2)
}

function Get-CreteDbfs {
  <#
  .SYNOPSIS
    Crête du nœud de mesure, en dBFS, tous canaux confondus.
  .DESCRIPTION
    « conduitctl meter <nœud> » (Command::ReadMeter) ne lit QUE les nœuds internes de type
    Meter — un câble n'en est pas un (crates/conduit-engine/src/engine.rs,
    crates/conduit-gui/src/cables.rs). D'où le nœud « add meter » explicite du graphe.
  #>
  param([Parameter(Mandatory)][string]$Noeud)
  $reponse = Invoke-CtlJson @("meter", $Noeud)
  $canaux = @(Get-Champ -Objet $reponse -Nom "channels" -Source "meter --json")
  if ($canaux.Count -eq 0) { throw "le nœud de mesure « $Noeud » n'a rendu aucun canal." }
  $crete = [double]0
  foreach ($canal in $canaux) {
    $valeur = [double](Get-Champ -Objet $canal -Nom "peak" -Source "meter --json")
    if ($valeur -gt $crete) { $crete = $valeur }
  }
  return (ConvertTo-Dbfs -Lineaire $crete)
}

function Get-Etat {
  <#
  .SYNOPSIS
    « conduitctl status --json » : l'objet EngineStatus, APLATI — l'enveloppe Reply est un
    enum étiqueté par « reply », donc backend, xruns, timing, devices… sont au premier
    niveau, à côté de la clé « reply ».
  #>
  return (Invoke-CtlJson @("status"))
}

function Format-Pilote {
  <#
  .SYNOPSIS
    DriverStatus ({"kind":"device","id":…}, internal, none) en une ligne.
  #>
  param([Parameter(Mandatory)][AllowNull()]$Driver)
  $genre = [string](Get-Champ -Objet $Driver -Nom "kind" -Source "status --json (driver)")
  if ($genre -ne "device") { return $genre }
  return "device " + [string](Get-Champ -Objet $Driver -Nom "id" -Source "status --json (driver)")
}

function Find-Peripherique {
  <#
  .SYNOPSIS
    L'entrée de status.devices dont l'id vaut $Id, ou $null s'il n'y en a pas
    exactement une.
  #>
  param([Parameter(Mandatory)][AllowNull()]$Etat, [Parameter(Mandatory)][string]$Id)
  $peripheriques = @(Get-Champ -Objet $Etat -Nom "devices" -Source "status --json")
  $trouves = @($peripheriques | Where-Object {
      [string](Get-Champ -Objet $_ -Nom "id" -Source "status --json (devices)") -eq $Id
    })
  if ($trouves.Count -ne 1) { return $null }
  return $trouves[0]
}

function Format-Extremes {
  <#
  .SYNOPSIS
    « remplissage 736–1248 trames, ratio 0,999500–1,000500 » pour une entrée de
    status.devices.
  .DESCRIPTION
    Pourquoi des extrêmes et pas le seul « fill » courant : le remplissage qu'un port
    asynchrone mesure est le NIVEAU INSTANTANÉ de son anneau, donc une dent de scie
    dont l'amplitude vaut un paquet du périphérique (480 trames à 10 ms). Un relevé
    périodique de « fill » échantillonne cette dent de scie au lieu de la mesurer : il
    ne dit ni sa hauteur, ni le creux réellement atteint, qui est pourtant ce qui
    s'annule en sous-alimentation. Les bornes, elles, sont tenues à jour à CHAQUE
    cycle du graphe, côté moteur.

    Elles sont CUMULÉES depuis la dernière remise à zéro des xruns, que le banc ne
    fait qu'une fois, juste avant la mesure (« conduitctl xruns --reset ») : ces
    bornes couvrent donc toute la campagne écoulée, et non l'intervalle. Un
    élargissement entre deux relevés date l'événement à l'intervalle près.
  #>
  param([Parameter(Mandatory)][AllowNull()]$Peripherique)
  $remplissageMin = [long](Get-Champ -Objet $Peripherique -Nom "fill_min" -Source "status --json (devices)")
  $remplissageMax = [long](Get-Champ -Objet $Peripherique -Nom "fill_max" -Source "status --json (devices)")
  $ratioMin = [double](Get-Champ -Objet $Peripherique -Nom "ratio_min_millionths" -Source "status --json (devices)") / 1e6
  $ratioMax = [double](Get-Champ -Objet $Peripherique -Nom "ratio_max_millionths" -Source "status --json (devices)") / 1e6
  return ("remplissage {0:n0}–{1:n0} trames, ratio {2:n6}–{3:n6}" -f `
      $remplissageMin, $remplissageMax, $ratioMin, $ratioMax)
}

function Split-LigneDeCommande {
  <#
  .SYNOPSIS
    Sépare une ligne de commande Windows en { Exe, Reste } : le premier jeton (cité ou
    non) et tout ce qui suit, tel quel. $null si la ligne est vide ou mal citée.
  .DESCRIPTION
    Sert à relancer à l'identique un conduitd que le banc a dû arrêter. Fonction pure.
  #>
  param([Parameter(Mandatory)][AllowEmptyString()][string]$Ligne)
  $valeur = $Ligne.Trim()
  if ($valeur -eq "") { return $null }
  if ($valeur.StartsWith('"')) {
    $fin = $valeur.IndexOf('"', 1)
    if ($fin -lt 0) { return $null }
    return [pscustomobject]@{
      Exe   = $valeur.Substring(1, $fin - 1)
      Reste = $valeur.Substring($fin + 1).Trim()
    }
  }
  $espace = $valeur.IndexOf(' ')
  if ($espace -lt 0) { return [pscustomobject]@{ Exe = $valeur; Reste = "" } }
  return [pscustomobject]@{
    Exe   = $valeur.Substring(0, $espace)
    Reste = $valeur.Substring($espace + 1).Trim()
  }
}

# --- État partagé ---------------------------------------------------------------------

if (-not $Dossier) { $Dossier = $PSScriptRoot }
if (-not $Dossier) { $Dossier = (Get-Location).Path }

$script:Conduitctl = Join-Path $Dossier "conduitctl.exe"
$Conduitd = Join-Path $Dossier "conduitd.exe"
$Helper = Join-Path $Dossier "conduit-helper.exe"
$script:Socket = Join-Path $Racine "conduitd.sock"

$serie = Join-Path $Racine "serie.jsonl"
$journal = Join-Path $Racine "conduitd.log"
$journalErreur = Join-Path $Racine "conduitd.err"
$recapitulatif = Join-Path $Racine "recapitulatif.txt"

$nomSine = "bench-sine"
$nomMesure = "bench-mesure"
$tacheAutostart = "Conduit\conduitd"   # autostart::DEFAULT_TASK_NAME
$seuilTemoinDbfs = -40.0
$ecartCreteMaxDb = 2.0
$toleranceCycles = 0.01

$demon = $null
$autostartARestaurer = $false
$demonsArretes = @()
$echec = $null
$verdict = $false
$script:lignesRecapitulatif = @()

function Add-Recapitulatif {
  <#
  .SYNOPSIS
    Écrit une ligne à l'écran ET dans le récapitulatif rapatrié par vm-bench-boucle.ps1.
  #>
  param([Parameter(Mandatory)][AllowEmptyString()][string]$Texte)
  $script:lignesRecapitulatif += $Texte
  Write-Host $Texte
}

function Add-Ligne {
  <#
  .SYNOPSIS
    Ligne « libellé : valeur » alignée. Format-Table serait tronqué à 80 colonnes une
    fois la sortie redirigée par vm-run-console.ps1 : le tableau est composé à la main.
  #>
  param(
    [Parameter(Mandatory)][string]$Libelle,
    [Parameter(Mandatory)][AllowEmptyString()][string]$Valeur
  )
  Add-Recapitulatif ("  {0,-24} {1}" -f ($Libelle + " :"), $Valeur)
}

# --- Banc -------------------------------------------------------------------------------

try {
  # 1. Session console. Voir .DESCRIPTION §1 : le refus est délibéré.
  $sessionId = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
  if ($sessionId -eq 0) {
    throw @(
      "REFUS DÉLIBÉRÉ : ce banc tourne dans la SESSION 0, celle des services."
      ""
      "La session 0 n'a aucun audio utilisateur. Les flux WASAPI s'y ouvriraient, les"
      "trames arriveraient à la bonne cadence, TOUTES SILENCIEUSES, et le banc rendrait"
      "« 0 xrun » sans avoir rien mesuré. C'est le piège qui a invalidé une journée de"
      "mesures (docs/vm-bringup.md §3 bis)."
      ""
      "Marche à suivre : ne pas lancer ce script par « Invoke-Command -VMName », mais par"
      "  drivers\windows\tools\vm-bench-boucle.ps1 -Credential (Get-Credential nathan)"
      "qui passe par vm-run-console.ps1 et sa tâche à jeton interactif — après avoir"
      "ouvert une session Windows À L'ÉCRAN dans l'invité (vmconnect, mode session"
      "étendue DÉSACTIVÉ : en session étendue l'audio est redirigé vers l'hôte et le"
      "câble Conduit disparaît de la liste)."
    ) -join "`n"
  }

  foreach ($exe in @($Conduitd, $script:Conduitctl, $Helper)) {
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
      throw @(
        "Exécutable absent : $exe"
        "Les trois binaires (conduitd.exe, conduitctl.exe, conduit-helper.exe) sont"
        "déposés à côté de ce script par vm-bench-boucle.ps1. Les construire en CRT"
        "statique : sans VCRUNTIME140.dll, l'invité les tue en -1073741515 (0xC0000135)"
        "sans dire quelle DLL manque."
      ) -join "`n"
    }
  }

  if (-not (Test-Path -LiteralPath $Racine)) {
    New-Item -ItemType Directory -Force -Path $Racine | Out-Null
  }

  Write-Host "== Banc de boucle M1b-31 — session $sessionId, câble « $Cable », $Duree s"

  # 2. Service d'assistance. « conduit-helper etat » est déclaratif et sort TOUJOURS en 0
  # (crates/conduit-helper/src/main.rs) : il sert à l'affichage. La décision se prend sur
  # le code de retour de « version », une commande CLIENTE, qui échoue en 1 si le canal
  # ne répond pas — un code, pas un texte traduisible.
  $etatHelper = Invoke-Natif -Exe $Helper -Arguments @("etat")
  Write-Host $etatHelper.Texte
  $versionHelper = Invoke-Natif -Exe $Helper -Arguments @("version")
  if ($versionHelper.Code -ne 0) {
    throw @(
      "Le service d'assistance Conduit ne répond pas (« conduit-helper version » : code $($versionHelper.Code))."
      $versionHelper.Texte
      ""
      "Sans lui, le démon ne peut pas activer un câble : le pilote exige"
      "SeLoadDriverPrivilege, que la session de l'utilisateur n'a pas (ADR-013, M1b-34)."
      "Depuis une invite ÉLEVÉE de l'invité :"
      "  conduit-helper installer --demarrer"
    ) -join "`n"
  }

  # 3. Démons déjà lancés : deux conduitd se disputeraient les périphériques. Leur ligne
  # de commande est relevée AVANT de les arrêter, pour les relancer à l'identique.
  foreach ($processus in @(Get-Process -Name "conduitd" -ErrorAction SilentlyContinue)) {
    $ligneOrigine = ""
    try {
      $info = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $($processus.Id)"
      if ($null -ne $info -and $null -ne $info.CommandLine) { $ligneOrigine = [string]$info.CommandLine }
    } catch {
      $ligneOrigine = ""
    }
    Write-Host "Arrêt du conduitd déjà lancé (PID $($processus.Id)) : $ligneOrigine"
    Stop-Process -Id $processus.Id -Force -ErrorAction SilentlyContinue
    if ($ligneOrigine -ne "") { $demonsArretes += $ligneOrigine }
  }

  # 4. Autodémarrage : la tâche planifiée relancerait un démon au milieu de la mesure.
  # « conduitd autostart status » sort en 0 si la tâche existe, 4 si elle est absente
  # (crates/conduitd/src/autostart/mod.rs). C'est bien conduitd, pas conduitctl, qui
  # porte cette sous-commande.
  $etatAutostart = Invoke-Natif -Exe $Conduitd -Arguments @("autostart", "status")
  if ($etatAutostart.Code -eq 0) {
    Write-Host "Tâche « $tacheAutostart » présente : suppression le temps de la mesure."
    Write-Host $etatAutostart.Texte
    $desactivation = Invoke-Natif -Exe $Conduitd -Arguments @("autostart", "disable")
    if ($desactivation.Code -ne 0) {
      throw "« conduitd autostart disable » a échoué (code $($desactivation.Code)) :`n$($desactivation.Texte)"
    }
    $autostartARestaurer = $true
  } else {
    Write-Host "Tâche « $tacheAutostart » absente (code $($etatAutostart.Code)) : rien à couper."
  }

  # 5. Démarrage du démon. --root isole configuration, données ET socket : le socket
  # devient <racine>\conduitd.sock, que le protocole projette sur un named pipe propre à
  # la racine (crates/conduitd/src/paths.rs). Aucun risque de parler au démon de la
  # session de l'utilisateur.
  foreach ($vestige in @($journal, $journalErreur, $serie, $recapitulatif)) {
    if (Test-Path -LiteralPath $vestige) { Remove-Item -LiteralPath $vestige -Force }
  }

  # Isolation : sans ce fichier, le démon créerait AUSSI « Conduit 2 », dont les endpoints
  # à vide alimentent le compteur global de xruns du verdict (voir .DESCRIPTION). Le numéro
  # du câble se lit dans son nom canonique — CableId s'affiche « Conduit <n> »,
  # crates/conduit-backend/src/cable.rs ; un alias ne le donne pas, et le banc préfère le
  # dire plutôt qu'isoler le mauvais câble.
  $configBanc = Join-Path (Join-Path $Racine "config") "conduit.toml"
  $idConfig = $null
  if ($Cable -match '^\s*Conduit\s+(\d+)\s*$') { $idConfig = [int]$Matches[1] }
  if (Test-Path -LiteralPath $configBanc -PathType Leaf) {
    Write-Host "Configuration présente, laissée intacte : $configBanc"
  } elseif ($null -eq $idConfig) {
    Write-Warning (@(
        "« $Cable » n'est pas un nom canonique « Conduit <n> » : le banc ne sait pas quel"
        "[[cable]] écrire et laisse la configuration par défaut s'appliquer — DEUX câbles,"
        "dont le second tourne à vide et gonfle le compteur global de xruns du verdict."
        "Relancer avec -Cable « Conduit <n> », ou déposer $configBanc à la main."
      ) -join "`n")
  } else {
    $dossierConfig = Split-Path -Parent $configBanc
    if (-not (Test-Path -LiteralPath $dossierConfig)) {
      New-Item -ItemType Directory -Force -Path $dossierConfig | Out-Null
    }
    $texteConfig = (@(
        "# Écrit par bench-boucle.ps1 : la mesure ne porte QUE sur « $Cable »."
        "# La configuration par défaut du produit (SPEC §1.2) crée deux câbles ; le second"
        "# tournerait à vide dans le graphe et ses xruns entreraient dans le compteur global"
        "# que lit le verdict. Ce fichier n'est PAS réécrit s'il existe déjà : le modifier"
        "# est la façon prévue de faire varier le quantum, le pilote ou le format."
        ""
        "[[cable]]"
        "id = $idConfig"
      ) -join "`r`n") + "`r`n"
    [System.IO.File]::WriteAllText($configBanc, $texteConfig, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "Isolation : $configBanc écrit, un seul câble (id $idConfig)."
  }

  $argumentsDemon = @("--root", $Racine, "--backend", "wasapi", "--no-persist", "--log-level", "info")
  $ligneDemon = @($argumentsDemon | ForEach-Object { Format-Argument -Valeur $_ })
  Write-Host "Démarrage : conduitd $($argumentsDemon -join ' ')"
  # -NoNewWindow et non -WindowStyle : en 5.1, -WindowStyle exige UseShellExecute, que
  # les redirections interdisent. Le démon partage donc la console de la tâche, et sa
  # sortie part quand même dans les deux fichiers.
  $demon = Start-Process -FilePath $Conduitd -ArgumentList $ligneDemon `
    -RedirectStandardOutput $journal -RedirectStandardError $journalErreur `
    -NoNewWindow -PassThru

  # Attente du point de contrôle : 30 s. La seule preuve de disponibilité est que
  # « status » réponde ; un démon mort est détecté tout de suite, avec son journal.
  $limite = (Get-Date).AddSeconds(30)
  $pret = $false
  while ((Get-Date) -lt $limite) {
    if ($demon.HasExited) {
      $detail = ""
      if (Test-Path -LiteralPath $journalErreur) {
        $detail = [string](Get-Content -LiteralPath $journalErreur -Raw -Encoding UTF8)
      }
      throw "conduitd s'est arrêté au démarrage (code $($demon.ExitCode)) :`n$detail"
    }
    $essai = Invoke-Natif -Exe $script:Conduitctl -Arguments @("--socket", $script:Socket, "status")
    if ($essai.Code -eq 0) { $pret = $true; break }
    Start-Sleep -Milliseconds 500
  }
  if (-not $pret) {
    throw "conduitd n'a pas répondu sur $($script:Socket) en 30 s. Journal : $journalErreur"
  }

  $etatInitial = Get-Etat
  $dorsal = [string](Get-Champ -Objet $etatInitial -Nom "backend" -Source "status --json")
  if ($dorsal -ne "wasapi") {
    throw @(
      "Le démon tourne sur le dorsal « $dorsal », pas « wasapi » — voir .DESCRIPTION §3."
      "Mesurer là-dessus rendrait un « 0 xrun » qui ne parlerait d'aucun matériel."
      "Vérifier que le service audio Windows (AudioSrv) tourne dans l'invité, puis lire :"
      "  $journalErreur"
    ) -join "`n"
  }
  $frequenceInitiale = Get-Champ -Objet $etatInitial -Nom "sample_rate" -Source "status --json"
  $quantumInitial = Get-Champ -Objet $etatInitial -Nom "quantum" -Source "status --json"
  Write-Host "Dorsal : $dorsal — fréquence $frequenceInitiale Hz, quantum $quantumInitial trames"

  # 6. Câble. Les deux endpoints portent le MÊME nom : seuls les DeviceId distinguent le
  # rendu de la capture (crates/conduit-backend/src/cable.rs, champs render / capture).
  $cables = @(Get-Champ -Objet (Invoke-CtlJson @("cable", "list")) -Nom "cables" -Source "cable list --json")
  $actifs = @($cables | Where-Object {
      ([string](Get-Champ -Objet $_ -Nom "name" -Source "cable list --json") -eq $Cable) -and
      ([bool](Get-Champ -Objet $_ -Nom "active" -Source "cable list --json"))
    })
  if ($actifs.Count -eq 0) {
    Write-Host "Câble « $Cable » absent ou inactif : création."
    Write-Host (Invoke-Ctl @("cable", "add", "--name", $Cable))
    $cables = @(Get-Champ -Objet (Invoke-CtlJson @("cable", "list")) -Nom "cables" -Source "cable list --json")
    $actifs = @($cables | Where-Object {
        ([string](Get-Champ -Objet $_ -Nom "name" -Source "cable list --json") -eq $Cable) -and
        ([bool](Get-Champ -Objet $_ -Nom "active" -Source "cable list --json"))
      })
  }
  if ($actifs.Count -eq 0) {
    throw "le câble « $Cable » n'est toujours pas actif après « cable add » : voir $journalErreur"
  }
  if ($actifs.Count -gt 1) {
    throw "« $Cable » désigne $($actifs.Count) câbles actifs : en supprimer avant de mesurer (conduitctl cable remove <n>)."
  }
  $cableInfo = $actifs[0]
  $idCable = Get-Champ -Objet $cableInfo -Nom "id" -Source "cable list --json"
  $idRendu = [string](Get-Champ -Objet $cableInfo -Nom "render" -Source "cable list --json")
  $idCapture = [string](Get-Champ -Objet $cableInfo -Nom "capture" -Source "cable list --json")
  Write-Host "Câble $idCable « $Cable » : rendu $idRendu, capture $idCapture"

  # Les endpoints n'entrent dans le graphe qu'après la notification MMDevice : on attend
  # que les DEUX nœuds soient là, plutôt que d'échouer sur un « link » prématuré.
  $limite = (Get-Date).AddSeconds(30)
  $noeuds = @()
  $vus = @()
  while ((Get-Date) -lt $limite) {
    $noeuds = @(Get-Champ -Objet (Invoke-CtlJson @("nodes")) -Nom "nodes" -Source "nodes --json")
    $vus = @($noeuds | Where-Object {
        $peripherique = Get-Champ -Objet $_ -Nom "device" -Source "nodes --json"
        ($null -ne $peripherique) -and
        (@($idRendu, $idCapture) -contains [string](Get-Champ -Objet $peripherique -Nom "id" -Source "nodes --json (device)"))
      })
    if ($vus.Count -ge 2) { break }
    Start-Sleep -Milliseconds 500
  }
  if ($vus.Count -lt 2) {
    throw @(
      "Les deux endpoints de « $Cable » ne sont pas apparus dans le graphe en 30 s"
      "($($vus.Count) sur 2 ; rendu $idRendu, capture $idCapture)."
      "Le pilote conduit_kmd est-il chargé et le câble connecté ? « conduitctl nodes »."
    ) -join "`n"
  }

  # 7. Graphe. Les identifiants passés à « link » sont les DeviceId, jamais les noms : les
  # deux nœuds s'appellent « $Cable » et conduitctl refuserait l'ambiguïté. find_port
  # découpe sur le DERNIER « : » (rsplit_once, crates/conduitctl/src/resolve.rs), donc un
  # DeviceId ne peut pas être coupé au mauvais endroit.
  Write-Host (Invoke-Ctl @("add", "sine", $nomSine, "--frequency", "440", "--amplitude", "0.25", "--channels", "2"))
  Write-Host (Invoke-Ctl @("add", "meter", $nomMesure, "--channels", "2"))
  foreach ($port in @("FL", "FR")) {
    Write-Host (Invoke-Ctl @("link", "$($nomSine):$port", "$($idRendu):$port"))
    Write-Host (Invoke-Ctl @("link", "$($idCapture):$port", "$($nomMesure):$port"))
  }
  Write-Host "Graphe : $nomSine (440 Hz, 0,25) → rendu → [boucle] → capture → $nomMesure"

  # 8. Pilote de graphe, avant le témoin : en changer remet les flux à plat.
  if ($Pilote -ne "auto") {
    Write-Host "Pilote demandé : $Pilote"
    Write-Host (Invoke-Ctl @("driver", $Pilote))
    Start-Sleep -Seconds 2
  }

  # 9. Témoin de signal. Voir .DESCRIPTION §2 : 0 xrun sur du silence ne prouverait rien.
  Start-Sleep -Seconds 3
  $creteInitiale = Get-CreteDbfs -Noeud $nomMesure
  Write-Host ("Témoin : crête {0} dBFS (seuil {1} dBFS)" -f $creteInitiale, $seuilTemoinDbfs)
  if ($creteInitiale -lt $seuilTemoinDbfs) {
    throw @(
      ("ARRÊT : la boucle ne porte pas de signal (crête {0} dBFS < {1} dBFS)." -f $creteInitiale, $seuilTemoinDbfs)
      ""
      "0 xrun sur du silence ne prouverait rien : le critère M1b-31 porte sur une boucle"
      "qui TRANSPORTE de l'audio à travers « $Cable ». Vérifier, dans cet ordre :"
      "  conduitctl --socket `"$($script:Socket)`" links   (les quatre liens sont-ils là ?)"
      "  conduitctl --socket `"$($script:Socket)`" nodes   (les deux endpoints sont-ils actifs ?)"
      "  $journalErreur"
      "et que la session vmconnect n'est PAS en mode session étendue : l'audio y est"
      "redirigé vers l'hôte et le câble n'est plus bouclé."
    ) -join "`n"
  }

  # 10. Mesure.
  Invoke-Ctl @("xruns", "--reset") | Out-Null
  $reference = Get-Etat
  $cyclesReference = [double](Get-Champ -Objet $reference -Nom "cycles" -Source "status --json")
  $debut = Get-Date
  $chrono = [System.Diagnostics.Stopwatch]::StartNew()
  $premierXrun = $null
  $utf8SansBom = New-Object System.Text.UTF8Encoding($false)
  Write-Host "== Mesure : $Duree s, relevé toutes les $Intervalle s → $serie"

  while ($chrono.Elapsed.TotalSeconds -lt $Duree) {
    $restant = $Duree - $chrono.Elapsed.TotalSeconds
    $pause = [Math]::Min([double]$Intervalle, $restant)
    if ($pause -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Ceiling($pause * 1000)) }

    if ($demon.HasExited) {
      throw "conduitd s'est arrêté au bout de $([Math]::Round($chrono.Elapsed.TotalSeconds)) s (code $($demon.ExitCode)) : $journalErreur"
    }
    $etat = Get-Etat
    $crete = Get-CreteDbfs -Noeud $nomMesure
    # Les champs ajoutés par le banc sont nommés en français et l'état brut est rangé
    # sous « etat » : rien de ce que nous inventons ne peut se faire passer pour un champ
    # du protocole.
    $enregistrement = [pscustomobject][ordered]@{
      horodatage = (Get-Date).ToString("o")
      ecoule_s   = [Math]::Round($chrono.Elapsed.TotalSeconds, 3)
      crete_dbfs = $crete
      etat       = $etat
    }
    [System.IO.File]::AppendAllText($serie, ($enregistrement | ConvertTo-Json -Compress -Depth 12) + "`r`n", $utf8SansBom)

    $xruns = [long](Get-Champ -Objet $etat -Nom "xruns" -Source "status --json")
    if ($xruns -gt 0 -and $null -eq $premierXrun) {
      $premierXrun = [Math]::Round($chrono.Elapsed.TotalSeconds, 1)
      Write-Warning "premier xrun à $premierXrun s — la mesure continue : la suite de la série dira si c'est un accident isolé ou une dérive."
    }
    $cyclesCourants = Get-Champ -Objet $etat -Nom "cycles" -Source "status --json"
    Write-Host ("  {0,6:n0} s / {1} s — xruns {2}, cycles {3}, crête {4} dBFS" -f `
        $chrono.Elapsed.TotalSeconds, $Duree, $xruns, $cyclesCourants, $crete)
    # Les bornes de remplissage et de ratio des deux bouts du câble : c'est là que la
    # dent de scie du remplissage et un éventuel écrêtage de la DLL se voient.
    foreach ($paire in @(, @("rendu", $idRendu)) + @(, @("capture", $idCapture))) {
      $peripheriqueReleve = Find-Peripherique -Etat $etat -Id ([string]$paire[1])
      if ($null -eq $peripheriqueReleve) { continue }
      Write-Host ("         {0,-8} {1}" -f $paire[0], (Format-Extremes -Peripherique $peripheriqueReleve))
    }
  }
  $chrono.Stop()

  # 11. Verdict.
  $final = Get-Etat
  $creteFinale = Get-CreteDbfs -Noeud $nomMesure
  $ecoule = $chrono.Elapsed.TotalSeconds

  $frequence = [double](Get-Champ -Objet $final -Nom "sample_rate" -Source "status --json")
  $quantum = [double](Get-Champ -Objet $final -Nom "quantum" -Source "status --json")
  $xrunsTotaux = [long](Get-Champ -Objet $final -Nom "xruns" -Source "status --json")
  $timing = Get-Champ -Objet $final -Nom "timing" -Source "status --json"
  $timingOverruns = [long](Get-Champ -Objet $timing -Nom "overruns" -Source "status --json (timing)")
  $cyclesFinaux = [double](Get-Champ -Objet $final -Nom "cycles" -Source "status --json")
  $cyclesMesures = $cyclesFinaux - $cyclesReference
  $cyclesAttendus = $ecoule * $frequence / $quantum
  $ecartCycles = 1.0
  if ($cyclesAttendus -gt 0) { $ecartCycles = [Math]::Abs($cyclesMesures - $cyclesAttendus) / $cyclesAttendus }
  $ecartCrete = [Math]::Abs($creteFinale - $creteInitiale)

  # Les deux périphériques du câble, NOMMÉMENT : un endpoint disparu du moteur est un
  # échec, pas une absence de compteur.
  $peripheriquesEtat = @(Get-Champ -Objet $final -Nom "devices" -Source "status --json")
  $noeuds = @(Get-Champ -Objet (Invoke-CtlJson @("nodes")) -Nom "nodes" -Source "nodes --json")
  $lignesPeripheriques = @()
  $peripheriquesSains = $true
  foreach ($paire in @(, @("rendu", $idRendu)) + @(, @("capture", $idCapture))) {
    $sens = [string]$paire[0]
    $identifiant = [string]$paire[1]
    $peripherique = Find-Peripherique -Etat $final -Id $identifiant
    if ($null -eq $peripherique) {
      $peripheriquesSains = $false
      $lignesPeripheriques += ("  {0,-8} ABSENT de status.devices (ou en double) — {1} périphérique(s) au moteur, id {2}" -f `
          $sens, $peripheriquesEtat.Count, $identifiant)
      continue
    }
    $sous = [long](Get-Champ -Objet $peripherique -Nom "underruns" -Source "status --json (devices)")
    $sur = [long](Get-Champ -Objet $peripherique -Nom "overruns" -Source "status --json (devices)")
    $ratio = [double](Get-Champ -Objet $peripherique -Nom "ratio" -Source "status --json (devices)")
    $verrou = [bool](Get-Champ -Objet $peripherique -Nom "locked" -Source "status --json (devices)")
    $etatNoeud = [string](Get-Champ -Objet $peripherique -Nom "state" -Source "status --json (devices)")
    if ($sous -ne 0 -or $sur -ne 0) { $peripheriquesSains = $false }
    $libelle = $identifiant
    $noeudsTrouves = @($noeuds | Where-Object {
        $peripheriqueNoeud = Get-Champ -Objet $_ -Nom "device" -Source "nodes --json"
        ($null -ne $peripheriqueNoeud) -and
        ([string](Get-Champ -Objet $peripheriqueNoeud -Nom "id" -Source "nodes --json (device)") -eq $identifiant)
      })
    if ($noeudsTrouves.Count -ge 1) {
      $libelle = [string](Get-Champ -Objet $noeudsTrouves[0] -Nom "label" -Source "nodes --json")
    }
    $verrouTexte = "LIBRE"
    if ($verrou) { $verrouTexte = "verrouillée" }
    $lignesPeripheriques += ("  {0,-8} « {1} » état {2} — underruns {3}, overruns {4}, ratio {5:n6}, DLL {6}" -f `
        $sens, $libelle, $etatNoeud, $sous, $sur, $ratio, $verrouTexte)
    $lignesPeripheriques += ("           {0}" -f (Format-Extremes -Peripherique $peripherique))
    $lignesPeripheriques += ("           id {0}" -f $identifiant)
  }

  $verdict = ($xrunsTotaux -eq 0) -and ($timingOverruns -eq 0) -and $peripheriquesSains -and
    ($ecartCycles -le $toleranceCycles) -and ($ecartCrete -le $ecartCreteMaxDb)

  Add-Recapitulatif ""
  Add-Recapitulatif "== Récapitulatif du banc de boucle (M1b-31)"
  Add-Ligne -Libelle "Début" -Valeur ("{0}, durée mesurée {1:n1} s" -f $debut.ToString("s"), $ecoule)
  Add-Ligne -Libelle "Câble" -Valeur "$idCable « $Cable »"
  Add-Ligne -Libelle "Dorsal" -Valeur $dorsal
  Add-Ligne -Libelle "Fréquence" -Valeur ("{0:n0} Hz" -f $frequence)
  Add-Ligne -Libelle "Quantum" -Valeur ("{0:n0} trames" -f $quantum)
  Add-Ligne -Libelle "Pilote de graphe" -Valeur (Format-Pilote -Driver (Get-Champ -Objet $final -Nom "driver" -Source "status --json"))
  Add-Ligne -Libelle "Cycles" -Valeur ("{0:n0} mesurés / {1:n0} attendus (écart {2:p3}, toléré {3:p0})" -f `
      $cyclesMesures, $cyclesAttendus, $ecartCycles, $toleranceCycles)
  Add-Ligne -Libelle "Xruns du moteur" -Valeur ("{0}" -f $xrunsTotaux)
  Add-Ligne -Libelle "Cycles hors budget" -Valeur ("{0} (timing.overruns)" -f $timingOverruns)
  Add-Ligne -Libelle "Temps de cycle" -Valeur ("min {0:n0} / moy {1:n0} / max {2:n0} ns, budget {3:n0} ns" -f `
    ([double](Get-Champ -Objet $timing -Nom "min_ns" -Source "status --json (timing)")),
    ([double](Get-Champ -Objet $timing -Nom "avg_ns" -Source "status --json (timing)")),
    ([double](Get-Champ -Objet $timing -Nom "max_ns" -Source "status --json (timing)")),
    ([double](Get-Champ -Objet $timing -Nom "budget_ns" -Source "status --json (timing)")))
  Add-Ligne -Libelle "Crête du VU" -Valeur ("initiale {0} dBFS, finale {1} dBFS (écart {2:n2} dB, toléré {3:n0} dB)" -f `
      $creteInitiale, $creteFinale, $ecartCrete, $ecartCreteMaxDb)
  if ($null -ne $premierXrun) { Add-Ligne -Libelle "Premier xrun" -Valeur ("à {0} s" -f $premierXrun) }
  Add-Recapitulatif "  Périphériques du câble (bornes cumulées depuis « xruns --reset ») :"
  foreach ($ligne in $lignesPeripheriques) { Add-Recapitulatif $ligne }
  Add-Recapitulatif ""
  if ($verdict) {
    Add-Recapitulatif ("VERDICT : RÉUSSITE — boucle à travers « {0} », {1} xrun sur {2:n0} s" -f $Cable, $xrunsTotaux, $ecoule)
  } else {
    Add-Recapitulatif "VERDICT : ÉCHEC"
    if ($xrunsTotaux -ne 0) { Add-Recapitulatif "  - $xrunsTotaux xrun(s) du moteur" }
    if ($timingOverruns -ne 0) { Add-Recapitulatif "  - $timingOverruns cycle(s) au-delà du budget" }
    if (-not $peripheriquesSains) { Add-Recapitulatif "  - underruns/overruns non nuls, ou périphérique absent du moteur" }
    if ($ecartCycles -gt $toleranceCycles) { Add-Recapitulatif ("  - cycles incohérents avec la durée (écart {0:p3})" -f $ecartCycles) }
    if ($ecartCrete -gt $ecartCreteMaxDb) { Add-Recapitulatif ("  - la crête a dérivé de {0:n2} dB : le signal n'est pas resté le même" -f $ecartCrete) }
  }
  Add-Recapitulatif "  Série temporelle : $serie"
  Add-Recapitulatif "  Journal du démon : $journal / $journalErreur"
} catch {
  $echec = $_
} finally {
  if ($null -ne $demon) {
    try {
      if (-not $demon.HasExited) {
        Stop-Process -Id $demon.Id -Force -ErrorAction SilentlyContinue
        Write-Host "conduitd du banc (PID $($demon.Id)) arrêté."
      }
    } catch {
      $Host.UI.WriteErrorLine("arrêt du conduitd du banc impossible : $_")
    }
  }
  if ($autostartARestaurer) {
    try {
      $restauration = Invoke-Natif -Exe $Conduitd -Arguments @("autostart", "enable")
      if ($restauration.Code -ne 0) {
        $Host.UI.WriteErrorLine("RESTAURATION INCOMPLÈTE : « conduitd autostart enable » a échoué (code $($restauration.Code)) :`n$($restauration.Texte)")
      } else {
        Write-Host "Tâche « $tacheAutostart » réenregistrée."
      }
    } catch {
      $Host.UI.WriteErrorLine("RESTAURATION INCOMPLÈTE de la tâche « $tacheAutostart » : $_")
    }
  }
  foreach ($ligneArretee in $demonsArretes) {
    try {
      $decoupe = Split-LigneDeCommande -Ligne $ligneArretee
      if ($null -eq $decoupe) {
        $Host.UI.WriteErrorLine("conduitd arrêté NON relancé (ligne de commande illisible) : $ligneArretee")
        continue
      }
      if ($decoupe.Reste -eq "") {
        Start-Process -FilePath $decoupe.Exe | Out-Null
      } else {
        Start-Process -FilePath $decoupe.Exe -ArgumentList $decoupe.Reste | Out-Null
      }
      Write-Host "conduitd relancé : $ligneArretee"
    } catch {
      $Host.UI.WriteErrorLine("conduitd NON relancé (« $ligneArretee ») : $_")
    }
  }
  if ($script:lignesRecapitulatif.Count -gt 0) {
    try {
      [System.IO.File]::WriteAllText($recapitulatif,
        (($script:lignesRecapitulatif -join "`r`n") + "`r`n"),
        (New-Object System.Text.UTF8Encoding($false)))
    } catch {
      $Host.UI.WriteErrorLine("écriture de $recapitulatif impossible : $_")
    }
  }
  if ($null -ne $encodagePrecedent) {
    try { [Console]::OutputEncoding = $encodagePrecedent } catch { }
  }
}

if ($echec) {
  $Host.UI.WriteErrorLine("bench-boucle : ÉCHEC — $echec")
  exit 1
}
if (-not $verdict) { exit 1 }
exit 0
