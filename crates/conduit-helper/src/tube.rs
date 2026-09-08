//! Le canal nommé : le serveur qui écoute en `LocalSystem`, et le client que le démon
//! utilisera.
//!
//! # Le serveur ne fait jamais confiance à ce qu'il reçoit
//!
//! Trois règles, appliquées dans cet ordre à chaque connexion :
//!
//! 1. **la taille avant l'allocation** — l'en-tête de quatre octets est confronté à
//!    `MAX_TRAME_OCTETS` par [`longueur_annoncee`] *avant* qu'un tampon ne soit
//!    réservé ; sans cela, quatre octets suffiraient à faire réserver 4 Gio au processus
//!    le plus privilégié du produit ;
//! 2. **la forme exacte** — la charge est confiée au parseur **pur**
//!    [`Requete::from_bytes`], qui refuse toute longueur inattendue, tout champ réservé
//!    non nul et tout champ qu'un ordre n'utilise pas ;
//! 3. **une requête par connexion** — le client se connecte, envoie un ordre, lit la
//!    réponse, se déconnecte. Pas de session, pas d'état conservé entre deux ordres, donc
//!    aucune machine à états à faire déraper.
//!
//! # Une instance, réutilisée, et des attentes bornées
//!
//! Le service crée **une seule** instance du canal et la réutilise
//! (`DisconnectNamedPipe` puis `ConnectNamedPipe`), au lieu d'en créer une par client.
//! Ce n'est pas un choix de confort : avec le descripteur de `crate::securite`, qui
//! refuse `FILE_CREATE_PIPE_INSTANCE` aux utilisateurs ordinaires pour empêcher le
//! squattage du nom, un processus non élevé se voit refuser la **seconde** instance par
//! `ERROR_ACCESS_DENIED` — mesuré le 2026-09-08 sur le poste de développement. Voir
//! `creer_instance`.
//!
//! Tout est donc en **recouvrement** (`FILE_FLAG_OVERLAPPED`) : l'attente d'un client se
//! fait sur deux objets à la fois (le client et l'arrêt), et chaque lecture ou écriture
//! est bornée par [`DELAI_REQUETE`]. C'est ce qui remplace le fil par connexion — un
//! client qui se tairait ne garde l'instance que cinq secondes — et c'est aussi ce qui
//! permet au service de s'arrêter sans qu'un client se présente. Un second client trouve
//! l'instance occupée et attend son tour (`WaitNamedPipeW`, côté client).
//!
//! # L'identité de l'appelant
//!
//! Elle est lue **après** la requête et **avant** l'exécution, par
//! `ImpersonateNamedPipeClient` : le service prend le jeton du client le temps de lire
//! son SID, puis `GardeImpersonation` appelle `RevertToSelf` à sa destruction. C'est
//! le point le plus délicat du fichier : un service qui resterait à impersonner son
//! client tenterait ensuite l'écriture KS avec un jeton **sans** le privilège, et le
//! pilote la refuserait — le symptôme serait un `ERROR_PRIVILEGE_NOT_HELD` sur un
//! service pourtant `LocalSystem`. Le garde est ce qui rend cet oubli impossible.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, RevertToSelf, TokenUser, PSID, SID_NAME_USE,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    ImpersonateNamedPipeClient, WaitNamedPipeW, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThread, OpenThreadToken, ResetEvent, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject, INFINITE,
};
use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};

use crate::cables;
use crate::journal::{Appelant, Journal};
use crate::protocole::{
    decouper, longueur_annoncee, ErreurReponse, Reponse, Requete, Statut, EN_TETE_OCTETS, NOM_TUBE,
    PROTOCOLE_VERSION, TAILLE_REPONSE,
};
use crate::securite::{DescripteurTube, ErreurSecurite, ACCES_OUVERTURE_CLIENT};

/// Temps accordé à un client pour envoyer sa requête et lire sa réponse, en
/// millisecondes.
///
/// Cinq secondes : un ordre traverse le canal, l'énumération et l'IOCTL en quelques
/// dizaines de millisecondes (l'endpoint suit en 77 ms, mesuré). C'est donc une borne
/// très large, et c'est le point — elle n'est pas là pour régler un débit mais pour
/// qu'un client qui se connecte sans rien envoyer ne garde pas l'unique instance du canal
/// pour lui.
pub const DELAI_REQUETE: u32 = 5_000;

/// `ERROR_TIMEOUT` (`winerror.h`) : ce que le service rapporte quand [`DELAI_REQUETE`]
/// expire.
///
/// La caisse `windows` ne le publie pas sous une feature qu'on ait déjà ; la valeur est
/// celle de `winerror.h`, et elle n'apparaît que dans le journal.
pub const ERROR_TIMEOUT: u32 = 1460;

/// Temps qu'un client attend qu'une instance se libère, en millisecondes.
///
/// Le service ne tient qu'une instance : un second client trouve le canal occupé
/// (`ERROR_PIPE_BUSY`) et attend son tour par `WaitNamedPipeW` au lieu d'échouer. Le
/// délai est celui d'une requête, plus la sienne.
const DELAI_ATTENTE_CLIENT: u32 = DELAI_REQUETE;

/// Taille des tampons du canal, en octets.
///
/// Bien plus grande que les 8 et 28 octets échangés : c'est le tampon que le noyau
/// réserve par instance, et le prendre trop juste ferait bloquer une écriture qui
/// pourrait tenir d'un coup.
const TAMPON_TUBE: u32 = 4096;

/// Un handle Windows possédé, fermé à la destruction.
///
/// Le même motif que `conduit_backend_wasapi::cable::Jeton` et `TopologyFilter`, qui ne
/// sont pas exportés : trois lignes plutôt qu'une dépendance à leurs internes.
struct Poignee(HANDLE);

// Un `HANDLE` est un `*mut c_void`, donc `!Send` et `!Sync` par défaut ; un handle noyau
// est pourtant tout à fait utilisable depuis un autre fil, et l'événement d'arrêt l'est
// depuis deux (la boucle l'attend, le gestionnaire de contrôle du service le signale).
// Le type garantit la propriété **exclusive** (ni `Copy` ni `Clone`), donc la fermeture
// n'a lieu qu'une fois.
// SAFETY: propriété exclusive d'un handle noyau ; les seules opérations concurrentes
// possibles sur lui (`SetEvent` / `WaitForMultipleObjects`) sont sûres par contrat du
// système.
unsafe impl Send for Poignee {}
// SAFETY: même raison.
unsafe impl Sync for Poignee {}

impl Drop for Poignee {
    fn drop(&mut self) {
        if self.0.is_invalid() {
            return;
        }
        // SAFETY: handle rendu par `CreateNamedPipeW`, `CreateFileW`, `CreateEventW` ou
        // `OpenThreadToken`, fermé une seule fois : le type n'est ni `Copy` ni clonable,
        // et `Drop` ne s'exécute qu'une fois.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Le dernier code d'erreur du fil courant.
fn dernier_code() -> u32 {
    // SAFETY: `GetLastError` ne prend aucun paramètre et lit le code du fil courant.
    unsafe { GetLastError() }.0
}

/// Le code Win32 caché dans un `HRESULT` de la caisse `windows`.
///
/// Les enveloppes rangent `GetLastError` dans un `HRESULT` par `HRESULT_FROM_WIN32` :
/// `0x8007` suivi des seize bits de poids faible du code. On refait le chemin inverse,
/// comme `conduit_backend_wasapi::cable::OsError::from_hresult`.
fn code_win32(erreur: &windows::core::Error) -> u32 {
    (erreur.code().0 as u32) & 0xFFFF
}

/// Crée l'événement à réarmement manuel que le service utilise pour ses attentes.
///
/// **Manuel et non automatique** : `attendre_client` attend deux objets et lit ensuite le
/// résultat de l'opération ; un événement à réarmement automatique serait consommé par
/// l'attente et `GetOverlappedResult` n'aurait plus rien à constater.
fn creer_evenement() -> Result<Poignee, ErreurTube> {
    // SAFETY: aucun attribut de sécurité (l'événement n'est pas nommé, donc inaccessible
    // à quiconque d'autre), réarmement manuel, état initial non signalé, pas de nom.
    let handle = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(|_| {
        ErreurTube::Creation {
            code: dernier_code(),
        }
    })?;
    Ok(Poignee(handle))
}

// ---------------------------------------------------------------------------------
// L'impersonation, et sa garde.
// ---------------------------------------------------------------------------------

/// Tant qu'il vit, le fil porte le jeton du client ; sa destruction rend le jeton du
/// service.
///
/// **Le service doit être revenu à lui-même avant toute écriture KS** : le jeton du
/// client n'a pas `SeLoadDriverPrivilege`, et l'écriture serait refusée. Voir l'en-tête
/// de module.
struct GardeImpersonation;

impl Drop for GardeImpersonation {
    fn drop(&mut self) {
        // SAFETY: `RevertToSelf` ne prend aucun paramètre et défait l'impersonation du
        // fil courant. Son échec ne se rattrape pas : le fil ne servira plus rien après
        // cette connexion, et le processus n'est pas affecté.
        let _ = unsafe { RevertToSelf() };
    }
}

/// L'identité du client d'une instance de canal **déjà lue** (le client a écrit).
///
/// Ne rend jamais d'erreur : l'identité sert au **journal**, pas au contrôle d'accès —
/// celui-ci est fait par le descripteur de sécurité du canal. Un ordre dont on n'a pas
/// su lire l'auteur est servi quand même, et la ligne le dit
/// ([`Appelant::inconnu`]) plutôt que d'inventer un nom.
fn identifier(tube: HANDLE) -> Appelant {
    let mut pid: u32 = 0;
    // SAFETY: `tube` est une instance de canal connectée, vivante pendant l'appel ;
    // `pid` est une variable de cette pile, écrite par l'appelé seul.
    let _ = unsafe { GetNamedPipeClientProcessId(tube, &mut pid) };

    // SAFETY: `tube` est connecté et le client a déjà écrit sa requête — la condition
    // que documente `ImpersonateNamedPipeClient`. En cas d'échec le fil n'impersonne
    // rien, et on rend l'appelant inconnu sans poser de garde.
    if unsafe { ImpersonateNamedPipeClient(tube) }.is_err() {
        return Appelant::inconnu(pid);
    }
    // Posé **immédiatement** après le succès : tout retour de cette fonction repasse par
    // `RevertToSelf`.
    let _garde = GardeImpersonation;

    let mut jeton = HANDLE::default();
    // SAFETY: `GetCurrentThread` rend un pseudo-handle constant ; `jeton` est une
    // variable de cette pile. `openasself = false` : le jeton voulu est justement celui
    // du client qu'on vient d'impersonner.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, false, &mut jeton) }.is_err() {
        return Appelant::inconnu(pid);
    }
    let jeton = Poignee(jeton);

    let mut taille: u32 = 0;
    // SAFETY: premier appel de la paire documentée : aucun tampon transmis, la seule
    // sortie est `taille`. L'échec (`ERROR_INSUFFICIENT_BUFFER`) est attendu.
    let _ = unsafe { GetTokenInformation(jeton.0, TokenUser, None, 0, &mut taille) };
    if taille == 0 {
        return Appelant::inconnu(pid);
    }
    let mut tampon = vec![0u8; taille as usize];
    let mut rendus: u32 = 0;
    // SAFETY: `tampon` fait exactement `taille` octets — la taille que l'appel précédent
    // a demandée — et vit pendant tout l'appel ; c'est cette longueur qui borne ce que le
    // système écrit. `rendus` est une variable de cette pile.
    let lecture = unsafe {
        GetTokenInformation(
            jeton.0,
            TokenUser,
            Some(tampon.as_mut_ptr().cast()),
            taille,
            &mut rendus,
        )
    };
    if lecture.is_err() {
        return Appelant::inconnu(pid);
    }
    if (rendus as usize) < size_of::<TOKEN_USER>() {
        return Appelant::inconnu(pid);
    }
    // SAFETY: `tampon` contient un `TOKEN_USER` que le système vient d'y écrire, d'au
    // moins `size_of::<TOKEN_USER>()` octets (vérifié juste au-dessus). Le tampon vient
    // d'un `Vec<u8>`, donc aligné sur 1 : la structure est lue par `read_unaligned`,
    // jamais déréférencée en place. Elle ne contient qu'un pointeur, qui vise plus loin
    // dans ce même tampon et reste valide tant qu'il vit.
    let utilisateur = unsafe { tampon.as_ptr().cast::<TOKEN_USER>().read_unaligned() };
    let sid = utilisateur.User.Sid;
    if sid.is_invalid() {
        return Appelant::inconnu(pid);
    }
    Appelant {
        nom: nom_du_sid(sid),
        sid: sid_en_texte(sid),
        pid,
    }
}

/// Le SID sous sa forme textuelle (`S-1-5-21-…`), ou `?`.
fn sid_en_texte(sid: PSID) -> String {
    let mut texte = windows::core::PWSTR::null();
    // SAFETY: `sid` vise un SID valide dans le tampon de `identifier`, vivant pendant
    // l'appel ; `texte` est une variable de cette pile, que l'appelé remplit d'un bloc
    // qu'il alloue et dont il nous confie la libération.
    if unsafe { ConvertSidToStringSidW(sid, &mut texte) }.is_err() || texte.is_null() {
        return "?".to_owned();
    }
    // SAFETY: `texte` est une chaîne large terminée par NUL, rendue par l'appel qui vient
    // de réussir.
    let sortie = unsafe { texte.to_string() }.unwrap_or_else(|_| "?".to_owned());
    // SAFETY: le bloc vient de `ConvertSidToStringSidW`, qui documente `LocalFree` comme
    // sa libération ; il n'est libéré qu'ici, et `texte` n'est plus lu ensuite.
    let _ = unsafe { LocalFree(Some(HLOCAL(texte.0.cast()))) };
    sortie
}

/// `DOMAINE\utilisateur` pour ce SID, ou `?` si la résolution échoue.
///
/// La résolution peut échouer légitimement — un compte supprimé, un contrôleur de
/// domaine injoignable — et ce n'est pas une raison de refuser l'ordre : le SID, lui,
/// est toujours dans la ligne de journal.
fn nom_du_sid(sid: PSID) -> String {
    let mut taille_nom: u32 = 0;
    let mut taille_domaine: u32 = 0;
    let mut genre = SID_NAME_USE::default();
    // SAFETY: premier appel de la paire documentée : aucun tampon transmis, les seules
    // sorties sont les deux tailles et le genre, variables de cette pile. L'échec
    // (`ERROR_INSUFFICIENT_BUFFER`) est attendu.
    let _ = unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            sid,
            None,
            &mut taille_nom,
            None,
            &mut taille_domaine,
            &mut genre,
        )
    };
    if taille_nom == 0 {
        return "?".to_owned();
    }
    let mut nom = vec![0u16; taille_nom as usize];
    let mut domaine = vec![0u16; taille_domaine.max(1) as usize];
    // SAFETY: les deux tampons font exactement les tailles que l'appel précédent a
    // demandées et vivent pendant tout l'appel ; les longueurs transmises sont les leurs,
    // et c'est ce qui borne ce que le système écrit.
    if unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            sid,
            Some(windows::core::PWSTR(nom.as_mut_ptr())),
            &mut taille_nom,
            Some(windows::core::PWSTR(domaine.as_mut_ptr())),
            &mut taille_domaine,
            &mut genre,
        )
    }
    .is_err()
    {
        return "?".to_owned();
    }
    let nom = String::from_utf16_lossy(nom.get(..taille_nom as usize).unwrap_or(&[]));
    let domaine = String::from_utf16_lossy(domaine.get(..taille_domaine as usize).unwrap_or(&[]));
    if domaine.is_empty() {
        nom
    } else {
        format!("{domaine}\\{nom}")
    }
}

// ---------------------------------------------------------------------------------
// Le serveur.
// ---------------------------------------------------------------------------------

/// Ce qui a empêché le serveur de démarrer ou de servir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErreurTube {
    /// Le descripteur de sécurité n'a pas pu être construit ou vérifié : le service ne
    /// démarre pas.
    Securite(ErreurSecurite),
    /// `CreateNamedPipeW` a échoué.
    Creation {
        /// Le code Win32 rendu, tel quel.
        code: u32,
    },
    /// `ConnectNamedPipe` a échoué.
    Connexion {
        /// Le code Win32 rendu, tel quel.
        code: u32,
    },
}

impl core::fmt::Display for ErreurTube {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Securite(erreur) => write!(f, "{erreur}"),
            Self::Creation { code } => write!(
                f,
                "création de {NOM_TUBE} impossible (erreur Win32 {code}) : un autre service \
                 tient-il déjà ce nom ?"
            ),
            Self::Connexion { code } => {
                write!(
                    f,
                    "attente d'un client sur {NOM_TUBE} : erreur Win32 {code}"
                )
            }
        }
    }
}

impl std::error::Error for ErreurTube {}

/// Le drapeau d'arrêt, partagé entre la boucle d'acceptation et le gestionnaire de
/// contrôle du service.
///
/// Un booléen **et** un événement : le premier dit *quoi*, le second réveille la boucle
/// qui attend un client. Le gestionnaire de contrôle du service, appelé par un fil du
/// système, ne fait que les toucher tous les deux — c'est tout ce qu'il a le droit de
/// faire sans bloquer le contrôleur de services.
#[derive(Debug)]
pub struct Arret {
    /// Levé une fois, jamais rabaissé.
    drapeau: AtomicBool,
    /// L'événement que [`Self::demander`] signale et que la boucle attend.
    evenement: Poignee,
}

impl core::fmt::Debug for Poignee {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Poignee").finish_non_exhaustive()
    }
}

impl Arret {
    /// Un drapeau non levé, avec son événement.
    ///
    /// # Erreurs
    ///
    /// [`ErreurTube::Creation`] si le système refuse de créer l'événement. Un service qui
    /// ne peut pas se faire arrêter proprement ne démarre pas : mieux vaut un échec au
    /// démarrage qu'une console des services bloquée sur « arrêt en cours ».
    pub fn nouveau() -> Result<Self, ErreurTube> {
        Ok(Self {
            drapeau: AtomicBool::new(false),
            evenement: creer_evenement()?,
        })
    }

    /// Demande l'arrêt, et **réveille** la boucle d'acceptation.
    ///
    /// Lever le drapeau ne suffit pas : la boucle attend un client. Elle attend donc
    /// **deux** objets à la fois — l'événement du `ConnectNamedPipe` en recouvrement et
    /// son événement — et c'est le second que cette méthode signale. Pas de client
    /// factice à envoyer à soi-même, pas de connexion parasite à distinguer d'une vraie.
    pub fn demander(&self) {
        self.drapeau.store(true, Ordering::SeqCst);
        // SAFETY: `self.evenement` est un événement créé par `Self::nouveau` et vivant
        // tant que `self`. Son échec ne se rattrape pas : le drapeau est déjà levé, et la
        // boucle le relira au prochain tour ou à l'expiration de son attente.
        let _ = unsafe { SetEvent(self.evenement.0) };
    }

    /// L'arrêt a-t-il été demandé ?
    #[must_use]
    pub fn demande(&self) -> bool {
        self.drapeau.load(Ordering::SeqCst)
    }
}

/// Crée **l'unique** instance du canal, avec le descripteur de sécurité vérifié.
///
/// # Une seule instance, et c'est un choix de sécurité autant que de simplicité
///
/// `nMaxInstances = 1` : personne ne peut créer une seconde instance de ce nom, même en
/// ayant le droit de le faire. C'est la seconde moitié de la défense contre le squattage
/// de `\\.\pipe\conduit-helper` (la première est le masque d'accès de
/// `crate::securite`), et `FILE_FLAG_FIRST_PIPE_INSTANCE` en est la troisième : le
/// service refuse de démarrer si quelqu'un tient déjà ce nom, au lieu de s'y ajouter et
/// de servir un client sur deux.
///
/// **Mesuré le 2026-09-08** sur le poste de développement : avec le descripteur de
/// `crate::securite`, un processus non élevé qui a créé la première instance se voit
/// refuser la seconde par `ERROR_ACCESS_DENIED` (5) — c'est le masque d'`INTERACTIVE`,
/// qui ne contient pas `FILE_CREATE_PIPE_INSTANCE, qui joue. Le service n'a donc pas le
/// choix : il réutilise son instance (`DisconnectNamedPipe` puis `ConnectNamedPipe`)
/// plutôt que d'en créer une par client. Ce qu'on y perd — un client servi à la fois —
/// est borné par [`DELAI_REQUETE`] ; ce qu'on y gagne est un serveur sans pool de fils
/// dans le processus le plus privilégié du produit.
fn creer_instance(descripteur: &DescripteurTube) -> Result<Poignee, ErreurTube> {
    let large: Vec<u16> = NOM_TUBE.encode_utf16().chain(core::iter::once(0)).collect();
    let attributs = descripteur.attributs();
    // Mode octet, et non message : le protocole porte sa propre longueur préfixée
    // (`protocole::encadrer`), qui survit à un client qui ouvrirait le canal dans l'autre
    // mode. `PIPE_REJECT_REMOTE_CLIENTS` ferme la porte aux connexions venues du réseau
    // — un canal nommé est accessible par SMB par défaut, et rien de ce service n'a de
    // sens à distance.
    let mode: NAMED_PIPE_MODE =
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    // SAFETY: `large` est une chaîne large terminée par NUL et `attributs` un
    // `SECURITY_ATTRIBUTES` de cette pile qui emprunte le descripteur de `descripteur` —
    // les deux vivent pendant tout l'appel, et le descripteur survit à la poignée rendue
    // (l'appelant le tient).
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(large.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            mode,
            1,
            TAMPON_TUBE,
            TAMPON_TUBE,
            0,
            Some(&raw const attributs),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
        return Err(ErreurTube::Creation {
            code: dernier_code(),
        });
    }
    Ok(Poignee(handle))
}

/// Ce qu'une attente de client a donné.
enum Attente {
    /// Un client est connecté.
    Client,
    /// L'arrêt a été demandé.
    Arret,
    /// Le système a refusé ; le code est rendu tel quel.
    Erreur(u32),
}

/// Attend un client **ou** l'arrêt, sans jamais bloquer indéfiniment sur le premier.
///
/// C'est le seul endroit du service qui attende sans borne, et il attend les deux objets
/// à la fois : l'événement du `ConnectNamedPipe` en recouvrement, et celui de l'arrêt.
/// Un service qui n'attendrait que le premier ne se laisserait arrêter qu'à la faveur
/// d'une connexion, et la console des services resterait sur « arrêt en cours » jusqu'au
/// redémarrage du poste.
fn attendre_client(instance: &Poignee, evenement: &Poignee, arret: &Arret) -> Attente {
    // SAFETY: `evenement` est un événement à réarmement manuel créé par `Evenement`.
    let _ = unsafe { ResetEvent(evenement.0) };
    let mut recouvrement = OVERLAPPED {
        hEvent: evenement.0,
        ..OVERLAPPED::default()
    };
    // SAFETY: `instance` est l'unique instance du canal, vivante pendant tout l'appel ;
    // `recouvrement` est une variable de cette pile qui vit jusqu'au `GetOverlappedResult`
    // ci-dessous — c'est-à-dire jusqu'à la fin de l'opération, ce qu'exige le contrat de
    // l'API. Son `hEvent` est l'événement, vivant lui aussi.
    match unsafe { ConnectNamedPipe(instance.0, Some(&raw mut recouvrement)) } {
        // Rare mais possible : la connexion a abouti tout de suite.
        Ok(()) => return Attente::Client,
        Err(erreur) => {
            let code = code_win32(&erreur);
            // `ERROR_PIPE_CONNECTED` (535) : le client s'est présenté entre la création
            // de l'instance et l'attente. C'est un succès, pas une panne.
            if code == ERROR_PIPE_CONNECTED.0 {
                return Attente::Client;
            }
            if code != ERROR_IO_PENDING.0 {
                return Attente::Erreur(code);
            }
        }
    }
    let objets = [evenement.0, arret.evenement.0];
    // SAFETY: les deux handles sont vivants pendant tout l'appel (l'un appartient à
    // l'appelant, l'autre à `arret`, qui lui survit). `false` : le premier signalé suffit.
    let issue = unsafe { WaitForMultipleObjects(&objets, false, INFINITE) };
    if issue == WAIT_OBJECT_0 {
        let mut transferes: u32 = 0;
        // SAFETY: `recouvrement` décrit l'opération qui vient de se terminer et vit
        // encore ; `transferes` est une variable de cette pile. `false` : l'événement est
        // déjà signalé, il n'y a plus rien à attendre.
        return match unsafe {
            GetOverlappedResult(instance.0, &raw const recouvrement, &mut transferes, false)
        } {
            Ok(()) => Attente::Client,
            Err(erreur) => Attente::Erreur(code_win32(&erreur)),
        };
    }
    // Arrêt demandé, ou attente en échec : dans les deux cas on annule l'opération en
    // cours avant que `recouvrement` ne meure — le système écrirait sinon dans une
    // structure de pile disparue.
    // SAFETY: `instance` est vivante ; `CancelIo` annule les opérations que **ce fil** a
    // lancées sur ce handle, c'est-à-dire exactement le `ConnectNamedPipe` ci-dessus.
    let _ = unsafe { CancelIo(instance.0) };
    let mut transferes: u32 = 0;
    // SAFETY: `true` attend la fin de l'annulation : c'est ce qui garantit que le système
    // ne touchera plus à `recouvrement` après ce retour.
    let _ =
        unsafe { GetOverlappedResult(instance.0, &raw const recouvrement, &mut transferes, true) };
    if arret.demande() {
        Attente::Arret
    } else {
        Attente::Erreur(issue.0)
    }
}

/// Sert le canal nommé jusqu'à ce que `arret` soit levé.
///
/// Le descripteur de sécurité est construit **une fois**, avant la création de
/// l'instance : si sa DACL ne peut pas être prouvée non nulle, le service ne démarre pas
/// du tout (`crate::securite`).
///
/// Un client à la fois, chacun borné par [`DELAI_REQUETE`] : voir `creer_instance` pour
/// la raison, qui est mesurée et non théorique.
///
/// # Erreurs
///
/// [`ErreurTube`] quand le descripteur ou le canal ne peuvent pas être créés. Un refus
/// ponctuel d'une connexion, lui, n'arrête pas le service : il est journalisé et la
/// boucle continue.
pub fn servir(journal: &Arc<Journal>, arret: &Arc<Arret>) -> Result<(), ErreurTube> {
    let descripteur = DescripteurTube::nouveau().map_err(ErreurTube::Securite)?;
    let instance = creer_instance(&descripteur)?;
    let evenement = creer_evenement()?;
    journal.info(&format!(
        "service prêt sur {NOM_TUBE} (protocole v{PROTOCOLE_VERSION}, contrat KS v{}, \
         descripteur « {} »)",
        cables::version_contrat_connue(),
        crate::securite::SDDL_TUBE
    ));

    while !arret.demande() {
        match attendre_client(&instance, &evenement, arret) {
            Attente::Client => {
                if arret.demande() {
                    break;
                }
                servir_une_connexion(&instance, &evenement, journal);
            }
            Attente::Arret => break,
            Attente::Erreur(code) => {
                journal.alerte(&format!("attente d'un client : erreur Win32 {code}"));
            }
        }
        // Quoi qu'il soit arrivé, l'instance est rendue disponible pour le client
        // suivant. C'est cette réutilisation qui remplace la création d'une instance par
        // connexion (voir `creer_instance`).
        // SAFETY: `instance` est vivante ; la déconnexion d'une instance qui n'a pas de
        // client est sans effet et sans danger.
        let _ = unsafe { DisconnectNamedPipe(instance.0) };
    }
    journal.info("arrêt demandé : le service ne prend plus de connexion");
    Ok(())
}

/// Sert **un** ordre sur l'instance connectée, puis rend la main.
fn servir_une_connexion(instance: &Poignee, evenement: &Poignee, journal: &Journal) {
    let reponse = match lire_requete(instance, evenement) {
        Ok(charge) => {
            // Le parseur pur : c'est lui, et lui seul, qui décide si la trame a la forme
            // attendue.
            match Requete::from_bytes(&charge) {
                Ok(requete) => {
                    let appelant = identifier(instance.0);
                    if requete.modifie() {
                        journal.info(&format!(
                            "ordre « {} » du câble {} demandé par {appelant}",
                            requete.label(),
                            requete.cable().map_or(0, |c| c.0)
                        ));
                    } else {
                        journal.debug(&format!(
                            "ordre « {} » demandé par {appelant}",
                            requete.label()
                        ));
                    }
                    cables::executer(requete, &appelant, journal)
                }
                Err(erreur) => {
                    let appelant = identifier(instance.0);
                    journal.alerte(&format!("trame refusée de {appelant} : {erreur}"));
                    let statut = erreur.statut();
                    // Le code d'ordre est repris **brut** des octets reçus quand il y en
                    // a un : c'est ce qui permet au client de reconnaître à quoi le refus
                    // répond, même quand l'ordre est inconnu.
                    let code = charge.get(1).copied().unwrap_or(u8::MAX);
                    if statut == Statut::VersionInconnue {
                        Reponse::refus_detaille(code, statut, u32::from(PROTOCOLE_VERSION))
                    } else {
                        Reponse::refus(code, statut)
                    }
                }
            }
        }
        Err(erreur) => {
            journal.alerte(&format!("trame illisible : {erreur}"));
            Reponse::refus(u8::MAX, Statut::TrameInvalide)
        }
    };
    if let Err(erreur) = ecrire_trame(instance, evenement, &reponse.encadrer()) {
        journal.alerte(&format!("réponse non transmise : erreur Win32 {erreur}"));
    }
    // SAFETY: `instance` est connectée et vivante ; la vidange pousse la réponse vers le
    // client avant la déconnexion, que la boucle de `servir` fait juste après.
    let _ = unsafe { FlushFileBuffers(instance.0) };
}

/// Attend la fin d'une opération en recouvrement, au plus [`DELAI_REQUETE`].
///
/// C'est la borne qui empêche un client de garder le service pour lui : sans elle, un
/// processus qui se connecterait sans jamais écrire bloquerait l'unique instance du canal
/// aussi longtemps qu'il vivrait, et personne d'autre ne pourrait plus activer un câble.
fn attendre_operation(
    tube: &Poignee,
    evenement: &Poignee,
    recouvrement: &OVERLAPPED,
) -> Result<u32, u32> {
    // SAFETY: `evenement` est vivant, et c'est celui du `recouvrement` en cours.
    let issue = unsafe { WaitForSingleObject(evenement.0, DELAI_REQUETE) };
    if issue != WAIT_OBJECT_0 {
        // SAFETY: `tube` est vivant ; `CancelIo` n'annule que les opérations lancées par
        // ce fil sur ce handle.
        let _ = unsafe { CancelIo(tube.0) };
        let mut perdus: u32 = 0;
        // SAFETY: `true` attend la fin de l'annulation, ce qui garantit que le système ne
        // touchera plus à `recouvrement` après ce retour — condition nécessaire pour que
        // la structure de pile de l'appelant puisse mourir.
        let _ = unsafe { GetOverlappedResult(tube.0, recouvrement, &mut perdus, true) };
        return Err(if issue == WAIT_TIMEOUT {
            ERROR_TIMEOUT
        } else {
            issue.0
        });
    }
    let mut transferes: u32 = 0;
    // SAFETY: l'opération est terminée (l'événement est signalé) et `recouvrement` la
    // décrit ; `transferes` est une variable de cette pile.
    unsafe { GetOverlappedResult(tube.0, recouvrement, &mut transferes, false) }
        .map_err(|e| code_win32(&e))?;
    Ok(transferes)
}

/// Lit exactement `attendu` octets, ou échoue.
///
/// Un client qui envoie moins que ce qu'il annonce n'est pas attendu indéfiniment :
/// `ReadFile` rend 0 octet quand il raccroche, et [`attendre_operation`] borne le cas où
/// il ne raccroche même pas.
fn lire_exactement(tube: &Poignee, evenement: &Poignee, attendu: usize) -> Result<Vec<u8>, u32> {
    let mut tampon = vec![0u8; attendu];
    let mut lus = 0usize;
    while lus < attendu {
        let reste = tampon.get_mut(lus..).ok_or(0_u32)?;
        // SAFETY: `evenement` est à réarmement manuel et n'appartient qu'à ce fil.
        let _ = unsafe { ResetEvent(evenement.0) };
        let mut recouvrement = OVERLAPPED {
            hEvent: evenement.0,
            ..OVERLAPPED::default()
        };
        // SAFETY: `tube` est une instance connectée, vivante pendant l'appel ; `reste`
        // est une tranche de `tampon`, vivante jusqu'au retour de la fonction, et c'est
        // sa longueur qui borne ce que le système y écrit. `recouvrement` vit jusqu'au
        // `GetOverlappedResult` d'`attendre_operation`, qui est appelé sur tous les
        // chemins avant que la structure ne meure. Le compte d'octets est `None` : le
        // contrat de l'API l'exige en recouvrement, c'est `GetOverlappedResult` qui le
        // rend.
        let lancement = unsafe { ReadFile(tube.0, Some(reste), None, Some(&raw mut recouvrement)) };
        let recus = match lancement {
            Ok(()) => attendre_operation(tube, evenement, &recouvrement)?,
            Err(erreur) if code_win32(&erreur) == ERROR_IO_PENDING.0 => {
                attendre_operation(tube, evenement, &recouvrement)?
            }
            Err(erreur) => return Err(code_win32(&erreur)),
        };
        if recus == 0 {
            // Le client a raccroché avant d'avoir tout envoyé.
            return Err(ERROR_BROKEN_PIPE.0);
        }
        lus = lus.saturating_add(recus as usize);
    }
    Ok(tampon)
}

/// Lit une trame complète : l'en-tête, sa **borne**, puis la charge.
///
/// C'est ici que la règle « la taille avant l'allocation » s'applique :
/// [`longueur_annoncee`] refuse tout ce qui dépasse `MAX_TRAME_OCTETS` avant qu'un `Vec`
/// de cette taille ne soit réservé.
fn lire_requete(tube: &Poignee, evenement: &Poignee) -> Result<Vec<u8>, String> {
    let en_tete = lire_exactement(tube, evenement, EN_TETE_OCTETS)
        .map_err(|code| format!("en-tête illisible (erreur Win32 {code})"))?;
    let en_tete: [u8; EN_TETE_OCTETS] = en_tete
        .as_slice()
        .try_into()
        .map_err(|_| "en-tête tronqué".to_owned())?;
    let longueur = longueur_annoncee(en_tete).map_err(|e| e.to_string())?;
    lire_exactement(tube, evenement, longueur)
        .map_err(|code| format!("charge illisible (erreur Win32 {code})"))
}

/// Écrit une trame entière sur le canal, en recouvrement et bornée dans le temps.
fn ecrire_trame(tube: &Poignee, evenement: &Poignee, trame: &[u8]) -> Result<(), u32> {
    let mut ecrits = 0usize;
    while ecrits < trame.len() {
        let reste = trame.get(ecrits..).ok_or(0_u32)?;
        // SAFETY: `evenement` est à réarmement manuel et n'appartient qu'à ce fil.
        let _ = unsafe { ResetEvent(evenement.0) };
        let mut recouvrement = OVERLAPPED {
            hEvent: evenement.0,
            ..OVERLAPPED::default()
        };
        // SAFETY: mêmes garanties qu'en lecture — `reste` et `recouvrement` vivent
        // jusqu'au `GetOverlappedResult`, et la longueur transmise borne ce que le système
        // lit.
        let lancement =
            unsafe { WriteFile(tube.0, Some(reste), None, Some(&raw mut recouvrement)) };
        let pousses = match lancement {
            Ok(()) => attendre_operation(tube, evenement, &recouvrement)?,
            Err(erreur) if code_win32(&erreur) == ERROR_IO_PENDING.0 => {
                attendre_operation(tube, evenement, &recouvrement)?
            }
            Err(erreur) => return Err(code_win32(&erreur)),
        };
        if pousses == 0 {
            return Err(ERROR_BROKEN_PIPE.0);
        }
        ecrits = ecrits.saturating_add(pousses as usize);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------
// Le client.
// ---------------------------------------------------------------------------------

/// Écrit une trame entière sur un handle **synchrone**.
///
/// Le pendant client d'[`ecrire_trame`] : le client n'ouvre pas son handle en
/// recouvrement, parce qu'il n'a rien à attendre en parallèle — un seul échange, puis il
/// raccroche. La borne de temps, côté client, est celle que le serveur s'applique à
/// lui-même : si le service ne répond pas, la lecture se termine par la fermeture du
/// canal.
fn ecrire_synchrone(tube: HANDLE, trame: &[u8]) -> Result<(), u32> {
    let mut ecrits = 0usize;
    while ecrits < trame.len() {
        let reste = trame.get(ecrits..).ok_or(0_u32)?;
        let mut pousses: u32 = 0;
        // SAFETY: `tube` est ouvert et vivant pendant l'appel ; `reste` est une tranche de
        // `trame`, vivante elle aussi, et sa longueur borne ce que le système lit.
        // `pousses` est une variable de cette pile. Pas de recouvrement : le handle est
        // synchrone.
        unsafe { WriteFile(tube, Some(reste), Some(&mut pousses), None) }
            .map_err(|e| code_win32(&e))?;
        if pousses == 0 {
            return Err(ERROR_BROKEN_PIPE.0);
        }
        ecrits = ecrits.saturating_add(pousses as usize);
    }
    Ok(())
}

/// Ce qui a empêché un client de se faire servir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErreurClient {
    /// `CreateFileW` a refusé d'ouvrir le canal.
    ///
    /// `ERROR_FILE_NOT_FOUND` (2) : le service n'écoute pas. `ERROR_ACCESS_DENIED` (5) :
    /// le descripteur de sécurité a refusé — le processus n'est pas dans un jeton
    /// `INTERACTIVE` (voir `crate::securite`).
    Ouverture {
        /// Le code Win32 rendu, tel quel.
        code: u32,
    },
    /// L'échange a échoué en cours de route.
    Echange {
        /// Ce qui a échoué, en français.
        cause: String,
    },
    /// La réponse n'a pas la forme attendue.
    Reponse(ErreurReponse),
}

impl core::fmt::Display for ErreurClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ouverture { code: 2 } => write!(
                f,
                "{NOM_TUBE} introuvable : le service Conduit n'est pas démarré (installez-le \
                 par « conduit-helper installer », ou lancez « conduit-helper console »)"
            ),
            Self::Ouverture { code: 5 } => write!(
                f,
                "{NOM_TUBE} : accès refusé — ce processus n'est pas dans une session \
                 interactive, seule catégorie autorisée par le descripteur du canal"
            ),
            Self::Ouverture { code } => {
                write!(f, "ouverture de {NOM_TUBE} refusée : erreur Win32 {code}")
            }
            Self::Echange { cause } => write!(f, "échange avec le service : {cause}"),
            Self::Reponse(erreur) => write!(f, "réponse du service : {erreur}"),
        }
    }
}

impl std::error::Error for ErreurClient {}

/// Ouvre le canal du service en client, avec le masque d'accès **exact** que le
/// descripteur accorde.
///
/// Ni `GENERIC_READ` ni `GENERIC_WRITE` : le second s'étendrait en un masque contenant
/// `FILE_CREATE_PIPE_INSTANCE`, que le descripteur ne donne pas à `INTERACTIVE`, et
/// l'ouverture échouerait en `ERROR_ACCESS_DENIED`. Voir `crate::securite`.
fn ouvrir_client() -> Result<Poignee, ErreurClient> {
    let large: Vec<u16> = NOM_TUBE.encode_utf16().chain(core::iter::once(0)).collect();
    // Le service ne tient qu'une instance : un second client la trouve occupée et attend
    // son tour plutôt que d'échouer. Deux tentatives suffisent — la première dit
    // « occupé », `WaitNamedPipeW` attend qu'une instance se libère, la seconde passe.
    for tentative in 0..2 {
        // SAFETY: `large` est une chaîne large terminée par NUL, vivante pendant tout
        // l'appel. Aucun partage, aucun attribut de sécurité (le canal existe déjà, sa
        // sécurité est celle que le serveur lui a posée), pas de recouvrement : les
        // échanges du client sont synchrones et courts.
        let ouverture = unsafe {
            CreateFileW(
                PCWSTR(large.as_ptr()),
                ACCES_OUVERTURE_CLIENT,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match ouverture {
            Ok(handle) => return Ok(Poignee(handle)),
            Err(erreur) => {
                let code = code_win32(&erreur);
                if code != ERROR_PIPE_BUSY.0 || tentative > 0 {
                    return Err(ErreurClient::Ouverture { code });
                }
                // SAFETY: `large` est vivante pendant l'appel. Le résultat n'est pas
                // consulté : qu'une instance se soit libérée ou que le délai ait expiré,
                // la tentative suivante tranche, et son code d'erreur est plus précis
                // qu'un booléen.
                let _ = unsafe { WaitNamedPipeW(PCWSTR(large.as_ptr()), DELAI_ATTENTE_CLIENT) };
            }
        }
    }
    Err(ErreurClient::Ouverture {
        code: ERROR_PIPE_BUSY.0,
    })
}

/// Envoie un ordre au service et rend sa réponse.
///
/// Une connexion par ordre : c'est ce que le serveur sert, et cela évite au client de
/// tenir un état qu'un redémarrage du service rendrait faux.
///
/// # Erreurs
///
/// [`ErreurClient`] : canal absent, accès refusé, échange interrompu, réponse malformée.
pub fn demander(requete: &Requete) -> Result<Reponse, ErreurClient> {
    let charge = demander_octets(&requete.encadrer())?;
    Reponse::from_bytes(&charge).map_err(ErreurClient::Reponse)
}

/// Envoie une trame **arbitraire** au service et rend la charge de sa réponse, sans
/// l'analyser.
///
/// C'est le pendant de `conduit_backend_wasapi::cable::TopologyFilter::write_raw` : on ne
/// prouve rien de la validation d'un serveur si l'on ne peut pas lui envoyer ce que le
/// protocole refuse. La fonction sert donc aux tests de bout en bout, qui vérifient
/// qu'une trame trop longue, d'une autre version ou d'un ordre inconnu reçoit le statut
/// qu'il faut — et non un silence.
///
/// # Erreurs
///
/// [`ErreurClient`] : canal absent, accès refusé, échange interrompu, réponse dont le
/// cadrage est invalide.
pub fn demander_octets(trame: &[u8]) -> Result<Vec<u8>, ErreurClient> {
    let tube = ouvrir_client()?;
    ecrire_synchrone(tube.0, trame).map_err(|code| ErreurClient::Echange {
        cause: format!("envoi de l'ordre : erreur Win32 {code}"),
    })?;
    let mut brut = vec![0u8; EN_TETE_OCTETS.saturating_add(TAILLE_REPONSE)];
    let mut lus = 0usize;
    // Le serveur répond puis raccroche : on lit jusqu'à la fermeture, en bornant par la
    // taille d'une réponse cadrée. Un serveur qui en enverrait plus ne serait pas le
    // nôtre, et le découpage refuse alors la trame.
    while let Some(reste) = brut.get_mut(lus..) {
        if reste.is_empty() {
            break;
        }
        let mut recus: u32 = 0;
        // SAFETY: `tube` est ouvert et vivant ; `reste` est une tranche de `brut`,
        // vivante elle aussi, et sa longueur borne ce que le système y écrit.
        let lecture = unsafe { ReadFile(tube.0, Some(reste), Some(&mut recus), None) };
        if lecture.is_err() || recus == 0 {
            break;
        }
        lus = lus.saturating_add(recus as usize);
    }
    let recu = brut.get(..lus).unwrap_or(&[]);
    let (charge, _) = decouper(recu)
        .map_err(|e| ErreurClient::Echange {
            cause: e.to_string(),
        })?
        .ok_or_else(|| ErreurClient::Echange {
            cause: format!("réponse tronquée : {lus} octets reçus"),
        })?;
    Ok(charge.to_vec())
}

/// Le canal du service répond-il ?
///
/// Sert au diagnostic (`conduit-helper etat`) : un service « démarré » d'après le
/// contrôleur de services mais dont le canal ne répond pas est un cas qu'il faut savoir
/// distinguer.
#[must_use]
pub fn repond() -> bool {
    demander(&Requete::Version).is_ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Les bornes du serveur : le tampon du canal dépasse largement une trame, et le
    /// délai accordé à un client dépasse largement un ordre.
    ///
    /// Les deux existent pour la même raison — qu'un client ne puisse ni faire attendre
    /// ni faire réserver quoi que ce soit d'appréciable au processus `LocalSystem` — et
    /// aucune ne sert à régler un débit.
    #[test]
    fn les_bornes_du_serveur_ont_de_la_marge() {
        // Une écriture tient toujours d'un coup dans le tampon que le noyau réserve.
        assert!(TAMPON_TUBE as usize > EN_TETE_OCTETS + crate::protocole::MAX_TRAME_OCTETS);
        // Cinq secondes contre les ~77 ms mesurées entre l'écriture et l'endpoint : deux
        // ordres de grandeur de marge.
        assert_eq!(DELAI_REQUETE, 5_000);
        const _: () = assert!(DELAI_REQUETE > 77 * 10);
        assert_eq!(DELAI_ATTENTE_CLIENT, DELAI_REQUETE);
        // `ERROR_TIMEOUT` de `winerror.h`, recopié parce que la caisse `windows` ne le
        // publie pas sous les features de ce crate.
        assert_eq!(ERROR_TIMEOUT, 1460);
    }

    /// Les messages d'erreur du client disent **quoi faire**, pas seulement qu'il y a eu
    /// une erreur.
    #[test]
    fn les_erreurs_du_client_disent_quoi_faire() {
        let absent = ErreurClient::Ouverture { code: 2 }.to_string();
        assert!(absent.contains("installer"), "{absent}");
        assert!(absent.contains("console"), "{absent}");

        let refuse = ErreurClient::Ouverture { code: 5 }.to_string();
        assert!(refuse.contains("interactive"), "{refuse}");

        let autre = ErreurClient::Ouverture { code: 231 }.to_string();
        assert!(autre.contains("231"), "{autre}");
    }

    /// Le drapeau d'arrêt se lève une fois et reste levé.
    ///
    /// `demander` tente aussi d'ouvrir le canal pour réveiller la boucle : sans service
    /// en écoute, l'ouverture échoue silencieusement, ce qui est le comportement voulu.
    /// Rien n'est installé ni créé par ce test.
    #[test]
    fn le_drapeau_d_arret_se_leve_et_reste_leve() {
        let arret = Arret::nouveau().expect("événement d'arrêt");
        assert!(!arret.demande());
        arret.demander();
        assert!(arret.demande());
        arret.demander();
        assert!(arret.demande());
    }
}
