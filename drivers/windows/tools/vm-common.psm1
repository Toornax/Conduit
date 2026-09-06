<#
.SYNOPSIS
  Fonctions partagées par les scripts de VM (vm-new.ps1, vm-prepare.ps1, vm-cycle.ps1).
.DESCRIPTION
  Les fonctions pures (analyse de sorties, génération de clé, décision d'accès) sont
  testées par tests\vm-common.Tests.ps1 (Pester 3/4). Les enveloppes qui touchent au
  système (jeton courant, registre, réseau de l'hôte, Hyper-V) ne sont pas testées : la
  décision en est systématiquement extraite dans une fonction pure à laquelle on passe
  les données lues, pour qu'elle soit testable sans jeton ni hyperviseur réels.

    Fonction pure (testée)                | Enveloppe système
    --------------------------------------|--------------------------------
    Test-SidPresent                       | Get-TokenSid
    Get-HyperVAccessMessage               | Assert-HyperVAccess
    Get-HyperVAccessHint                  | Invoke-HyperVChecked
    Test-AccessDeniedError                | —
    Get-KdCommandLine, Get-NamedPipeName  | Get-KdPath (vm-debug.ps1)
    Get-ConsoleSessionDecision            | lectures CIM dans l'invité (vm-run-console.ps1)
    Get-ConsoleRunScript, Get-TaskRunAsUser | schtasks dans l'invité (vm-run-console.ps1)
    Test-ConduitGhostDevice               | Get-PnpDevice dans l'invité (vm-cycle.ps1)
    Test-DevnodeGone                      | Get-PnpDevice dans l'invité (vm-cycle.ps1)
    Get-DebuggerAttachWarning             | Get-VM (vm-debug.ps1)
#>
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Builtin\Hyper-V Administrators : « complete and unrestricted access to all features of
# Hyper-V ». Appartenir à ce groupe suffit pour piloter Hyper-V, l'élévation n'est pas
# requise :
#   - « Ensure that your user account belongs to the Administrators group or the Hyper-V
#     Administrators group »
#     https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/manage/remotely-manage-hyper-v-hosts
#   - PowerShell Direct (Invoke-Command -VMName, New-PSSession -VMName,
#     Copy-Item -To/FromSession) : « You must be logged into the host computer as a
#     Hyper-V administrator »
#     https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/powershell-direct
#   - Table des SID connus (S-1-5-32-578 = Builtin\Hyper-V Administrators)
#     https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/manage/understand-security-identifiers
#
# Toujours par le SID, JAMAIS par le nom du groupe : « Hyper-V Administrators » est
# traduit selon la langue de Windows (« Administrateurs Hyper-V » en français). C'est
# exactement le piège qui avait cassé la création de VM avant le commit 25b3557, où le
# composant d'intégration était cherché par son nom traduit.
$script:HyperVAdminsSid = "S-1-5-32-578"

function Test-Elevated {
  <#
  .SYNOPSIS
    Vrai si le processus courant appartient au groupe Administrateurs.
  .DESCRIPTION
    Conservée bien que les scripts de VM n'exigent plus l'élévation (voir
    Test-HyperVOperator) : elle resservira si une opération précise s'avère l'exiger.
  #>
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-SidPresent {
  <#
  .SYNOPSIS
    Vrai si $Sid figure dans la liste de SID fournie (comparaison insensible à la casse).
  .DESCRIPTION
    Fonction pure : la lecture du jeton est faite à part (Get-TokenSid), pour que la
    décision d'accès soit testable sans jeton réel. Les SID sont des chaînes ASCII dont
    la casse n'est pas significative (« s-1-5-32-578 » désigne le même groupe).
  .PARAMETER Sid
    SID recherché, sous forme textuelle (S-1-5-32-578).
  .PARAMETER TokenSids
    SID présents dans le jeton, sous forme textuelle. Liste vide, $null ou contenant des
    entrées vides acceptés (une lecture de jeton peut légitimement ne rien rendre).
  #>
  param(
    [Parameter(Mandatory)][string]$Sid,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyCollection()][AllowEmptyString()][string[]]$TokenSids
  )
  $wanted = $Sid.Trim()
  foreach ($candidate in @($TokenSids)) {
    if ($null -eq $candidate) { continue }
    if ($candidate.Trim() -ieq $wanted) { return $true }
  }
  return $false
}

function Get-TokenSid {
  <#
  .SYNOPSIS
    SID du jeton EFFECTIF du processus courant (compte + groupes), sous forme textuelle.
  .DESCRIPTION
    WindowsIdentity.Groups n'expose que les SID réellement actifs : les SID marqués
    « refus uniquement » par le filtrage UAC en sont exclus (vérifié : en session non
    élevée, S-1-5-32-544 apparaît dans `whoami /groups` mais pas ici). C'est la propriété
    voulue : la documentation ne dit pas si S-1-5-32-578 survit à ce filtrage, mais le
    code n'en dépend pas puisqu'il lit le jeton effectif. Si le SID était filtré,
    Test-HyperVOperator renverrait faux et l'utilisateur verrait le message d'aide :
    dégradation propre, jamais de faux positif.
  #>
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $sids = New-Object System.Collections.Generic.List[string]
  if ($identity.User) { $sids.Add($identity.User.Value) }
  foreach ($group in @($identity.Groups)) {
    if ($null -eq $group) { continue }
    # Un SID sans traduction possible lève sur .Translate, jamais sur .Value : on ne
    # traduit pas, justement (les noms de groupes sont localisés).
    $sids.Add($group.Value)
  }
  return $sids.ToArray()
}

function Test-HyperVOperator {
  <#
  .SYNOPSIS
    Vrai si la session courante peut piloter Hyper-V : élevée (groupe Administrateurs),
    ou porteuse du SID du groupe Administrateurs Hyper-V (S-1-5-32-578).
  #>
  if (Test-Elevated) { return $true }
  return (Test-SidPresent -Sid $script:HyperVAdminsSid -TokenSids (Get-TokenSid))
}

function Get-HyperVAccessHint {
  <#
  .SYNOPSIS
    Indice à ajouter à un refus d'accès venu d'Hyper-V : la cause la plus probable est
    une appartenance au groupe acquise sans réouverture de session.
  .DESCRIPTION
    Fonction pure (aucune lecture du système), pour être testable et réutilisable dans
    Invoke-HyperVChecked.
  #>
  return @(
    "Cause la plus probable : l'appartenance au groupe Administrateurs Hyper-V"
    "(SID $script:HyperVAdminsSid) a été ajoutée sans réouverture de session. Le jeton ne"
    "prend une appartenance de groupe en compte qu'à l'OUVERTURE de la session Windows :"
    "fermer la session et la rouvrir (redémarrer le terminal ne suffit pas)."
  ) -join "`n"
}

function Get-HyperVAccessMessage {
  <#
  .SYNOPSIS
    Message d'échec quand la session ne peut pas piloter Hyper-V : il donne LES DEUX
    remèdes (relancer en administrateur, ou rejoindre le groupe Administrateurs Hyper-V).
  .DESCRIPTION
    Fonction pure : le nom d'utilisateur est passé en paramètre plutôt que lu dans
    l'environnement, pour que le message soit testable tel quel.
  .PARAMETER Reason
    Ce à quoi l'accès sert (affiché entre parenthèses).
  .PARAMETER UserName
    Compte à ajouter au groupe dans la commande proposée.
  #>
  param(
    [Parameter(Mandatory)][string]$Reason,
    [Parameter(Mandatory)][AllowEmptyString()][string]$UserName
  )
  $account = if ([string]::IsNullOrWhiteSpace($UserName)) { "<votre compte>" } else { $UserName }
  return @(
    "Accès à Hyper-V refusé ($Reason) : cette session n'est ni administrateur, ni membre"
    "du groupe Administrateurs Hyper-V (SID $script:HyperVAdminsSid)."
    ""
    "Deux remèdes, au choix :"
    "  1. Relancer ce script dans un PowerShell lancé en tant qu'administrateur ;"
    "  2. ou, une seule fois, depuis un PowerShell administrateur :"
    "       Add-LocalGroupMember -SID $script:HyperVAdminsSid -Member `"$account`""
    "     PUIS FERMER ET ROUVRIR LA SESSION WINDOWS. Le jeton ne prend l'appartenance"
    "     de groupe en compte qu'à l'ouverture de session : redémarrer le terminal ne"
    "     suffit pas."
  ) -join "`n"
}

function Assert-HyperVAccess {
  <#
  .SYNOPSIS
    Échoue avec le message de Get-HyperVAccessMessage si la session ne peut pas piloter
    Hyper-V (ni administrateur, ni membre du groupe Administrateurs Hyper-V).
  .PARAMETER Reason
    Ce à quoi l'accès sert (affiché dans le message).
  #>
  param([Parameter(Mandatory)][string]$Reason)
  if (-not (Test-HyperVOperator)) {
    throw (Get-HyperVAccessMessage -Reason $Reason -UserName $env:USERNAME)
  }
}

function Test-AccessDeniedError {
  <#
  .SYNOPSIS
    Vrai si l'erreur est un refus d'accès (E_ACCESSDENIED, 0x80070005), y compris
    enveloppée dans une ou plusieurs exceptions.
  .DESCRIPTION
    Reconnu par le HRESULT et par le type de l'exception, en remontant toute la chaîne
    InnerException. JAMAIS par le texte du message : il est traduit selon la langue de
    Windows, et s'y fier est le même piège que chercher un groupe par son nom.
    La catégorie PermissionDenied de l'ErrorRecord est une énumération, pas du texte :
    elle peut donc servir de signal supplémentaire.
  .PARAMETER ErrorRecord
    Un ErrorRecord (le `$_` d'un catch) ou directement une exception. $null accepté.
  #>
  param([Parameter(Mandatory)][AllowNull()]$ErrorRecord)
  if ($null -eq $ErrorRecord) { return $false }

  $accessDenied = -2147024891   # 0x80070005 (E_ACCESSDENIED), lu en Int32 signé.
  $exception = $ErrorRecord
  if ($ErrorRecord -is [System.Management.Automation.ErrorRecord]) {
    if ($ErrorRecord.CategoryInfo -and
        $ErrorRecord.CategoryInfo.Category -eq [System.Management.Automation.ErrorCategory]::PermissionDenied) {
      return $true
    }
    $exception = $ErrorRecord.Exception
  }

  $depth = 0
  while ($exception -and $depth -lt 16) {
    if ($exception -is [System.UnauthorizedAccessException] -or
        $exception -is [System.Security.SecurityException] -or
        $exception -is [System.Management.Automation.PSSecurityException]) {
      return $true
    }
    if ($exception.HResult -eq $accessDenied) { return $true }
    $exception = $exception.InnerException
    $depth++
  }
  return $false
}

function Invoke-HyperVChecked {
  <#
  .SYNOPSIS
    Exécute la PREMIÈRE commande Hyper-V d'un script en transformant un refus d'accès en
    message actionnable (Get-HyperVAccessHint).
  .DESCRIPTION
    Assert-HyperVAccess a déjà validé le jeton ; un refus d'accès à ce stade signifie
    presque toujours que l'appartenance au groupe a été ajoutée sans réouverture de
    session. Toute autre erreur est relancée telle quelle.
  .PARAMETER What
    Ce que la commande faisait (« inventaire des VM »), affiché en tête du message.
  .PARAMETER Script
    Le bloc à exécuter ; sa sortie est renvoyée telle quelle.
  #>
  param(
    [Parameter(Mandatory)][string]$What,
    [Parameter(Mandatory)][scriptblock]$Script
  )
  try {
    return (& $Script)
  } catch {
    if (Test-AccessDeniedError -ErrorRecord $_) {
      throw "$What : accès refusé par Hyper-V.`n$(Get-HyperVAccessHint)`n`nErreur d'origine : $($_.Exception.Message)"
    }
    throw
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

function Format-NativeArgument {
  <#
  .SYNOPSIS
    Entoure un argument de guillemets s'il en a besoin (espace, tabulation, ou argument
    vide), pour composer une ligne de commande destinée à un exécutable natif.
  .DESCRIPTION
    Privée (non exportée) : elle sert à Get-KdCommandLine et Get-ConsoleRunScript, dont
    les tests la couvrent. Windows PowerShell 5.1 ne cite RIEN quand on passe un tableau
    à `Start-Process -ArgumentList` : la citation est donc à notre charge.
  #>
  param([Parameter(Mandatory)][AllowEmptyString()][string]$Value)
  if ($Value -match '[\s"]') { return '"' + ($Value -replace '"', '\"') + '"' }
  if ($Value -eq "") { return '""' }
  return $Value
}

function Get-NamedPipeName {
  <#
  .SYNOPSIS
    Nom court d'un canal nommé local (« \\.\pipe\conduitdbg » → « conduitdbg »).
  .DESCRIPTION
    Fonction pure. Sert à chercher le canal dans l'espace de noms des canaux
    (\\.\pipe\), qui n'expose que les noms courts : c'est ainsi que vm-debug.ps1 vérifie
    que le débogueur tient bien le canal AVANT de démarrer la VM.
  .PARAMETER Pipe
    Chemin du canal, avec ou sans le préfixe « \\.\pipe\ » (le préfixe accepte aussi un
    nom de machine, « \\HOTE\pipe\… », ignoré ici : seul le nom local a un sens).
  .OUTPUTS
    Le nom court, ou $null si la chaîne ne désigne pas un canal nommé.
  #>
  param([Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$Pipe)
  if ([string]::IsNullOrWhiteSpace($Pipe)) { return $null }
  $value = $Pipe.Trim()
  if ($value -match '^\\\\[^\\]+\\pipe\\(.+)$') { return $Matches[1] }
  if ($value -match '[\\/]') { return $null }
  return $value
}

function Get-KdCommandLine {
  <#
  .SYNOPSIS
    Compose l'appel de kd.exe pour un débogage noyau par canal nommé série.
  .DESCRIPTION
    Fonction pure : c'est la partie de vm-debug.ps1 qui se teste sans Hyper-V ni WDK.
    `resets=0,reconnect` est ce qui rend l'ordre « débogueur d'abord, machine ensuite »
    praticable : le débogueur tient le canal et ne lâche pas tant que la VM n'a pas
    démarré, puis se raccroche aux redémarrages SUIVANTS de l'invité. Il ne rattrape pas
    pour autant une machine partie avant lui : voir Get-DebuggerAttachWarning.
  .PARAMETER KdPath
    Chemin de kd.exe (ou windbg.exe : mêmes options).
  .PARAMETER Pipe
    Canal nommé partagé avec le port COM de la VM (\\.\pipe\conduitdbg).
  .PARAMETER LogPath
    Fichier journal (-logo : écrase ; -loga ajouterait à la suite).
  .PARAMETER InitialCommands
    Commandes jouées à la connexion (-c). Les vides sont ignorées.
  .OUTPUTS
    Un objet { Path ; Arguments (tableau brut, pour Start-Process) ; ArgumentString
    (arguments cités) ; CommandLine (ligne complète, pour l'affichage et le journal) }.
  #>
  param(
    [Parameter(Mandatory)][string]$KdPath,
    [Parameter(Mandatory)][string]$Pipe,
    [Parameter(Mandatory)][string]$LogPath,
    [AllowNull()][AllowEmptyCollection()][string[]]$InitialCommands = @()
  )
  $arguments = @("-k", "com:pipe,port=$Pipe,resets=0,reconnect", "-logo", $LogPath)
  $commands = @(@($InitialCommands) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  if ($commands.Count -gt 0) { $arguments += @("-c", ($commands -join "; ")) }
  $quoted = @($arguments | ForEach-Object { Format-NativeArgument -Value $_ })
  return [pscustomobject]@{
    Path           = $KdPath
    Arguments      = $arguments
    ArgumentString = ($quoted -join " ")
    CommandLine    = "$(Format-NativeArgument -Value $KdPath) $($quoted -join " ")"
  }
}

function Get-DebuggerAttachWarning {
  <#
  .SYNOPSIS
    Avertissement à donner AVANT de lancer kd sur une VM déjà démarrée sans -StartVM : la
    connexion n'aboutira très probablement pas. Chaîne vide dans tous les autres cas.
  .DESCRIPTION
    Fonction pure (l'état de la VM est lu à part), pour que le texte soit testable sans
    Hyper-V.

    MESURÉ le 2026-09-06, et non déduit : `vm-debug.ps1` sans -StartVM sur une VM en
    marche écrit « Opened \\.\pipe\conduitdbg » puis reste INDÉFINIMENT sur « Waiting to
    reconnect... ». La connexion ne s'établit jamais, et les commandes initiales (-c) ne
    sont donc jamais jouées non plus. La règle « le débogueur doit tenir le canal avant
    que la machine démarre » vaut donc AUSSI pour un rattachement : `resets=0,reconnect`
    fait patienter le débogueur avant un démarrage, il ne rattrape pas un invité déjà
    parti. L'aide du script annonçait le contraire jusqu'à cette mesure.

    Le cas n'est pas interdit pour autant : rattacher un invité figé après un plantage
    reste utile, ne serait-ce que pour tenir le canal jusqu'au redémarrage suivant. On
    avertit, on ne bloque pas.
  .PARAMETER State
    État de la VM tel que rendu par Get-VM (énumération VMState : Running, Off, Paused…).
    Le NOM d'un membre d'énumération .NET n'est pas traduit, contrairement aux libellés du
    gestionnaire Hyper-V : la comparaison porte sur ce nom, jamais sur un affichage — même
    choix que Wait-VMOff. Vide ou $null quand l'état n'a pas pu être lu : aucun
    avertissement alors, faute de savoir.
  .PARAMETER StartVM
    Vrai quand -StartVM a été passé : le script impose alors l'ordre lui-même, rien à dire.
  .PARAMETER Name
    Nom de la VM, cité dans le message.
  #>
  param(
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$State,
    [Parameter(Mandatory)][bool]$StartVM,
    [Parameter(Mandatory)][AllowEmptyString()][string]$Name
  )
  if ($StartVM) { return "" }
  $value = if ($null -eq $State) { "" } else { $State.Trim() }
  if ($value -ine "Running") { return "" }
  return @(
    "La VM « $Name » est DÉMARRÉE et -StartVM n'a pas été passé : la connexion du"
    "débogueur n'aboutira très probablement pas."
    "Mesuré le 2026-09-06 : sur une VM déjà en marche, kd ouvre bien le canal"
    "(« Opened … ») puis reste indéfiniment sur « Waiting to reconnect... » ; la connexion"
    "ne s'établit jamais, et les commandes initiales (-c) ne sont donc jamais jouées."
    "La règle « le débogueur doit tenir le canal AVANT que la machine démarre » vaut aussi"
    "pour un rattachement."
    "Remède : relancer avec -StartVM, qui arrête la VM, place le débogueur, puis démarre."
    "On poursuit quand même : rattacher un invité figé après un plantage peut se justifier."
  ) -join "`n"
}

function Test-SameAccount {
  <#
  .SYNOPSIS
    Vrai si deux désignations de compte visent le même compte : « CONDUITTEST\nathan »,
    « .\nathan » et « nathan » sont le même utilisateur.
  .DESCRIPTION
    Fonction pure. On compare le nom de compte seul, sans le domaine : l'invité nomme le
    même utilisateur de trois façons selon la source (Win32_ComputerSystem, Win32_Process
    GetOwner, le [pscredential] saisi par l'opérateur). La comparaison ignore la casse,
    les noms de comptes Windows n'y étant pas sensibles.
  #>
  param(
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$Left,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$Right
  )
  $a = Get-AccountName -UserName $Left
  $b = Get-AccountName -UserName $Right
  if ([string]::IsNullOrEmpty($a) -or [string]::IsNullOrEmpty($b)) { return $false }
  return ($a -ieq $b)
}

function Get-AccountName {
  <#
  .SYNOPSIS
    Nom de compte seul, sans domaine ni machine : « CONDUITTEST\nathan » → « nathan »,
    « nathan@exemple » → « nathan », « .\nathan » → « nathan ».
  #>
  param([Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$UserName)
  if ([string]::IsNullOrWhiteSpace($UserName)) { return "" }
  $value = $UserName.Trim()
  $slash = $value.LastIndexOf("\")
  if ($slash -ge 0) { $value = $value.Substring($slash + 1) }
  $at = $value.IndexOf("@")
  if ($at -gt 0) { $value = $value.Substring(0, $at) }
  return $value.Trim()
}

function Get-TaskRunAsUser {
  <#
  .SYNOPSIS
    Compte à passer à `schtasks /ru` : qualifié par la machine invitée quand le
    [pscredential] ne l'est pas (« nathan » → « CONDUITTEST\nathan »).
  .DESCRIPTION
    Fonction pure. Le compte vient TOUJOURS du [pscredential] fourni, jamais d'une
    constante : le compte réel de la VM (`nathan`) n'est pas celui qu'annonçait la
    documentation (`test`), et une tâche créée pour le mauvais compte ne se déclenche
    jamais — l'attente irait jusqu'au délai maximal sans rien dire d'utile.
  .PARAMETER UserName
    Nom tel que saisi (nathan, .\nathan, CONDUITTEST\nathan, nathan@exemple).
  .PARAMETER ComputerName
    Nom de la machine invitée ($env:COMPUTERNAME lu DANS l'invité). Vide accepté.
  #>
  param(
    [Parameter(Mandatory)][string]$UserName,
    [Parameter(Mandatory)][AllowEmptyString()][string]$ComputerName
  )
  $value = $UserName.Trim()
  if ($value.StartsWith(".\")) { $value = $value.Substring(2) }
  if ($value.Contains("\") -or $value.Contains("@")) { return $value }
  if ([string]::IsNullOrWhiteSpace($ComputerName)) { return $value }
  return "$($ComputerName.Trim())\$value"
}

function Get-ConsoleSessionDecision {
  <#
  .SYNOPSIS
    Décide, à partir de données brutes lues dans l'invité, si une session console
    interactive y est ouverte — et laquelle.
  .DESCRIPTION
    Fonction pure : les lectures système sont faites à part (Win32_ComputerSystem et
    Win32_Process dans l'invité), pour que la décision soit testable sans VM.

    Aucune dépendance à la langue : ni `quser`, dont toute la sortie est traduite, ni le
    moindre libellé d'état. On croise deux faits : l'utilisateur ouvert à la CONSOLE
    (Win32_ComputerSystem.UserName, vide si personne) et l'identifiant de session de son
    processus `explorer` (Win32_Process.SessionId), qui doit être DIFFÉRENT de 0 — la
    session 0 est celle des services, sans audio utilisateur.
  .PARAMETER ConsoleUser
    (Get-CimInstance Win32_ComputerSystem).UserName de l'invité. Vide ou $null accepté.
  .PARAMETER ExplorerSessions
    Un objet par processus explorer.exe, portant UserName et SessionId.
  .OUTPUTS
    { Ready ; UserName ; SessionId (-1 si inconnu) ; Reason (vide si Ready) }.
  #>
  param(
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$ConsoleUser,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyCollection()][object[]]$ExplorerSessions
  )
  $user = if ($null -eq $ConsoleUser) { "" } else { $ConsoleUser.Trim() }
  if ([string]::IsNullOrEmpty($user)) {
    return [pscustomobject]@{
      Ready = $false; UserName = ""; SessionId = -1
      Reason = "aucun utilisateur n'est ouvert à la console de l'invité (Win32_ComputerSystem.UserName est vide)"
    }
  }

  $matched = @()
  foreach ($entry in @($ExplorerSessions)) {
    if ($null -eq $entry) { continue }
    $properties = $entry.PSObject.Properties
    $owner = if ($properties.Match("UserName").Count -gt 0) { [string]$entry.UserName } else { "" }
    $session = -1
    if ($properties.Match("SessionId").Count -gt 0 -and $null -ne $entry.SessionId) {
      $parsed = 0
      if ([int]::TryParse([string]$entry.SessionId, [ref]$parsed)) { $session = $parsed }
    }
    if (Test-SameAccount -Left $owner -Right $user) { $matched += $session }
  }

  foreach ($session in $matched) {
    if ($session -gt 0) {
      return [pscustomobject]@{ Ready = $true; UserName = $user; SessionId = $session; Reason = "" }
    }
  }
  if ($matched.Count -gt 0) {
    return [pscustomobject]@{
      Ready = $false; UserName = $user; SessionId = 0
      Reason = "« $user » n'a d'explorer que dans la session 0, celle des services : elle n'a pas d'audio utilisateur"
    }
  }
  return [pscustomobject]@{
    Ready = $false; UserName = $user; SessionId = -1
    Reason = "« $user » est ouvert mais son bureau n'est pas chargé (aucun processus explorer) : session verrouillée, déconnectée, ou console jamais ouverte"
  }
}

function Get-ConsoleRunScript {
  <#
  .SYNOPSIS
    Texte du fichier de commandes (.cmd) exécuté par la tâche planifiée dans la session
    console : il lance l'exécutable, redirige sa sortie, PUIS écrit son code de retour
    dans un second fichier qui sert de sentinelle de fin.
  .DESCRIPTION
    Fonction pure. Deux points ne sont pas négociables :

    - la sentinelle. `schtasks /query` rend un statut TRADUIT (« En cours d'exécution »,
      « Prêt ») : s'y fier reviendrait à analyser du texte localisé. Le fichier de code de
      retour, lui, n'apparaît qu'une fois la commande terminée, et son contenu est un
      entier. C'est le seul signal de fin utilisé ;
    - l'ordre `> fichier echo %CODE%`. Écrit dans l'autre sens, `echo %CODE%>fichier`,
      cmd.exe lirait le dernier chiffre du code comme un numéro de flux (`1>`) et le
      fichier serait vide ou faux.
  .PARAMETER Executable
    Chemin de l'exécutable DANS l'invité.
  .PARAMETER Arguments
    Ses arguments, un par élément (cités au besoin).
  .PARAMETER OutputPath
    Fichier où la sortie (standard et erreur) est redirigée.
  .PARAMETER ExitCodePath
    Fichier sentinelle recevant le code de retour.
  .PARAMETER WorkingDirectory
    Dossier courant de la commande (facultatif).
  #>
  param(
    [Parameter(Mandatory)][string]$Executable,
    [AllowNull()][AllowEmptyCollection()][string[]]$Arguments = @(),
    [Parameter(Mandatory)][string]$OutputPath,
    [Parameter(Mandatory)][string]$ExitCodePath,
    [string]$WorkingDirectory
  )
  # Un « % » d'argument doit être doublé pour traverser cmd.exe sans être pris pour une
  # variable ; le fichier étant engendré, l'opérateur n'a rien à échapper lui-même.
  $quoted = @(@($Arguments) | ForEach-Object {
    if ($null -eq $_) { "" } else { (Format-NativeArgument -Value $_) -replace "%", "%%" }
  })
  # Les CHEMINS sont cités systématiquement, même sans espace : un dossier de travail
  # changé un jour ne doit pas transformer un fichier de commandes correct en commande
  # tronquée au premier espace. Un chemin Windows ne peut pas contenir de guillemet.
  $exe = '"' + $Executable + '"'
  $command = (@($exe) + $quoted) -join " "
  $lines = @(
    "@echo off",
    "rem Engendré par vm-run-console.ps1 : ne pas modifier, il est réécrit à chaque appel."
  )
  if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
    $lines += 'cd /d "' + $WorkingDirectory + '"'
  }
  $lines += "$command >`"$OutputPath`" 2>&1"
  $lines += "set CONDUIT_CODE=%ERRORLEVEL%"
  $lines += ">`"$ExitCodePath`" echo %CONDUIT_CODE%"
  return (($lines -join "`r`n") + "`r`n")
}

function Test-ConduitGhostDevice {
  <#
  .SYNOPSIS
    Vrai si un périphérique NON PRÉSENT (fantôme) nous appartient et peut donc être
    retiré : périphérique Root\ConduitCable, ou endpoint audio dont le nom convivial
    contient « Conduit ».
  .DESCRIPTION
    Fonction pure, aussi envoyée telle quelle dans l'invité (voir Get-FunctionSource) par
    vm-cycle.ps1 — elle ne référence donc AUCUNE variable de module.

    Prudence délibérée : cette VM sert aussi à comparer avec de vraies cartes son, et un
    fantôme supprimé par erreur ne revient qu'au prochain branchement. Trois garde-fous :
    un périphérique PRÉSENT n'est jamais candidat ; la classe est reconnue par son GUID
    (le nom de classe affiché est traduit) ; et le nom convivial doit contenir « Conduit ».
  .PARAMETER InstanceId
    Identifiant d'instance (ROOT\CONDUITCABLE\0000, SWD\MMDEVAPI\{…}).
  .PARAMETER FriendlyName
    Nom convivial ; $null ou vide accepté (un fantôme peut n'en avoir plus).
  .PARAMETER ClassGuid
    GUID de classe, avec ou sans accolades ; $null accepté.
  .PARAMETER Present
    Faux pour un périphérique fantôme (Get-PnpDevice, propriété Present : un booléen,
    contrairement à Status, qui est du texte traduit).
  #>
  param(
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$InstanceId,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$FriendlyName,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$ClassGuid,
    [Parameter(Mandatory)][bool]$Present
  )
  if ($Present) { return $false }
  $id = if ($null -eq $InstanceId) { "" } else { $InstanceId.Trim() }
  if ($id -eq "") { return $false }

  # Nos propres périphériques racine : l'identifiant d'instance suffit et ne dépend de rien.
  if ($id -imatch '^ROOT\\CONDUITCABLE\\') { return $true }

  # Endpoints audio : classe AudioEndpoint {c166523c-fe0c-4a94-a586-f1a80cfbbf3e}, ou
  # énumérateur SWD\MMDEVAPI (les deux désignent un endpoint ; on accepte l'un OU l'autre
  # pour ne pas dépendre d'un seul identifiant). Et le nom doit être le nôtre.
  $name = if ($null -eq $FriendlyName) { "" } else { $FriendlyName }
  if ($name -inotlike "*conduit*") { return $false }
  $guid = if ($null -eq $ClassGuid) { "" } else { ($ClassGuid -replace '[{}\s]', '') }
  if ($guid -ieq "c166523c-fe0c-4a94-a586-f1a80cfbbf3e") { return $true }
  if ($id -imatch '^SWD\\MMDEVAPI\\') { return $true }
  return $false
}

function Test-DevnodeGone {
  <#
  .SYNOPSIS
    Vrai si le devnode $InstanceId a bien disparu de l'inventaire PnP fourni : soit il n'y
    figure plus du tout, soit il n'y figure qu'en périphérique NON PRÉSENT (fantôme).
  .DESCRIPTION
    Fonction pure, aussi envoyée telle quelle dans l'invité (voir Get-FunctionSource) par
    vm-cycle.ps1 — elle ne référence donc AUCUNE variable de module. L'inventaire est lu à
    part (Get-PnpDevice -InstanceId … dans l'invité), pour que la décision soit testable
    sans VM.

    À QUOI ELLE SERT (mesuré le 2026-09-06, voir vm-cycle.ps1) : `devgen /remove` rend la
    main avant que PnP ait fini de démonter le devnode. Enchaîner tout de suite
    `pnputil /delete-driver … /uninstall /force` retire le paquet sous les pieds d'un
    devnode encore vivant, et PnP journalise un dernier démarrage raté (Kernel-PnP 411,
    état 0xC00000E5). Cette fonction dit quand l'attente peut cesser.

    DÉCISION STRUCTURELLE, JAMAIS UN LIBELLÉ : on compare des identifiants d'instance
    (chaînes ASCII engendrées par PnP, jamais traduites) et on lit `Present`, un BOOLÉEN —
    contrairement à `Status`, qui est du texte traduit (« OK », « Erreur », « Inconnu »).
    Le cas normal après un retrait est la disparition pure et simple ; on accepte aussi le
    fantôme, parce qu'un devnode non présent n'est plus un devnode que PnP démarre, et
    parce que l'invité en laisse parfois un derrière lui.
  .PARAMETER InstanceId
    Identifiant d'instance attendu disparu (SWD\DEVGEN\{…}). Vide ou $null : rien à
    attendre, donc vrai.
  .PARAMETER Devices
    Ce que Get-PnpDevice a rendu : des objets portant InstanceId et Present. Liste vide,
    $null ou entrées incomplètes acceptées (un périphérique retiré ne rend plus rien).
  #>
  param(
    [Parameter(Mandatory)][AllowNull()][AllowEmptyString()][string]$InstanceId,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyCollection()][object[]]$Devices
  )
  $wanted = if ($null -eq $InstanceId) { "" } else { $InstanceId.Trim() }
  if ($wanted -eq "") { return $true }
  foreach ($device in @($Devices)) {
    if ($null -eq $device) { continue }
    $properties = $device.PSObject.Properties
    if ($properties.Match("InstanceId").Count -eq 0) { continue }
    $id = [string]$device.InstanceId
    if ($null -eq $id) { $id = "" }
    # Les identifiants d'instance ne sont pas sensibles à la casse (« SWD\ » et « swd\ »
    # désignent le même énumérateur) : la comparaison l'ignore, comme partout ailleurs.
    if ($id.Trim() -ine $wanted) { continue }
    $present = $true
    if ($properties.Match("Present").Count -gt 0 -and $null -ne $device.Present) {
      $present = [bool]$device.Present
    }
    if ($present) { return $false }
  }
  return $true
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

function Get-KdPath {
  <#
  .SYNOPSIS
    Chemin d'un débogueur des Debugging Tools for Windows sur l'hôte
    (<KitsRoot10>\Debuggers\x64\kd.exe), résolu par le registre comme Get-DevgenPath.
  .PARAMETER Name
    kd.exe (console, celui qu'automatise vm-debug.ps1) ou windbg.exe (interface).
  #>
  param([ValidateSet("kd.exe", "windbg.exe")][string]$Name = "kd.exe")
  $path = Join-Path (Get-WdkRoot) "Debuggers\x64\$Name"
  if (-not (Test-Path $path)) {
    throw "$Name introuvable : $path (installer les Debugging Tools for Windows du WDK)."
  }
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

function Wait-VMOff {
  <#
  .SYNOPSIS
    Attend que la VM soit à l'arrêt complet (état Off), avec un délai borné.
  .DESCRIPTION
    Nécessaire avant `Set-VMComPort`, qui refuse toute VM allumée : c'est la seule
    fenêtre où le canal nommé série peut être attaché. L'état est comparé à l'énumération
    VMState, jamais à un libellé affiché.
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [int]$TimeoutSeconds = 300
  )
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $vm = Get-VM -Name $Name
    if ($vm.State -eq "Off") { return }
    Start-Sleep -Seconds 2
  }
  throw "La VM « $Name » ne s'est pas arrêtée après $TimeoutSeconds s (état : $((Get-VM -Name $Name).State))."
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

Export-ModuleMember -Function Test-Elevated, Test-SidPresent, Get-TokenSid,
  Test-HyperVOperator, Get-HyperVAccessHint, Get-HyperVAccessMessage, Assert-HyperVAccess,
  Test-AccessDeniedError, Invoke-HyperVChecked, New-DebugKey, Test-DebugKey,
  Invoke-NativeChecked, Get-FunctionSource, ConvertFrom-DevgenAddOutput, Find-PublishedInf,
  Get-CycleSummary, Get-NamedPipeName, Get-KdCommandLine, Get-DebuggerAttachWarning,
  Get-AccountName, Test-SameAccount,
  Get-TaskRunAsUser, Get-ConsoleSessionDecision, Get-ConsoleRunScript, Test-ConduitGhostDevice,
  Test-DevnodeGone, Get-WdkRoot, Get-DevgenPath, Get-KdPath, Get-DefaultSwitchHostIp, Wait-VMHeartbeat,
  Wait-VMReboot, Wait-VMOff, New-GuestSession
