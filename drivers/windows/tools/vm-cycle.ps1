<#
.SYNOPSIS
  Charge et décharge le pilote conduit_kmd N fois dans la VM de test (critère M1a-02).
.DESCRIPTION
  Étape 3/3 de docs/driver-dev.md §3, sur l'hôte, VM préparée par vm-prepare.ps1 et
  paquet produit par build.ps1. Par PowerShell Direct : copie le paquet et devgen.exe
  (WDK de l'hôte) dans l'invité, importe WDRLocalTestCert.cer dans Root et
  TrustedPublisher de la machine invitée, nettoie les restes d'une exécution précédente,
  puis répète Count fois :
    1. pnputil /add-driver conduit_kmd.inf /install
    2. devgen /add /hardwareid "Root\ConduitCable"   (identifiant d'instance récupéré)
    3. attente de Get-PnpDevice -InstanceId … en état OK (délai borné)
    4. devgen /remove <id>
    5. pnputil /delete-driver oemN.inf /uninstall /force   (oemN.inf via /enum-drivers)
    6. aucun événement Kernel-PnP d'erreur ni BugCheck depuis le début
  Tout devgen/pnputil qui échoue arrête le script avec sa sortie. À la fin, un
  MEMORY.DMP ou un minidump plus récent que le début est rapatrié dans
  drivers\windows\target\dumps\ et le script échoue. Sortie : itérations réussies et
  durée moyenne d'un cycle. Ce script ne lance jamais pnputil ni devgen sur l'hôte.
.PARAMETER Name
  Nom de la VM (ConduitTest).
.PARAMETER Credential
  Compte local administrateur de l'invité (Get-Credential test).
.PARAMETER Count
  Nombre de cycles (100).
.PARAMETER Package
  Dossier du paquet (défaut : target\<profil>\conduit_kmd_package du workspace noyau).
.PARAMETER Profile
  Profil Cargo du paquet par défaut : dev (target\debug) ou release.
.PARAMETER StartTimeoutSeconds
  Délai maximal pour que le périphérique passe en état OK après devgen /add (30 s).
.EXAMPLE
  .\vm-cycle.ps1 -Name ConduitTest -Credential (Get-Credential test) -Count 100
#>
[CmdletBinding()]
param(
  [string]$Name = "ConduitTest",
  [Parameter(Mandatory)][pscredential]$Credential,
  [ValidateRange(1, 100000)][int]$Count = 100,
  [string]$Package,
  [ValidateSet("dev", "release")][string]$Profile = "dev",
  [ValidateRange(5, 600)][int]$StartTimeoutSeconds = 30
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
Assert-Elevated -Reason "PowerShell Direct vers la VM"

$workspace = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent (Split-Path -Parent $workspace)
$hardwareId = "Root\ConduitCable"
$infName = "conduit_kmd.inf"
$guestDir = "C:\ConduitTest"
$dumpsDir = Join-Path $workspace "target\dumps"

# Paquet du pilote (build.ps1) : tout doit y être avant de toucher à la VM.
if (-not $Package) {
  $targetSubdir = if ($Profile -eq "release") { "release" } else { "debug" }
  $Package = Join-Path $workspace "target\$targetSubdir\conduit_kmd_package"
}
foreach ($file in @($infName, "conduit_kmd.sys", "conduit_kmd.cat", "WDRLocalTestCert.cer")) {
  if (-not (Test-Path -LiteralPath (Join-Path $Package $file))) {
    throw "Paquet incomplet : $file absent de $Package (lancer tools\build.ps1)."
  }
}

# devgen.exe vient du WDK de l'hôte (version épinglée dans versions.json).
$versions = Get-Content (Join-Path $repoRoot "packaging\windows\versions.json") -Raw | ConvertFrom-Json
$devgen = Get-DevgenPath -WdkVersion $versions.wdk

if (-not (Get-VM -Name $Name -ErrorAction SilentlyContinue)) {
  throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1 puis vm-prepare.ps1."
}

# Fonctions redéfinies dans l'invité à partir de leur texte.
$helpers = @("Invoke-NativeChecked", "ConvertFrom-DevgenAddOutput", "Find-PublishedInf") |
  ForEach-Object { Get-FunctionSource -Name $_ }
$helpers = $helpers -join "`n"

# Un cycle complet, exécuté dans l'invité. Renvoie l'identifiant d'instance et les
# oemN.inf supprimés ; lève une erreur détaillée sinon.
$cycleBlock = {
  param([string]$Helpers, [string]$GuestDir, [string]$InfName, [string]$HardwareId, [int]$StartTimeoutSeconds)
  Set-StrictMode -Version Latest
  $ErrorActionPreference = "Stop"
  . ([scriptblock]::Create($Helpers))
  $devgen = Join-Path $GuestDir "devgen.exe"
  $inf = Join-Path $GuestDir "package\$InfName"

  Invoke-NativeChecked pnputil @("/add-driver", $inf, "/install") | Out-Null

  $added = Invoke-NativeChecked $devgen @("/add", "/hardwareid", $HardwareId)
  $id = ConvertFrom-DevgenAddOutput -Output $added
  if (-not $id) { throw "Identifiant d'instance introuvable dans la sortie de devgen /add :`n$($added -join "`n")" }

  $deadline = (Get-Date).AddSeconds($StartTimeoutSeconds)
  $device = $null
  do {
    $device = Get-PnpDevice -InstanceId $id -ErrorAction SilentlyContinue
    if ($device -and $device.Status -eq "OK") { break }
    Start-Sleep -Milliseconds 250
  } while ((Get-Date) -lt $deadline)
  if (-not $device -or $device.Status -ne "OK") {
    $status = if ($device) { "état $($device.Status), problème $($device.Problem) ($($device.ProblemDescription))" } else { "périphérique introuvable" }
    throw "Le périphérique $id n'est pas passé en état OK en $StartTimeoutSeconds s : $status"
  }

  Invoke-NativeChecked $devgen @("/remove", $id) | Out-Null

  $enum = Invoke-NativeChecked pnputil @("/enum-drivers")
  $oems = @(Find-PublishedInf -Output $enum -OriginalName $InfName)
  if ($oems.Count -eq 0) { throw "Aucun oemN.inf pour $InfName dans pnputil /enum-drivers :`n$($enum -join "`n")" }
  foreach ($oem in $oems) {
    Invoke-NativeChecked pnputil @("/delete-driver", $oem, "/uninstall", "/force") | Out-Null
  }
  [pscustomobject]@{ InstanceId = $id; Oem = ($oems -join ",") }
}

# Événements Kernel-PnP d'erreur et BugCheck depuis $Since (heure de l'invité).
$eventsBlock = {
  param([datetime]$Since)
  $ErrorActionPreference = "Stop"
  $filters = @(
    @{ LogName = "System"; ProviderName = "Microsoft-Windows-Kernel-PnP"; Level = 1, 2; StartTime = $Since },
    @{ LogName = "Microsoft-Windows-Kernel-PnP/Configuration"; Level = 1, 2; StartTime = $Since },
    @{ LogName = "System"; Id = 1001, 6008; StartTime = $Since }
  )
  $found = @()
  foreach ($filter in $filters) {
    # « Aucun événement trouvé » est une erreur (localisée) pour Get-WinEvent, pas ici.
    $found += @(Get-WinEvent -FilterHashtable $filter -ErrorAction SilentlyContinue)
  }
  $found |
    Where-Object { $_.Id -ne 1001 -or $_.ProviderName -match "BugCheck|SystemErrorReporting" } |
    ForEach-Object { "{0:s} {1} {2} : {3}" -f $_.TimeCreated, $_.ProviderName, $_.Id, ($_.Message -replace "\s+", " ") }
}

# Vidages mémoire plus récents que $Since ; renvoie leurs chemins.
$dumpsBlock = {
  param([datetime]$Since)
  $ErrorActionPreference = "Stop"
  $candidates = @("$env:SystemRoot\MEMORY.DMP") + @(Get-ChildItem "$env:SystemRoot\Minidump\*.dmp" -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
  $candidates | Where-Object { (Test-Path $_) -and (Get-Item $_).LastWriteTime -gt $Since }
}

$durations = New-Object System.Collections.Generic.List[double]
$failure = $null
$guestStart = $null
$session = $null
try {
  Write-Host "Connexion à « $Name » par PowerShell Direct…"
  $session = New-GuestSession -Name $Name -Credential $Credential
  $guestStart = Invoke-Command -Session $session -ScriptBlock { Get-Date }

  Write-Host "Copie du paquet ($Package) et de devgen.exe vers $guestDir"
  Invoke-Command -Session $session -ArgumentList $guestDir -ScriptBlock {
    param([string]$GuestDir)
    if (Test-Path $GuestDir) { Remove-Item -Recurse -Force $GuestDir }
    New-Item -ItemType Directory -Path (Join-Path $GuestDir "package") | Out-Null
  }
  Copy-Item -ToSession $session -Path (Join-Path $Package "*") -Destination (Join-Path $guestDir "package") -Recurse
  Copy-Item -ToSession $session -Path $devgen -Destination (Join-Path $guestDir "devgen.exe")

  Write-Host "Certificat de test dans Root et TrustedPublisher de l'invité ; nettoyage des restes"
  Invoke-Command -Session $session -ArgumentList $helpers, $guestDir, $infName, $hardwareId -ScriptBlock {
    param([string]$Helpers, [string]$GuestDir, [string]$InfName, [string]$HardwareId)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    . ([scriptblock]::Create($Helpers))
    $cer = Join-Path $GuestDir "package\WDRLocalTestCert.cer"
    foreach ($store in @("Root", "TrustedPublisher")) {
      Import-Certificate -FilePath $cer -CertStoreLocation "Cert:\LocalMachine\$store" | Out-Null
    }
    # Restes d'une exécution interrompue : périphériques Root\ConduitCable, paquets oemN.inf.
    $leftovers = @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $HardwareId })
    foreach ($dev in $leftovers) {
      Write-Host "  retrait du périphérique restant $($dev.InstanceId)"
      Invoke-NativeChecked pnputil @("/remove-device", $dev.InstanceId) | Out-Null
    }
    $enum = Invoke-NativeChecked pnputil @("/enum-drivers")
    foreach ($oem in @(Find-PublishedInf -Output $enum -OriginalName $InfName)) {
      Write-Host "  suppression du paquet restant $oem"
      Invoke-NativeChecked pnputil @("/delete-driver", $oem, "/uninstall", "/force") | Out-Null
    }
  }

  Write-Host "Début des $Count cycles (heure invité : $guestStart)"
  for ($i = 1; $i -le $Count; $i++) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $result = Invoke-Command -Session $session -ScriptBlock $cycleBlock `
      -ArgumentList $helpers, $guestDir, $infName, $hardwareId, $StartTimeoutSeconds
    $events = @(Invoke-Command -Session $session -ScriptBlock $eventsBlock -ArgumentList $guestStart)
    if ($events.Count -gt 0) {
      throw "Cycle $i : événements d'erreur dans l'invité :`n$($events -join "`n")"
    }
    $watch.Stop()
    $durations.Add($watch.Elapsed.TotalSeconds)
    Write-Host ("{0}/{1} OK  ({2:n1} s, {3}, {4})" -f $i, $Count, $watch.Elapsed.TotalSeconds, $result.InstanceId, $result.Oem)
  }
} catch {
  $failure = $_
} finally {
  # Vidages : la session peut être morte (bug check) ; on en rouvre une, l'invité
  # redémarre seul (AutoReboot) et l'on rapatrie ce qui est plus récent que le début.
  if ($guestStart) {
    try {
      if (-not $session -or $session.State -ne "Opened") {
        Write-Host "Reconnexion à l'invité pour la collecte des vidages…"
        $session = New-GuestSession -Name $Name -Credential $Credential
      }
      $dumps = @(Invoke-Command -Session $session -ScriptBlock $dumpsBlock -ArgumentList $guestStart)
      if ($dumps.Count -gt 0) {
        New-Item -ItemType Directory -Force -Path $dumpsDir | Out-Null
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        foreach ($dump in $dumps) {
          $dest = Join-Path $dumpsDir ("{0}_{1}" -f $stamp, (Split-Path -Leaf $dump))
          Write-Host "Vidage $dump → $dest"
          Copy-Item -FromSession $session -Path $dump -Destination $dest
        }
        if (-not $failure) { $failure = "Vidage(s) mémoire produit(s) pendant les cycles : $($dumps -join ', ')" }
      }
    } catch {
      if (-not $failure) { $failure = $_ }
      else { $Host.UI.WriteErrorLine("collecte des vidages impossible : $_") }
    }
  }
  if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
}

$summary = Get-CycleSummary -Durations $durations.ToArray()
Write-Host ""
Write-Host ("Cycles réussis : {0}/{1} ; durée moyenne d'un cycle : {2} s (min {3}, max {4})" -f `
  $summary.Count, $Count, $summary.MeanSeconds, $summary.MinSeconds, $summary.MaxSeconds)
if ($failure) {
  $Host.UI.WriteErrorLine("vm-cycle : ÉCHEC — $failure")
  exit 1
}
Write-Host "vm-cycle : $Count cycles sans erreur."
