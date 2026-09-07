<#
.SYNOPSIS
  Exécute une commande DANS LA SESSION CONSOLE de la VM de test, la seule où une mesure
  audio ait un sens, et rend sa sortie et son code de retour.
.DESCRIPTION
  Outille la recette de docs/vm-bringup.md §3 bis. Deux pièges cumulés, qui ont invalidé
  une journée entière de mesures, et que ce script rend impossibles à oublier :

  1. PowerShell Direct ouvre ses sessions dans la SESSION 0, celle des services, qui n'a
     aucun audio utilisateur. Un test lancé par `Invoke-Command -VMName` énumère les
     endpoints, ouvre les flux et reçoit des trames À LA BONNE CADENCE — toutes
     silencieuses, sans le moindre signal d'erreur. Ce n'est pas une mesure ratée, c'est
     une mesure sans objet, et elle ressemble trait pour trait à un pilote en panne.
  2. Le mode session étendue de vmconnect redirige l'audio vers l'hôte : la session
     ouverte ainsi ne voit qu'un périphérique audio distant, plus le câble Conduit.

  Le script REFUSE donc de travailler tant qu'une session console interactive n'est pas
  ouverte dans l'invité, et le dit avec la marche à suivre. Il ne rend jamais un résultat
  silencieusement faux.

  Séquence : accès Hyper-V, session PowerShell Direct, vérification de la session console
  (indépendante de la langue : Win32_ComputerSystem.UserName pour l'utilisateur ouvert à
  la console, et l'identifiant de session de SON processus explorer, qui doit différer de
  0 — jamais `quser`, dont toute la sortie est traduite), copie facultative de
  l'exécutable, création d'une tâche planifiée à JETON INTERACTIF (`schtasks … /ru
  <utilisateur> /it`, aucun mot de passe nécessaire), exécution, attente du fichier de
  code de retour qui sert de sentinelle de fin, relecture de la sortie, suppression de la
  tâche (même en cas d'échec), et propagation du code de retour.

  Le compte de la tâche vient TOUJOURS du -Credential, jamais d'une constante : le compte
  réel de la VM (`nathan`) n'est pas celui qu'annonce encore la documentation (`test`).

  Prévu pour les exécutions longues (charge d'une heure sous Driver Verifier) : la session
  PowerShell Direct est rouverte si elle tombe, et l'avancement est imprimé chaque minute.
.PARAMETER Name
  Nom de la VM (ConduitTest).
.PARAMETER Credential
  Compte de l'invité, celui-là même qui doit être ouvert à la console (Get-Credential nathan).
.PARAMETER Path
  Exécutable local à copier dans l'invité. Facultatif s'il y est déjà : indiquer alors
  -RemoteExecutable.

  L'INVITÉ N'A PAS LE RUNTIME VISUAL C++. Un binaire Rust construit normalement dépend de
  VCRUNTIME140.dll et meurt dans l'invité avec le code -1073741515 (0xC0000135,
  STATUS_DLL_NOT_FOUND), sans dire quelle DLL manque. Construire avec le CRT statique :

    $env:RUSTFLAGS = "-C target-feature=+crt-static"
    cargo build -p conduit-looptest --target-dir target\static

  (Le -target-dir séparé évite de reconstruire tout le workspace au changement de
  RUSTFLAGS.) Le pilote, lui, est déjà en CRT statique : wdk-build l'exige.
.PARAMETER RemoteExecutable
  Exécutable DANS l'invité (nom relatif à -RemoteDirectory, ou chemin complet). Par défaut,
  le nom de fichier de -Path.
.PARAMETER Arguments
  Arguments de la commande, un par élément.
.PARAMETER TimeoutSeconds
  Délai maximal d'exécution (900 s ; 4000 pour la charge d'une heure).
.PARAMETER RemoteDirectory
  Dossier de travail dans l'invité (C:\ConduitTest).
.EXAMPLE
  # Depuis la racine du dépôt (conduit-looptest est construit par le workspace racine) :
  .\drivers\windows\tools\vm-run-console.ps1 -Credential $cred -Path target\debug\conduit-looptest.exe -Arguments @("--repeat", "10")
.EXAMPLE
  .\vm-run-console.ps1 -Credential $cred -RemoteExecutable conduit-looptest.exe -Arguments @("--repeat", "200") -TimeoutSeconds 4000
#>
[CmdletBinding()]
param(
  [string]$Name = "ConduitTest",
  [Parameter(Mandatory)][pscredential]$Credential,
  [string]$Path,
  [string]$RemoteExecutable,
  [AllowEmptyCollection()][string[]]$Arguments = @(),
  [ValidateRange(10, 86400)][int]$TimeoutSeconds = 900,
  [string]$RemoteDirectory = "C:\ConduitTest"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
# PowerShell Direct exige d'être « logged into the host computer as a Hyper-V
# administrator » : le groupe Administrateurs Hyper-V suffit (voir vm-common.psm1).
Assert-HyperVAccess -Reason "PowerShell Direct vers la VM"

# Inventaire puis filtrage par nom, sans -ErrorAction SilentlyContinue, qui avalerait un
# refus d'accès en le présentant comme une VM introuvable.
$vms = @(Invoke-HyperVChecked -What "inventaire des VM" -Script { Get-VM })
if (-not ($vms | Where-Object { $_.Name -eq $Name })) {
  throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1 puis vm-prepare.ps1."
}

if ($Path) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "Exécutable introuvable sur l'hôte : $Path"
  }
  $Path = (Resolve-Path -LiteralPath $Path).Path
  if (-not $RemoteExecutable) { $RemoteExecutable = Split-Path -Leaf $Path }
}
if (-not $RemoteExecutable) {
  throw "Rien à exécuter : donner -Path (exécutable local à copier) ou -RemoteExecutable (déjà présent dans l'invité)."
}
$remotePath = if ([System.IO.Path]::IsPathRooted($RemoteExecutable)) { $RemoteExecutable }
              else { Join-Path $RemoteDirectory $RemoteExecutable }

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$taskName = "Conduit-RunConsole-$stamp"
$cmdPath = Join-Path $RemoteDirectory "console-$stamp.cmd"
$outputPath = Join-Path $RemoteDirectory "console-$stamp.out.txt"
$exitCodePath = Join-Path $RemoteDirectory "console-$stamp.code.txt"

$session = $null
function Invoke-Guest {
  <#
  .SYNOPSIS
    Exécute un bloc dans l'invité, en rouvrant la session PowerShell Direct si elle est
    tombée (une exécution d'une heure survit ainsi à une coupure passagère).
  #>
  param(
    [Parameter(Mandatory)][scriptblock]$Script,
    [object[]]$ArgumentList = @(),
    [int]$Retries = 2
  )
  for ($attempt = 0; ; $attempt++) {
    try {
      if (-not $session -or $session.State -ne "Opened") {
        $script:session = New-GuestSession -Name $Name -Credential $Credential
      }
      return Invoke-Command -Session $session -ScriptBlock $Script -ArgumentList $ArgumentList
    } catch {
      # On ne réessaie que si la SESSION est tombée : une erreur rendue par le bloc
      # lui-même (mauvais compte, exécutable absent) doit remonter telle quelle, tout de
      # suite, et non trois fois de suite.
      $sessionLost = (-not $session) -or ($session.State -ne "Opened")
      if (-not $sessionLost -or $attempt -ge $Retries) { throw }
      Write-Warning "Session vers « $Name » perdue ($($_.Exception.Message)) : nouvelle tentative."
      if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
      $script:session = $null
      Start-Sleep -Seconds 5
    }
  }
}

$output = ""
$exitCode = $null
$failure = $null
$taskCreated = $false
try {
  Write-Host "Connexion à « $Name » par PowerShell Direct…"
  $session = New-GuestSession -Name $Name -Credential $Credential

  # --- Exigence centrale : une session console interactive est-elle ouverte ? ------------
  # On lit des FAITS (un nom de compte, un identifiant de session numérique), jamais un
  # libellé d'état : tout ce que Windows affiche ici est traduit sur cette machine.
  $probe = Invoke-Guest -Script {
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $computer = Get-CimInstance -ClassName Win32_ComputerSystem
    $explorers = @()
    foreach ($process in @(Get-CimInstance -ClassName Win32_Process -Filter "Name = 'explorer.exe'")) {
      $owner = $null
      try { $owner = Invoke-CimMethod -InputObject $process -MethodName GetOwner } catch { $owner = $null }
      $user = ""
      if ($owner) { $user = "$($owner.Domain)\$($owner.User)" }
      $explorers += [pscustomobject]@{ UserName = $user; SessionId = [int]$process.SessionId }
    }
    [pscustomobject]@{
      ConsoleUser  = [string]$computer.UserName
      ComputerName = [string]$env:COMPUTERNAME
      Explorers    = $explorers
    }
  }

  $decision = Get-ConsoleSessionDecision -ConsoleUser $probe.ConsoleUser -ExplorerSessions @($probe.Explorers)
  if (-not $decision.Ready) {
    throw @(
      "Aucune session console interactive dans « $Name » : $($decision.Reason)."
      ""
      "REFUS DÉLIBÉRÉ. Lancée ici, la commande tournerait dans la session 0 (celle des"
      "services) : elle énumérerait les endpoints, ouvrirait les flux et recevrait des"
      "trames à la bonne cadence, TOUTES SILENCIEUSES, sans le moindre signal d'erreur."
      "Le résultat aurait l'air d'un pilote en panne alors qu'il ne mesurerait rien."
      "C'est le piège qui a invalidé une journée de mesures (docs/vm-bringup.md §3 bis)."
      ""
      "Marche à suivre, sur l'hôte :"
      "  1. vmconnect.exe localhost $Name"
      "  2. DÉSACTIVER le mode session étendue (menu Affichage). En session étendue,"
      "     vmconnect redirige l'audio vers l'hôte : l'invité ne voit plus qu'un"
      "     périphérique audio distant, et le câble Conduit disparaît de la liste."
      "  3. Ouvrir une session Windows À L'ÉCRAN avec le compte « $($Credential.UserName) »"
      "     et attendre que le bureau soit chargé."
      "  4. Relancer ce script. La fenêtre vmconnect peut être réduite ensuite ; ne pas"
      "     fermer la session Windows de l'invité (la verrouiller reste possible)."
    ) -join "`n"
  }
  Write-Host "Session console : « $($decision.UserName) », session $($decision.SessionId) (≠ 0, l'audio utilisateur est là)."

  # La tâche à jeton interactif ne se déclenche QUE pour l'utilisateur ouvert : si le
  # -Credential désigne un autre compte, elle ne partirait jamais et l'attente irait
  # jusqu'au délai maximal sans rien dire d'utile. Autant le dire tout de suite.
  if (-not (Test-SameAccount -Left $Credential.UserName -Right $decision.UserName)) {
    throw @(
      "Le compte ouvert à la console est « $($decision.UserName) », mais -Credential désigne"
      "« $($Credential.UserName) ». La tâche est créée avec /it (jeton interactif) : elle ne se"
      "déclenche que pour l'utilisateur ouvert à la console."
      "Ouvrir la console avec « $($Credential.UserName) », ou relancer avec le -Credential du"
      "compte réellement ouvert."
    ) -join "`n"
  }

  # --- Préparation dans l'invité --------------------------------------------------------
  $cmdContent = Get-ConsoleRunScript -Executable $remotePath -Arguments $Arguments `
    -OutputPath $outputPath -ExitCodePath $exitCodePath -WorkingDirectory $RemoteDirectory

  Invoke-Guest -ArgumentList @($RemoteDirectory, $cmdPath, $cmdContent, $outputPath, $exitCodePath) -Script {
    param([string]$Directory, [string]$CmdPath, [string]$Content, [string]$OutputPath, [string]$ExitCodePath)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    if (-not (Test-Path -LiteralPath $Directory)) {
      New-Item -ItemType Directory -Force -Path $Directory | Out-Null
    }
    foreach ($stale in @($OutputPath, $ExitCodePath)) {
      if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Force }
    }
    # Page de codes du système, JAMAIS UTF-8 : cmd.exe lit un fichier de commandes dans la
    # page de codes ANSI/OEM, et un BOM UTF-8 en tête ferait échouer la première ligne.
    [System.IO.File]::WriteAllText($CmdPath, $Content, [System.Text.Encoding]::Default)
  } | Out-Null

  if ($Path) {
    Write-Host "Copie de $Path vers $remotePath"
    Copy-Item -ToSession $session -Path $Path -Destination $remotePath -Force
  }
  $exists = Invoke-Guest -ArgumentList @($remotePath) -Script {
    param([string]$Exe) Test-Path -LiteralPath $Exe -PathType Leaf
  }
  if (-not $exists) {
    throw "Exécutable absent de l'invité : $remotePath (le copier avec -Path)."
  }

  # --- Tâche planifiée à jeton interactif -----------------------------------------------
  $runAs = Get-TaskRunAsUser -UserName $Credential.UserName -ComputerName $probe.ComputerName
  Write-Host "Tâche « $taskName » pour $runAs (jeton interactif, aucun mot de passe requis)"
  $created = Invoke-Guest -ArgumentList @((Get-FunctionSource -Name Invoke-NativeChecked), $taskName, $cmdPath, $runAs) -Script {
    param([string]$Helpers, [string]$TaskName, [string]$CmdPath, [string]$RunAs)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    . ([scriptblock]::Create($Helpers))
    # /sc once /st <heure> n'est qu'un déclencheur obligatoire : c'est /run qui lance la
    # tâche, et le finally la supprime. L'heure est calculée dans l'invité en culture
    # invariante (HH:mm), et aucune DATE n'est passée : le format de /sd, lui, est traduit.
    $time = (Get-Date).AddMinutes(10).ToString("HH:mm", [System.Globalization.CultureInfo]::InvariantCulture)
    Invoke-NativeChecked schtasks @(
      "/create", "/tn", $TaskName, "/tr", $CmdPath, "/sc", "once", "/st", $time,
      "/ru", $RunAs, "/it", "/f"
    )
  }
  $taskCreated = $true
  Write-Verbose (@($created) -join "`n")

  Write-Host "Exécution : $remotePath $($Arguments -join ' ')"
  Invoke-Guest -ArgumentList @((Get-FunctionSource -Name Invoke-NativeChecked), $taskName) -Script {
    param([string]$Helpers, [string]$TaskName)
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    . ([scriptblock]::Create($Helpers))
    Invoke-NativeChecked schtasks @("/run", "/tn", $TaskName)
  } | Out-Null

  # --- Attente de la sentinelle ---------------------------------------------------------
  # Le fichier de code de retour, et RIEN d'autre : le statut rendu par `schtasks /query`
  # est traduit (« En cours d'exécution », « Prêt ») et ne peut pas servir de signal.
  $interval = if ($TimeoutSeconds -le 120) { 2 } else { 5 }
  $started = Get-Date
  $deadline = $started.AddSeconds($TimeoutSeconds)
  $nextTick = $started.AddSeconds(60)
  $finished = $false
  while ((Get-Date) -lt $deadline) {
    $finished = Invoke-Guest -ArgumentList @($exitCodePath) -Script {
      param([string]$Sentinel)
      # Le fichier est CRÉÉ par la redirection avant que `echo` n'y écrive : on n'y voit la
      # fin que lorsqu'il contient quelque chose, sans quoi on relirait un fichier vide.
      if (-not (Test-Path -LiteralPath $Sentinel -PathType Leaf)) { return $false }
      $texte = Get-Content -LiteralPath $Sentinel -Raw -ErrorAction SilentlyContinue
      return ($null -ne $texte -and $texte -match '\S')
    }
    if ($finished) { break }
    if ((Get-Date) -ge $nextTick) {
      Write-Host ("  … en cours depuis {0:n0} min (délai maximal {1:n0} min)" -f `
        ((Get-Date) - $started).TotalMinutes, ($TimeoutSeconds / 60))
      $nextTick = (Get-Date).AddSeconds(60)
    }
    Start-Sleep -Seconds $interval
  }
  if (-not $finished) {
    $partial = Invoke-Guest -ArgumentList @($outputPath) -Script {
      param([string]$Out)
      if (Test-Path -LiteralPath $Out) { Get-Content -LiteralPath $Out -Tail 40 -Encoding UTF8 } else { @() }
    }
    throw @(
      "Délai dépassé : la commande n'a pas rendu la main en $TimeoutSeconds s."
      "Sortie partielle ($outputPath, dans l'invité) :"
      (@($partial) -join "`n")
    ) -join "`n"
  }

  # Les outils du dépôt écrivent en UTF-8 (Rust) : la sortie est relue comme telle.
  $result = Invoke-Guest -ArgumentList @($outputPath, $exitCodePath) -Script {
    param([string]$Out, [string]$Sentinel)
    $text = ""
    if (Test-Path -LiteralPath $Out) { $text = Get-Content -LiteralPath $Out -Raw -Encoding UTF8 }
    $code = (Get-Content -LiteralPath $Sentinel -Raw).Trim()
    [pscustomobject]@{ Output = [string]$text; ExitCode = [string]$code }
  }
  $output = $result.Output
  $parsed = 0
  if (-not [int]::TryParse($result.ExitCode.Trim(), [ref]$parsed)) {
    throw "Code de retour illisible dans $exitCodePath : « $($result.ExitCode) »."
  }
  $exitCode = $parsed
} catch {
  $failure = $_
} finally {
  # La tâche est supprimée dans tous les cas : une tâche oubliée se rejouerait toute seule
  # à son heure de déclenchement, au milieu d'une autre mesure.
  if ($taskCreated) {
    try {
      Invoke-Guest -ArgumentList @((Get-FunctionSource -Name Invoke-NativeChecked), $taskName) -Script {
        param([string]$Helpers, [string]$TaskName)
        Set-StrictMode -Version Latest
        $ErrorActionPreference = "Stop"
        . ([scriptblock]::Create($Helpers))
        Invoke-NativeChecked schtasks @("/delete", "/tn", $TaskName, "/f")
      } | Out-Null
    } catch {
      $Host.UI.WriteErrorLine("suppression de la tâche « $taskName » impossible : $_")
    }
  }
  if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
}

if ($failure) {
  $Host.UI.WriteErrorLine("vm-run-console : ÉCHEC — $failure")
  exit 1
}

if ($output) { Write-Host $output }
Write-Host ""
Write-Host "vm-run-console : code de retour $exitCode (session console de « $Name »)."
Write-Host "  Sortie complète dans l'invité : $outputPath"
exit $exitCode
