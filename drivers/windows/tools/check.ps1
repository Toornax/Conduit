<#
.SYNOPSIS
  Vérifications du workspace noyau (format, clippy, tests utilisateur, build du pilote).
.DESCRIPTION
  Même séquence que le job CI `driver` (.github/workflows/windows.yml). Tests en mode
  utilisateur de portcls-sys (avec et sans la feature `com`) et de portcls (faux
  PortCls), build de portcls avec la feature `kernel` (enveloppes des fonctions Pc*) ;
  ne lance pas `cargo test` sur conduit-kmd : wdk-sys lie les bibliothèques noyau même
  en test.
  Vérifie aussi que portcls-sys\tests\layout.golden (oracle cl.exe des bindings) est à
  jour : régénération dans un dossier temporaire (regen-layout.ps1) et comparaison.

  Cohérence INF ↔ Rust (M1a-09) : `cargo test -p portcls` lance portcls\tests\inf.rs, qui
  relit conduit-kmd\conduit_kmd.inx et le compare aux constantes du pilote — noms de
  sous-périphériques des AddInterface contre WAVE_RENDER_0…, GUID de nom de broche contre
  pin_name_guid(0), GUID de catégorie contre portcls_sys::KSCATEGORY_*, et encodage
  UTF-16 LE de la copie de travail. Une divergence y est une panne muette dans la VM
  (périphérique installé, aucun endpoint, ou endpoint mal nommé) : elle échoue ici.

  portcls est testé DEUX FOIS, en `dev` puis en `--release`. Ce n'est pas une redondance :
  la garde de vtable de portcls\src\property.rs compare des ADRESSES de vtable, et
  `&T::VTBL` est une constante promue, donc `unnamed_addr`, donc duplicable d'une unité de
  génération de code à l'autre. Seul le `#[inline(never)]` porté par topology::vtbl_of et
  wavert::vtbl_of lui garantit une allocation unique — et cet attribut n'est *load-bearing*
  qu'une fois l'inlining actif. Symptôme mesuré si on le retire : DOUZE tests de
  portcls\tests\property.rs tombent sous `cargo test -p portcls --release` (opt-level 3 +
  LTO, le profil du pilote livré) avec STATUS_INVALID_DEVICE_REQUEST là où
  STATUS_SUCCESS/STATUS_BUFFER_OVERFLOW est attendu, et AUCUN ne tombe en `dev`, faute
  d'inlining. Sans cette passe, la régression traverserait l'intégration continue sans un
  bruit et ne se manifesterait que dans la VM, en pilote muet à toutes ses propriétés
  (volume et sourdine compris). Ne pas la supprimer pour gagner du temps de compilation :
  c'est le seul endroit où ce défaut est visible.
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
    # Voir .DESCRIPTION : la garde de vtable de property.rs ne peut échouer qu'une fois
    # l'inlining actif. Douze tests de tests/property.rs tombent en --release, zéro en
    # dev, si `#[inline(never)]` disparaît de topology::vtbl_of / wavert::vtbl_of.
    @("test", "-p", "portcls", "--release"),
    @("build", "-p", "portcls", "--features", "kernel"),
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

  # Scripts de VM : analyse syntaxique seulement (ils exigent Hyper-V et un accès
  # administrateur ou Administrateurs Hyper-V) ;
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
