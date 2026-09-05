<#
.SYNOPSIS
  Prépare la VM de test au chargement de pilotes de test et au débogage noyau.
.DESCRIPTION
  Étape 2/3 de docs/driver-dev.md §3, à lancer sur l'hôte une fois Windows 11 installé
  dans la VM (compte local administrateur). Par PowerShell Direct, dans l'invité :
  `bcdedit /set testsigning on`, `bcdedit /debug on`, `bcdedit /dbgsettings net` (hôte,
  port, clé), vidage noyau complet (CrashDumpEnabled = 2), veille et hibernation
  désactivées ; puis redémarre l'invité, vérifie que testsigning est actif et prend le
  point de contrôle « propre » (Checkpoint-VM) auquel revenir après un plantage
  inexpliqué. Ce script ne lance jamais bcdedit sur l'hôte.
.PARAMETER Name
  Nom de la VM (ConduitTest).
.PARAMETER Credential
  Compte local administrateur de l'invité (Get-Credential test).
.PARAMETER HostIp
  Adresse IPv4 de l'hôte vue depuis la VM (défaut : celle de « vEthernet (Default Switch) »).
.PARAMETER DebugPort
  Port UDP du débogueur noyau réseau (50000 ; plage autorisée 49152-65535).
.PARAMETER DebugKey
  Clé kdnet (a.b.c.d) ; générée et affichée si absente.
.EXAMPLE
  .\vm-prepare.ps1 -Name ConduitTest -Credential (Get-Credential test)
#>
[CmdletBinding()]
param(
  [string]$Name = "ConduitTest",
  [Parameter(Mandatory)][pscredential]$Credential,
  [string]$HostIp,
  [ValidateRange(49152, 65535)][int]$DebugPort = 50000,
  [string]$DebugKey
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
Assert-Elevated -Reason "PowerShell Direct et Checkpoint-VM"

if (-not (Get-VM -Name $Name -ErrorAction SilentlyContinue)) {
  throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1."
}
if (-not $HostIp) { $HostIp = Get-DefaultSwitchHostIp }
if (-not $DebugKey) { $DebugKey = New-DebugKey }
if (-not (Test-DebugKey -Key $DebugKey)) {
  throw "Clé de débogage invalide « $DebugKey » : attendu quatre groupes alphanumériques séparés par des points."
}

# Invoke-NativeChecked est redéfinie dans l'invité à partir de son texte.
$helpers = Get-FunctionSource -Name Invoke-NativeChecked

Write-Host "Connexion à « $Name » par PowerShell Direct…"
$session = New-GuestSession -Name $Name -Credential $Credential
try {
  Invoke-Command -Session $session -ArgumentList $helpers, $HostIp, $DebugPort, $DebugKey -ScriptBlock {
    param([string]$Helpers, [string]$HostIp, [int]$DebugPort, [string]$DebugKey)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    . ([scriptblock]::Create($Helpers))

    Write-Host "  bcdedit : testsigning, debug, dbgsettings net"
    Invoke-NativeChecked bcdedit @("/set", "testsigning", "on") | Out-Null
    Invoke-NativeChecked bcdedit @("/debug", "on") | Out-Null
    Invoke-NativeChecked bcdedit @("/dbgsettings", "net", "hostip:$HostIp", "port:$DebugPort", "key:$DebugKey") | Out-Null

    Write-Host "  CrashControl : CrashDumpEnabled = 2 (vidage noyau), redémarrage automatique"
    $crash = "HKLM:\SYSTEM\CurrentControlSet\Control\CrashControl"
    Set-ItemProperty -Path $crash -Name CrashDumpEnabled -Value 2 -Type DWord
    Set-ItemProperty -Path $crash -Name AutoReboot -Value 1 -Type DWord

    Write-Host "  Alimentation : veille, écran et hibernation désactivés"
    Invoke-NativeChecked powercfg @("/change", "standby-timeout-ac", "0") | Out-Null
    Invoke-NativeChecked powercfg @("/change", "monitor-timeout-ac", "0") | Out-Null
    Invoke-NativeChecked powercfg @("/hibernate", "off") | Out-Null

    Write-Host "  Redémarrage de l'invité"
    Invoke-NativeChecked shutdown @("/r", "/t", "3") | Out-Null
  }
} finally {
  Remove-PSSession $session -ErrorAction SilentlyContinue
}

Write-Host "Attente du redémarrage…"
Wait-VMReboot -Name $Name
$session = New-GuestSession -Name $Name -Credential $Credential
try {
  $state = Invoke-Command -Session $session -ScriptBlock {
    (bcdedit /enum "{current}" | Out-String)
  }
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
Write-Host "  Débogueur noyau réseau : hôte $HostIp, port $DebugPort"
Write-Host "  Clé : $DebugKey   (à conserver ; WinDbg → Attach to kernel → Net)"
Write-Host "  Équivalent : windbg -k net:port=$DebugPort,key=$DebugKey"
Write-Host "  Retour à l'état propre : Restore-VMCheckpoint -VMName $Name -Name $checkpoint -Confirm:`$false"
Write-Host "  Suite : .\vm-cycle.ps1 -Name $Name -Credential `$cred"
