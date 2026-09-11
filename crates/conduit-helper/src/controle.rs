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
//! | `set_format` | écrit `CableFormat<n>` et redémarre le devnode | `Requete::Format` |
//! | `rename` | écrit le nom d'endpoint dans le registre | `Requete::Renommer` |
//! | `rename` vers `Conduit N` | **efface** le nom personnalisé | `Requete::NomDefaut` |
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
//! # Renommer ne descend pas jusqu'au pilote (M1b-21)
//!
//! `rename` n'écrit rien dans le pilote : le nom d'un endpoint audio vit dans
//! `HKLM\...\MMDevices\Audio`, et c'est le service qui l'y met. Le pilote continue de
//! publier `Conduit N`, et c'est très bien — c'est la description d'origine, celle qui
//! sert de pont entre un numéro de câble et ses clés de registre. Tout ce qui établit
//! *quelle* valeur, *comment* on retrouve un câble déjà renommé et *comment* on revient
//! en arrière est dans l'en-tête de [`crate::registre`].
//!
//! # `set_channels` et `set_format` : deux verbes, un seul chemin qui aboutit
//!
//! Depuis **M1b-05** le pilote sert 1 à 8 canaux par câble, mais il ne peut pas en changer
//! à chaud : ses tables KS sont immuables et PortCls en retient les pointeurs pour toute
//! la vie du filtre. [`CableControl::set_channels`] envoie donc `Requete::Canaux`, que le
//! service refuse ([`Statut::CanauxNonApplicables`]) dès que le compte demandé n'est pas
//! celui du format configuré — refus propagé tel quel, avec le compte réellement servi
//! dans le message et le nom de la commande qui aboutit, elle.
//!
//! Celle qui aboutit, c'est [`CableControl::set_format`] : écrire `CableFormat<n>` dans la
//! clé matérielle du périphérique puis redémarrer le devnode, ce que le service sait faire
//! depuis l'ordre 7 du protocole. Elle coûte **environ une seconde de silence sur les
//! seize câbles** et exige un câble déconnecté ; les deux sont dits à l'utilisateur avant
//! qu'il la tape, dans l'aide de `conduitctl cable set-format`.
//!
//! # Aucun test de ce module ne touche la machine
//!
//! Tout ce qui **décide** — traduction d'une réponse en [`CableInfo`], d'un statut en
//! [`CableError`], choix du câble libre, lecture d'un nom `Conduit N`, choix de l'ordre
//! d'un renommage ([`ordre_de_renommage`]) — est pur, hors `cfg(windows)`, et vérifié en
//! table de cas. Seul l'aller-retour sur le canal nommé est propre à Windows, et il n'est
//! exercé que par `tests/tube.rs`, `#[ignore]`.

use conduit_backend::cable::validate_cable_name;
use conduit_backend::{CableError, CableFormat, CableId, CableInfo, DeviceId, SampleDepth};
use conduit_core::types::{ChannelCount, SampleRate};
use conduit_kmd_core::config::{CableFormat as FormatPilote, CABLE_MAX};
use conduit_kmd_core::format::SAMPLE_RATES;
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
use conduit_kmd_core::ring::SampleFormat;

use crate::protocole::{Reponse, Requete, Statut, CANAUX_PAR_DEFAUT, PROTOCOLE_VERSION};

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

/// Le nom **d'origine** d'un câble : `Conduit N`, celui que le pilote publie.
///
/// Ce n'est plus forcément le nom que l'utilisateur voit — M1b-21 permet d'en écrire un
/// autre dans le registre —, mais c'est celui auquel on revient, et le pont entre un
/// numéro de câble et ses clés MMDevices. Rendu par le `Display` de [`CableId`], non
/// recopié.
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
// La politique du renommage (M1b-21).
// ---------------------------------------------------------------------------------

/// L'ordre qu'un `rename` doit envoyer au service, ou la raison de le refuser.
///
/// **Pure**, donc vérifiable sans machine : c'est ici qu'est toute la politique de
/// M1b-21 côté démon, et `CableControl::rename` ne fait qu'appliquer ce qu'elle rend.
///
/// Trois cas, et un seul est un refus :
///
/// | `voulu` | ordre | pourquoi |
/// |---|---|---|
/// | un nom libre (`Musique`) | [`Requete::Renommer`] | le cas ordinaire |
/// | son propre nom d'origine (`Conduit 3` pour le câble 3) | [`Requete::NomDefaut`] | c'est **la** façon d'effacer un nom personnalisé (F-52) |
/// | le nom d'origine d'un **autre** câble (`Conduit 5` pour le câble 3) | refus | un numéro de câble n'est pas un nom libre |
///
/// Le troisième cas mérite d'être refusé plutôt qu'accepté : `Conduit 5` est le nom que
/// le câble 5 porte à la sortie d'usine et celui par lequel le service le retrouve dans
/// le registre. L'écrire sur le câble 3 ferait deux endpoints portant le même nom, et le
/// pont câble ↔ clés — une **égalité exacte** avec `Conduit N` — désignerait alors deux
/// câbles à la fois.
///
/// Le nom est débarrassé de ses espaces de bord, comme
/// [`conduit_backend::cable::validate_cable_name`] le fait pour juger du vide : sans
/// cela, « `Musique ` » et « `Musique` » seraient deux noms différents dans le registre
/// pour le même nom à l'écran.
///
/// # Erreurs
///
/// [`CableError::InvalidName`] : le nom est refusé par la règle du dépôt, ou c'est le
/// nom d'origine d'un autre câble.
pub fn ordre_de_renommage(cable: CableId, voulu: &str) -> Result<Requete, CableError> {
    validate_cable_name(voulu)?;
    let voulu = voulu.trim();
    match cable_nomme(voulu) {
        Some(autre) if autre != cable => Err(CableError::InvalidName(format!(
            "« {voulu} » est le nom d'origine du câble {}, pas un nom libre : \
             choisissez-en un autre, ou renommez le câble {} en « {} » pour lui rendre le \
             sien",
            autre.0,
            cable.0,
            nom(cable)
        ))),
        Some(_) => Ok(Requete::NomDefaut(cable)),
        None => Ok(Requete::Renommer {
            cable,
            nom: voulu.to_owned(),
        }),
    }
}

// ---------------------------------------------------------------------------------
// Les deux formats : le type portable et l'encodage du pilote.
// ---------------------------------------------------------------------------------
//
// Ce module est le **seul** du dépôt qui voie les deux mondes à la fois, et c'est
// délibéré. `conduit_backend::CableFormat` est la couche partagée des trois plateformes
// et ne connaît pas `conduit-kmd-core` ; `conduit_kmd_core::config::CableFormat` est le
// contrat d'un pilote Windows et ne connaît pas `SampleRate`. Mettre la traduction
// ailleurs ferait dépendre l'un de l'autre ; la mettre en deux endroits ferait deux
// règles à tenir d'accord. Elle tient ici, en deux fonctions, testées en table.

/// Le défaut portable, celui sur lequel un mot nul se replie.
///
/// Écrit comme une constante pour que l'assertion ci-dessous le compare à celui du
/// contrat : ce sont deux écritures de la **même** valeur — 48 kHz, float 32, stéréo —
/// dans deux crates qui ne se voient pas, et rien d'autre que ce test ne les tient
/// d'accord.
const FORMAT_DEFAUT: CableFormat = CableFormat {
    sample_rate: SampleRate::HZ_48000,
    depth: SampleDepth::F32,
    channels: ChannelCount::STEREO,
};

// Le nombre de canaux du contrat tient dans un `ChannelCount`, et c'est celui du défaut
// portable. Les trois assertions d'origine sont conservées : elles disaient pourquoi le
// repli sur `CANAUX_PAR_DEFAUT` ne pouvait pas échouer, elles disent maintenant pourquoi
// les deux défauts coïncident.
const _: () = assert!(MIN_CHANNELS <= CANAUX_PAR_DEFAUT && CANAUX_PAR_DEFAUT <= MAX_CHANNELS);
const _: () = assert!(CANAUX_PAR_DEFAUT <= ChannelCount::MAX as u32);
const _: () = assert!(FORMAT_DEFAUT.channels.get() as u32 == CANAUX_PAR_DEFAUT);

/// La profondeur du pilote qui correspond à la profondeur portable.
///
/// Total dans les deux sens, sans repli : les trois profondeurs sont les mêmes trois, et
/// une variante ajoutée d'un côté doit faire échouer la compilation de l'autre plutôt que
/// de tomber dans un `_ =>` silencieux.
const fn profondeur_pilote(depth: SampleDepth) -> SampleFormat {
    match depth {
        SampleDepth::Pcm16 => SampleFormat::I16,
        SampleDepth::Pcm24 => SampleFormat::Pcm24,
        SampleDepth::F32 => SampleFormat::F32,
    }
}

/// L'inverse. `SampleFormat` est `#[non_exhaustive]`, d'où le repli — qui rend `None`
/// plutôt que d'inventer une profondeur : un format qu'on ne saurait pas nommer ne vaut
/// pas mieux qu'un format inconnu.
const fn profondeur_portable(depth: SampleFormat) -> Option<SampleDepth> {
    match depth {
        SampleFormat::I16 => Some(SampleDepth::Pcm16),
        SampleFormat::Pcm24 => Some(SampleDepth::Pcm24),
        SampleFormat::F32 => Some(SampleDepth::F32),
        _ => None,
    }
}

/// Le format du pilote que décrit `format`, ou le refus qui **nomme les domaines**.
///
/// La couche portable accepte tout ce qu'un câble PipeWire pourrait servir — 22 050 Hz,
/// par exemple. Le pilote Conduit, lui, ne déclare que trois fréquences
/// ([`SAMPLE_RATES`]) parce que `copy_frames` ne rééchantillonne pas : c'est ici que la
/// frontière se franchit, et c'est donc ici que le refus doit dire lesquelles.
///
/// # Erreurs
///
/// [`CableError::Unsupported`], dont le message porte la valeur refusée **et** le domaine
/// attendu : l'utilisateur corrige sa ligne sans relire la documentation.
pub fn format_pilote(format: CableFormat) -> Result<FormatPilote, CableError> {
    let hz = format.sample_rate.hz();
    if !SAMPLE_RATES.contains(&hz) {
        return Err(CableError::Unsupported(format!(
            "fréquence {hz} Hz non servie par le pilote Conduit : attendu {}",
            SAMPLE_RATES
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let canaux = u32::from(format.channels.get());
    if !(MIN_CHANNELS..=MAX_CHANNELS).contains(&canaux) {
        return Err(CableError::Unsupported(format!(
            "{canaux} canaux hors des bornes du pilote ({MIN_CHANNELS} à {MAX_CHANNELS})"
        )));
    }
    let candidat = FormatPilote {
        sample_rate: hz,
        depth: profondeur_pilote(format.depth),
        channels: format.channels.get(),
    };
    // Ceinture et bretelles : ce qu'on vient de composer doit passer **le** codec du
    // contrat, celui-là même que le pilote applique au démarrage d'un devnode. Sans ce
    // contrôle, un champ ajouté au contrat sans être ajouté ici partirait quand même sur
    // le canal et se ferait refuser côté service, avec un message qui demande un rapport
    // de bogue au lieu de dire ce qui manque.
    FormatPilote::decode(candidat.encode()).map_err(|cause| {
        CableError::Unsupported(format!(
            "format « {format} » refusé par le contrat du pilote : {cause}"
        ))
    })
}

/// Le format portable que décrit le mot `brut` d'une réponse, ou `None`.
///
/// `None` a **deux** causes qui se traitent pareil : le mot est nul — le service ne
/// connaît pas le format de ce câble, ou l'ordre n'en visait aucun —, ou il est
/// indécodable. Dans les deux cas l'appelant se replie sur [`CableFormat::default`]
/// plutôt que d'afficher une valeur inventée.
///
/// Le mot nul est traité **avant** le codec plutôt que laissé lui tomber dessus : le
/// contrat le refuserait de toute façon — le code 0 n'existe pour aucun des trois champs
/// —, mais dire ici que 0 veut dire « inconnu » écrit l'intention du protocole là où on la
/// lit, au lieu de la faire dépendre d'une coïncidence d'encodage.
#[must_use]
pub fn format_portable(brut: u32) -> Option<CableFormat> {
    if brut == 0 {
        return None;
    }
    let pilote = FormatPilote::decode(brut).ok()?;
    Some(CableFormat {
        sample_rate: SampleRate::new(pilote.sample_rate)?,
        depth: profondeur_portable(pilote.depth)?,
        channels: ChannelCount::new(pilote.channels)?,
    })
}

// ---------------------------------------------------------------------------------
// D'une réponse à un CableInfo.
// ---------------------------------------------------------------------------------

/// Le format du câble `cable` d'après `reponse`, ou [`CableFormat::default`].
///
/// # Deux champs, jamais en concurrence
///
/// Une réponse porte le format à deux endroits, et un seul parle à la fois :
///
/// | ordre | [`Reponse::formats`] (la table) | [`Reponse::format`] (l'en-tête) |
/// |---|---|---|
/// | `lister` | les seize | **0** — l'ordre ne vise aucun câble |
/// | les sept autres | **0** — la table ne voyage pas | celui du câble visé |
///
/// On lit donc la table d'abord — elle est indexée par câble et ne peut pas se tromper de
/// cible — puis l'en-tête. Le repli ne peut pas mal attribuer un format : quand l'en-tête
/// est renseigné, la table est vide et `cable` est le câble que l'ordre visait, puisque
/// c'est le seul que les appelants passent.
#[must_use]
pub fn format_de(reponse: &Reponse, cable: CableId) -> CableFormat {
    format_portable(reponse.format_de(cable))
        .or_else(|| format_portable(reponse.format))
        .unwrap_or(FORMAT_DEFAUT)
}

/// Le [`CableInfo`] du câble `cable`, tel que `reponse` le décrit.
///
/// Les deux endpoints portent l'identifiant de repli : voir [`ENDPOINT_REPLI`].
///
/// **Les canaux viennent du format**, plus de [`Reponse::canaux`] : ils en font partie, et
/// deux sources pour le même nombre finiraient par se contredire. Le champ `canaux` de la
/// réponse reste ce qu'il a toujours été — un écho du câble visé — et ce sont les
/// messages de refus ([`Statut::CanauxNonApplicables`]) qui l'emploient.
#[must_use]
pub fn info(reponse: &Reponse, cable: CableId) -> CableInfo {
    let format = format_de(reponse, cable);
    CableInfo {
        id: cable,
        name: nom(cable),
        channels: format.channels,
        format,
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
///
/// Chaque câble porte **son** format, lu dans la table des seize que le lot A1 a ajoutée à
/// la réponse de `lister`. C'est ce qui a fait disparaître l'ancien repli sur les canaux
/// du contrat : `conduitctl cable list` annonçait deux canaux à un câble qui en servait
/// six, ce qui n'était pas une approximation mais une erreur.
#[must_use]
pub fn liste(reponse: &Reponse) -> Vec<CableInfo> {
    numeros()
        .filter(|cable| reponse.est_present(*cable))
        .map(|cable| info(reponse, cable))
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
        // Depuis M1b-05, `detail` porte le compte **réellement servi par ce câble-là**, et
        // le message nomme la manœuvre qui le changerait plutôt qu'une tâche à faire :
        // « ce câble sert 6 canaux » se comprend, « paramètre invalide » non.
        Statut::CanauxNonApplicables => CableError::Unsupported(format!(
            "ce câble sert {detail} canaux : en changer demande d'écrire son format dans \
             la clé matérielle du périphérique puis de redémarrer celui-ci — environ une \
             seconde de silence sur tous les câbles — utilisez « conduitctl cable \
             set-format »"
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
        // M1b-21. Le nom a déjà été validé par le démon avant d'être envoyé : si le
        // service le refuse quand même, c'est que les deux ne jugent pas avec la même
        // règle, et cela se rapporte.
        Statut::NomInvalide => CableError::InvalidName(format!(
            "{} — le service d'assistance a refusé ce nom alors que le démon l'avait \
             accepté : joignez « conduitctl dump » à un rapport de bogue",
            Statut::NomInvalide
        )),
        // Le cas bénin et courant : le câble est déconnecté, donc Windows n'a publié
        // aucun endpoint et il n'y a rien à renommer. Un `Driver` et non un `NotFound` :
        // le câble existe, ce sont ses endpoints qui n'existent pas encore.
        Statut::EndpointAbsent => CableError::Driver(format!(
            "{} ({detail} côté(s) sur 2 trouvé(s))",
            Statut::EndpointAbsent
        )),
        // M1b-05. Un refus qui **nomme la séquence** : le format d'un endpoint est figé à
        // sa création, donc changer celui d'un câble connecté ne le déplacerait pas. Un
        // `Unsupported` et non un `Driver` : rien n'est en panne, c'est l'ordre des gestes
        // qui n'est pas le bon, et l'appelant peut le corriger seul.
        Statut::CableActif => CableError::Unsupported(
            "ce câble est connecté : désactivez-le, réglez son format, puis \
             réactivez-le — le format d'un endpoint audio est figé à sa création, et le \
             redémarrage du périphérique ne le déplacerait pas"
                .to_owned(),
        ),
        // Le nom a déjà été validé par le démon avant d'être envoyé, comme pour
        // `NomInvalide` : si le service le refuse quand même, les deux ne jugent pas avec
        // le même codec, et cela se rapporte.
        Statut::FormatInvalide => CableError::Unsupported(format!(
            "le service d'assistance a refusé le format {detail:#010x} alors que le démon \
             l'avait accepté : joignez « conduitctl dump » à un rapport de bogue"
        )),
        // **Rien n'a changé** : le câble sert toujours son ancien format, et réessayer est
        // sans danger. C'est la moitié « avant l'écriture » de `crate::devnode`.
        Statut::FormatNonEcrit => CableError::Driver(format!(
            "le format n'a pas pu être écrit dans la clé matérielle du périphérique : rien \
             n'a changé, le câble sert toujours son ancien format (code du système : \
             {detail})"
        )),
        // **La valeur est écrite** : l'inverse du précédent, et la conduite est opposée —
        // provoquer le redémarrage, non réessayer l'écriture.
        Statut::RedemarrageEchoue => CableError::Driver(format!(
            "le format est écrit mais le périphérique n'a pas redémarré : il prendra effet \
             au prochain démarrage du périphérique, au redémarrage de la machine au pire. \
             Fermez ce qui tient un flux audio, puis réessayez (code du système : {detail})"
        )),
    })
}

// ---------------------------------------------------------------------------------
// Le contrôle proprement dit.
// ---------------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use super::{
        cable_nomme, format_de, format_pilote, info, liste, nom, ordre_de_renommage, premier_libre,
        verifier, CableError, CableId, CableInfo, ChannelCount, CABLE_MAX, CANAUX_PAR_DEFAUT,
    };
    use crate::protocole::{Reponse, Requete, NOM_TUBE};
    use crate::tube::{demander, ErreurClient};
    use conduit_backend::cable::validate_cable_name;
    use conduit_backend::{CableControl, CableFormat, CableSpec};

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
            let reponse = demander(&requete).map_err(|erreur| erreur_client(&erreur))?;
            verifier(&reponse, cible)?;
            Ok(reponse)
        }

        /// L'état des seize câbles, sans rien modifier.
        fn etat(&self) -> Result<Reponse, CableError> {
            self.ordre(Requete::Lister)
        }

        /// Renomme un câble **qu'on vient d'activer**, en laissant à Windows le temps de
        /// publier ses endpoints.
        ///
        /// Seul le refus « endpoint absent » fait attendre : c'est le seul qui puisse
        /// disparaître tout seul. Un nom refusé, un service absent ou un refus du
        /// registre sortent au premier essai — réessayer n'y changerait rien et ne ferait
        /// que retarder le message.
        ///
        /// # Erreurs
        ///
        /// La [`CableError`] du dernier essai, enrichie du fait que **le câble, lui, est
        /// bien actif** et de la commande qui termine le travail : sans cela, l'appelant
        /// croirait que rien n'a eu lieu et laisserait un câble activé derrière lui.
        fn renommer_apres_activation(
            &mut self,
            cable: CableId,
            nouveau: &str,
        ) -> Result<CableInfo, CableError> {
            let mut dernier = None;
            for essai in 0..RENOMMAGE_ESSAIS {
                if essai > 0 {
                    std::thread::sleep(RENOMMAGE_PAS);
                }
                match self.rename(cable, nouveau) {
                    Ok(info) => return Ok(info),
                    Err(erreur) => {
                        let a_reessayer = matches!(&erreur, CableError::Driver(message)
                            if message.contains(ENDPOINT_PAS_ENCORE));
                        dernier = Some(erreur);
                        if !a_reessayer {
                            break;
                        }
                    }
                }
            }
            Err(match dernier {
                Some(erreur) => CableError::Driver(format!(
                    "le câble {} est activé mais n'a pas pu être renommé en « {nouveau} » : \
                     {erreur} — le câble reste disponible sous « {} », et « conduitctl \
                     cable rename {} {nouveau} » terminera le travail",
                    cable.0,
                    nom(cable),
                    cable.0
                )),
                // Injoignable : `RENOMMAGE_ESSAIS` est non nul, donc la boucle rend un
                // `Ok` ou remplit `dernier`. Un repli plutôt qu'un `unwrap`.
                None => CableError::Driver(format!(
                    "le câble {} est activé mais n'a pas pu être renommé",
                    cable.0
                )),
            })
        }
    }

    /// Le fragment du message de [`Statut::EndpointAbsent`] auquel
    /// `renommer_apres_activation` reconnaît un refus qui peut disparaître tout seul.
    ///
    /// Reconnu sur le **texte** parce que `CableControl::rename` rend une `CableError`,
    /// qui ne transporte pas le statut du protocole : c'est le prix de l'interface
    /// portable du trait. Le fragment est court, en français, et le test
    /// `le_refus_d_endpoint_absent_se_reconnait` le tient d'accord avec le message que
    /// `verifier` construit — s'ils divergent, l'attente disparaît en silence, et c'est
    /// ce test qui l'attrape.
    pub(super) const ENDPOINT_PAS_ENCORE: &str = "aucun endpoint audio pour ce câble";

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

    /// Combien de fois `create` réessaie de renommer un câble qu'il vient d'activer, et
    /// à quel rythme.
    ///
    /// **Une course réelle, mesurée.** Activer un câble écrit dans le pilote ; Windows
    /// publie ensuite ses endpoints, et la clé `MMDevices` du registre n'existe qu'à ce
    /// moment-là — **77 ms** après l'écriture dans la machine virtuelle. Un `create` qui
    /// renommerait immédiatement trouverait donc souvent un
    /// [`Statut::EndpointAbsent`](crate::protocole::Statut::EndpointAbsent) sur un câble
    /// pourtant bien actif.
    ///
    /// Une seconde au total, par pas de 50 ms : plus de dix fois la latence mesurée, et
    /// assez court pour qu'un `conduitctl cable add` reste instantané à l'usage. L'attente
    /// n'a lieu **que** tant que le service dit « endpoint absent » ; toute autre issue
    /// sort immédiatement.
    const RENOMMAGE_ESSAIS: u32 = 20;
    /// Le pas d'attente entre deux essais. Voir [`RENOMMAGE_ESSAIS`].
    const RENOMMAGE_PAS: std::time::Duration = std::time::Duration::from_millis(50);

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
        /// erreur (voir l'en-tête de module).
        ///
        /// # Tout autre nom : on active, puis on renomme (M1b-21)
        ///
        /// `conduitctl cable add --name Musique` prend donc le premier câble libre et lui
        /// donne ce nom dans le registre. Les deux étapes sont distinctes parce que la
        /// machine les distingue : le pilote ne connaît que la connexion, et le nom vit
        /// dans `MMDevices`. Le câble n'existe pour Windows — et sa clé de registre avec
        /// lui — qu'une fois activé, d'où l'ordre, et d'où l'attente bornée de
        /// [`RENOMMAGE_ESSAIS`].
        ///
        /// Si le renommage échoue malgré l'attente, **le câble reste actif** et l'erreur
        /// le dit, avec la commande qui termine le travail : le défaire serait détruire
        /// ce que l'appelant a obtenu pour n'avoir pas obtenu le reste.
        ///
        /// # `spec.format` : le format **avant** l'activation
        ///
        /// `Some` demande la seule séquence qui produise un endpoint au bon format —
        /// écrire `CableFormat<n>`, redémarrer le devnode, **puis** connecter. L'ordre
        /// n'est pas négociable : le format du moteur d'un endpoint est mis en cache à sa
        /// création (mesuré en M1b-05), et régler celui d'un câble déjà connecté ne le
        /// déplacerait pas. D'où le refus, qui nomme la séquence, quand le câble visé est
        /// **déjà actif dans un autre format** : mieux vaut le dire que redémarrer son
        /// périphérique pour rien.
        ///
        /// Quand `spec.format` est `Some`, `spec.channels` est **ignoré** : le format porte
        /// déjà ses canaux.
        ///
        /// # Sans format, un nombre de canaux hors du défaut reste refusé
        ///
        /// Le refus est prononcé **avant** d'activer quoi que ce soit : activer puis
        /// échouer sur les canaux laisserait derrière un câble que l'appelant n'a pas
        /// demandé. `create` **choisit** son câble (le premier libre) et ne peut donc pas
        /// relire à l'avance le format de celui qu'il prendra ; un `--channels 6` ne
        /// réussirait que si le câble tiré au sort se trouvait réglé sur six.
        ///
        /// Ce qui a changé depuis M1b-05 : le message nomme désormais la sortie, parce
        /// qu'elle existe — `--format 48000:f32:6`, qui passe par `spec.format` et paie la
        /// seconde de silence en connaissance de cause.
        fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError> {
            // Le format demandé est traduit **d'abord** : une fréquence que le pilote ne
            // sert pas doit être refusée avant qu'un câble soit activé, pas après.
            let format_voulu = match spec.format {
                Some(format) => {
                    format_pilote(format)?;
                    Some(format)
                }
                None => {
                    if u32::from(spec.channels.get()) != CANAUX_PAR_DEFAUT {
                        return Err(CableError::Unsupported(format!(
                            "un câble neuf est en {CANAUX_PAR_DEFAUT} canaux : pour en \
                             obtenir un autre nombre, demandez le format entier — son \
                             écriture dans la clé matérielle du périphérique et le \
                             redémarrage de celui-ci coûtent environ une seconde de \
                             silence sur les seize câbles"
                        )));
                    }
                    None
                }
            };
            // Le nom voulu, et le câble qu'il désigne s'il en désigne un. Un nom libre
            // ne vise aucun câble en particulier : on prendra le premier libre, puis on
            // le renommera.
            let voulu = match spec.name.as_deref() {
                None => None,
                Some(voulu) => {
                    validate_cable_name(voulu)?;
                    Some(voulu.trim().to_owned())
                }
            };
            let vise = voulu.as_deref().and_then(cable_nomme);
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
            // Le nom libre à appliquer, s'il y en a un : `Conduit N` ne se renomme pas,
            // c'est déjà le nom d'origine.
            let a_renommer = voulu.filter(|_| vise.is_none());

            // Déjà connecté : la demande est satisfaite, on rend l'état constaté.
            if etat.est_actif(cible) {
                // Sauf si l'appelant voulait un **autre** format : on ne le lui donnera
                // pas sans déconnecter, et le taire ferait croire à un succès.
                if let Some(format) = format_voulu {
                    let courant = format_de(&etat, cible);
                    if courant != format {
                        return Err(CableError::Unsupported(format!(
                            "le câble {} est connecté et sert « {courant} » : \
                             désactivez-le, réglez son format, puis réactivez-le — le \
                             format d'un endpoint audio est figé à sa création, et le \
                             redémarrage du périphérique ne le déplacerait pas",
                            cible.0
                        )));
                    }
                }
                return match a_renommer {
                    // Les endpoints sont publiés depuis longtemps : un seul essai suffit.
                    Some(nouveau) => self.rename(cible, &nouveau),
                    None => Ok(info(&etat, cible)),
                };
            }
            // Le format **avant** l'activation : c'est la seule séquence qui produise un
            // endpoint au bon format. Son échec sort ici, sur un câble encore déconnecté,
            // donc sans rien laisser derrière.
            if let Some(format) = format_voulu {
                self.set_format(cible, format)?;
            }
            let reponse = self.ordre(Requete::Activer(cible))?;
            match a_renommer {
                Some(nouveau) => self.renommer_apres_activation(cible, &nouveau),
                None => Ok(info(&reponse, cible)),
            }
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

        /// Écrit le **format** du câble dans la clé matérielle de son périphérique, puis
        /// redémarre celui-ci (M1b-05, ordre 7 du protocole).
        ///
        /// Environ **une seconde de silence sur les seize câbles** : le redémarrage du
        /// devnode republie les endpoints du pilote, tous les câbles compris, et les flux
        /// ouverts s'interrompent. Ce n'est pas une manœuvre qu'on déclenche par surprise,
        /// et c'est pourquoi ni `ensure_cables` ni aucune convergence silencieuse ne
        /// l'appellent d'eux-mêmes.
        ///
        /// # Le pré-contrôle d'inactivité, et pourquoi le service reste le juge
        ///
        /// Un `lister` précède l'ordre pour deux choses : dire « câble inconnu » sur un
        /// numéro hors réserve — comme `set_channels` et `remove` —, et donner
        /// **immédiatement** le message « désactivez-le d'abord » sans faire payer un
        /// aller-retour privilégié pour un refus certain. Ce n'est qu'un raccourci de
        /// diagnostic : entre notre lecture et l'écriture, le câble peut être connecté par
        /// quelqu'un d'autre, et c'est le service qui tranche pour de bon
        /// ([`Statut::CableActif`]). Décider ici **à sa place** ferait deux politiques à
        /// tenir d'accord, ce que M1b-21 a déjà appris à ne pas faire.
        fn set_format(
            &mut self,
            id: CableId,
            format: CableFormat,
        ) -> Result<CableInfo, CableError> {
            // La traduction d'abord : une fréquence hors des trois du pilote se refuse
            // sans ouvrir le canal, avec un message qui nomme le domaine.
            let format_du_pilote = format_pilote(format)?;
            let etat = self.etat()?;
            if !etat.est_present(id) {
                return Err(CableError::NotFound(id));
            }
            if etat.est_actif(id) {
                return Err(CableError::Unsupported(format!(
                    "le câble {} est connecté : désactivez-le, réglez son format, puis \
                     réactivez-le — le format d'un endpoint audio est figé à sa création, \
                     et le redémarrage du périphérique ne le déplacerait pas",
                    id.0
                )));
            }
            let reponse = self.ordre(Requete::Format {
                cable: id,
                format: format_du_pilote,
            })?;
            Ok(info(&reponse, id))
        }

        /// **Renomme le câble côté OS** (M1b-21), par le registre et sans toucher au
        /// pilote.
        ///
        /// Le nom d'un endpoint audio vit dans
        /// `HKLM\...\MMDevices\Audio\{Render|Capture}\{id}\Properties`, sous
        /// `PKEY_Device_DeviceDesc` ; c'est le service qui l'y écrit parce que la clé est
        /// sous `HKLM`. Tout ce qui établit *quelle* valeur et *pourquoi* est dans
        /// l'en-tête de [`crate::registre`].
        ///
        /// # Renommer un câble en `Conduit N` **efface** son nom personnalisé
        ///
        /// C'est la façon d'exposer le retour en arrière sans ajouter une méthode au
        /// trait : `rename(CableId(3), "Conduit 3")` envoie
        /// [`Requete::NomDefaut`](crate::protocole::Requete::NomDefaut), qui remet la
        /// description d'origine et supprime la marque de Conduit — ce que F-52 exige, et
        /// ce qu'un utilisateur cherchera naturellement à taper. Renommer le câble 3 en
        /// « Conduit 5 » n'a en revanche aucun sens et est refusé : le numéro est celui
        /// du pilote, pas un nom libre.
        ///
        /// # Le `CableInfo` rendu porte le nom **demandé**
        ///
        /// Le service ne relit pas le registre après avoir écrit — la valeur qu'il vient
        /// d'y mettre est celle-là. Ce que le dorsal WASAPI publiera ensuite dans
        /// `DeviceInfo::name` dépend, lui, du moment où le moteur audio relira la clé :
        /// voir la section « rafraîchissement » de [`crate::registre`].
        ///
        /// Le **format**, lui, est celui que la réponse porte : renommer ne touche ni au
        /// pilote ni à la clé matérielle, et le service relit l'état des câbles après
        /// chaque écriture. Seul le nom est remplacé dans le [`CableInfo`] rendu.
        fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError> {
            let requete = ordre_de_renommage(id, name)?;
            let voulu = requete.nom().unwrap_or(name).trim().to_owned();
            let reponse = self.ordre(requete)?;
            Ok(CableInfo {
                name: voulu,
                ..info(&reponse, id)
            })
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
    use conduit_kmd_core::config::{ACTIVE_CABLES_MASK, CABLE_FORMAT_DEFAULT};

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
            format: 0,
            formats: [0; CABLE_MAX as usize],
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
                    // Le refus **nomme la manœuvre** — écrire le format, redémarrer le
                    // périphérique — et le compte servi. C'est ce qui le distingue d'un
                    // « paramètre invalide » sur lequel personne ne sait quoi faire.
                    assert!(texte.contains("redémarrer"), "{texte}");
                    assert!(texte.contains("canaux"), "{texte}");
                }
                Statut::VersionInconnue => {
                    assert!(texte.contains("réinstallez"), "{texte}");
                }
                Statut::ErreurSysteme => {
                    // Le code du système est rendu tel quel, jamais traduit.
                    assert!(texte.contains("1314"), "{texte}");
                }
                Statut::EndpointAbsent => {
                    // Le message dit **quoi faire** : connecter le câble d'abord.
                    assert!(texte.contains("connecté"), "{texte}");
                }
                Statut::NomInvalide => {
                    assert!(matches!(erreur, CableError::InvalidName(_)));
                }
                // M1b-05. Le refus **nomme la séquence**, dans l'ordre : désactiver,
                // régler, réactiver. Un « paramètre invalide » laisserait chercher.
                Statut::CableActif => {
                    assert!(texte.contains("désactiv"), "{texte}");
                    assert!(texte.contains("réactiv"), "{texte}");
                }
                Statut::FormatInvalide => {
                    // Le démon a déjà validé le format : un refus du service est un
                    // désaccord entre deux codecs, et cela se rapporte.
                    assert!(texte.contains("rapport de bogue"), "{texte}");
                }
                Statut::FormatNonEcrit => {
                    // **Rien n'a changé** : c'est ce qui distingue 14 de 15, et la
                    // conduite qui en découle est « réessayez ».
                    assert!(texte.contains("rien n'a changé"), "{texte}");
                    assert!(texte.contains("1314"), "{texte}");
                }
                Statut::RedemarrageEchoue => {
                    // **La valeur est écrite** et prendra effet au prochain démarrage du
                    // périphérique : la conduite opposée de la précédente.
                    assert!(texte.contains("écrit"), "{texte}");
                    assert!(texte.contains("prochain démarrage"), "{texte}");
                }
                _ => {}
            }
        }

        // Les deux statuts qu'il ne faut jamais confondre ne disent pas la même chose.
        let non_ecrit = verdict_texte(Statut::FormatNonEcrit);
        let echoue = verdict_texte(Statut::RedemarrageEchoue);
        assert_ne!(non_ecrit, echoue);
        assert!(!non_ecrit.contains("prochain démarrage"), "{non_ecrit}");
        assert!(!echoue.contains("rien n'a changé"), "{echoue}");
    }

    /// Le message que `verifier` rend pour `statut`, pour comparer deux statuts entre eux.
    fn verdict_texte(statut: Statut) -> String {
        let mut r = reponse(ACTIVE_CABLES_MASK, 0);
        r.statut = statut;
        r.detail = 1314;
        verifier(&r, Some(CableId(3)))
            .expect_err("statut de refus")
            .to_string()
    }

    /// **La politique du renommage**, en table de cas : ce qui renomme, ce qui efface,
    /// ce qui est refusé.
    #[test]
    fn ordre_de_renommage_table() {
        // Un nom libre : on renomme.
        assert_eq!(
            ordre_de_renommage(CableId(3), "Musique"),
            Ok(Requete::Renommer {
                cable: CableId(3),
                nom: "Musique".to_owned()
            })
        );
        // Les espaces de bord sont retirés : « Musique » et « Musique  » sont le même
        // nom à l'écran, ils doivent l'être aussi dans le registre.
        assert_eq!(
            ordre_de_renommage(CableId(3), "  Musique  "),
            Ok(Requete::Renommer {
                cable: CableId(3),
                nom: "Musique".to_owned()
            })
        );

        // Son propre nom d'origine : c'est **la** façon d'effacer le nom personnalisé.
        for numero in 1..=CABLE_MAX {
            let cable = CableId(numero);
            assert_eq!(
                ordre_de_renommage(cable, &nom(cable)),
                Ok(Requete::NomDefaut(cable)),
                "câble {numero}"
            );
        }

        // Le nom d'origine d'un **autre** câble : refusé, et le message dit les deux
        // sorties possibles.
        let erreur = ordre_de_renommage(CableId(3), "Conduit 5")
            .expect_err("le nom d'origine d'un autre câble est refusé");
        assert!(matches!(erreur, CableError::InvalidName(_)));
        let texte = erreur.to_string();
        assert!(texte.contains("Conduit 5"), "{texte}");
        assert!(texte.contains("Conduit 3"), "{texte}");

        // Le piège du préfixe, dans les deux sens : « Conduit 1 » ne vise pas le câble
        // 16, et renommer le câble 1 en « Conduit 16 » est refusé, pas accepté.
        assert!(ordre_de_renommage(CableId(1), "Conduit 16").is_err());
        assert!(ordre_de_renommage(CableId(16), "Conduit 1").is_err());
        // Un « nom » qui ressemble à un numéro sans en être un reste un nom libre.
        for libre in ["Conduit 01", "Conduit 17", "conduit 3", "Conduit"] {
            assert!(
                matches!(
                    ordre_de_renommage(CableId(3), libre),
                    Ok(Requete::Renommer { .. })
                ),
                "« {libre} »"
            );
        }

        // La règle du dépôt s'applique avant tout le reste, et c'est bien elle.
        for refuse in ["", "   ", "a/b", "a\nb", &"x".repeat(65)] {
            assert!(
                matches!(
                    ordre_de_renommage(CableId(3), refuse),
                    Err(CableError::InvalidName(_))
                ),
                "« {refuse} »"
            );
            assert!(validate_cable_name(refuse).is_err(), "« {refuse} »");
        }
    }

    /// Le fragment auquel `create` reconnaît un « endpoint pas encore publié » est bien
    /// dans le message que [`verifier`] construit.
    ///
    /// Sans ce test, les deux dériveraient en silence et l'attente de `create`
    /// disparaîtrait : le renommage échouerait alors une fois sur deux après un
    /// `cable add --name`, pour une raison invisible.
    #[cfg(windows)]
    #[test]
    fn le_refus_d_endpoint_absent_se_reconnait() {
        let r = Reponse::refus_detaille(ORDRE_ACTIVER, Statut::EndpointAbsent, 0);
        let erreur = verifier(&r, Some(CableId(3))).expect_err("endpoint absent est un refus");
        let CableError::Driver(message) = &erreur else {
            panic!("un endpoint absent doit rester un CableError::Driver : {erreur:?}");
        };
        assert!(
            message.contains(super::windows::ENDPOINT_PAS_ENCORE),
            "« {} » ne contient pas « {} »",
            message,
            super::windows::ENDPOINT_PAS_ENCORE
        );
        // Et aucun autre statut ne se fait prendre pour celui-là.
        for statut in Statut::ALL {
            if statut == Statut::EndpointAbsent || statut.succes() {
                continue;
            }
            let autre = Reponse::refus_detaille(ORDRE_ACTIVER, statut, 0);
            let texte = verifier(&autre, Some(CableId(3)))
                .err()
                .map_or_else(String::new, |e| e.to_string());
            assert!(
                !texte.contains(super::windows::ENDPOINT_PAS_ENCORE),
                "{statut:?} : {texte}"
            );
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

    /// **Le défaut portable est celui du pilote.** Deux crates qui ne se voient pas
    /// écrivent la même valeur ; rien d'autre que ce test ne les tient d'accord.
    #[test]
    fn le_defaut_portable_est_celui_du_pilote() {
        assert_eq!(
            format_portable(CABLE_FORMAT_DEFAULT.encode()),
            Some(CableFormat::default())
        );
        assert_eq!(FORMAT_DEFAUT, CableFormat::default());
        assert_eq!(
            format_pilote(CableFormat::default()).unwrap(),
            CABLE_FORMAT_DEFAULT
        );
        assert_eq!(
            CableFormat::default().channels.get() as u32,
            CANAUX_PAR_DEFAUT
        );
    }

    /// **L'aller-retour entre les deux mondes**, sur tout le domaine du contrat : trois
    /// fréquences, trois profondeurs, huit comptes de canaux — soixante-douze formats.
    #[test]
    fn les_deux_mondes_du_format_se_traduisent() {
        for hz in SAMPLE_RATES {
            for depth in SampleDepth::ALL {
                for canaux in 1..=ChannelCount::MAX {
                    let portable = CableFormat {
                        sample_rate: SampleRate::new(hz).expect("fréquence du contrat"),
                        depth,
                        channels: ChannelCount::new(canaux).expect("canaux du contrat"),
                    };
                    let pilote = format_pilote(portable).expect("format du domaine");
                    assert_eq!(pilote.sample_rate, hz);
                    assert_eq!(u32::from(pilote.channels), u32::from(canaux));
                    assert_eq!(
                        format_portable(pilote.encode()),
                        Some(portable),
                        "{portable}"
                    );
                }
            }
        }
    }

    /// **Le mot nul est « inconnu »**, jamais un format.
    ///
    /// Le protocole s'en sert pour dire « je ne sais pas » ; le décoder donnerait un code
    /// de fréquence 0, que le contrat refuse de toute façon — mais le dire ici plutôt que
    /// de s'en remettre au codec rend l'intention vérifiable.
    #[test]
    fn le_mot_nul_n_est_pas_un_format() {
        assert_eq!(format_portable(0), None);
        // Chaque champ aberrant donne « inconnu » lui aussi : un octet de poids fort non
        // nul, un code de fréquence inexistant, zéro canal.
        for aberrant in [0xFF00_0302_u32, 0x0002_03FF, 0x0000_0302, 0x0002_0300] {
            assert_eq!(format_portable(aberrant), None, "{aberrant:#010x}");
        }
    }

    /// Ce que la couche portable accepte et que le pilote refuse : la frontière, et le
    /// message qui la nomme.
    #[test]
    fn la_frontiere_du_pilote_nomme_son_domaine() {
        let hors_domaine = CableFormat {
            sample_rate: SampleRate::new(22_050).expect("fréquence du dépôt"),
            ..CableFormat::default()
        };
        let erreur = format_pilote(hors_domaine).expect_err("22 050 Hz n'est pas servi");
        assert!(matches!(erreur, CableError::Unsupported(_)));
        let texte = erreur.to_string();
        assert!(texte.contains("22050"), "{texte}");
        for hz in SAMPLE_RATES {
            assert!(texte.contains(&hz.to_string()), "{texte} ne dit pas {hz}");
        }
    }

    /// **Les canaux viennent du format**, pour les seize câbles d'un `lister`.
    ///
    /// C'est ce que la table de la réponse a rendu possible : avant elle, un `lister`
    /// annonçait le défaut du contrat pour tout le monde, y compris pour un câble qui
    /// servait six canaux.
    #[test]
    fn les_canaux_viennent_du_format() {
        let mut r = reponse(0b111, 0b001);
        r.formats[0] = CABLE_FORMAT_DEFAULT.encode();
        r.formats[1] = FormatPilote {
            sample_rate: 96_000,
            depth: SampleFormat::Pcm24,
            channels: 6,
        }
        .encode();
        // Le troisième reste inconnu : le service n'a pas su lire sa clé matérielle.
        r.formats[2] = 0;

        let cables = liste(&r);
        assert_eq!(cables.len(), 3);
        assert_eq!(cables[0].format, CableFormat::default());
        assert_eq!(cables[1].format.channels.get(), 6);
        assert_eq!(cables[1].format.sample_rate, SampleRate::HZ_96000);
        assert_eq!(cables[1].format.depth, SampleDepth::Pcm24);
        // Mot nul : le défaut, comme les canaux se repliaient autrefois — mais c'est
        // désormais le cas exceptionnel.
        assert_eq!(cables[2].format, CableFormat::default());
        // Et `channels` est le raccourci, jamais une seconde vérité.
        for cable in &cables {
            assert_eq!(cable.channels, cable.format.channels, "{}", cable.name);
        }
    }

    /// Les deux champs de format d'une réponse ne se marchent pas dessus : la table pour
    /// `lister`, l'en-tête pour les sept autres ordres.
    #[test]
    fn le_format_se_lit_dans_la_table_ou_dans_l_en_tete() {
        let six = FormatPilote {
            sample_rate: 48_000,
            depth: SampleFormat::F32,
            channels: 6,
        }
        .encode();

        // Un `activer` : la table ne voyage pas, l'en-tête porte le câble visé.
        let mut active = reponse(ACTIVE_CABLES_MASK, 0b100);
        active.ordre = ORDRE_ACTIVER;
        active.format = six;
        assert_eq!(format_de(&active, CableId(3)).channels.get(), 6);
        assert_eq!(info(&active, CableId(3)).channels.get(), 6);

        // Un `lister` : la table parle, et l'en-tête vaut 0 — le repli est inerte.
        let mut listee = reponse(ACTIVE_CABLES_MASK, 0);
        listee.formats[2] = six;
        assert_eq!(listee.format, 0);
        assert_eq!(format_de(&listee, CableId(3)).channels.get(), 6);
        assert_eq!(
            format_de(&listee, CableId(1)),
            CableFormat::default(),
            "un câble sans format dans la table garde le défaut"
        );
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
