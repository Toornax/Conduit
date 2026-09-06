<#
.SYNOPSIS
  Prépare la VM de test au chargement de pilotes de test et au débogage noyau.
.DESCRIPTION
  Étape 2/3 de docs/driver-dev.md §3, à lancer sur l'hôte une fois Windows 11 installé
  dans la VM (compte local administrateur). Par PowerShell Direct, dans l'invité :
  `bcdedit /set testsigning on`, `bcdedit /debug on`, le transport du débogueur
  (`bcdedit /dbgsettings serial` par défaut, `net` sur demande), le filtre de traces du
  noyau (Debug Print Filter, qui ouvre les traces des composants du système), vidage
  noyau complet (CrashDumpEnabled = 2), veille et hibernation désactivées ; puis, en
  série, attache le canal nommé au port COM de la VM (Set-VMComPort, VM arrêtée) ;
  redémarre l'invité, vérifie que testsigning est actif et prend le point de contrôle
  « propre » (Checkpoint-VM) auquel revenir après un plantage inexpliqué. Ce script ne
  lance jamais bcdedit sur l'hôte.

  Le transport SÉRIE est le défaut parce que c'est le seul qui se soit jamais connecté
  sur ce poste : le transport réseau (kdnet) reste disponible avec -DebugTransport net,
  mais n'a jamais abouti ici.
.PARAMETER Name
  Nom de la VM (ConduitTest).
.PARAMETER Credential
  Compte local administrateur de l'invité (Get-Credential nathan).
.PARAMETER DebugTransport
  Transport du débogueur noyau : « serial » (canal nommé, défaut, celui qui marche) ou
  « net » (kdnet, jamais parvenu à se connecter sur ce poste).
.PARAMETER DebugPipe
  Canal nommé côté hôte, en série (\\.\pipe\conduitdbg). Le même que -Pipe de vm-debug.ps1.
.PARAMETER DebugComPort
  Numéro du port COM de la VM (1) ; l'invité reçoit `debugport:<n>` à 115200 bauds.
.PARAMETER HostIp
  Réseau seulement : adresse IPv4 de l'hôte vue depuis la VM (défaut : celle de
  « vEthernet (Default Switch) »).
.PARAMETER DebugPort
  Réseau seulement : port UDP du débogueur (50000 ; plage autorisée 49152-65535).
.PARAMETER DebugKey
  Réseau seulement : clé kdnet (a.b.c.d) ; générée et affichée si absente.
.EXAMPLE
  .\vm-prepare.ps1 -Name ConduitTest -Credential (Get-Credential nathan)
.EXAMPLE
  .\vm-prepare.ps1 -Credential $cred -DebugTransport net   # transport historique
#>
[CmdletBinding()]
param(
  [string]$Name = "ConduitTest",
  [Parameter(Mandatory)][pscredential]$Credential,
  [ValidateSet("serial", "net")][string]$DebugTransport = "serial",
  [string]$DebugPipe = "\\.\pipe\conduitdbg",
  [ValidateRange(1, 2)][int]$DebugComPort = 1,
  [string]$HostIp,
  [ValidateRange(49152, 65535)][int]$DebugPort = 50000,
  [string]$DebugKey
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
# PowerShell Direct exige d'être « logged into the host computer as a Hyper-V
# administrator » : le groupe Administrateurs Hyper-V suffit (voir vm-common.psm1).
Assert-HyperVAccess -Reason "PowerShell Direct et Checkpoint-VM"

# Inventaire puis filtrage par nom, sans -ErrorAction SilentlyContinue, qui avalerait un
# refus d'accès en le présentant comme une VM introuvable.
$vms = @(Invoke-HyperVChecked -What "inventaire des VM" -Script { Get-VM })
if (-not ($vms | Where-Object { $_.Name -eq $Name })) {
  throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1."
}
$serial = ($DebugTransport -eq "serial")
if ($serial) {
  # Le nom court sert à vm-debug.ps1 pour vérifier que le débogueur tient le canal ;
  # une valeur mal formée doit être refusée ici, pas trois étapes plus loin.
  if (-not (Get-NamedPipeName -Pipe $DebugPipe)) {
    throw "Canal nommé invalide « $DebugPipe » : attendu \\.\pipe\<nom> (ou un simple nom)."
  }
} else {
  if (-not $HostIp) { $HostIp = Get-DefaultSwitchHostIp }
  if (-not $DebugKey) { $DebugKey = New-DebugKey }
  if (-not (Test-DebugKey -Key $DebugKey)) {
    throw "Clé de débogage invalide « $DebugKey » : attendu quatre groupes alphanumériques séparés par des points."
  }
}

# Invoke-NativeChecked est redéfinie dans l'invité à partir de son texte.
$helpers = Get-FunctionSource -Name Invoke-NativeChecked

Write-Host "Connexion à « $Name » par PowerShell Direct…"
$session = New-GuestSession -Name $Name -Credential $Credential
try {
  Invoke-Command -Session $session -ScriptBlock {
    param([string]$Helpers, [string]$Transport, [int]$ComPort, [string]$HostIp, [int]$DebugPort, [string]$DebugKey)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    . ([scriptblock]::Create($Helpers))

    Write-Host "  bcdedit : testsigning, debug, dbgsettings $Transport"
    Invoke-NativeChecked bcdedit @("/set", "testsigning", "on") | Out-Null
    Invoke-NativeChecked bcdedit @("/debug", "on") | Out-Null
    if ($Transport -eq "serial") {
      Invoke-NativeChecked bcdedit @("/dbgsettings", "serial", "debugport:$ComPort", "baudrate:115200") | Out-Null
    } else {
      Invoke-NativeChecked bcdedit @("/dbgsettings", "net", "hostip:$HostIp", "port:$DebugPort", "key:$DebugKey") | Out-Null
    }

    # Filtre de traces du noyau. Depuis Windows Vista, DbgPrint et DbgPrintEx sont filtrés
    # par défaut : SEUL le niveau erreur (bit 0) est armé, les autres niveaux sont perdus.
    # DEFAULT = 0xF ouvre les quatre niveaux (erreur, avertissement, trace, information)
    # dès l'amorçage — donc pour DriverEntry — et pour tous les démarrages suivants.
    #
    # LES TRACES DU PILOTE N'EN DÉPENDENT PLUS : kmd_log! (conduit-kmd\src\log.rs) émet
    # par DbgPrintEx au niveau DPFLTR_ERROR_LEVEL, justement parce que c'est le seul armé
    # par défaut ; les kmd_log! arrivent donc sans cette valeur ni la commande `ed` de
    # vm-debug.ps1. La pose est conservée parce qu'elle reste utile aux traces des AUTRES
    # composants du système, qui, elles, sortent aux niveaux filtrés.
    #
    # Question ouverte, à ne pas transformer en explication : avec l'implémentation
    # précédente (DbgPrint, composant DPFLTR_DEFAULT_ID, niveau information), la livraison
    # des traces s'est révélée INTERMITTENTE À CONFIGURATION IDENTIQUE — deux séances de
    # 100 cycles muettes encadrant une séance de dix traces, cette valeur de registre
    # valant 0xFFFFFFFF dans les trois cas (détail dans conduit-kmd\src\log.rs et au
    # paramètre -InitialCommands de vm-debug.ps1). La cause n'a pas été identifiée.
    # La clé n'existe pas sur une installation neuve : la créer.
    Write-Host "  Debug Print Filter : DEFAULT = 0xF (traces des composants du système)"
    $filter = "HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print Filter"
    if (-not (Test-Path -LiteralPath $filter)) { New-Item -Path $filter -Force | Out-Null }
    Set-ItemProperty -LiteralPath $filter -Name DEFAULT -Value 0xF -Type DWord

    Write-Host "  CrashControl : CrashDumpEnabled = 2 (vidage noyau), redémarrage automatique"
    $crash = "HKLM:\SYSTEM\CurrentControlSet\Control\CrashControl"
    Set-ItemProperty -Path $crash -Name CrashDumpEnabled -Value 2 -Type DWord
    Set-ItemProperty -Path $crash -Name AutoReboot -Value 1 -Type DWord

    Write-Host "  Alimentation : veille, écran et hibernation désactivés"
    Invoke-NativeChecked powercfg @("/change", "standby-timeout-ac", "0") | Out-Null
    Invoke-NativeChecked powercfg @("/change", "monitor-timeout-ac", "0") | Out-Null
    Invoke-NativeChecked powercfg @("/hibernate", "off") | Out-Null

    if ($Transport -eq "serial") {
      # Arrêt complet, et non redémarrage : Set-VMComPort exige la VM éteinte.
      Write-Host "  Arrêt de l'invité (le port COM s'attache VM éteinte)"
      Invoke-NativeChecked shutdown @("/s", "/t", "3") | Out-Null
    } else {
      Write-Host "  Redémarrage de l'invité"
      Invoke-NativeChecked shutdown @("/r", "/t", "3") | Out-Null
    }
  } -ArgumentList $helpers, $DebugTransport, $DebugComPort, $HostIp, $DebugPort, $DebugKey
} finally {
  Remove-PSSession $session -ErrorAction SilentlyContinue
}

if ($serial) {
  Write-Host "Attente de l'arrêt complet…"
  Wait-VMOff -Name $Name
  # Set-VMComPort refuse toute VM allumée ; Wait-VMOff a garanti l'état Off, et le message
  # ci-dessous dit quoi faire si l'arrêt n'a pas abouti (invité figé, mise à jour en cours).
  $vmState = (Get-VM -Name $Name).State
  if ($vmState -ne "Off") {
    throw @(
      "La VM « $Name » est dans l'état $vmState : Set-VMComPort exige une VM ARRÊTÉE."
      "L'arrêter (Stop-VM -Name $Name), vérifier qu'aucune mise à jour n'est en cours dans"
      "l'invité, puis relancer ce script."
    ) -join "`n"
  }
  Write-Host "Canal nommé $DebugPipe sur le port COM $DebugComPort de « $Name »"
  Invoke-HyperVChecked -What "attachement du canal nommé au port COM" -Script {
    Set-VMComPort -VMName $Name -Number $DebugComPort -Path $DebugPipe
  }
  Write-Host "Redémarrage de l'invité…"
  Start-VM -Name $Name
  Wait-VMHeartbeat -Name $Name
} else {
  Write-Host "Attente du redémarrage…"
  Wait-VMReboot -Name $Name
}
$session = New-GuestSession -Name $Name -Credential $Credential
try {
  $state = Invoke-Command -Session $session -ScriptBlock {
    (bcdedit /enum "{current}" | Out-String)
  }
  # « Yes » et non « Oui » : bcdedit traduit ses en-têtes et la description de l'entrée,
  # mais **pas** les noms d'éléments ni les valeurs booléennes. Vérifié le 2026-09-06 sur
  # la VM ConduitTest, installée en français (`locale fr-FR`), qui rend bien
  # « testsigning             Yes ». Ce n'est donc pas une exception à la règle du dépôt
  # sur les textes traduits : ne pas « corriger » ce motif sans le remesurer.
  if ($state -notmatch "testsigning\s+Yes") {
    throw "testsigning n'est pas actif après redémarrage (Secure Boot encore actif ?) :`n$state"
  }
} finally {
  Remove-PSSession $session -ErrorAction SilentlyContinue
}

$checkpoint = "propre"
Get-VMCheckpoint -VMName $Name -Name $checkpoint -ErrorAction SilentlyContinue |
  Remove-VMCheckpoint -ErrorAction SilentlyContinue
Checkpoint-VM -Name $Name -SnapshotName $checkpoint
Write-Host ""
Write-Host "VM « $Name » prête ; point de contrôle « $checkpoint » pris."
if ($serial) {
  Write-Host "  Débogueur noyau série : canal $DebugPipe, port COM $DebugComPort, 115200 bauds"
} else {
  Write-Host "  Débogueur noyau réseau : hôte $HostIp, port $DebugPort"
  Write-Host "  Clé : $DebugKey   (à conserver ; WinDbg → Attach to kernel → Net)"
  Write-Host "  Équivalent : windbg -k net:port=$DebugPort,key=$DebugKey"
}
Write-Host "  Traces du pilote : elles arrivent SANS CONFIGURATION (niveau « erreur », le seul"
Write-Host "  armé par défaut). Debug Print Filter DEFAULT = 0xF est posé quand même, pour les"
Write-Host "  traces des autres composants du système ; vm-debug.ps1 ouvre les mêmes masques."
Write-Host "  Retour à l'état propre : Restore-VMCheckpoint -VMName $Name -Name $checkpoint -Confirm:`$false"
Write-Host ""
Write-Host "SUITE — attacher le débogueur noyau :"
if ($serial) {
  Write-Host "  .\vm-debug.ps1 -Name $Name -Pipe $DebugPipe -StartVM -Follow"
  Write-Host ""
  Write-Host "  LA RÈGLE À NE PAS OUBLIER : le débogueur doit TENIR LE CANAL NOMMÉ AVANT que"
  Write-Host "  la machine démarre. Une VM démarrée en premier ne se laisse pas rattraper :"
  Write-Host "  la connexion n'arrive jamais et la séance se perd à chercher pourquoi."
  Write-Host "  C'est tout l'objet de -StartVM : vm-debug.ps1 arrête la VM, lance kd.exe,"
  Write-Host "  attend qu'il tienne le canal, PUIS démarre la machine. Rien à retenir."
  Write-Host ""
  Write-Host "  À la main, dans cet ordre et pas un autre :"
  Write-Host "    1. Stop-VM -Name $Name"
  Write-Host "    2. kd.exe -k com:pipe,port=$DebugPipe,resets=0,reconnect"
  Write-Host "    3. Start-VM -Name $Name"
} else {
  Write-Host "  .\vm-debug.ps1 attend un canal nommé : ce transport réseau se raccroche à la main"
  Write-Host "  (WinDbg → Attach to kernel → Net, port $DebugPort, clé ci-dessus)."
  Write-Host "  Rappel : ce transport ne s'est jamais connecté sur ce poste ; le transport série"
  Write-Host "  est le défaut (relancer sans -DebugTransport net)."
}
Write-Host ""
Write-Host "  Puis : .\vm-cycle.ps1 -Name $Name -Credential `$cred"
