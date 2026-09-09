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
//! # Aucun flux audio, aucun son
//!
//! `IOCTL_KS_PROPERTY` est une requête de contrôle sur un filtre de topologie. Ce module
//! n'ouvre aucun `IAudioClient` et n'émet rien.

use conduit_backend::CableId;
use conduit_backend_wasapi::cable::{
    armer_privilege, cable_id, contract_version, driver_index, topology_interfaces, Armement,
    CableConfigError, CableState, FilterSide, TopologyFilter, CABLE_MAX,
};
use conduit_kmd_core::config::{is_active, with_active};

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
    let chemins = topology_interfaces()?;
    let mut etat = EtatCables::default();
    for index in 0..CABLE_MAX {
        let Some(numero) = cable_id(index) else {
            continue;
        };
        let Ok(filtre) = TopologyFilter::open_in(&chemins, numero, COTE) else {
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
    Ok(etat)
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
            let mut reponse = reponse_de_lecture(code, Statut::Succes, 0);
            if reponse.statut.succes() {
                reponse.detail = u32::from(crate::protocole::PROTOCOLE_VERSION);
            }
            reponse
        }
        Requete::Lister => reponse_de_lecture(code, Statut::Succes, 0),
        Requete::Activer(cable) => ecrire_connexion(code, cable, true, appelant, journal),
        Requete::Desactiver(cable) => ecrire_connexion(code, cable, false, appelant, journal),
        Requete::Canaux { cable, canaux } => regler_canaux(code, cable, canaux, appelant, journal),
        Requete::Renommer { cable, ref nom } => {
            ecrire_nom(code, cable, Some(nom), appelant, journal)
        }
        Requete::NomDefaut(cable) => ecrire_nom(code, cable, None, appelant, journal),
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
            reponse_de_lecture(code, Statut::Succes, 0)
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

/// La réponse d'un ordre qui ne modifie rien : l'état des câbles, ou le refus qui
/// explique pourquoi on n'a pas pu le lire.
fn reponse_de_lecture(code: u8, statut: Statut, canaux: u32) -> Reponse {
    match lire_etat() {
        Ok(etat) => Reponse {
            ordre: code,
            statut,
            detail: 0,
            presents: etat.presents,
            actifs: etat.actifs,
            version_ks: etat.version_ks,
            canaux,
        },
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
fn ecrire_connexion(
    code: u8,
    cable: CableId,
    connecte: bool,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    let verbe = if connecte { "activer" } else { "désactiver" };
    ecrire(code, cable, appelant, journal, verbe, |filtre, _actuel| {
        filtre.write_state(connecte)
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
/// ([`CableState::channels_appliquables`]), et cet ordre le refuse **avant** l'écriture, à
/// la valeur près : ce n'est plus un `2` universel, c'est le compte relu sur le câble visé.
///
/// L'envoyer quand même donnerait un `ERROR_INVALID_PARAMETER` du pilote, qui ressemblerait
/// à un défaut du service. Le refus, lui, porte dans `detail` le nombre réellement servi —
/// de quoi afficher « ce câble est en 6 canaux » plutôt que « paramètre invalide ».
///
/// Une valeur **égale** à celle servie part quand même dans le pilote : la requête est
/// alors sans effet, mais elle vaut confirmation, et le chemin d'écriture reste celui des
/// autres ordres (relecture comprise).
///
/// # Ce qui manque encore, et où
///
/// Changer réellement le format d'un câble demande d'écrire `CableFormat<n>` dans la clé
/// **matérielle** du périphérique puis de **redémarrer le devnode** (`cfgmgr32`, environ une
/// seconde de silence sur les seize câbles). Les deux gestes sont en espace utilisateur, et
/// c'est exactement là qu'ils doivent être — mais le chemin d'accès au périphérique
/// (énumération `cfgmgr32`, chemin d'instance) vit dans `conduit_backend_wasapi::cable`, que
/// ce service consomme sans le posséder. Tant qu'il n'expose pas ce chemin, l'ordre
/// `canaux` ne peut que constater.
fn regler_canaux(
    code: u8,
    cable: CableId,
    canaux: u32,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    // Le compte réellement servi, relu sur le câble visé. `None` : le filtre n'a pas pu
    // être lu — on laisse alors partir l'écriture, et c'est le pilote qui tranchera, ce qui
    // vaut mieux qu'un refus fondé sur une supposition.
    if let Some(servis) = canaux_du_cable(cable) {
        let etat = CableState {
            channels: canaux,
            ..CableState::new(0, false)
        };
        if !etat.channels_appliquables(servis) {
            journal.info(&format!(
                "refus : canaux {canaux} sur le câble {} demandés par {appelant} — ce câble \
                 sert {servis} canaux, et en changer demande d'écrire son format au registre \
                 puis de redémarrer le périphérique",
                cable.0
            ));
            return Reponse::refus_detaille(code, Statut::CanauxNonApplicables, servis);
        }
    }
    ecrire(
        code,
        cable,
        appelant,
        journal,
        "canaux",
        |filtre, actuel| {
            let voulu = CableState {
                channels: canaux,
                ..actuel
            };
            filtre.write_raw(&voulu.to_bytes()).map(|_| ())
        },
    )
}

/// Le nombre de canaux que sert le câble `cable`, ou `None` si son filtre est illisible.
///
/// Une ouverture de plus sur le chemin d'un ordre d'administration rare : le prix d'un
/// refus qui dit la vérité plutôt qu'une constante.
fn canaux_du_cable(cable: CableId) -> Option<u32> {
    TopologyFilter::open(cable, COTE)
        .and_then(|filtre| filtre.read_state())
        .ok()
        .map(|etat| etat.channels)
}

/// Le corps commun des trois ordres qui écrivent : énumérer, ouvrir, armer, écrire,
/// relire, journaliser.
fn ecrire<F>(
    code: u8,
    cable: CableId,
    appelant: &Appelant,
    journal: &Journal,
    verbe: &str,
    action: F,
) -> Reponse
where
    F: FnOnce(&TopologyFilter, CableState) -> Result<(), CableConfigError>,
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
    // L'état **avant**, pour que le journal dise ce qui a changé plutôt que ce qu'on a
    // demandé. Une lecture qui échoue n'empêche pas d'écrire : on repart de l'état au
    // repos du contrat.
    let avant = filtre
        .read_state()
        .unwrap_or_else(|_| CableState::new(index, false));

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

    if let Err(erreur) = action(&filtre, avant) {
        journal.erreur(&format!(
            "{verbe} câble {} par {appelant} : refusé par le pilote — {erreur}",
            cable.0
        ));
        return refus_de_transport(code, &erreur);
    }

    // Le filtre est fermé avant la relecture générale : elle rouvre les seize, et garder
    // deux handles sur le même filtre n'apporterait rien.
    drop(filtre);
    let reponse = reponse_de_lecture(code, Statut::Succes, canaux_lus(index));
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
    /// Test hors machine : `canaux_du_cable` rendrait `None` ici (aucun pilote chargé), et
    /// `regler_canaux` laisserait alors passer l'écriture. Ce qui se vérifie sans machine,
    /// c'est la règle elle-même, sur les 64 couples possibles — et c'est elle qui a changé
    /// avec M1b-05.
    #[test]
    fn le_refus_des_canaux_se_decide_contre_le_compte_du_cable() {
        use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
        for servis in MIN_CHANNELS..=MAX_CHANNELS {
            for demandes in MIN_CHANNELS..=MAX_CHANNELS {
                let etat = CableState {
                    channels: demandes,
                    ..CableState::new(0, false)
                };
                assert_eq!(
                    etat.channels_appliquables(servis),
                    demandes == servis,
                    "{demandes} demandés sur un câble à {servis}"
                );
            }
        }
        // Le défaut du protocole reste ce qu'un poste neuf sert, et rien de plus.
        assert_eq!(crate::protocole::CANAUX_PAR_DEFAUT, 2);
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
