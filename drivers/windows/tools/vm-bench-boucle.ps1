<#
.SYNOPSIS
  Lance le banc de boucle M1b-31 dans la VM de test, depuis l'hôte, et rapatrie ses
  mesures. Une seule commande pour la recette « xruns = 0 sur 10 min ».
.DESCRIPTION
  Enchaîne, dans cet ordre :

    1. construction des trois binaires en CRT STATIQUE, dans un target-dir séparé ;
    2. session PowerShell Direct vers l'invité (New-GuestSession de vm-common.psm1) ;
    3. copie de conduitd.exe, conduitctl.exe, conduit-helper.exe et bench-boucle.ps1
       dans C:\ConduitTest\ ;
    4. exécution de bench-boucle.ps1 PAR vm-run-console.ps1, donc dans la SESSION
       CONSOLE de l'invité — la seule où une mesure audio ait un sens ;
    5. rapatriement de serie.jsonl, conduitd.log, conduitd.err et recapitulatif.txt dans
       un dossier local horodaté, puis impression du récapitulatif.

  L'INVITÉ N'A PAS LE RUNTIME VISUAL C++. Un binaire Rust construit normalement dépend de
  VCRUNTIME140.dll et meurt dans l'invité avec le code -1073741515 (0xC0000135,
  STATUS_DLL_NOT_FOUND) sans dire quelle DLL manque. D'où RUSTFLAGS=-C
  target-feature=+crt-static, et un --target-dir séparé pour ne pas reconstruire tout le
  workspace au prochain changement de RUSTFLAGS.

  PIÈGE À NE JAMAIS REFAIRE : ne PAS combiner -Path et -RemoteExecutable dans l'appel à
  vm-run-console.ps1. Quand -Path est donné, la destination de la copie est CALCULÉE
  depuis -RemoteExecutable ; avec -RemoteExecutable pointant sur powershell.exe, la copie
  écraserait C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe dans l'invité.
  Ici, les fichiers sont copiés par Copy-Item -ToSession (étape 3) et vm-run-console.ps1
  n'a QUE -RemoteExecutable.

  Prérequis dans l'invité, à faire une fois : le pilote conduit_kmd chargé (vm-cycle.ps1)
  et le service d'assistance installé depuis une invite ÉLEVÉE
  (« conduit-helper installer --demarrer »). Une session Windows doit être ouverte À
  L'ÉCRAN avec le compte de -Credential, mode session étendue de vmconnect DÉSACTIVÉ :
  en session étendue, l'audio est redirigé vers l'hôte et le câble Conduit disparaît.
  vm-run-console.ps1 refuse de travailler sans, et dit la marche à suivre.

  Code de retour : celui de bench-boucle.ps1 (0 verdict favorable, 1 sinon).
.PARAMETER Name
  Nom de la VM (ConduitTest).
.PARAMETER Credential
  Compte de l'invité, celui-là même qui doit être ouvert à la console
  (Get-Credential nathan).
.PARAMETER Duree
  Durée de la mesure, en secondes (600 = les 10 min du critère M1b-31).
.PARAMETER Pilote
  Pilote de graphe passé au banc : « auto », « internal », ou un périphérique.
.PARAMETER Racine
  Dossier de travail du banc DANS L'INVITÉ. Passé à bench-boucle.ps1 et relu ici pour le
  rapatriement : les deux scripts ne peuvent pas diverger.
.PARAMETER SansConstruction
  Saute cargo et réutilise les binaires déjà présents dans target\static\release.
.PARAMETER RemoteDirectory
  Dossier de dépôt dans l'invité (C:\ConduitTest).
.PARAMETER Sortie
  Dossier local où créer le dossier horodaté des mesures
  (défaut : drivers\windows\target\bench).
.EXAMPLE
  # La mesure du critère, en une commande, depuis la racine du dépôt :
  .\drivers\windows\tools\vm-bench-boucle.ps1 -Credential (Get-Credential nathan)
.EXAMPLE
  # Répétition rapide sans reconstruire :
  .\vm-bench-boucle.ps1 -Credential $cred -Duree 60 -SansConstruction
#>
[CmdletBinding()]
param(
  [string]$Name = "ConduitTest",
  [Parameter(Mandatory)][pscredential]$Credential,
  [ValidateRange(10, 86400)][int]$Duree = 600,
  [string]$Pilote = "auto",
  [string]$Racine = "C:\ConduitTest\bench",
  [switch]$SansConstruction,
  [string]$RemoteDirectory = "C:\ConduitTest",
  [string]$Sortie
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
# PowerShell Direct exige d'être « logged into the host computer as a Hyper-V
# administrator » : le groupe Administrateurs Hyper-V suffit (voir vm-common.psm1).
Assert-HyperVAccess -Reason "PowerShell Direct vers la VM"

$workspace = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent (Split-Path -Parent $workspace)
$targetDir = Join-Path $repoRoot "target\static"
$binaires = @("conduitd.exe", "conduitctl.exe", "conduit-helper.exe")
$banc = Join-Path $PSScriptRoot "bench-boucle.ps1"
$artefacts = @("serie.jsonl", "recapitulatif.txt", "conduitd.log", "conduitd.err")

if (-not $Sortie) { $Sortie = Join-Path $workspace "target\bench" }
if (-not (Test-Path -LiteralPath $banc -PathType Leaf)) {
  throw "bench-boucle.ps1 introuvable à côté de ce script : $banc"
}

$vms = @(Invoke-HyperVChecked -What "inventaire des VM" -Script { Get-VM })
if (-not ($vms | Where-Object { $_.Name -eq $Name })) {
  throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1 puis vm-prepare.ps1."
}

# --- 1. Construction en CRT statique ----------------------------------------------------

if (-not $SansConstruction) {
  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  if (($env:Path -split ";") -notcontains $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
  $rustflagsPrecedent = $env:RUSTFLAGS
  Push-Location $repoRoot
  try {
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
    $etapes = @("build", "--release", "-p", "conduitd", "-p", "conduitctl", "-p", "conduit-helper",
      "--target-dir", $targetDir)
    Write-Host "== cargo $($etapes -join ' ')  (RUSTFLAGS=$($env:RUSTFLAGS))"
    # Sortie laissée en clair : cargo écrit son avancement sur stderr, et l'envelopper
    # dans 2>&1 avec $ErrorActionPreference = Stop en ferait des erreurs terminantes.
    & cargo @etapes
    if ($LASTEXITCODE -ne 0) {
      throw "la construction des binaires a échoué (code $LASTEXITCODE)."
    }
  } finally {
    $env:RUSTFLAGS = $rustflagsPrecedent
    Pop-Location
  }
}

$aCopier = @()
foreach ($binaire in $binaires) {
  $chemin = Join-Path $targetDir "release\$binaire"
  if (-not (Test-Path -LiteralPath $chemin -PathType Leaf)) {
    throw @(
      "Binaire absent : $chemin"
      "Relancer sans -SansConstruction, ou construire à la main depuis la racine :"
      "  `$env:RUSTFLAGS = `"-C target-feature=+crt-static`""
      "  cargo build --release -p conduitd -p conduitctl -p conduit-helper --target-dir target\static"
    ) -join "`n"
  }
  $aCopier += $chemin
}
$aCopier += $banc

# --- 2 et 3. Session et dépôt dans l'invité --------------------------------------------

$session = $null
$dossierLocal = $null
$codeBanc = 1
try {
  Write-Host "Connexion à « $Name » par PowerShell Direct…"
  $session = New-GuestSession -Name $Name -Credential $Credential

  Invoke-Command -Session $session -ArgumentList @($RemoteDirectory, $Racine) -ScriptBlock {
    param([string]$Depot, [string]$RacineBanc)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    foreach ($dossier in @($Depot, $RacineBanc)) {
      if (-not (Test-Path -LiteralPath $dossier)) {
        New-Item -ItemType Directory -Force -Path $dossier | Out-Null
      }
    }
  } | Out-Null

  foreach ($fichier in $aCopier) {
    $destination = Join-Path $RemoteDirectory (Split-Path -Leaf $fichier)
    Write-Host "Copie de $fichier vers $destination"
    Copy-Item -ToSession $session -Path $fichier -Destination $destination -Force
  }

  # --- 4. Exécution dans la session console ---------------------------------------------

  # -RemoteExecutable SEUL, jamais avec -Path : voir le piège en tête de fichier.
  $powershell = "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
  $arguments = @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", (Join-Path $RemoteDirectory "bench-boucle.ps1"),
    "-Duree", "$Duree",
    "-Pilote", $Pilote,
    "-Racine", $Racine
  )
  $runConsole = Join-Path $PSScriptRoot "vm-run-console.ps1"
  Write-Host "== Banc dans la session console de « $Name » (au plus $($Duree + 300) s)"
  & $runConsole -Name $Name -Credential $Credential -RemoteExecutable $powershell `
    -Arguments $arguments -TimeoutSeconds ($Duree + 300) -RemoteDirectory $RemoteDirectory
  $codeBanc = $LASTEXITCODE

  # --- 5. Rapatriement ------------------------------------------------------------------

  # La session a pu tomber pendant une mesure longue : vm-run-console.ps1 rouvre la
  # sienne, celle-ci est à vérifier avant de s'en servir.
  if ($null -eq $session -or $session.State -ne "Opened") {
    if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
    $session = New-GuestSession -Name $Name -Credential $Credential
  }
  $horodatage = Get-Date -Format "yyyyMMdd-HHmmss"
  $dossierLocal = Join-Path $Sortie "bench-$horodatage"
  New-Item -ItemType Directory -Force -Path $dossierLocal | Out-Null
  foreach ($artefact in $artefacts) {
    $source = Join-Path $Racine $artefact
    $present = Invoke-Command -Session $session -ArgumentList @($source) -ScriptBlock {
      param([string]$Chemin) Test-Path -LiteralPath $Chemin -PathType Leaf
    }
    if (-not $present) {
      Write-Warning "artefact absent de l'invité : $source"
      continue
    }
    Copy-Item -FromSession $session -Path $source -Destination (Join-Path $dossierLocal $artefact) -Force
  }
} finally {
  if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
}

if ($dossierLocal) {
  $recapitulatifLocal = Join-Path $dossierLocal "recapitulatif.txt"
  if (Test-Path -LiteralPath $recapitulatifLocal -PathType Leaf) {
    Write-Host ""
    Get-Content -LiteralPath $recapitulatifLocal -Encoding UTF8 | ForEach-Object { Write-Host $_ }
  }
  Write-Host ""
  Write-Host "Mesures rapatriées dans $dossierLocal"
}

if ($codeBanc -ne 0) {
  $Host.UI.WriteErrorLine("vm-bench-boucle : le banc a rendu $codeBanc (verdict défavorable ou banc interrompu).")
}
exit $codeBanc
