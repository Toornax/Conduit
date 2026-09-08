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
use crate::protocole::{Reponse, Requete, Statut, CANAUX_APPLICABLES};

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
/// **Le pilote ne sait pas encore appliquer autre chose que la valeur par défaut**
/// (M1b-05) : le prédicat qui le dit est celui du contrat partagé,
/// [`CableState::channels_applicables`], et non un `== 2` recopié ici. Une valeur qu'il
/// refuserait est donc refusée **avant** l'écriture, avec un statut qui nomme la tâche
/// manquante ; l'envoyer quand même donnerait un `ERROR_INVALID_PARAMETER` qui
/// ressemblerait à un défaut du service.
///
/// Une valeur applicable, elle, part réellement dans le pilote : l'état de connexion
/// courant est relu et réécrit tel quel, seuls les canaux changent.
fn regler_canaux(
    code: u8,
    cable: CableId,
    canaux: u32,
    appelant: &Appelant,
    journal: &Journal,
) -> Reponse {
    if canaux != CANAUX_APPLICABLES {
        journal.info(&format!(
            "refus : canaux {canaux} sur le câble {} demandés par {appelant} — le pilote \
             n'applique que {CANAUX_APPLICABLES} tant que M1b-05 n'est pas faite",
            cable.0
        ));
        return Reponse::refus_detaille(code, Statut::CanauxNonApplicables, CANAUX_APPLICABLES);
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

    /// Les canaux non applicables sont refusés **sans** toucher au pilote, et la réponse
    /// dit quelle valeur passerait.
    ///
    /// Test hors machine : `regler_canaux` sort avant toute énumération dès que la
    /// valeur n'est pas applicable, ce qui est précisément la propriété vérifiée ici.
    #[test]
    fn les_canaux_non_applicables_sont_refuses_avant_toute_ecriture() {
        let journal = Journal::vers_stderr(crate::journal::Niveau::Erreur);
        let appelant = Appelant::inconnu(0);
        for canaux in [1, 3, 4, 6, 8] {
            let reponse = regler_canaux(
                crate::protocole::ORDRE_CANAUX,
                CableId(1),
                canaux,
                &appelant,
                &journal,
            );
            assert_eq!(reponse.statut, Statut::CanauxNonApplicables, "{canaux}");
            // Le détail dit la valeur que le pilote accepte, pour que le client n'ait
            // pas à la deviner.
            assert_eq!(reponse.detail, CANAUX_APPLICABLES, "{canaux}");
        }
        assert_eq!(CANAUX_APPLICABLES, 2);
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
