<#
.SYNOPSIS
  Fonctions partagées par les scripts de VM (vm-new.ps1, vm-prepare.ps1, vm-cycle.ps1).
.DESCRIPTION
  Les fonctions pures (analyse de sorties, génération de clé) sont testées par
  tests\vm-common.Tests.ps1 (Pester 3/4). Les autres touchent au système (élévation,
  registre, réseau de l'hôte) et ne sont pas testées.
#>
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Test-Elevated {
  <#
  .SYNOPSIS
    Vrai si le processus courant appartient au groupe Administrateurs.
  #>
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-Elevated {
  <#
  .SYNOPSIS
    Échoue avec un message clair si le script n'est pas lancé en administrateur.
  .PARAMETER Reason
    Ce que l'élévation autorise (affiché dans le message).
  #>
  param([Parameter(Mandatory)][string]$Reason)
  if (-not (Test-Elevated)) {
    throw "Ce script doit être lancé dans un PowerShell administrateur ($Reason)."
  }
}

function New-DebugKey {
  <#
  .SYNOPSIS
    Génère une clé de débogage noyau réseau (kdnet) : quatre groupes de neuf caractères
    en base 36 (0-9, a-z) séparés par des points, comme celles produites par kdnet.exe.
  #>
  $alphabet = "0123456789abcdefghijklmnopqrstuvwxyz".ToCharArray()
  $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
  $bytes = New-Object byte[] 36
  $rng.GetBytes($bytes)
  $chars = foreach ($b in $bytes) { $alphabet[$b % $alphabet.Length] }
  $groups = for ($i = 0; $i -lt 4; $i++) { -join $chars[($i * 9)..($i * 9 + 8)] }
  return ($groups -join ".")
}

function Test-DebugKey {
  <#
  .SYNOPSIS
    Vrai si la chaîne a le format d'une clé kdnet (quatre groupes alphanumériques
    séparés par des points).
  #>
  param([Parameter(Mandatory)][AllowEmptyString()][string]$Key)
  return $Key -match '^[0-9a-zA-Z]+\.[0-9a-zA-Z]+\.[0-9a-zA-Z]+\.[0-9a-zA-Z]+$'
}

function Invoke-NativeChecked {
  <#
  .SYNOPSIS
    Lance un exécutable natif (devgen, pnputil, bcdedit…) en capturant sa sortie, et
    échoue avec cette sortie si le code de retour n'est pas 0.
  .DESCRIPTION
    Windows PowerShell 5.1 transforme les lignes stderr d'un exécutable en erreurs
    terminantes quand `$ErrorActionPreference` vaut Stop et que `2>&1` est utilisé : la
    préférence est relâchée le temps de l'appel. Cette fonction est aussi envoyée telle
    quelle dans l'invité (voir Get-FunctionSource) par vm-prepare.ps1 et vm-cycle.ps1.
  .PARAMETER Exe
    Exécutable (nom dans le PATH ou chemin complet).
  .PARAMETER Arguments
    Arguments, un par élément.
  .OUTPUTS
    Les lignes de sortie (stdout et stderr) sous forme de chaînes.
  #>
  param(
    [Parameter(Mandatory)][string]$Exe,
    [AllowEmptyCollection()][string[]]$Arguments = @()
  )
  $previous = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $global:LASTEXITCODE = 0
    $out = @(& $Exe @Arguments 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $previous
  }
  if ($code -ne 0) {
    throw "« $Exe $($Arguments -join ' ') » a échoué (code $code) :`n$($out -join "`n")"
  }
  return $out
}

function Get-FunctionSource {
  <#
  .SYNOPSIS
    Texte `function X { … }` d'une fonction du module, pour la redéfinir dans l'invité
    (Invoke-Command ne transporte pas les fonctions de l'hôte).
  #>
  param([Parameter(Mandatory)][string]$Name)
  $command = Get-Command -Name $Name -CommandType Function
  return "function $Name {`n$($command.Definition)`n}"
}

function ConvertFrom-DevgenAddOutput {
  <#
  .SYNOPSIS
    Extrait l'identifiant d'instance imprimé par `devgen /add` (« Device node created.
    Instance ID: SWD\DEVGEN\{…} »). Indépendant de la langue : on cherche un chemin
    d'instance SWD\ ou ROOT\ n'importe où dans la sortie.
  .OUTPUTS
    L'identifiant d'instance, ou $null s'il est introuvable.
  #>
  param([Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][AllowNull()][string[]]$Output)
  foreach ($line in @($Output)) {
    if ($null -eq $line) { continue }
    if ($line -match '((?:SWD|ROOT)\\[^\s"]+)') { return $Matches[1] }
  }
  return $null
}

function Find-PublishedInf {
  <#
  .SYNOPSIS
    Retrouve le nom publié (oemN.inf) d'un paquet dans la sortie de
    `pnputil /enum-drivers`, à partir de son nom d'origine (conduit_kmd.inf).
  .DESCRIPTION
    La sortie est une suite de blocs « Étiquette : valeur » séparés par des lignes vides ;
    les étiquettes dépendent de la langue du système, pas les valeurs. Un bloc qui
    contient une valeur `oemN.inf` et une valeur égale au nom d'origine désigne le paquet.
  .OUTPUTS
    Les noms publiés trouvés (plusieurs si le paquet a été ajouté plusieurs fois).
  #>
  param(
    [Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][AllowNull()][string[]]$Output,
    [Parameter(Mandatory)][string]$OriginalName
  )
  $found = @()
  $published = $null
  $isOriginal = $false
  foreach ($line in @($Output) + @("")) {
    if ($null -eq $line -or $line.Trim() -eq "") {
      if ($published -and $isOriginal) { $found += $published }
      $published = $null
      $isOriginal = $false
      continue
    }
    if ($line -match '^[^:]+:\s*(\S.*?)\s*$') {
      $value = $Matches[1]
      if ($value -match '^oem\d+\.inf$') { $published = $value }
      elseif ($value -ieq $OriginalName) { $isOriginal = $true }
    }
  }
  return $found
}

function Get-CycleSummary {
  <#
  .SYNOPSIS
    Résume une série de durées de cycle (secondes) : nombre, moyenne, minimum, maximum.
  #>
  param([Parameter(Mandatory)][AllowEmptyCollection()][double[]]$Durations)
  $n = @($Durations).Count
  if ($n -eq 0) {
    return [pscustomobject]@{ Count = 0; MeanSeconds = 0.0; MinSeconds = 0.0; MaxSeconds = 0.0 }
  }
  $stats = $Durations | Measure-Object -Average -Minimum -Maximum
  return [pscustomobject]@{
    Count       = $n
    MeanSeconds = [math]::Round($stats.Average, 2)
    MinSeconds  = [math]::Round($stats.Minimum, 2)
    MaxSeconds  = [math]::Round($stats.Maximum, 2)
  }
}

function Get-WdkRoot {
  <#
  .SYNOPSIS
    Racine du WDK (KitsRoot10) lue dans le registre, comme le fait wdk-build.
  #>
  $key = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots"
  $root = (Get-ItemProperty -Path $key -Name KitsRoot10).KitsRoot10
  if (-not (Test-Path $root)) { throw "WDK introuvable : $root (clé KitsRoot10)" }
  return $root
}

function Get-DevgenPath {
  <#
  .SYNOPSIS
    Chemin de devgen.exe (WDK 26100, Tools\<version>\x64), à copier dans la VM.
  .PARAMETER WdkVersion
    Version du WDK (packaging\windows\versions.json, champ `wdk`).
  #>
  param([Parameter(Mandatory)][string]$WdkVersion)
  $path = Join-Path (Get-WdkRoot) "Tools\$WdkVersion\x64\devgen.exe"
  if (-not (Test-Path $path)) { throw "devgen.exe introuvable : $path" }
  return $path
}

function Get-DefaultSwitchHostIp {
  <#
  .SYNOPSIS
    Adresse IPv4 de l'hôte sur le commutateur « Default Switch » (celle que la VM joint
    pour le débogueur noyau réseau).
  #>
  $addr = Get-NetIPAddress -AddressFamily IPv4 |
    Where-Object { $_.InterfaceAlias -like "vEthernet (Default Switch)*" } |
    Select-Object -First 1
  if (-not $addr) { throw "Aucune adresse IPv4 sur « vEthernet (Default Switch) » : Hyper-V est-il actif ?" }
  return $addr.IPAddress
}

function Wait-VMHeartbeat {
  <#
  .SYNOPSIS
    Attend que l'invité réponde au service Heartbeat d'Hyper-V (VM démarrée et système
    chargé), avec un délai borné.
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [int]$TimeoutSeconds = 600
  )
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $vm = Get-VM -Name $Name
    if ($vm.State -eq "Running" -and $vm.Heartbeat -like "Ok*") { return }
    Start-Sleep -Seconds 5
  }
  throw "La VM « $Name » ne répond pas (Heartbeat) après $TimeoutSeconds s."
}

function Wait-VMReboot {
  <#
  .SYNOPSIS
    Après un `shutdown /r` dans l'invité : attend que le Heartbeat disparaisse (arrêt
    effectif, au plus deux minutes) puis revienne (système rechargé), avec délai borné.
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [int]$TimeoutSeconds = 600
  )
  $deadline = (Get-Date).AddSeconds(120)
  while ((Get-Date) -lt $deadline) {
    $vm = Get-VM -Name $Name
    if ($vm.State -ne "Running" -or $vm.Heartbeat -notlike "Ok*") { break }
    Start-Sleep -Seconds 2
  }
  Wait-VMHeartbeat -Name $Name -TimeoutSeconds $TimeoutSeconds
}

function New-GuestSession {
  <#
  .SYNOPSIS
    Ouvre une session PowerShell Direct vers l'invité, en réessayant jusqu'à ce que le
    service soit prêt (après un redémarrage par exemple).
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [Parameter(Mandatory)][pscredential]$Credential,
    [int]$TimeoutSeconds = 600
  )
  Wait-VMHeartbeat -Name $Name -TimeoutSeconds $TimeoutSeconds
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $lastError = $null
  while ((Get-Date) -lt $deadline) {
    try {
      return New-PSSession -VMName $Name -Credential $Credential -ErrorAction Stop
    } catch {
      $lastError = $_
      Start-Sleep -Seconds 5
    }
  }
  throw "Impossible d'ouvrir une session PowerShell Direct vers « $Name » : $lastError"
}

Export-ModuleMember -Function Test-Elevated, Assert-Elevated, New-DebugKey, Test-DebugKey,
  Invoke-NativeChecked, Get-FunctionSource, ConvertFrom-DevgenAddOutput, Find-PublishedInf,
  Get-CycleSummary, Get-WdkRoot, Get-DevgenPath, Get-DefaultSwitchHostIp, Wait-VMHeartbeat,
  Wait-VMReboot, New-GuestSession
