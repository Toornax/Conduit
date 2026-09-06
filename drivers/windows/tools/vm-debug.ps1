<#
.SYNOPSIS
  Attache le débogueur noyau (kd.exe) à la VM de test par canal nommé série, dans le bon
  ordre : le débogueur d'abord, la machine ensuite.
.DESCRIPTION
  Complément de vm-prepare.ps1 -DebugTransport serial (docs/driver-dev.md §4). Il
  automatise ce qui était jusqu'ici une consigne à retenir :

    LE DÉBOGUEUR DOIT TENIR LE CANAL NOMMÉ AVANT QUE LA MACHINE DÉMARRE.

  Une VM démarrée en premier ne se laisse pas rattraper : la connexion n'arrive jamais et
  la séance se perd à chercher pourquoi. Avec -StartVM, le script arrête la VM si besoin,
  lance kd.exe, ATTEND qu'il tienne le canal, puis seulement démarre la machine. L'ordre
  n'est plus une consigne mais un comportement.

  Cette règle vaut AUSSI pour un rattachement : sans -StartVM, sur une VM déjà en marche,
  la connexion ne s'établit pas (mesuré ; voir Get-DebuggerAttachWarning dans
  vm-common.psm1). Le script le dit avant de lancer kd, mais laisse faire : tenir le canal
  face à un invité figé garde son intérêt.

  kd.exe est lancé dans sa propre fenêtre (c'est une console interactive : on y tape
  `!analyze -v`, `lm m conduit*`, Ctrl+Inter pour interrompre l'invité) et journalise tout
  dans -LogPath. Les commandes initiales lèvent les masques de traces puis relâchent
  l'invité, pour que les kmd_log! du pilote apparaissent tout de suite ; le masque DEFAULT
  est aussi posé en registre par vm-prepare.ps1, qui lui vaut dès l'amorçage. LES DEUX SONT
  NÉCESSAIRES, l'un n'est pas un substitut de l'autre (voir -InitialCommands).

  Ce script ne touche jamais au débogage de l'hôte : il ne lance ni bcdedit, ni verifier.
.PARAMETER Name
  Nom de la VM (ConduitTest). Utilisé seulement avec -StartVM.
.PARAMETER Pipe
  Canal nommé partagé avec le port COM de la VM ; le même que -DebugPipe de vm-prepare.ps1.
.PARAMETER LogPath
  Journal du débogueur (défaut : drivers\windows\target\debug-logs\kd-<VM>-<horodatage>.log).
.PARAMETER StartVM
  Arrête la VM si nécessaire, puis la démarre une fois le débogueur en place.
.PARAMETER Follow
  Suit le journal dans cette fenêtre, en mettant en évidence les lignes du pilote.
.PARAMETER InitialCommands
  Commandes jouées par kd à la connexion. Par défaut : ouverture des masques de traces
  Kd_DEFAULT_Mask PUIS Kd_IHVDRIVER_Mask, puis `g`, qui relâche l'invité. C'est le masque
  DEFAULT qui porte nos traces — voir le commentaire du paramètre plus bas.
.PARAMETER HoldTimeoutSeconds
  Délai maximal d'attente que le débogueur tienne le canal (30 s).
.EXAMPLE
  .\vm-debug.ps1 -StartVM -Follow
.EXAMPLE
  .\vm-debug.ps1 -Pipe \\.\pipe\conduitdbg   # VM ÉTEINTE : la démarrer une fois kd en place
#>
[CmdletBinding()]
param(
  [string]$Name = "ConduitTest",
  [string]$Pipe = "\\.\pipe\conduitdbg",
  [string]$LogPath,
  [switch]$StartVM,
  [switch]$Follow,
  # Kd_DEFAULT_Mask D'ABORD, et c'est lui qui compte : kmd_log!
  # (drivers\windows\conduit-kmd\src\log.rs) passe par `wdk::println!`, qui appelle
  # **DbgPrint** — donc le composant DPFLTR_DEFAULT_ID, jamais DPFLTR_IHVDRIVER_ID, qui
  # exigerait un DbgPrintEx explicite. Kd_IHVDRIVER_Mask est conservé derrière, sans coût,
  # pour un éventuel DbgPrintEx futur.
  #
  # MESURÉ le 2026-09-06, pas déduit : avec les seules commandes d'origine
  # (`ed nt!Kd_IHVDRIVER_Mask 0xf`), une séance complète — débogueur connecté dès
  # l'amorçage, 100 cycles de chargement — n'a rendu AUCUNE trace du pilote, alors que les
  # DbgPrint d'autres composants passaient. Avec `ed nt!Kd_DEFAULT_Mask 0xf` en plus, la
  # séance suivante a immédiatement rendu les dix traces attendues (DriverEntry, AddDevice,
  # StartDevice, les quatre Init, DriverUnload…).
  #
  # Autre constat de la même séance : la valeur de registre « Debug Print Filter\DEFAULT »
  # valait DÉJÀ 0xFFFFFFFF dans l'invité pendant que la séance restait muette. Elle n'a
  # donc pas suffi à elle seule ; c'est le `ed` joué à la connexion qui a débloqué les
  # traces. Les DEUX sont nécessaires et aucun ne remplace l'autre : la valeur de registre
  # (posée par vm-prepare.ps1) vaut dès l'amorçage, avant que le débogueur puisse agir ;
  # le `ed` vaut pour la séance en cours. Constat de mesure, pas théorie : ne pas retirer
  # l'un des deux sans le remesurer.
  [string[]]$InitialCommands = @("ed nt!Kd_DEFAULT_Mask 0xf", "ed nt!Kd_IHVDRIVER_Mask 0xf", "g"),
  [ValidateRange(5, 600)][int]$HoldTimeoutSeconds = 30
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force

# Lignes mises en évidence par -Follow : les nôtres (traces kmd_log!, tag de pool Cndt) et
# ce qui annonce un plantage. Motif volontairement large : mieux vaut surligner une ligne
# de trop qu'en manquer une.
$highlight = '(?i)conduit|kmd_log|Cndt|bugcheck|\*\*\*'

$pipeName = Get-NamedPipeName -Pipe $Pipe
if (-not $pipeName) {
  throw "Canal nommé invalide « $Pipe » : attendu \\.\pipe\<nom> (ou un simple nom)."
}

# Hyper-V n'est sollicité que par -StartVM : sans lui, ce script n'est qu'un lanceur de
# débogueur et n'a aucune raison d'exiger les droits correspondants.
if ($StartVM) {
  Assert-HyperVAccess -Reason "arrêt et démarrage de la VM autour du débogueur"
  $vms = @(Invoke-HyperVChecked -What "inventaire des VM" -Script { Get-VM })
  if (-not ($vms | Where-Object { $_.Name -eq $Name })) {
    throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1."
  }
}

$kdPath = Get-KdPath
$workspace = Split-Path -Parent $PSScriptRoot
if (-not $LogPath) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $LogPath = Join-Path $workspace "target\debug-logs\kd-$Name-$stamp.log"
}
$logDir = Split-Path -Parent $LogPath
if ($logDir -and -not (Test-Path -LiteralPath $logDir)) {
  New-Item -ItemType Directory -Force -Path $logDir | Out-Null
}

# La VM doit être éteinte AVANT que le débogueur prenne le canal : c'est l'ordre qui rend
# la connexion possible.
if ($StartVM) {
  $state = (Get-VM -Name $Name).State
  if ($state -ne "Off") {
    Write-Host "Arrêt de « $Name » (état $state) avant de placer le débogueur…"
    Invoke-HyperVChecked -What "arrêt de la VM" -Script { Stop-VM -Name $Name -Force }
    Wait-VMOff -Name $Name
  }
}

# Rattachement à une VM déjà démarrée : mesuré inopérant (Get-DebuggerAttachWarning). On
# le dit AVANT de lancer kd, pour que personne n'attende une connexion qui ne viendra pas.
# L'état est lu au mieux : sans -StartVM ce script n'exige pas les droits Hyper-V (ni même
# qu'Hyper-V soit présent), et une lecture refusée ne doit pas empêcher de déboguer.
if (-not $StartVM) {
  $vmState = ""
  try {
    $vmState = [string](Get-VM -Name $Name -ErrorAction Stop).State
  } catch {
    $vmState = ""
  }
  $attachWarning = Get-DebuggerAttachWarning -State $vmState -StartVM $false -Name $Name
  if ($attachWarning) { Write-Warning $attachWarning }
}

$kd = Get-KdCommandLine -KdPath $kdPath -Pipe $Pipe -LogPath $LogPath -InitialCommands $InitialCommands
Write-Host "Débogueur : $($kd.CommandLine)"
$process = Start-Process -FilePath $kd.Path -ArgumentList $kd.ArgumentString -PassThru

# « Tenir le canal » se constate de deux façons, aucune ne dépendant de la langue : le
# canal apparaît dans l'espace de noms des canaux nommés, ou kd a écrit le chemin du canal
# dans son journal (« Opened \\.\pipe\… »). On cherche NOTRE chaîne, jamais un mot anglais
# ou traduit.
function Test-PipeHeld {
  param([Parameter(Mandatory)][string]$ShortName, [Parameter(Mandatory)][string]$Log, [Parameter(Mandatory)][string]$Full)
  try {
    # Get-NamedPipeName et non Split-Path : ce dernier rend une chaîne VIDE sur
    # « \\.\pipe\nom », qu'il prend pour la racine d'un partage réseau (vérifié).
    foreach ($entry in [System.IO.Directory]::GetFileSystemEntries("\\.\pipe\")) {
      if ((Get-NamedPipeName -Pipe $entry) -ieq $ShortName) { return $true }
    }
  } catch {
    # L'énumération des canaux peut être refusée : le journal reste le second témoin.
  }
  if (Test-Path -LiteralPath $Log) {
    $content = Get-Content -LiteralPath $Log -Raw -ErrorAction SilentlyContinue
    if ($content -and $content.Contains($Full)) { return $true }
  }
  return $false
}

Write-Host "Attente que le débogueur tienne le canal $Pipe…"
$deadline = (Get-Date).AddSeconds($HoldTimeoutSeconds)
$held = $false
while ((Get-Date) -lt $deadline) {
  if ($process.HasExited) {
    $tail = ""
    if (Test-Path -LiteralPath $LogPath) { $tail = (Get-Content -LiteralPath $LogPath -Tail 20) -join "`n" }
    throw "kd.exe s'est arrêté aussitôt (code $($process.ExitCode)) : le canal n'a pas été pris.`n$tail"
  }
  if (Test-PipeHeld -ShortName $pipeName -Log $LogPath -Full $Pipe) { $held = $true; break }
  Start-Sleep -Milliseconds 500
}
if ($held) {
  Write-Host "Canal tenu par kd.exe (PID $($process.Id))."
} else {
  # Le débogueur tourne toujours : on continue, mais en le disant. Bloquer ici priverait
  # d'une séance de débogage pour un témoin manquant, pas pour un vrai problème.
  Write-Warning "Le canal $Pipe n'a pu être constaté en $HoldTimeoutSeconds s, mais kd.exe tourne (PID $($process.Id)) : on poursuit. Si la connexion n'arrive pas, vérifier que la VM a bien un port COM sur ce canal (Get-VMComPort -VMName $Name)."
}

if ($StartVM) {
  Write-Host "Démarrage de « $Name » (le débogueur est en place)…"
  Invoke-HyperVChecked -What "démarrage de la VM" -Script { Start-VM -Name $Name }
}

Write-Host ""
Write-Host "Journal : $LogPath"
Write-Host "Dans la fenêtre de kd.exe : Ctrl+Inter interrompt l'invité, « g » le relâche,"
Write-Host "  !analyze -v après un écran bleu, lm m conduit* pour les symboles,"
Write-Host "  .sympath+ <dossier du paquet> puis .reload pour charger le .pdb du pilote."
Write-Host "Fermer cette fenêtre met fin à la session de débogage."
if (-not $StartVM) {
  Write-Host ""
  Write-Host "La VM n'a pas été démarrée par ce script. Si elle est éteinte, la démarrer"
  Write-Host "MAINTENANT (le débogueur tient déjà le canal) : Start-VM -Name $Name"
  Write-Host "— ou relancer avec -StartVM, qui enchaîne l'ordre tout seul."
  Write-Host "Si elle tourne DÉJÀ, la connexion n'aboutira probablement pas (mesuré) : kd"
  Write-Host "restera sur « Waiting to reconnect... ». Il faut alors -StartVM."
}

if ($Follow) {
  Write-Host ""
  Write-Host "Suivi du journal (Ctrl+C pour arrêter le suivi ; kd.exe continue) :"
  $waitFile = (Get-Date).AddSeconds(30)
  while (-not (Test-Path -LiteralPath $LogPath) -and (Get-Date) -lt $waitFile) {
    Start-Sleep -Milliseconds 500
  }
  if (-not (Test-Path -LiteralPath $LogPath)) {
    Write-Warning "Journal $LogPath toujours absent : rien à suivre."
  } else {
    Get-Content -LiteralPath $LogPath -Tail 40 -Wait | ForEach-Object {
      if ($_ -match $highlight) { Write-Host $_ -ForegroundColor Green }
      else { Write-Host $_ }
    }
  }
}
