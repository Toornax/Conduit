<#
.SYNOPSIS
  Construit et empaquette le pilote conduit-kmd avec cargo-wdk (workspace noyau).
.DESCRIPTION
  Équivaut à `cargo wdk build --profile <profil>` depuis drivers/windows. Produit
  target\<debug|release>\conduit_kmd_package\ (.sys signé avec le certificat de test,
  .inf, .cat, .pdb, .map, WDRLocalTestCert.cer). Voir docs/driver-dev.md.
.PARAMETER Profile
  Profil Cargo : dev (défaut) ou release.
#>
param([ValidateSet("dev", "release")][string]$Profile = "dev")

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (($env:Path -split ";") -notcontains $cargoBin) { $env:Path = "$cargoBin;$env:Path" }

$workspace = Split-Path -Parent $PSScriptRoot
Push-Location $workspace
try {
  & cargo wdk build --profile $Profile
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

  $targetSubdir = if ($Profile -eq "release") { "release" } else { "debug" }
  $package = Join-Path $workspace "target\$targetSubdir\conduit_kmd_package"
  if (-not (Test-Path $package)) {
    $Host.UI.WriteErrorLine("build : dossier de package absent : $package")
    exit 1
  }
  Write-Host "package du pilote : $package"
  Get-ChildItem $package | ForEach-Object { Write-Host ("  {0,-24} {1,9} octets" -f $_.Name, $_.Length) }
} finally {
  Pop-Location
}
