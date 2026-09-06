<#
.SYNOPSIS
  Charge et décharge le pilote conduit_kmd N fois dans la VM de test (critère M1a-02).
.DESCRIPTION
  Étape 3/3 de docs/driver-dev.md §3, sur l'hôte, VM préparée par vm-prepare.ps1 et
  paquet produit par build.ps1. Par PowerShell Direct : copie le paquet et devgen.exe
  (WDK de l'hôte) dans l'invité, importe WDRLocalTestCert.cer dans Root et
  TrustedPublisher de la machine invitée, nettoie les restes d'une exécution précédente
  (périphériques et paquets restants, et endpoints FANTÔMES à nous, qui fausseraient les
  mesures audio en dédoublant les instances),
  puis répète Count fois :
    1. pnputil /add-driver conduit_kmd.inf /install
    2. devgen /add /hardwareid "Root\ConduitCable"   (identifiant d'instance récupéré)
    3. attente de Get-PnpDevice -InstanceId … en état OK (délai borné)
    4. devgen /remove <id>
    5. attente que le devnode ait VRAIMENT disparu (délai borné) — voir la course décrite
       plus bas, qui produit des 0xC00000E5 sans que le pilote y soit pour rien
    6. pnputil /delete-driver oemN.inf /uninstall /force   (oemN.inf via /enum-drivers)
    7. aucun événement Kernel-PnP d'erreur ni BugCheck depuis le début
  Tout devgen/pnputil qui échoue arrête le script avec sa sortie. En cas d'échec, un
  MEMORY.DMP ou un minidump plus récent que le début est rapatrié dans
  drivers\windows\target\dumps\, avec les 500 dernières lignes de
  C:\Windows\INF\setupapi.dev.log — le journal qui dit pourquoi une installation ou un
  démarrage PnP a échoué. Sortie : itérations réussies et durée moyenne d'un cycle. Ce
  script ne lance jamais pnputil ni devgen sur l'hôte.
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
.PARAMETER RemoveTimeoutSeconds
  Délai maximal pour que le devnode disparaisse après devgen /remove, AVANT de supprimer
  le paquet (30 s). Ne pas attendre est la course qui produit les 0xC00000E5 (voir le
  commentaire du cycle).
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
  [ValidateRange(5, 600)][int]$StartTimeoutSeconds = 30,
  [ValidateRange(5, 600)][int]$RemoveTimeoutSeconds = 30
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path $PSScriptRoot "vm-common.psm1") -Force
# PowerShell Direct exige d'être « logged into the host computer as a Hyper-V
# administrator » : le groupe Administrateurs Hyper-V suffit (voir vm-common.psm1).
Assert-HyperVAccess -Reason "PowerShell Direct vers la VM"

$workspace = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent (Split-Path -Parent $workspace)
$hardwareId = "Root\ConduitCable"
$infName = "conduit_kmd.inf"
$guestDir = "C:\ConduitTest"
$dumpsDir = Join-Path $workspace "target\dumps"
# Assez pour couvrir plusieurs installations PnP, assez peu pour rester lisible et pour
# traverser PowerShell Direct sans peser.
$setupapiTailLines = 500

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

# Inventaire puis filtrage par nom, sans -ErrorAction SilentlyContinue, qui avalerait un
# refus d'accès en le présentant comme une VM introuvable.
$vms = @(Invoke-HyperVChecked -What "inventaire des VM" -Script { Get-VM })
if (-not ($vms | Where-Object { $_.Name -eq $Name })) {
  throw "VM « $Name » introuvable : lancer d'abord vm-new.ps1 puis vm-prepare.ps1."
}

# Fonctions redéfinies dans l'invité à partir de leur texte.
$helpers = @("Invoke-NativeChecked", "ConvertFrom-DevgenAddOutput", "Find-PublishedInf",
  "Test-ConduitGhostDevice", "Test-DevnodeGone") |
  ForEach-Object { Get-FunctionSource -Name $_ }
$helpers = $helpers -join "`n"

# Un cycle complet, exécuté dans l'invité. Renvoie l'identifiant d'instance et les
# oemN.inf supprimés ; lève une erreur détaillée sinon.
$cycleBlock = {
  param([string]$Helpers, [string]$GuestDir, [string]$InfName, [string]$HardwareId,
        [int]$StartTimeoutSeconds, [int]$RemoveTimeoutSeconds)
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

  # ATTENDRE QUE LE RETRAIT SOIT EFFECTIF avant de toucher au paquet. `devgen /remove` rend
  # la main dès que la demande est déposée, pas quand PnP a fini de démonter le devnode.
  #
  # DIAGNOSTIQUÉ le 2026-09-06, traces à l'appui, sur une série de deux cycles dont le
  # second a échoué :
  #   Kernel-PnP 411 : L'appareil SWD\DEVGEN\{0c9acf52-…} a eu un problème de démarrage.
  #   Problème : 0x0   État du problème : 0xC00000E5
  # Le détail qui identifie la course : {0c9acf52-…} est l'appareil du cycle 1, PAS celui
  # du cycle 2, et l'horodatage tombe à l'instant de son retrait. Les traces du pilote
  # montrent un cycle 1 complet et propre, DriverUnload compris ; le cycle 2 n'a jamais
  # chargé le pilote. Après l'échec, l'invité ne garde ni périphérique résiduel ni pilote
  # publié. Le pilote est donc hors de cause : le script enchaînait `/enum-drivers` puis
  # `/delete-driver … /uninstall /force` immédiatement après `/remove`, et PnP tentait un
  # dernier démarrage sur le devnode mourant pendant qu'on lui retirait son paquet sous les
  # pieds. C'est ce qui a produit l'échec au 33e cycle sur 100 la veille et au 2e ce
  # soir-là : une course, dont la probabilité monte quand le débogueur ralentit l'invité.
  #
  # Le critère d'arrêt est STRUCTUREL (identifiant d'instance et booléen Present), jamais
  # un libellé d'état traduit : voir Test-DevnodeGone. `-ErrorAction SilentlyContinue` sur
  # Get-PnpDevice parce qu'un identifiant disparu y est une erreur (localisée), pas ici.
  $deadline = (Get-Date).AddSeconds($RemoveTimeoutSeconds)
  $gone = $false
  do {
    $remaining = @(Get-PnpDevice -InstanceId $id -ErrorAction SilentlyContinue)
    if (Test-DevnodeGone -InstanceId $id -Devices $remaining) { $gone = $true; break }
    Start-Sleep -Milliseconds 250
  } while ((Get-Date) -lt $deadline)
  if (-not $gone) {
    throw ("Le devnode $id est toujours actif $RemoveTimeoutSeconds s après devgen /remove : " +
      "le paquet n'est PAS supprimé, pour ne pas le retirer sous les pieds d'un devnode " +
      "encore vivant (course Kernel-PnP 411 / 0xC00000E5). Augmenter -RemoveTimeoutSeconds " +
      "si l'invité est simplement lent, sinon chercher ce qui retient le périphérique.")
  }

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

# Dernières lignes de setupapi.dev.log, le journal d'installation PnP de l'invité : c'est
# lui qui dit POURQUOI une installation ou un démarrage a échoué (section [Device Install],
# code de fin de chaque étape), là où le journal d'événements ne donne qu'un code. Il était
# jusqu'ici perdu à chaque échec, alors qu'il ne coûte rien à rapatrier.
$setupapiBlock = {
  param([int]$Tail)
  $ErrorActionPreference = "Stop"
  $path = Join-Path $env:SystemRoot "INF\setupapi.dev.log"
  if (-not (Test-Path -LiteralPath $path)) { return @() }
  # Le fichier est tenu ouvert par PnP : une lecture peut échouer sans que ce soit une
  # raison d'ajouter un échec à l'échec en cours.
  return @(Get-Content -LiteralPath $path -Tail $Tail -ErrorAction SilentlyContinue)
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
    # Chaque étape annonce ce qu'elle tente : un refus d'accès nu, sans nom d'opération,
    # est ininterprétable à distance (constaté le 2026-09-06).
    $etape = "import du certificat de test"
    try {
      $cer = Join-Path $GuestDir "package\WDRLocalTestCert.cer"
      # `certutil -addstore`, pas `Import-Certificate` : sur une machine neuve le magasin
      # TrustedPublisher n'existe pas encore (aucun éditeur jamais approuvé) et le
      # fournisseur Cert: échoue alors en « accès refusé » au lieu de le créer, tandis que
      # Root, lui, existe toujours et passe. Vérifié dans la VM le 2026-09-06 :
      # Root OK / TrustedPublisher UnauthorizedAccessException, en session pourtant élevée.
      # certutil crée le magasin au besoin ; c'est aussi la méthode de la documentation WDK.
      foreach ($store in @("Root", "TrustedPublisher")) {
        $etape = "import du certificat de test dans $store"
        Invoke-NativeChecked certutil @("-addstore", "-f", $store, $cer) | Out-Null
      }

      # Restes d'une exécution interrompue : périphériques Root\ConduitCable, paquets oemN.inf.
      $etape = "inventaire des périphériques restants"
      $leftovers = @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $HardwareId })
      foreach ($dev in $leftovers) {
        Write-Host "  retrait du périphérique restant $($dev.InstanceId)"
        $etape = "retrait du périphérique restant $($dev.InstanceId)"
        Invoke-NativeChecked pnputil @("/remove-device", $dev.InstanceId) | Out-Null
      }
      # Endpoints FANTÔMES (périphériques non présents) laissés par les cycles précédents.
      # Ils faussent les mesures : l'outil peut jouer sur une instance et écouter sur une
      # autre, et le symptôme est un silence, pas une erreur. On ne retire QUE ce qui nous
      # appartient — Test-ConduitGhostDevice exige un périphérique non présent qui soit un
      # Root\ConduitCable, ou un endpoint audio dont le nom contient « Conduit » — parce
      # que cette VM sert aussi à comparer avec de vraies cartes son.
      $etape = "inventaire des périphériques fantômes"
      $ghosts = @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object {
        Test-ConduitGhostDevice -InstanceId $_.InstanceId -FriendlyName $_.FriendlyName `
          -ClassGuid $_.ClassGuid -Present ([bool]$_.Present)
      })
      foreach ($ghost in $ghosts) {
        Write-Host "  retrait du fantôme « $($ghost.FriendlyName) » [$($ghost.InstanceId)]"
        $etape = "retrait du périphérique fantôme $($ghost.InstanceId)"
        try {
          Invoke-NativeChecked pnputil @("/remove-device", $ghost.InstanceId) | Out-Null
        } catch {
          # Un fantôme qui résiste n'est pas une raison d'annuler la série : le cycle,
          # lui, ne dépend pas de sa disparition. On le signale et on continue.
          Write-Host "    (retrait refusé, ignoré) $($_.Exception.Message)"
        }
      }

      $etape = "inventaire des paquets de pilote publiés"
      $enum = Invoke-NativeChecked pnputil @("/enum-drivers")
      foreach ($oem in @(Find-PublishedInf -Output $enum -OriginalName $InfName)) {
        Write-Host "  suppression du paquet restant $oem"
        $etape = "suppression du paquet restant $oem"
        Invoke-NativeChecked pnputil @("/delete-driver", $oem, "/uninstall", "/force") | Out-Null
      }
    } catch {
      throw "Préparation de l'invité, échec pendant « $etape » : $($_.Exception.Message)"
    }
  }

  Write-Host "Début des $Count cycles (heure invité : $guestStart)"
  for ($i = 1; $i -le $Count; $i++) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $result = Invoke-Command -Session $session -ScriptBlock $cycleBlock `
      -ArgumentList $helpers, $guestDir, $infName, $hardwareId, $StartTimeoutSeconds, $RemoveTimeoutSeconds
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
  # Vidages et journal PnP : la session peut être morte (bug check) ; on en rouvre une,
  # l'invité redémarre seul (AutoReboot) et l'on rapatrie ce qui est plus récent que le
  # début, plus setupapi.dev.log en cas d'échec.
  if ($guestStart) {
    try {
      if (-not $session -or $session.State -ne "Opened") {
        Write-Host "Reconnexion à l'invité pour la collecte des vidages et du journal PnP…"
        $session = New-GuestSession -Name $Name -Credential $Credential
      }
      $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
      $dumps = @(Invoke-Command -Session $session -ScriptBlock $dumpsBlock -ArgumentList $guestStart)
      if ($dumps.Count -gt 0) {
        New-Item -ItemType Directory -Force -Path $dumpsDir | Out-Null
        foreach ($dump in $dumps) {
          $dest = Join-Path $dumpsDir ("{0}_{1}" -f $stamp, (Split-Path -Leaf $dump))
          Write-Host "Vidage $dump → $dest"
          Copy-Item -FromSession $session -Path $dump -Destination $dest
        }
        if (-not $failure) { $failure = "Vidage(s) mémoire produit(s) pendant les cycles : $($dumps -join ', ')" }
      }
      # Journal d'installation PnP, à côté des vidages, dès qu'il y a eu un échec (y
      # compris un échec né d'un vidage : le test vient donc APRÈS la collecte ci-dessus).
      if ($failure) {
        $lines = @(Invoke-Command -Session $session -ScriptBlock $setupapiBlock -ArgumentList $setupapiTailLines)
        if ($lines.Count -gt 0) {
          New-Item -ItemType Directory -Force -Path $dumpsDir | Out-Null
          $dest = Join-Path $dumpsDir ("{0}_setupapi.dev.log" -f $stamp)
          # -Encoding explicite : Set-Content écrit sinon dans la page de codes ANSI, et le
          # journal contient des chemins et des noms accentués.
          Set-Content -LiteralPath $dest -Value $lines -Encoding UTF8
          Write-Host "Journal PnP (dernières $($lines.Count) lignes de setupapi.dev.log) → $dest"
        } else {
          Write-Host "setupapi.dev.log introuvable ou illisible dans l'invité : rien à rapatrier."
        }
      }
    } catch {
      if (-not $failure) { $failure = $_ }
      else { $Host.UI.WriteErrorLine("collecte des vidages et du journal PnP impossible : $_") }
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
