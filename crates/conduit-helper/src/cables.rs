//! L'exécution d'un ordre : du [`Requete`] validé au jeu de propriétés KS du pilote.
//!
//! **Rien du transport n'est réécrit ici.** L'énumération des interfaces
//! `KSCATEGORY_TOPOLOGY`, l'appariement exact de la chaîne de référence, l'en-tête
//! `KSPROPERTY`, `IOCTL_KS_PROPERTY`, l'armement de `SeLoadDriverPrivilege` et sa
//! restauration par garde vivent dans `conduit_backend_wasapi::cable` (M1b-04), écrits et
//! **vérifiés en machine virtuelle**. Ce module ne fait que choisir les requêtes,
//! rassembler l'état des seize câbles et le mettre dans une [`Reponse`].
//!
//! # Pourquoi ce service existe — une raison mesurée
//!
//! Le gestionnaire de propriété du pilote exige
//! `SeSinglePrivilegeCheck(SE_LOAD_DRIVER_PRIVILEGE)` pour **toute** écriture. Mesuré en
//! machine virtuelle le 2026-09-08 : une écriture depuis une session non élevée est
//! refusée par `ERROR_PRIVILEGE_NOT_HELD` (1314) ; le privilège est présent mais
//! **désactivé** dans tout jeton neuf, y compris celui de `LocalSystem`, et doit être
//! armé par `AdjustTokenPrivileges` ; une fois armé, l'écriture passe et l'endpoint
//! apparaît en **77 ms**. Le démon tourne dans la session de l'utilisateur, sans
//! privilèges : il ne peut donc pas écrire lui-même, et c'est tout l'objet de ce
//! service.
//!
//! # Le privilège est armé au dernier moment, et rendu tout de suite
//!
//! [`executer`] n'arme le privilège que pour les ordres qui **modifient**
//! ([`Requete::modifie`]), et le garde qu'il tient meurt à la fin de la fonction, ce qui
//! restaure le jeton dans l'état où il a été trouvé. Un privilège armé en permanence
//! dans un processus `LocalSystem` qui écoute un canal nommé serait une surface offerte
//! pour rien.
//!
//! # Un seul côté ouvert, et c'est le rendu
//!
//! Les deux côtés d'un câble (`TopoRender<n>` et `TopoCapture<n>`) portent la **même**
//! propriété et le même état : le câble est une entité, pas un côté. On ouvre donc le
//! rendu, et on lit et écrit là. La vérification que les deux côtés coïncident appartient
//! à l'outil de mesure (`conduit-looptest --cable-cote`), pas au service.
//!
//! # Deux ordres ne descendent pas jusqu'au pilote (M1b-21)
//!
//! `renommer` et `nom par défaut` écrivent le nom de l'endpoint dans
//! `HKLM\...\MMDevices\Audio`, ce que `LocalSystem` fait de plein droit : ils n'ouvrent
//! aucun filtre de topologie, n'arment **pas** `SeLoadDriverPrivilege`, et leur travail
//! est entièrement dans [`crate::registre`]. C'est l'application du principe qui commande
//! la conception : ce qui peut être fait hors du noyau y est fait, et le pilote ne connaît
//! toujours que la connexion et les canaux.
//!
//! # Tout ordre qui écrit est une lecture-modification-écriture, et un refus le précède
//!
//! Deux champs de [`CableState`] sont des **échos vérifiés** par le pilote : `cable` et,
//! depuis M1b-05, `channels`. Un état fabriqué de toutes pièces porte donc les canaux d'un
//! câble neuf, et se fait refuser dès que le câble visé est configuré autrement — c'est le
//! défaut mesuré entre M1b-05 et M1b-20, un `activer` sur un câble en six canaux soldé par
//! un `ERROR_INVALID_PARAMETER` (87) que rien ne reliait au format.
//!
//! [`ecrire`] relit donc l'état **avant** toute écriture — il le faisait déjà pour le
//! journal — et les trois ordres qui parlent au pilote ne font que le modifier
//! ([`CableState::avec_connexion`] pour la connexion, le champ `channels` pour `canaux`).
//! Le même endroit porte le seul refus que le service prononce lui-même : si l'état à
//! écrire n'est pas applicable au format relu, il rend [`Statut::CanauxNonApplicables`]
//! avec le compte servi dans `detail`, **sans** armer le privilège ni écrire. Laisser
//! partir la requête donnerait le code 87 du pilote, qui ne dit pas quelle valeur était
//! attendue.
//!
//! # Aucun flux audio, aucun son
//!
//! `IOCTL_KS_PROPERTY` est une requête de contrôle sur un filtre de topologie. Ce module
//! n'ouvre aucun `IAudioClient` et n'émet rien.

use conduit_backend::CableId;
use conduit_backend_wasapi::cable::{
    armer_privilege, cable_id, contract_version, devnode_instance, driver_index,
    topology_interfaces, Armement, CableConfigError, CableState, FilterSide, TopologyFilter,
    CABLE_MAX,
};
use conduit_kmd_core::config::{is_active, with_active, CableFormat};

use crate::devnode::ErreurDevnode;
use crate::journal::{Appelant, Journal};
use crate::protocole::{Reponse, Requete, Statut};

/// Le côté ouvert par le service. Voir l'en-tête de module.
const COTE: FilterSide = FilterSide::Render;

/// L'état des seize câbles, tel qu'une énumération vient de le trouver.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EtatCables {
    /// Masque des câbles dont le filtre de topologie existe.
    pub presents: u32,
    /// Masque des câbles connectés.
    pub actifs: u32,
    /// Version du contrat KS servie par le pilote, 0 si aucun câble n'a pu être lu.
    pub version_ks: u32,
    /// Encodage du format des seize câbles, index **pilote**, 0 quand il est inconnu
    /// (M1b-05).
    ///
    /// # La source est le registre, pas `CableState`
    ///
    /// [`CableState`] ne porte que les **canaux** : la fréquence et la profondeur préférée
    /// n'existent que dans `CableFormat<n>`, dans la clé matérielle du devnode. Lire le
    /// registre est donc la seule façon de rendre un format entier.
    ///
    /// Les deux peuvent diverger — un câble dont le format vient d'être écrit sert encore
    /// l'ancien jusqu'au redémarrage du devnode. **C'est un renseignement, pas une
    /// panne** : l'écart se journalise, et rien ici ne tente de le corriger. Le corriger
    /// reviendrait à cacher ce qu'on est venu montrer.
    pub formats: [u32; CABLE_MAX as usize],
}

/// Lit l'état de tous les câbles : lesquels existent, lesquels sont connectés.
///
/// Une **seule** énumération du gestionnaire de configuration pour les seize câbles,
/// puis une ouverture par câble présent. Un câble dont le filtre existe mais dont la
/// lecture échoue est compté présent et non actif : c'est le verdict prudent, celui qui
/// ne fait pas croire à une connexion qu'on n'a pas constatée.
///
/// La version du contrat est celle du **premier** câble lu. Le pilote sert la même
/// partout — c'est une constante de sa compilation — et interroger les seize
/// n'apprendrait rien de plus.
///
/// # Erreurs
///
/// [`CableConfigError::Enumeration`] si le gestionnaire de configuration refuse : c'est
/// le seul cas où l'on ne sait rien dire du tout.
pub fn lire_etat() -> Result<EtatCables, CableConfigError> {
    Ok(lire_etat_dans(&topology_interfaces()?))
}

/// La même lecture, sur une énumération **déjà faite**.
///
/// Exposée séparément pour que [`regler_format`] n'énumère pas deux fois : il a besoin des
/// chemins pour trouver le devnode, et de l'état pour savoir si le câble est connecté. Une
/// seconde traversée du gestionnaire de configuration rendrait la même réponse, et rien ne
/// garantit qu'elle la rendrait au même instant — deux vues de la machine pour un seul
/// ordre, c'est un écart qui finit par se voir.
///
/// Ne rend pas de `Result` : une fois les chemins obtenus, plus rien ne peut échouer au
/// point de ne rien savoir dire. Un câble illisible est simplement absent des masques.
#[must_use]
pub fn lire_etat_dans(chemins: &[String]) -> EtatCables {
    let mut etat = EtatCables::default();
    for index in 0..CABLE_MAX {
        let Some(numero) = cable_id(index) else {
            continue;
        };
        let Ok(filtre) = TopologyFilter::open_in(chemins, numero, COTE) else {
            // Absent de la machine, ou refus d'ouverture : dans les deux cas ce câble
            // n'est pas pilotable, et le masque `presents` le dit.
            continue;
        };
        etat.presents = with_active(etat.presents, index, true);
        if etat.version_ks == 0 {
            if let Ok(version) = filtre.read_version() {
                etat.version_ks = version;
            }
        }
        if let Ok(lu) = filtre.read_state() {
            etat.actifs = with_active(etat.actifs, index, lu.is_connected());
        }
    }
    // Les seize formats, en **une seule** ouverture de la clé matérielle. Un échec n'est
    // pas une panne de la lecture d'état : les masques et la version sont là, la table
    // reste à zéro, et le protocole sait dire « format inconnu ». Refuser de lister parce
    // qu'on n'a pas pu lire un registre serait perdre plus qu'on ne gagne.
    if let Ok(instance) = devnode_instance(chemins) {
        if let Ok(formats) = crate::devnode::lire_formats(&instance) {
            etat.formats = formats;
        }
    }
    etat
}

/// Exécute un ordre **déjà validé** par le parseur du protocole et rend la réponse à
/// envoyer.
///
/// L'ordre des étapes est le même pour tous les ordres modifiants : énumérer, vérifier
/// que le câble existe, armer le privilège, écrire, relire. La relecture n'est pas
/// décorative — c'est elle qui fait que la réponse décrit la machine et non l'intention.
///
/// `appelant` ne sert qu'au journal : le contrôle d'accès a déjà été fait par le
/// descripteur de sécurité du canal (`crate::securite`), et refaire ici un jugement sur
/// l'identité donnerait deux politiques à tenir d'accord.
#[must_use]
pub fn executer(requete: Requete, appelant: &Appelant, journal: &Journal) -> Reponse {
    let code = requete.code();
    match requete {
        Requete::Version => {
            // La version du **protocole** dans `detail`, celle du contrat KS dans
            // `version_ks` : un client qui interroge la version veut les deux, et il ne
            // doit pas avoir à déduire la première du simple fait qu'on lui a répondu.
            let mut reponse = reponse_de_lecture(code, Statut::Succes, 0, None);
            if reponse.statut.succes() {
                reponse.detail = u32::from(crate::protocole::PROTOCOLE_VERSION);
            }
            reponse
        }
        Requete::Lister => reponse_de_lecture(code, Statut::Succes, 0, None),
        Requete::Activer(cable) => ecrire_connexion(code, cable, true, appelant, journal),
        Requete::Desactiver(cable) => ecrire_connexion(code, cable, false, appelant, journal),
        Requete::Canaux { cable, canaux } => regler_canaux(code, cable, canaux, appelant, journal),
        Requete::Renommer { cable, ref nom } => {
            ecrire_nom(code, cable, Some(nom), appelant, journal)
        }
        Requete::NomDefaut(cable) => ecrire_nom(code, cable, None, appelant, journal),
        Requete::Format { cable, format } => regler_format(code, cable, format, appelant, journal),
    }
}

/// Écrit le nom d'un câble dans le registre, ou le lui rend (M1b-21).
///
/// **Ne touche pas au pilote et n'arme aucun privilège** : le nom d'un endpoint vit dans
/// `HKLM\...\MMDevices\Audio`, et y écrire est un droit de `LocalSystem`. Tout le travail
/// est dans [`crate::registre`] ; ce qui suit ne fait que traduire son issue en
/// [`Reponse`] et journaliser, avec l'identité de l'appelant comme tout ordre qui
/// modifie l'état.
///
/// Le nom a déjà été validé **deux fois** avant d'arriver ici : par le démon
/// (`controle::ControleCables::rename`) et par le parseur du protocole, qui appelle l'un
/// et l'autre `conduit_backend::cable::validate_cable_name`. Le service ne rejuge donc
/// pas la règle — il l'aurait fait avec le même verdict.
fn ecrire_nom(
    code: u8,
    cable: CableId,
    voulu: Option<&str>,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    let verbe = match voulu {
        Some(_) => "renommer",
        None => "rendre son nom d'origine à",
    };
    match crate::registre::appliquer(cable, voulu) {
        Ok(cotes) => {
            journal.info(&format!(
                "{verbe} le câble {} par {appelant} : « {} » écrit sur {cotes} côté(s)",
                cable.0,
                voulu.unwrap_or(&crate::registre::nom_d_origine(cable))
            ));
            // L'état des câbles est relu comme après toute écriture : la réponse décrit
            // la machine, pas l'intention. Le renommage ne change ni les masques ni les
            // canaux, mais un client qui vient de renommer doit pouvoir constater que le
            // câble est toujours là.
            reponse_de_lecture(code, Statut::Succes, 0, Some(cable))
        }
        Err(erreur) => {
            journal.erreur(&format!(
                "{verbe} le câble {} par {appelant} : {erreur}",
                cable.0
            ));
            Reponse::refus_detaille(code, statut_registre(&erreur), erreur.detail())
        }
    }
}

/// Le statut qui correspond à un refus du registre.
///
/// Un endpoint absent est le cas **bénin et courant** — le câble est déconnecté, Windows
/// n'a rien publié — et il a son propre statut pour que le message dise « activez-le
/// d'abord » plutôt que « refus du système ». Tout le reste est un code Win32 rendu tel
/// quel.
fn statut_registre(erreur: &crate::registre::ErreurRegistre) -> Statut {
    match erreur {
        crate::registre::ErreurRegistre::Aucun { .. } => Statut::EndpointAbsent,
        _ => Statut::ErreurSysteme,
    }
}

/// Change le **format** d'un câble : écriture dans la clé matérielle, puis redémarrage du
/// devnode (M1b-05).
///
/// C'est ce que [`Statut::CanauxNonApplicables`] annonçait depuis M1b-05 et que personne
/// ne savait demander. La forme est celle de [`ecrire_nom`] — traduire l'issue d'un
/// module de registre en [`Reponse`] et journaliser — avec deux choses en plus.
///
/// # Le refus qui précède tout : un câble **connecté** ne change pas de format
///
/// Le format du moteur d'un endpoint est mis en cache à sa **création** (mesuré en M1b-05,
/// tableau de la ROADMAP) : redémarrer le devnode d'un câble actif republierait ses
/// endpoints sans déplacer leur format, et l'utilisateur constaterait un silence d'une
/// seconde pour rien. On refuse donc **avant** toute écriture, avant tout privilège armé,
/// et le message dit la séquence : désactiver, régler, réactiver.
///
/// # Le privilège est armé, alors que rien ne passe par le pilote
///
/// `CM_Query_And_Remove_SubTreeW` exige `SeLoadDriverPrivilege` armé — la même règle que
/// le gestionnaire de propriété KS, pour un chemin qui n'a rien à voir avec lui. C'est ce
/// qui fait que [`Requete::touche_le_pilote`] ne peut plus se lire comme « faut-il armer
/// le privilège ? » : il rend `false` pour cet ordre-ci, et le privilège est armé quand
/// même. Le garde meurt à la sortie de la fonction, comme dans [`ecrire`].
fn regler_format(
    code: u8,
    cable: CableId,
    format: CableFormat,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    let chemins = match topology_interfaces() {
        Ok(chemins) => chemins,
        Err(erreur) => {
            journal.erreur(&format!(
                "format câble {} par {appelant} : énumération impossible — {erreur}",
                cable.0
            ));
            return refus_de_transport(code, &erreur);
        }
    };
    // L'état des seize, sur **ces** chemins-là : une seule énumération pour tout l'ordre,
    // donc une seule vue de la machine.
    let etat = lire_etat_dans(&chemins);
    let Some(index) = driver_index(cable) else {
        // Ne peut pas arriver : le parseur du protocole a déjà borné le numéro.
        return Reponse::refus(code, Statut::CableInconnu);
    };
    if !is_active(etat.presents, index) {
        journal.info(&format!(
            "format câble {} par {appelant} : refus — ce câble n'est pas enregistré par le \
             pilote sur cette machine",
            cable.0
        ));
        return Reponse::refus(code, Statut::PiloteAbsent);
    }
    if is_active(etat.actifs, index) {
        journal.info(&format!(
            "format câble {} par {appelant} : refus — le câble est connecté, {}",
            cable.0,
            Statut::CableActif
        ));
        return Reponse::refus(code, Statut::CableActif);
    }

    // Le devnode qui porte les seize câbles, depuis les mêmes chemins.
    let instance = match devnode_instance(&chemins) {
        Ok(instance) => instance,
        Err(erreur) => {
            journal.erreur(&format!(
                "format câble {} par {appelant} : devnode introuvable — {erreur}",
                cable.0
            ));
            return refus_de_transport(code, &erreur);
        }
    };

    let (issue, _garde) = match armer_privilege() {
        Ok(couple) => couple,
        Err(erreur) => {
            journal.erreur(&format!(
                "format câble {} par {appelant} : {erreur}",
                cable.0
            ));
            return refus_de_transport(code, &erreur);
        }
    };
    if !issue.arme() {
        let statut = match issue {
            Armement::Absent => Statut::PrivilegeAbsent,
            Armement::NonActivable { .. } | Armement::Arme => Statut::ErreurSysteme,
        };
        let detail = match issue {
            Armement::NonActivable { code } => code,
            Armement::Absent | Armement::Arme => 0,
        };
        journal.erreur(&format!(
            "format câble {} par {appelant} : {issue}",
            cable.0
        ));
        return Reponse::refus_detaille(code, statut, detail);
    }

    match crate::devnode::appliquer(cable, format, &instance) {
        Ok(fait) => {
            journal.info(&format!(
                "format câble {} par {appelant} : {:#010x} → {:#010x} ({} Hz, {:?}, {} canaux) ; \
                 devnode « {instance} » retiré en {} ms et rétabli en {} ms",
                cable.0,
                fait.ancien,
                fait.nouveau,
                format.sample_rate,
                format.depth,
                format.channels,
                fait.retrait.as_millis(),
                fait.retablissement.as_millis()
            ));
            reponse_de_lecture(
                code,
                Statut::Succes,
                u32::from(format.channels),
                Some(cable),
            )
        }
        Err(erreur) => {
            journal.erreur(&format!(
                "format câble {} par {appelant} : {erreur}",
                cable.0
            ));
            Reponse::refus_detaille(code, statut_devnode(&erreur), erreur.detail())
        }
    }
}

/// Le statut qui correspond à un refus du devnode.
///
/// Sur le modèle de [`statut_registre`], et avec le même principe : le cas qui appelle une
/// conduite particulière a son propre statut, plutôt qu'un « refus du système » qui
/// laisserait chercher.
///
/// **Le partage n'est pas prononcé ici** : c'est [`ErreurDevnode::ecrite`] qui dit de quel
/// côté de l'écriture l'échec est tombé, à l'endroit où les variantes sont définies. Deux
/// listes à tenir d'accord finiraient par diverger, et l'écart se solderait par un statut
/// « rien n'a changé » sur une valeur écrite — exactement le contresens que ces deux
/// statuts existent pour éviter.
fn statut_devnode(erreur: &ErreurDevnode) -> Statut {
    if erreur.ecrite() {
        Statut::RedemarrageEchoue
    } else {
        Statut::FormatNonEcrit
    }
}

/// La réponse d'un ordre qui ne modifie rien : l'état des câbles, ou le refus qui
/// explique pourquoi on n'a pas pu le lire.
///
/// `vise` est le câble sur lequel l'ordre portait, quand il y en a un : c'est lui dont le
/// format part dans le champ de tête ([`Reponse::format`]). La **table** des seize,
/// elle, part toujours — c'est [`crate::protocole::Reponse::to_bytes`] qui décide de
/// l'émettre ou non, et il ne l'émet que pour `lister`.
fn reponse_de_lecture(code: u8, statut: Statut, canaux: u32, vise: Option<CableId>) -> Reponse {
    match lire_etat() {
        Ok(etat) => {
            let mut reponse = Reponse {
                ordre: code,
                statut,
                detail: 0,
                presents: etat.presents,
                actifs: etat.actifs,
                version_ks: etat.version_ks,
                canaux,
                format: 0,
                formats: etat.formats,
            };
            reponse.format = vise.map_or(0, |cable| reponse.format_de(cable));
            reponse
        }
        Err(erreur) => refus_de_transport(code, &erreur),
    }
}

/// Traduit un refus du transport en réponse, en gardant le code du système **tel quel**.
///
/// Le code brut est le seul renseignement qui permette de distinguer « le pilote n'est
/// pas là » de « le pilote a refusé » : le traduire en un message générique reviendrait à
/// jeter ce qu'on est venu chercher.
fn refus_de_transport(code: u8, erreur: &CableConfigError) -> Reponse {
    let (statut, detail) = match erreur {
        CableConfigError::Numero(_) => (Statut::CableInconnu, 0),
        CableConfigError::FiltreAbsent { .. } => (Statut::PiloteAbsent, 0),
        CableConfigError::Enumeration { configret, .. } => (Statut::ErreurSysteme, *configret),
        CableConfigError::Ouverture { erreur, .. }
        | CableConfigError::Requete { erreur, .. }
        | CableConfigError::Privilege { erreur, .. } => {
            (Statut::ErreurSysteme, erreur.win32().unwrap_or(0))
        }
        CableConfigError::Reponse { .. } => (Statut::ErreurSysteme, 0),
    };
    Reponse::refus_detaille(code, statut, detail)
}

/// Connecte ou déconnecte un câble, puis relit l'état de tous les câbles.
///
/// **Ne fabrique pas l'état qu'il écrit** : il reprend celui que [`ecrire`] vient de relire
/// et n'en change que la connexion. Les canaux partent donc tels que le câble les sert, et
/// non tels qu'un câble neuf les servirait — voir l'en-tête de module.
fn ecrire_connexion(
    code: u8,
    cable: CableId,
    connecte: bool,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    let verbe = if connecte { "activer" } else { "désactiver" };
    ecrire(code, cable, appelant, journal, verbe, |actuel| {
        actuel.avec_connexion(connecte)
    })
}

/// Règle le nombre de canaux d'un câble.
///
/// # Ce qui a changé avec M1b-05, et ce qui n'a pas changé
///
/// Le pilote sert désormais **1 à 8 canaux**, celui que `CableFormat<n>` fixe pour ce
/// câble-là. Ce qu'il ne sait toujours pas faire, c'est **en changer à chaud** : ses tables
/// KS sont immuables et PortCls en retient les pointeurs pour toute la vie du filtre. Le
/// gestionnaire de propriété refuse donc toute valeur différente de celle que le câble sert
/// ([`CableState::channels_appliquables`]).
///
/// Cet ordre n'a plus rien de particulier pour autant : c'est [`ecrire`] qui prononce le
/// refus, avant l'écriture et contre le compte **relu sur le câble visé**, et il le fait
/// pour les trois ordres qui parlent au pilote. Ce module ne fait ici que dire quel champ
/// changer. La relecture qui servait à ce contrôle est celle de [`ecrire`] : le câble n'est
/// plus ouvert deux fois.
///
/// Une valeur **égale** à celle servie part quand même dans le pilote : la requête est
/// alors sans effet, mais elle vaut confirmation, et le chemin d'écriture reste celui des
/// autres ordres (relecture comprise).
///
/// # Ce qui manquait est fait, et cet ordre ne le fera pas pour autant
///
/// Changer réellement le format d'un câble demande d'écrire `CableFormat<n>` dans la clé
/// **matérielle** du périphérique puis de **redémarrer le devnode** — environ une seconde
/// de silence sur les seize câbles. C'est ce que fait [`regler_format`] depuis M1b-05 (lot
/// A1), par [`crate::devnode`] et sur le chemin d'instance que
/// `conduit_backend_wasapi::cable::devnode_instance` expose désormais.
///
/// Cet ordre-ci ne s'en trouve pas changé, et ce n'est pas un oubli. `canaux` dit « écris
/// cette valeur dans le pilote » et rend [`Statut::CanauxNonApplicables`] quand le câble en
/// sert une autre ; il ne peut pas se mettre à couper le son des seize câbles au motif que
/// quelqu'un a tapé un nombre. Le refus renvoie donc vers l'ordre qui le fait, et le dit :
/// `conduitctl cable set-format`.
fn regler_canaux(
    code: u8,
    cable: CableId,
    canaux: u32,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    ecrire(code, cable, appelant, journal, "canaux", |actuel| {
        CableState {
            channels: canaux,
            ..actuel
        }
    })
}

/// Le compte de canaux à annoncer quand l'état qu'on s'apprête à écrire n'est **pas**
/// applicable au câble, `None` quand il l'est.
///
/// Pure, et c'est tout le jugement que le service porte lui-même sur une écriture : le
/// reste est du transport. `lu` est l'état que le câble vient de rendre, `None` si la
/// relecture a échoué — on laisse alors partir l'écriture, et c'est le pilote qui
/// tranchera, ce qui vaut mieux qu'un refus fondé sur une supposition.
fn refus_de_canaux(lu: Option<CableState>, voulu: &CableState) -> Option<u32> {
    let servis = lu?.channels;
    (!voulu.channels_appliquables(servis)).then_some(servis)
}

/// Le corps commun des trois ordres qui écrivent : énumérer, ouvrir, **relire**, décider,
/// armer, écrire, relire, journaliser.
///
/// `voulu` ne fait que **modifier l'état relu** : il ne fabrique rien, parce que `cable` et
/// `channels` sont des échos que le pilote compare (voir l'en-tête de module). Entre les
/// deux, ce module prononce son unique jugement, [`refus_de_canaux`] — le seul refus qui
/// n'a pas besoin du privilège, et le seul qui sache dire **quelle valeur était attendue**.
fn ecrire<F>(
    code: u8,
    cable: CableId,
    appelant: &Appelant,
    journal: &Journal,
    verbe: &str,
    voulu: F,
) -> Reponse
where
    F: FnOnce(CableState) -> CableState,
{
    let Some(index) = driver_index(cable) else {
        // Ne peut pas arriver : le parseur du protocole a déjà borné le numéro. Le
        // chemin existe pour que ce module n'ait pas à faire confiance à un autre.
        return Reponse::refus(code, Statut::CableInconnu);
    };
    let chemins = match topology_interfaces() {
        Ok(chemins) => chemins,
        Err(erreur) => {
            journal.erreur(&format!(
                "{verbe} câble {} par {appelant} : énumération impossible — {erreur}",
                cable.0
            ));
            return refus_de_transport(code, &erreur);
        }
    };
    let filtre = match TopologyFilter::open_in(&chemins, cable, COTE) {
        Ok(filtre) => filtre,
        Err(erreur) => {
            journal.erreur(&format!(
                "{verbe} câble {} par {appelant} : filtre inaccessible — {erreur}",
                cable.0
            ));
            return refus_de_transport(code, &erreur);
        }
    };
    // L'état **avant** : la base de ce qu'on va écrire, et ce qui permet au journal de dire
    // ce qui a changé plutôt que ce qu'on a demandé. Une lecture qui échoue n'empêche pas
    // d'écrire : on repart de l'état au repos du contrat, faute de mieux — et le jugement
    // ci-dessous s'abstient alors, `lu` valant `None`.
    let lu = filtre.read_state().ok();
    let avant = lu.unwrap_or_else(|| CableState::new(index, false));
    let etat = voulu(avant);

    // Le seul refus que ce service prononce lui-même, **avant** d'armer quoi que ce soit :
    // le pilote rendrait `ERROR_INVALID_PARAMETER` (87), qui ne dit pas quelle valeur il
    // attendait ; `detail` porte le compte servi, et le client l'affiche.
    if let Some(servis) = refus_de_canaux(lu, &etat) {
        journal.info(&format!(
            "{verbe} câble {} par {appelant} : refus — {} canaux dans la requête, ce câble en \
             sert {servis} ; en changer demande d'écrire son format au registre puis de \
             redémarrer le périphérique",
            cable.0, etat.channels
        ));
        return Reponse::refus_detaille(code, Statut::CanauxNonApplicables, servis);
    }

    // Le privilège est armé **ici**, pour cette écriture, et le garde meurt à la sortie
    // de la fonction : le jeton du service retrouve alors l'état où il était.
    let (issue, _garde) = match armer_privilege() {
        Ok(couple) => couple,
        Err(erreur) => {
            journal.erreur(&format!(
                "{verbe} câble {} par {appelant} : {erreur}",
                cable.0
            ));
            return refus_de_transport(code, &erreur);
        }
    };
    if !issue.arme() {
        let statut = match issue {
            Armement::Absent => Statut::PrivilegeAbsent,
            Armement::NonActivable { .. } | Armement::Arme => Statut::ErreurSysteme,
        };
        let detail = match issue {
            Armement::NonActivable { code } => code,
            Armement::Absent | Armement::Arme => 0,
        };
        journal.erreur(&format!(
            "{verbe} câble {} par {appelant} : {issue}",
            cable.0
        ));
        return Reponse::refus_detaille(code, statut, detail);
    }

    if let Err(erreur) = filtre.write_raw(&etat.to_bytes()) {
        journal.erreur(&format!(
            "{verbe} câble {} par {appelant} : refusé par le pilote — {erreur}",
            cable.0
        ));
        return refus_de_transport(code, &erreur);
    }

    // Le filtre est fermé avant la relecture générale : elle rouvre les seize, et garder
    // deux handles sur le même filtre n'apporterait rien.
    drop(filtre);
    let reponse = reponse_de_lecture(code, Statut::Succes, canaux_lus(index), Some(cable));
    journal.info(&format!(
        "{verbe} câble {} par {appelant} : {} (câble {} avant, {} après ; actifs {:#06x})",
        cable.0,
        reponse.statut,
        etiquette(avant.is_connected()),
        etiquette(is_active(reponse.actifs, index)),
        reponse.actifs
    ));
    reponse
}

/// Le nombre de canaux du câble d'index `index`, ou 0 si la lecture échoue.
///
/// Une relecture séparée plutôt qu'une valeur retenue de l'écriture : c'est ce que la
/// machine dit, pas ce qu'on a demandé.
fn canaux_lus(index: u32) -> u32 {
    let Some(numero) = cable_id(index) else {
        return 0;
    };
    TopologyFilter::open(numero, COTE)
        .and_then(|filtre| filtre.read_state())
        .map_or(0, |etat| etat.channels)
}

/// « connecté » ou « déconnecté », pour le journal.
const fn etiquette(connecte: bool) -> &'static str {
    if connecte {
        "connecté"
    } else {
        "déconnecté"
    }
}

/// La version du contrat KS que **ce service** connaît, à comparer à celle du pilote.
///
/// Ré-exportée du transport, elle-même tenue de `conduit-kmd-core` : une seule source.
#[must_use]
pub fn version_contrat_connue() -> u32 {
    contract_version()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Chaque refus du transport donne le statut qui dit au client quoi faire, et garde
    /// le code du système tel quel.
    ///
    /// Test **hors machine** : les erreurs sont construites à la main, rien n'est ouvert.
    #[test]
    fn chaque_refus_de_transport_a_son_statut() {
        use conduit_backend_wasapi::cable::OsError;

        let cas: [(CableConfigError, Statut, u32); 6] = [
            (
                CableConfigError::Numero(CableId(99)),
                Statut::CableInconnu,
                0,
            ),
            (
                CableConfigError::FiltreAbsent {
                    reference: "TopoRender0".to_owned(),
                },
                Statut::PiloteAbsent,
                0,
            ),
            (
                CableConfigError::Enumeration {
                    appel: "CM_Get_Device_Interface_ListW",
                    configret: 26,
                },
                Statut::ErreurSysteme,
                26,
            ),
            (
                CableConfigError::Ouverture {
                    chemin: "\\\\?\\x".to_owned(),
                    erreur: OsError::Win32(5),
                },
                Statut::ErreurSysteme,
                5,
            ),
            (
                CableConfigError::Requete {
                    propriete: "KSPROPERTY_CONDUIT_CABLE_STATE",
                    verbe: "SET",
                    // Le refus **mesuré** quand le privilège n'est pas armé : c'est la
                    // raison d'être du service, et il doit se lire tel quel.
                    erreur: OsError::Win32(1314),
                },
                Statut::ErreurSysteme,
                1314,
            ),
            (
                CableConfigError::Reponse {
                    cause: "3 octets".to_owned(),
                },
                Statut::ErreurSysteme,
                0,
            ),
        ];
        for (erreur, statut, detail) in cas {
            let reponse = refus_de_transport(crate::protocole::ORDRE_ACTIVER, &erreur);
            assert_eq!(reponse.statut, statut, "{erreur:?}");
            assert_eq!(reponse.detail, detail, "{erreur:?}");
            assert_eq!(reponse.ordre, crate::protocole::ORDRE_ACTIVER);
            // Un refus ne prétend jamais connaître l'état des câbles.
            assert_eq!(reponse.presents, 0);
            assert_eq!(reponse.actifs, 0);
        }
    }

    /// Le prédicat de refus est celui du contrat, contre le compte **du câble** — plus
    /// contre une constante.
    ///
    /// Test hors machine : c'est la règle elle-même qui se vérifie, sur les 64 couples
    /// possibles, et c'est elle qui a changé avec M1b-05.
    #[test]
    fn le_refus_des_canaux_se_decide_contre_le_compte_du_cable() {
        use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
        for servis in MIN_CHANNELS..=MAX_CHANNELS {
            let lu = CableState {
                channels: servis,
                ..CableState::new(0, false)
            };
            for demandes in MIN_CHANNELS..=MAX_CHANNELS {
                let etat = CableState {
                    channels: demandes,
                    ..lu
                };
                assert_eq!(
                    etat.channels_appliquables(servis),
                    demandes == servis,
                    "{demandes} demandés sur un câble à {servis}"
                );
                // Et le jugement du service dit la même chose, en portant le compte servi.
                assert_eq!(
                    refus_de_canaux(Some(lu), &etat),
                    (demandes != servis).then_some(servis),
                    "{demandes} demandés sur un câble à {servis}"
                );
            }
        }
        // Sans relecture, aucun jugement : on laisse le pilote trancher.
        let fabrique = CableState::new(0, true);
        assert_eq!(refus_de_canaux(None, &fabrique), None);
        // Le défaut du protocole reste ce qu'un poste neuf sert, et rien de plus.
        assert_eq!(crate::protocole::CANAUX_PAR_DEFAUT, 2);
    }

    /// **Le défaut de frontière de M1b-05, du côté du service.**
    ///
    /// Mesuré en machine virtuelle : `conduit-helper activer 3` sur un câble en
    /// `0x00060303` (96 kHz, six canaux) rendait « refus du système, code 87 », là où le
    /// câble 4 en `0x00020302` réussissait — seule la configuration du câble changeait.
    ///
    /// Ce que ce test fixe, sans machine : l'état que l'ordre `activer` s'apprête à écrire
    /// est celui que le câble vient de rendre, la connexion changée, et il est donc
    /// applicable **quel que soit** le format. Avant la correction, `ecrire_connexion`
    /// passait par `write_state`, qui fabriquait `CableState::new` — deux canaux, toujours.
    #[test]
    fn activer_ecrit_les_canaux_du_cable_et_non_deux() {
        use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};

        for servis in MIN_CHANNELS..=MAX_CHANNELS {
            // Ce qu'un `GET` rend sur le câble « Conduit 3 » configuré pour `servis` canaux.
            let lu = CableState {
                channels: servis,
                ..CableState::new(2, false)
            };
            for connecte in [true, false] {
                let etat = lu.avec_connexion(connecte);
                assert_eq!(etat.channels, servis, "{servis} canaux");
                assert_eq!(etat.cable, lu.cable, "{servis} canaux");
                assert_eq!(etat.is_connected(), connecte, "{servis} canaux");
                assert_eq!(
                    refus_de_canaux(Some(lu), &etat),
                    None,
                    "brancher le jack d'un câble à {servis} canaux doit passer"
                );
            }

            // L'état fabriqué de toutes pièces, celui d'avant : refusé partout sauf en
            // stéréo, et c'est le 87 mesuré.
            let fabrique = CableState::new(2, true);
            assert_eq!(
                refus_de_canaux(Some(lu), &fabrique),
                (servis != crate::protocole::CANAUX_PAR_DEFAUT).then_some(servis),
                "état fabriqué sur un câble à {servis} canaux"
            );
        }
    }

    /// Le refus des canaux **dit quelle valeur était attendue**, et il la dit par le
    /// statut prévu pour cela — pas par un code Win32 brut.
    #[test]
    fn le_refus_des_canaux_porte_le_compte_servi() {
        let lu = CableState {
            channels: 6,
            ..CableState::new(2, false)
        };
        let fabrique = CableState::new(2, true);
        let servis = refus_de_canaux(Some(lu), &fabrique).expect("six canaux, requête à deux");
        let reponse = Reponse::refus_detaille(
            crate::protocole::ORDRE_ACTIVER,
            Statut::CanauxNonApplicables,
            servis,
        );
        assert_eq!(reponse.detail, 6);
        // Ce que l'utilisateur lit : le compte servi, pas « refus du système, code 87 ».
        let texte =
            crate::rapport::rendre(&crate::protocole::Requete::Activer(CableId(3)), &reponse);
        assert!(texte.contains("6 canaux"), "{texte}");
        assert!(!texte.contains("87"), "{texte}");
    }

    /// Le côté ouvert est le rendu, et l'étiquette du journal suit l'état.
    #[test]
    fn les_constantes_du_module() {
        assert_eq!(COTE, FilterSide::Render);
        assert_eq!(etiquette(true), "connecté");
        assert_eq!(etiquette(false), "déconnecté");
        // La version connue vient du contrat partagé, pas d'un 1 recopié.
        assert_eq!(
            version_contrat_connue(),
            conduit_kmd_core::config::CONFIG_VERSION
        );
    }
}
