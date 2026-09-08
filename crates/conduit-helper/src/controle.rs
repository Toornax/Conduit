//! Le `CableControl` du dorsal Windows : le démon commande, le service exécute
//! (M1b-34).
//!
//! C'est la dernière pièce de `conduitctl cable add` sous Windows. Le chemin complet :
//!
//! ```text
//! conduitctl cable add
//!   → conduitd (IPC)            crates/conduitd/src/ipc.rs
//!   → moteur, cable_op()        crates/conduit-engine/src/engine.rs
//!   → dorsal WASAPI             conduit_backend_wasapi::WasapiBackend::cable_control()
//!   → ce module                 \\.\pipe\conduit-helper
//!   → service ConduitHelper     crate::cables::executer
//!   → pilote conduit-kmd        KSPROPERTY_CONDUIT_CABLE_STATE
//! ```
//!
//! # Tout passe par le service, lectures comprises — et c'est un choix
//!
//! Le transport KS direct existe (`conduit_backend_wasapi::cable`) et une `GET` de
//! `KSPROPERTY_CONDUIT_CABLE_STATE` n'exige aucun privilège : ce module **pourrait**
//! lire l'état des câbles lui-même et ne dépendre du service que pour écrire. Il ne le
//! fait pas, pour deux raisons. D'abord une seule source de vérité et un seul journal :
//! l'état des câbles n'est constaté qu'à un endroit, celui qui les modifie, et un
//! `lister` ne peut donc jamais contredire l'`activer` qui vient de passer. Ensuite le
//! coût : une réponse du service porte les seize câbles d'un coup ([`Reponse::presents`]
//! et [`Reponse::actifs`]), là où une lecture directe ouvrirait seize filtres de
//! topologie pour la même information.
//!
//! Ce que l'on y perd est réel et assumé : sans le service, `conduitctl cable list` ne
//! liste rien. Il rend alors un [`CableError::Unavailable`] qui **nomme la commande
//! d'installation**, ce qui vaut mieux qu'une liste vide silencieuse.
//!
//! # La correspondance des verbes : rien n'est créé, rien n'est détruit
//!
//! Le pilote a une **réserve fixe** de seize câbles (SPEC §5.4) : ils existent tous ou
//! aucun, et un ordre ne fait que les connecter ou les déconnecter. D'où :
//!
//! | `CableControl` | ici | ordre du protocole |
//! |---|---|---|
//! | `create` | **active** le premier câble libre | `Requete::Activer` |
//! | `remove` | **désactive** le câble | `Requete::Desactiver` |
//! | `list` | l'état des seize | `Requete::Lister` |
//! | `set_channels` | règle les canaux | `Requete::Canaux` |
//! | `rename` | refusé : **M1b-21** | aucun |
//!
//! # `create` sur un câble déjà actif rend son état, sans erreur
//!
//! **Décision de M1b-34.** `create` veut dire « connecte-moi un câble » ; un câble déjà
//! connecté satisfait la demande, et le refuser obligerait tout appelant à enchaîner un
//! `list` défensif avant chaque `create`. `conduitd::ensure_cables` en est le cas
//! concret : il rejoue la section `[[cable]]` de la configuration à **chaque**
//! démarrage, et un refus y ferait une ligne d'avertissement par câble et par
//! lancement, pour un état pourtant conforme. L'opération est donc idempotente, et la
//! réponse décrit la machine — pas l'intention.
//!
//! # Ce que ce module ne fait pas
//!
//! Il ne renomme pas (`CableControl::rename` rend une erreur qui **nomme M1b-21**) et
//! ne change pas le nombre de canaux par lui-même : le pilote scelle `CHANNELS` dans ses
//! tables KS et refuse toute autre valeur tant que **M1b-05** n'est pas faite. Le refus
//! du service ([`Statut::CanauxNonApplicables`]) est propagé tel quel, avec le nom de la
//! tâche dans le message.
//!
//! # Aucun test de ce module ne touche la machine
//!
//! Tout ce qui **décide** — traduction d'une réponse en [`CableInfo`], d'un statut en
//! [`CableError`], choix du câble libre, lecture d'un nom `Conduit N` — est pur, hors
//! `cfg(windows)`, et vérifié en table de cas. Seul l'aller-retour sur le canal nommé
//! est propre à Windows, et il n'est exercé que par `tests/tube.rs`, `#[ignore]`.

use conduit_backend::{CableError, CableId, CableInfo, DeviceId};
use conduit_core::types::ChannelCount;
use conduit_kmd_core::config::CABLE_MAX;
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};

use crate::protocole::{Reponse, Statut, CANAUX_APPLICABLES, PROTOCOLE_VERSION};

// ---------------------------------------------------------------------------------
// Les identifiants d'endpoint de repli.
// ---------------------------------------------------------------------------------

/// Préfixe des identifiants d'endpoint **de repli** de ce module.
///
/// Un [`CableInfo`] doit nommer ses deux endpoints, et le service ne les connaît pas :
/// il parle au pilote par ses filtres de topologie, pas à MMDevice. Ce module met donc
/// un identifiant de repli, que le dorsal WASAPI remplace par le véritable identifiant
/// d'endpoint dès qu'il le trouve dans son énumération
/// (`WasapiBackend::resoudre_endpoints`). Il reste visible dans deux cas légitimes : le
/// câble est inactif — ses endpoints n'existent pas —, ou il vient d'être activé et
/// Windows ne l'a pas encore publié (77 ms mesurées).
pub const ENDPOINT_REPLI: &str = "conduit:cable";

/// Le côté d'un câble, pour l'identifiant de repli.
///
/// Les mots sont ceux de `conduit_backend_wasapi::cable::FilterSide::label`, en
/// français comme tout ce que l'utilisateur peut lire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cote {
    /// Le côté où les applications **jouent**.
    Rendu,
    /// Le côté où les applications **lisent**.
    Capture,
}

impl Cote {
    /// Les deux côtés.
    pub const ALL: [Self; 2] = [Self::Rendu, Self::Capture];

    /// Le mot qui suffixe l'identifiant de repli.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rendu => "rendu",
            Self::Capture => "capture",
        }
    }
}

/// L'identifiant de repli d'un côté du câble `cable`. Voir [`ENDPOINT_REPLI`].
#[must_use]
pub fn endpoint_repli(cable: CableId, cote: Cote) -> DeviceId {
    DeviceId::new(format!("{ENDPOINT_REPLI}{}:{}", cable.0, cote.label()))
}

// ---------------------------------------------------------------------------------
// Le décalage de un, et le nom d'un câble.
// ---------------------------------------------------------------------------------

/// Les seize numéros **affichés** de la réserve du pilote, dans l'ordre.
///
/// **Le décalage de un ne se fait pas ici.** `CableId(1)` est « Conduit 1 » et l'index
/// *pilote* 0 ; la conversion est celle de [`Reponse::est_present`] et
/// [`Reponse::est_actif`], qui la font pour nous, et celle de
/// `conduit_backend_wasapi::cable::driver_index`, seul endroit du dépôt qui la connaisse
/// vraiment. Ce module ne manipule donc que des numéros affichés, et le test
/// `l_aller_retour_sur_les_seize` le vérifie sur les seize.
pub fn numeros() -> impl Iterator<Item = CableId> {
    (1..=CABLE_MAX).map(CableId)
}

/// Le nom OS d'un câble : `Conduit N`, celui que le pilote publie.
///
/// C'est le seul nom qu'un câble puisse porter aujourd'hui — le renommage est **M1b-21**
/// et passe par le registre. Rendu par [`CableId`]'s `Display`, non recopié.
#[must_use]
pub fn nom(cable: CableId) -> String {
    cable.to_string()
}

/// Le câble que désigne le nom `Conduit N`, ou `None`.
///
/// # Le piège du préfixe, encore
///
/// `nom.starts_with("Conduit 1")` retiendrait « Conduit 10 » à « Conduit 16 » — le
/// défaut corrigé côté endpoints (`conduit_backend_wasapi::cable_id_from_name`) et côté
/// chaînes de référence (`matches_reference`). On exige donc le préfixe **exact**, des
/// chiffres et rien d'autre, pas de zéro de tête, et un numéro dans la réserve. Le test
/// `nos_noms_sont_ceux_du_dorsal` vérifie sous Windows que les deux lectures coïncident.
#[must_use]
pub fn cable_nomme(nom: &str) -> Option<CableId> {
    let chiffres = nom.strip_prefix("Conduit ")?;
    if chiffres.is_empty()
        || !chiffres.bytes().all(|b| b.is_ascii_digit())
        || (chiffres.len() > 1 && chiffres.starts_with('0'))
    {
        return None;
    }
    let numero: u32 = chiffres.parse().ok()?;
    (1..=CABLE_MAX).contains(&numero).then_some(CableId(numero))
}

// ---------------------------------------------------------------------------------
// D'une réponse à un CableInfo.
// ---------------------------------------------------------------------------------

/// Le nombre de canaux que le pilote sert, quand la réponse ne le dit pas.
///
/// [`Reponse::canaux`] vaut 0 pour un ordre qui ne vise aucun câble — un `lister`, par
/// exemple. On retombe alors sur la valeur du contrat, qui est aujourd'hui la **seule**
/// que le pilote applique ([`CANAUX_APPLICABLES`], M1b-05).
const CANAUX_DEFAUT: ChannelCount = match ChannelCount::new(CANAUX_APPLICABLES as u8) {
    Some(canaux) => canaux,
    // Injoignable : le contrat borne `CANAUX_APPLICABLES` à `MIN_CHANNELS..=MAX_CHANNELS`,
    // et `ChannelCount` accepte 1 à 8. Un repli plutôt qu'un `unwrap` : ce fichier ne
    // contient aucune panique atteignable.
    None => ChannelCount::STEREO,
};

// Le repli ci-dessus n'est jamais pris tant que les deux bornes coïncident.
const _: () = assert!(MIN_CHANNELS <= CANAUX_APPLICABLES && CANAUX_APPLICABLES <= MAX_CHANNELS);
const _: () = assert!(CANAUX_APPLICABLES <= ChannelCount::MAX as u32);

/// Le nombre de canaux qu'annonce une réponse, ou `CANAUX_DEFAUT` si elle n'en annonce
/// pas (0) ou en annonce un que `ChannelCount` refuse.
#[must_use]
pub fn canaux(brut: u32) -> ChannelCount {
    u8::try_from(brut)
        .ok()
        .and_then(ChannelCount::new)
        .unwrap_or(CANAUX_DEFAUT)
}

/// Le [`CableInfo`] du câble `cable`, tel que `reponse` le décrit.
///
/// Les deux endpoints portent l'identifiant de repli : voir [`ENDPOINT_REPLI`].
#[must_use]
pub fn info(reponse: &Reponse, cable: CableId) -> CableInfo {
    CableInfo {
        id: cable,
        name: nom(cable),
        channels: canaux(reponse.canaux),
        active: reponse.est_actif(cable),
        render: endpoint_repli(cable, Cote::Rendu),
        capture: endpoint_repli(cable, Cote::Capture),
    }
}

/// Les câbles que le pilote **expose** sur cette machine, actifs ou non, dans l'ordre.
///
/// Un câble absent du masque [`Reponse::presents`] n'est pas listé : il n'est pas dans
/// la réserve que ce pilote a enregistrée (paramètre `Reserve` du registre, M1b-01), et
/// prétendre le contraire ferait espérer une activation qui échouerait.
#[must_use]
pub fn liste(reponse: &Reponse) -> Vec<CableInfo> {
    numeros()
        .filter(|cable| reponse.est_present(*cable))
        // Un `lister` ne vise aucun câble : ses canaux sont ceux du contrat, pas ceux
        // que la réponse porte pour un autre ordre.
        .map(|cable| CableInfo {
            channels: CANAUX_DEFAUT,
            ..info(reponse, cable)
        })
        .collect()
}

/// Le premier câble présent et **non connecté**, celui qu'un `create` sans nom active.
///
/// Le plus petit numéro d'abord : c'est l'ordre que l'utilisateur voit dans les réglages
/// Son de Windows, et celui qui rend « Conduit 1 » avant « Conduit 2 » sur une machine
/// neuve.
#[must_use]
pub fn premier_libre(reponse: &Reponse) -> Option<CableId> {
    numeros().find(|cable| reponse.est_present(*cable) && !reponse.est_actif(*cable))
}

// ---------------------------------------------------------------------------------
// D'un statut à une CableError.
// ---------------------------------------------------------------------------------

/// La réponse du service dit-elle que l'ordre a abouti ?
///
/// `cable` est le câble visé, quand il y en a un : il ne sert qu'à nommer le bon
/// [`CableError::NotFound`].
///
/// # Erreurs
///
/// La [`CableError`] qui correspond au statut, avec un message qui dit **quoi faire** :
/// installer le service, réinstaller la même version, faire la tâche qui manque, ou
/// rapporter le code Win32 tel quel.
pub fn verifier(reponse: &Reponse, cable: Option<CableId>) -> Result<(), CableError> {
    let detail = reponse.detail;
    Err(match reponse.statut {
        Statut::Succes => return Ok(()),
        Statut::VersionInconnue => CableError::Unsupported(format!(
            "le service d'assistance sert le protocole version {detail}, ce démon la \
             {PROTOCOLE_VERSION} : les deux viennent de la même version de Conduit, \
             réinstallez-les ensemble"
        )),
        // Les deux disent la même chose : le démon a émis une trame que le service n'a
        // pas comprise. Ce n'est pas la faute de l'utilisateur et aucune manœuvre ne le
        // corrige — d'où un message qui demande un rapport de bogue plutôt qu'une action.
        Statut::TrameInvalide | Statut::OrdreInconnu => CableError::Driver(format!(
            "le service d'assistance a refusé la trame du démon ({}) : c'est un défaut de \
             Conduit, joignez « conduitctl dump » à un rapport de bogue",
            reponse.statut
        )),
        Statut::CableInconnu => CableError::NotFound(cable.unwrap_or(CableId(0))),
        Statut::CanauxInvalides => CableError::Unsupported(format!(
            "nombre de canaux hors des bornes du pilote ({MIN_CHANNELS} à {MAX_CHANNELS})"
        )),
        // Le refus que M1b-05 lèvera. Le message nomme la tâche : c'est ce qui distingue
        // « pas encore fait » de « ne marchera jamais ».
        Statut::CanauxNonApplicables => CableError::Unsupported(format!(
            "le pilote n'applique que {detail} canaux : le nombre de canaux est scellé \
             dans ses tables KS tant que M1b-05 n'est pas faite"
        )),
        Statut::PiloteAbsent => CableError::Driver(
            "le pilote Conduit n'expose aucun filtre de topologie pour ce câble : il n'est \
             pas chargé, ou ce câble est hors de la réserve (paramètre « Reserve » du \
             registre)"
                .to_owned(),
        ),
        Statut::PrivilegeAbsent => CableError::PermissionDenied(
            "le service d'assistance n'a pas pu armer SeLoadDriverPrivilege : il ne tourne \
             pas en LocalSystem — réinstallez-le par « conduit-helper installer »"
                .to_owned(),
        ),
        Statut::ErreurSysteme => CableError::Driver(format!(
            "le pilote a refusé l'ordre : erreur Win32 {detail}"
        )),
    })
}

// ---------------------------------------------------------------------------------
// Le contrôle proprement dit.
// ---------------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use super::{
        cable_nomme, info, liste, nom, premier_libre, verifier, CableError, CableId, CableInfo,
        ChannelCount, CABLE_MAX, CANAUX_APPLICABLES,
    };
    use crate::protocole::{Reponse, Requete, NOM_TUBE};
    use crate::tube::{demander, ErreurClient};
    use conduit_backend::cable::{validate_cable_name, MAX_CABLE_NAME_LEN};
    use conduit_backend::{CableControl, CableSpec};

    /// Le [`CableControl`] du dorsal Windows : chaque appel est un ordre au service.
    ///
    /// Sans état : le service est la source de vérité, et un cache ne survivrait pas à
    /// son redémarrage ni à un `conduit-helper activer` lancé à la main. Une opération
    /// coûte une connexion au canal nommé, ce que le service sert en quelques
    /// millisecondes.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct ControleCables;

    impl ControleCables {
        /// Un contrôle prêt à servir. N'ouvre rien : c'est le premier ordre qui parle au
        /// service.
        #[must_use]
        pub const fn nouveau() -> Self {
            Self
        }

        /// Le service répond-il ?
        ///
        /// À interroger **au démarrage du démon** pour journaliser une absence, plutôt
        /// que de la laisser surgir au premier `cable add`.
        #[must_use]
        pub fn service_repond() -> bool {
            crate::tube::repond()
        }

        /// Envoie un ordre et vérifie son statut.
        fn ordre(&self, requete: Requete) -> Result<Reponse, CableError> {
            let cible = requete.cable();
            let reponse = demander(requete).map_err(|erreur| erreur_client(&erreur))?;
            verifier(&reponse, cible)?;
            Ok(reponse)
        }

        /// L'état des seize câbles, sans rien modifier.
        fn etat(&self) -> Result<Reponse, CableError> {
            self.ordre(Requete::Lister)
        }
    }

    /// Traduit un échec du **canal** — pas du pilote — en [`CableError`].
    ///
    /// Le service absent est le cas courant, et le seul que l'utilisateur puisse
    /// corriger : le message porte donc la commande d'installation, reprise du message
    /// du client plutôt que réécrite.
    fn erreur_client(erreur: &ErreurClient) -> CableError {
        match erreur {
            ErreurClient::Ouverture { code: 5 } => CableError::PermissionDenied(erreur.to_string()),
            ErreurClient::Ouverture { .. } => CableError::Unavailable(erreur.to_string()),
            // Le canal s'est ouvert puis l'échange a échoué, ou la réponse n'avait pas la
            // forme attendue : le service est là mais ne se comporte pas comme le nôtre.
            ErreurClient::Echange { .. } | ErreurClient::Reponse(_) => {
                CableError::Unavailable(format!("{NOM_TUBE} : {erreur}"))
            }
        }
    }

    /// Le refus de renommer, qui **nomme la tâche** plutôt que de paniquer.
    fn renommage_absent(cable: CableId) -> CableError {
        CableError::Unsupported(format!(
            "renommer « {} » demande d'écrire le nom d'endpoint dans le registre, ce que \
             M1b-21 n'a pas encore fait : le câble garde le nom que le pilote publie",
            nom(cable)
        ))
    }

    impl CableControl for ControleCables {
        /// La réserve du pilote : seize câbles, fixée à la compilation
        /// (`conduit_kmd_core::config::CABLE_MAX`).
        ///
        /// C'est la borne du **format**, pas ce que cette machine expose : le paramètre
        /// `Reserve` du registre peut en enregistrer moins, et `list` ne rend alors que
        /// ceux-là.
        fn max_cables(&self) -> usize {
            CABLE_MAX as usize
        }

        fn list(&self) -> Result<Vec<CableInfo>, CableError> {
            Ok(liste(&self.etat()?))
        }

        /// **Active** un câble : rien n'est créé, la réserve est fixe (SPEC §5.4).
        ///
        /// Sans nom, le premier câble libre est pris. Avec un nom `Conduit N`, c'est ce
        /// câble-là — et s'il est **déjà actif**, son état est rendu tel quel, sans
        /// erreur (voir l'en-tête de module). Tout autre nom est refusé en nommant
        /// M1b-21 : le pilote publie `Conduit N` et rien d'autre ne peut être appliqué
        /// aujourd'hui.
        ///
        /// Un nombre de canaux différent de celui que le pilote sert est refusé **avant**
        /// d'activer quoi que ce soit : activer puis échouer sur les canaux laisserait
        /// derrière un câble que l'appelant n'a pas demandé.
        fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError> {
            if u32::from(spec.channels.get()) != CANAUX_APPLICABLES {
                return Err(CableError::Unsupported(format!(
                    "le pilote n'applique que {CANAUX_APPLICABLES} canaux : le nombre de \
                     canaux est scellé dans ses tables KS tant que M1b-05 n'est pas faite"
                )));
            }
            let vise = match spec.name.as_deref() {
                None => None,
                Some(voulu) => {
                    validate_cable_name(voulu)?;
                    match cable_nomme(voulu) {
                        Some(cable) => Some(cable),
                        None => {
                            return Err(CableError::Unsupported(format!(
                                "« {} » : sous Windows un câble s'appelle « Conduit N » \
                                 (1 à {CABLE_MAX}) tant que M1b-21 n'a pas rendu le \
                                 renommage possible",
                                voulu.chars().take(MAX_CABLE_NAME_LEN).collect::<String>()
                            )))
                        }
                    }
                }
            };
            let etat = self.etat()?;
            let cible = match vise {
                Some(cable) if etat.est_present(cable) => cable,
                Some(cable) => return Err(CableError::NotFound(cable)),
                None => match premier_libre(&etat) {
                    Some(cable) => cable,
                    None if etat.presents == 0 => {
                        return Err(CableError::Driver(
                            "le pilote Conduit n'expose aucun câble sur cette machine : \
                             est-il installé ?"
                                .to_owned(),
                        ))
                    }
                    None => {
                        return Err(CableError::LimitReached {
                            max: etat.presents.count_ones() as usize,
                        })
                    }
                },
            };
            // Déjà connecté : la demande est satisfaite, on rend l'état constaté.
            if etat.est_actif(cible) {
                return Ok(info(&etat, cible));
            }
            let reponse = self.ordre(Requete::Activer(cible))?;
            Ok(info(&reponse, cible))
        }

        /// **Désactive** le câble : ses deux endpoints se rangent sous « Périphériques
        /// déconnectés », rien n'est détruit.
        ///
        /// Un câble déjà déconnecté rend `Ok` sans écrire — symétrique de `create`, et
        /// une écriture privilégiée de moins pour un état déjà atteint. Un numéro hors
        /// de la réserve, lui, est un [`CableError::NotFound`].
        fn remove(&mut self, id: CableId) -> Result<(), CableError> {
            let etat = self.etat()?;
            if !etat.est_present(id) {
                return Err(CableError::NotFound(id));
            }
            if !etat.est_actif(id) {
                return Ok(());
            }
            self.ordre(Requete::Desactiver(id))?;
            Ok(())
        }

        /// Règle le nombre de canaux — que le pilote **refuse** aujourd'hui dès qu'il
        /// n'est pas celui de ses tables KS (M1b-05).
        ///
        /// L'ordre part quand même : c'est le service qui tranche et son refus est
        /// propagé tel quel, avec le nom de la tâche dans le message. Décider ici à sa
        /// place ferait deux politiques à tenir d'accord, et la nôtre deviendrait fausse
        /// le jour où M1b-05 rendra la valeur dynamique.
        fn set_channels(
            &mut self,
            id: CableId,
            channels: ChannelCount,
        ) -> Result<CableInfo, CableError> {
            let etat = self.etat()?;
            if !etat.est_present(id) {
                return Err(CableError::NotFound(id));
            }
            let reponse = self.ordre(Requete::Canaux {
                cable: id,
                canaux: u32::from(channels.get()),
            })?;
            Ok(info(&reponse, id))
        }

        /// **Non fait** : le renommage d'un endpoint Windows passe par le registre et
        /// c'est **M1b-21**.
        ///
        /// Une erreur qui nomme la tâche, jamais un `unimplemented!()` : c'est un chemin
        /// que l'utilisateur emprunte (`conduitctl cable rename`) et le dépôt interdit
        /// les paniques atteignables. Le nom est tout de même validé d'abord, pour qu'un
        /// nom impossible soit refusé pour la bonne raison.
        fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError> {
            validate_cable_name(name)?;
            Err(renommage_absent(id))
        }
    }
}

#[cfg(windows)]
pub use windows::ControleCables;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::protocole::{ORDRE_ACTIVER, ORDRE_LISTER};
    use conduit_kmd_core::config::ACTIVE_CABLES_MASK;

    /// Une réponse de succès à un `lister`, avec les masques donnés.
    fn reponse(presents: u32, actifs: u32) -> Reponse {
        Reponse {
            ordre: ORDRE_LISTER,
            statut: Statut::Succes,
            detail: 0,
            presents,
            actifs,
            version_ks: 1,
            canaux: 0,
        }
    }

    /// **Le décalage de un, sur les seize.**
    ///
    /// « Conduit 1 » est l'index pilote 0 : le bit `n-1` du masque décrit le câble
    /// `CableId(n)`. On active un seul bit à la fois et l'on vérifie que c'est
    /// exactement le bon câble qui ressort — un décalage dans un sens ou dans l'autre
    /// ferait tomber quinze cas sur seize.
    #[test]
    fn l_aller_retour_sur_les_seize() {
        for numero in 1..=CABLE_MAX {
            let cable = CableId(numero);
            let bit = 1_u32 << (numero - 1);
            let r = reponse(ACTIVE_CABLES_MASK, bit);

            let cables = liste(&r);
            assert_eq!(cables.len(), CABLE_MAX as usize, "câble {numero}");
            for (rang, decrit) in cables.iter().enumerate() {
                let attendu = CableId(rang as u32 + 1);
                assert_eq!(decrit.id, attendu, "rang {rang}");
                assert_eq!(decrit.name, format!("Conduit {}", attendu.0));
                assert_eq!(
                    decrit.active,
                    attendu == cable,
                    "câble {numero} : {attendu} mal classé"
                );
            }
            // Et le chemin direct dit la même chose que la liste.
            assert!(info(&r, cable).active, "câble {numero}");
            assert!(r.est_present(cable) && r.est_actif(cable));
        }
    }

    /// Le premier libre est le plus petit numéro présent et non connecté, et la limite
    /// n'est atteinte que quand ils sont tous connectés.
    #[test]
    fn le_premier_libre_est_le_plus_petit() {
        // Rien d'actif : le premier libre est « Conduit 1 ».
        assert_eq!(
            premier_libre(&reponse(ACTIVE_CABLES_MASK, 0)),
            Some(CableId(1))
        );
        // Les trois premiers pris : le quatrième.
        assert_eq!(
            premier_libre(&reponse(ACTIVE_CABLES_MASK, 0b111)),
            Some(CableId(4))
        );
        // Tous pris : plus rien.
        assert_eq!(
            premier_libre(&reponse(ACTIVE_CABLES_MASK, ACTIVE_CABLES_MASK)),
            None
        );
        // Une réserve réduite à deux câbles : le second, puis plus rien.
        assert_eq!(premier_libre(&reponse(0b11, 0b01)), Some(CableId(2)));
        assert_eq!(premier_libre(&reponse(0b11, 0b11)), None);
        // Un câble absent de la réserve n'est jamais proposé, même inactif.
        assert_eq!(premier_libre(&reponse(0b1000, 0)), Some(CableId(4)));
    }

    /// Seuls les câbles **présents** sont listés : une réserve réduite ne fait pas
    /// espérer seize câbles.
    #[test]
    fn seuls_les_cables_presents_sont_listes() {
        let cables = liste(&reponse(0b101, 0b100));
        assert_eq!(
            cables.iter().map(|c| c.id.0).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(!cables[0].active);
        assert!(cables[1].active);
        assert!(liste(&reponse(0, 0)).is_empty());
    }

    /// Le nom d'un câble se relit, et le piège du préfixe ne se referme pas.
    #[test]
    fn le_nom_d_un_cable_se_relit() {
        for numero in 1..=CABLE_MAX {
            let cable = CableId(numero);
            assert_eq!(nom(cable), format!("Conduit {numero}"));
            assert_eq!(cable_nomme(&nom(cable)), Some(cable));
        }
        // Hors de la réserve, mal formés, ou d'un autre produit.
        for refuse in [
            "Conduit 0",
            "Conduit 17",
            "Conduit 01",
            "Conduit ",
            "Conduit 1x",
            "Conduit 1 (Conduit — câbles audio virtuels)",
            "conduit 1",
            "Conduit1",
            "Haut-parleurs",
            "",
        ] {
            assert_eq!(cable_nomme(refuse), None, "{refuse}");
        }
    }

    /// Les identifiants de repli nomment le câble et le côté, et ne se confondent pas.
    #[test]
    fn les_endpoints_de_repli_se_distinguent() {
        let rendu = endpoint_repli(CableId(1), Cote::Rendu);
        let capture = endpoint_repli(CableId(1), Cote::Capture);
        assert_ne!(rendu, capture);
        assert_eq!(rendu.as_str(), "conduit:cable1:rendu");
        assert_eq!(capture.as_str(), "conduit:cable1:capture");
        // Le piège du préfixe vaut aussi ici : « cable1 » n'est pas « cable16 ».
        assert_ne!(
            endpoint_repli(CableId(1), Cote::Rendu),
            endpoint_repli(CableId(16), Cote::Rendu)
        );
        for cote in Cote::ALL {
            assert!(endpoint_repli(CableId(9), cote)
                .as_str()
                .starts_with(ENDPOINT_REPLI));
        }
        let decrit = info(&reponse(ACTIVE_CABLES_MASK, 0), CableId(2));
        assert_eq!(decrit.render, endpoint_repli(CableId(2), Cote::Rendu));
        assert_eq!(decrit.capture, endpoint_repli(CableId(2), Cote::Capture));
    }

    /// Chaque statut de refus donne l'erreur qui dit **quoi faire**, et le succès n'en
    /// donne aucune.
    ///
    /// Table de cas exhaustive : [`Statut::ALL`] garantit qu'un statut ajouté plus tard
    /// tombera ici plutôt que sur un repli muet.
    #[test]
    fn chaque_statut_dit_quoi_faire() {
        for statut in Statut::ALL {
            let mut r = reponse(ACTIVE_CABLES_MASK, 0);
            r.ordre = ORDRE_ACTIVER;
            r.statut = statut;
            r.detail = 1314;
            let verdict = verifier(&r, Some(CableId(3)));
            if statut.succes() {
                assert!(verdict.is_ok(), "{statut:?}");
                continue;
            }
            let erreur = verdict.expect_err(&format!("{statut:?}"));
            let texte = erreur.to_string();
            assert!(!texte.is_empty(), "{statut:?}");
            match statut {
                Statut::CableInconnu => {
                    assert_eq!(erreur, CableError::NotFound(CableId(3)));
                }
                Statut::PrivilegeAbsent => {
                    assert!(matches!(erreur, CableError::PermissionDenied(_)));
                    assert!(texte.contains("conduit-helper installer"), "{texte}");
                }
                Statut::CanauxNonApplicables => {
                    // Le refus **nomme la tâche** : c'est ce qui le distingue d'un
                    // « ça ne marchera jamais ».
                    assert!(texte.contains("M1b-05"), "{texte}");
                }
                Statut::VersionInconnue => {
                    assert!(texte.contains("réinstallez"), "{texte}");
                }
                Statut::ErreurSysteme => {
                    // Le code du système est rendu tel quel, jamais traduit.
                    assert!(texte.contains("1314"), "{texte}");
                }
                _ => {}
            }
        }
    }

    /// Sans câble visé, un `CableInconnu` reste une erreur nommée — jamais une panique.
    #[test]
    fn un_cable_inconnu_sans_cible_ne_panique_pas() {
        let mut r = reponse(0, 0);
        r.statut = Statut::CableInconnu;
        assert_eq!(
            verifier(&r, None),
            Err(CableError::NotFound(CableId(0))),
            "un numéro nul plutôt qu'une panique"
        );
    }

    /// Les canaux d'une réponse sont ceux qu'elle porte, et le contrat sert de repli.
    #[test]
    fn les_canaux_viennent_de_la_reponse_ou_du_contrat() {
        assert_eq!(canaux(0), CANAUX_DEFAUT);
        assert_eq!(canaux(2), ChannelCount::STEREO);
        assert_eq!(canaux(1), ChannelCount::MONO);
        assert_eq!(canaux(8).get(), 8);
        // Hors bornes : le repli du contrat, pas une panique.
        assert_eq!(canaux(9), CANAUX_DEFAUT);
        assert_eq!(canaux(u32::MAX), CANAUX_DEFAUT);
        assert_eq!(CANAUX_DEFAUT.get() as u32, CANAUX_APPLICABLES);
        // Un `lister` ne vise aucun câble : ses canaux sont ceux du contrat.
        let mut r = reponse(0b1, 0b1);
        r.canaux = 0;
        assert_eq!(liste(&r)[0].channels, CANAUX_DEFAUT);
    }

    /// Notre lecture d'un nom `Conduit N` est **celle du dorsal**, pas une seconde.
    ///
    /// Deux analyseurs du même nom finiraient par diverger ; ce test les tient d'accord
    /// sur les cas qui comptent, dont le piège du préfixe qui a déjà mordu une fois.
    /// Sous Windows seulement : `conduit_backend_wasapi` n'y compile rien ailleurs.
    #[cfg(windows)]
    #[test]
    fn nos_noms_sont_ceux_du_dorsal() {
        for numero in 1..=CABLE_MAX {
            let nom = format!("Conduit {numero}");
            assert_eq!(
                cable_nomme(&nom),
                conduit_backend_wasapi::cable_id_from_name(&nom),
                "{nom}"
            );
        }
        for cas in ["Conduit 0", "Conduit 01", "Conduit 1x", "Conduit1", ""] {
            assert_eq!(
                cable_nomme(cas),
                conduit_backend_wasapi::cable_id_from_name(cas),
                "{cas}"
            );
        }
    }
}
