<#
.SYNOPSIS
  Crée la VM Hyper-V de test du pilote (Windows 11, génération 2, Secure Boot désactivé).
.DESCRIPTION
  Étape 1/3 de docs/driver-dev.md §3. Crée une VM de génération 2 sur « Default Switch »
  avec un VHDX dynamique, l'ISO Windows 11 en premier périphérique de démarrage, un vTPM
  (exigé par l'installeur de Windows 11), Secure Boot désactivé (sinon `testsigning` est
  refusé), sans points de contrôle automatiques, avec l'interface de services invité
  (Copy-VMFile), puis la démarre et affiche la marche à suivre pour l'installation.
  Ensuite : vm-prepare.ps1, puis vm-cycle.ps1.
.PARAMETER IsoPath
  ISO officielle de Windows 11 (téléchargée depuis microsoft.com).
.PARAMETER Name
  Nom de la VM (ConduitTest).
.PARAMETER MemoryGB
  Mémoire (4 Go, minimum de Windows 11).
.PARAMETER Cpu
  Nombre de processeurs virtuels (2, minimum de Windows 11).
.PARAMETER DiskGB
  Taille maximale du VHDX dynamique (64 Go).
.PARAMETER Path
  Dossier des fichiers de la VM et du disque (défaut : dossier par défaut d'Hyper-V).
.EXAMPLE
  .\vm-new.ps1 -IsoPath C:\iso\Win11_French_x64.iso
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$IsoPath,
  [string]$Name = "ConduitTest",
  [ValidateRange(4, 64)][int]$MemoryGB = 4,
  [ValidateRange(2, 32)][int]$Cpu = 2,
  [ValidateRange(32, 1024)][int]$DiskGB = 64,
  [string]$Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
Assert-Elevated -Reason "création d'une VM Hyper-V"

if (-not (Test-Path -LiteralPath $IsoPath -PathType Leaf)) {
  throw "ISO introuvable : $IsoPath"
}
$IsoPath = (Resolve-Path -LiteralPath $IsoPath).Path

if (Get-VM -Name $Name -ErrorAction SilentlyContinue) {
  throw "Une VM « $Name » existe déjà : la supprimer (Remove-VM) ou choisir un autre -Name."
}

$switch = "Default Switch"
if (-not (Get-VMSwitch -Name $switch -ErrorAction SilentlyContinue)) {
  throw "Commutateur virtuel « $switch » introuvable : Hyper-V est-il activé (et la machine redémarrée) ?"
}

# Dossier des fichiers de la VM et du disque.
if (-not $Path) { $Path = (Get-VMHost).VirtualMachinePath }
if (-not (Test-Path -LiteralPath $Path)) { New-Item -ItemType Directory -Path $Path | Out-Null }
$vhdx = Join-Path $Path "$Name.vhdx"
if (Test-Path -LiteralPath $vhdx) { throw "Le disque $vhdx existe déjà." }

Write-Host "Création de la VM « $Name » (gen 2, $MemoryGB Go, $Cpu vCPU, $DiskGB Go) dans $Path"
$vm = New-VM -Name $Name -Generation 2 -MemoryStartupBytes ($MemoryGB * 1GB) `
  -NewVHDPath $vhdx -NewVHDSizeBytes ($DiskGB * 1GB) -SwitchName $switch -Path $Path

try {
  Set-VMProcessor -VM $vm -Count $Cpu
  Set-VMMemory -VM $vm -DynamicMemoryEnabled $false

  # Secure Boot désactivé : `bcdedit /set testsigning on` est refusé sinon. Le vTPM reste
  # exigé par l'installeur de Windows 11 (vérification matérielle).
  Set-VMFirmware -VM $vm -EnableSecureBoot Off
  Set-VMKeyProtector -VM $vm -NewLocalKeyProtector
  Enable-VMTPM -VM $vm

  # ISO en premier périphérique de démarrage.
  $dvd = Add-VMDvdDrive -VM $vm -Path $IsoPath -Passthru
  Set-VMFirmware -VM $vm -FirstBootDevice $dvd

  # Points de contrôle manuels seulement (`propre`, pris par vm-prepare.ps1).
  Set-VM -VM $vm -AutomaticCheckpointsEnabled $false -CheckpointType Standard

  # Interface de services invité : Copy-VMFile hôte → invité.
  Enable-VMIntegrationService -VM $vm -Name "Guest Service Interface"

  Start-VM -VM $vm
} catch {
  # Ne pas laisser une VM à moitié configurée.
  Remove-VM -Name $Name -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $vhdx -Force -ErrorAction SilentlyContinue
  throw
}

$vmconnect = Join-Path $env:SystemRoot "System32\vmconnect.exe"
if (Test-Path $vmconnect) { Start-Process $vmconnect -ArgumentList "localhost", $Name }

Write-Host ""
Write-Host "VM « $Name » démarrée sur l'ISO. Marche à suivre dans la console (vmconnect) :"
Write-Host "  1. Appuyer sur une touche dès l'invite « Press any key to boot from CD or DVD »."
Write-Host "  2. Installer Windows 11 (édition Pro conseillée) sur le disque unique."
Write-Host "  3. À l'étape du compte : choisir un compte LOCAL, sans compte Microsoft"
Write-Host "     (au besoin : Maj+F10, `OOBE\BYPASSNRO`, la VM redémarre, puis « Je n'ai pas Internet »)."
Write-Host "     Nom d'utilisateur : test (administrateur), mot de passe au choix."
Write-Host "  4. Une fois sur le bureau, sur l'hôte (PowerShell administrateur) :"
Write-Host "       `$cred = Get-Credential test"
Write-Host "       .\vm-prepare.ps1 -Name $Name -Credential `$cred"
Write-Host "  L'ISO peut ensuite être retirée : Set-VMDvdDrive -VMName $Name -Path `$null"
