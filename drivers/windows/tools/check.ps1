<#
.SYNOPSIS
  Vérifications du workspace noyau (format, clippy, tests utilisateur, build du pilote).
.DESCRIPTION
  Même séquence que le job CI `driver` (.github/workflows/windows.yml). Ne lance pas
  `cargo test` sur conduit-kmd : wdk-sys lie les bibliothèques noyau même en test.
#>
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (($env:Path -split ";") -notcontains $cargoBin) { $env:Path = "$cargoBin;$env:Path" }

$workspace = Split-Path -Parent $PSScriptRoot
Push-Location $workspace
try {
  $steps = @(
    @("fmt", "--all", "--check"),
    @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings"),
    @("test", "-p", "portcls-sys"),
    @("build", "-p", "conduit-kmd")
  )
  foreach ($step in $steps) {
    Write-Host "== cargo $($step -join ' ')"
    & cargo @step
    if ($LASTEXITCODE -ne 0) {
      $Host.UI.WriteErrorLine("check : échec de « cargo $($step -join ' ') »")
      exit $LASTEXITCODE
    }
  }
  Write-Host "workspace noyau : vérifications vertes"
} finally {
  Pop-Location
}
