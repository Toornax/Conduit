<#
.SYNOPSIS
  Vérifications du workspace noyau (format, clippy, tests utilisateur, build du pilote).
.DESCRIPTION
  Même séquence que le job CI `driver` (.github/workflows/windows.yml). Tests en mode
  utilisateur de portcls-sys (avec et sans la feature `com`) et de portcls (faux
  PortCls) ; ne lance pas `cargo test` sur conduit-kmd : wdk-sys lie les bibliothèques
  noyau même en test.
  Vérifie aussi que portcls-sys\tests\layout.golden (oracle cl.exe des bindings) est à
  jour : régénération dans un dossier temporaire (regen-layout.ps1) et comparaison.
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
    @("test", "-p", "portcls-sys", "--features", "com"),
    @("test", "-p", "portcls"),
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

  # Golden de disposition (portcls-sys\tests\layout.golden) : régénéré par cl.exe dans
  # un dossier temporaire et comparé au fichier commité, pour qu'un changement du probe,
  # des en-têtes du WDK ou du script ne laisse pas un golden périmé.
  Write-Host "== layout.golden à jour (regen-layout.ps1 dans un dossier temporaire)"
  $golden = Join-Path $workspace "portcls-sys\tests\layout.golden"
  $tmpDir = Join-Path ([IO.Path]::GetTempPath()) ("conduit-layout-" + [IO.Path]::GetRandomFileName())
  New-Item -ItemType Directory -Force $tmpDir | Out-Null
  try {
    $regen = Join-Path $tmpDir "layout.golden"
    & (Join-Path $PSScriptRoot "regen-layout.ps1") -OutFile $regen
    if ($LASTEXITCODE -ne 0) {
      $Host.UI.WriteErrorLine("check : regen-layout.ps1 a échoué ($LASTEXITCODE)")
      exit 1
    }
    $attendu = [IO.File]::ReadAllText($regen)
    $commite = if (Test-Path $golden) { [IO.File]::ReadAllText($golden) } else { "" }
    if ($attendu -ne $commite) {
      $Host.UI.WriteErrorLine("check : $golden est périmé (diffère de la sortie de cl.exe).")
      $Host.UI.WriteErrorLine("        Régénérez-le : tools\regen-layout.ps1, puis commitez.")
      Compare-Object ($commite -split "`n") ($attendu -split "`n") |
        ForEach-Object { $Host.UI.WriteErrorLine("        $($_.SideIndicator) $($_.InputObject)") }
      exit 1
    }
  } finally {
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
  }

  # Scripts de VM : analyse syntaxique seulement (ils exigent Hyper-V et l'élévation) ;
  # les tests Pester des fonctions pures se lancent à part (Invoke-Pester tools\tests).
  Write-Host "== analyse syntaxique des scripts tools\*.ps1, *.psm1"
  $scripts = Get-ChildItem -Path $PSScriptRoot -Recurse -Include "*.ps1", "*.psm1"
  foreach ($script in $scripts) {
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($script.FullName, [ref]$tokens, [ref]$errors) | Out-Null
    if (@($errors).Count -gt 0) {
      foreach ($e in $errors) { $Host.UI.WriteErrorLine("$($script.Name):$($e.Extent.StartLineNumber): $($e.Message)") }
      exit 1
    }
  }
  Write-Host "workspace noyau : vérifications vertes"
} finally {
  Pop-Location
}
