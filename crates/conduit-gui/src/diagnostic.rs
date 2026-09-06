//! Vue « Diagnostic » : xruns, latence, pilote et export de rapport (M2-08).
//!
//! La vue n'affiche que ce que [`EngineStatus`] porte : quatre tuiles de
//! chiffres, un tableau des périphériques ouverts, et une colonne « Moteur »
//! où se choisit le pilote de graphe. Tout le reste — ce que le protocole ne
//! mesure pas — est **dit**, pas inventé.
//!
//! Deux couches, comme partout dans ce crate :
//!
//! - les fonctions **qui décident** — [`tuiles`], [`lignes`], [`charge_cpu`],
//!   [`choix_de_pilote`], [`nom_du_rapport`] — sont pures et testées sans
//!   fenêtre ni moteur de rendu ;
//! - les fonctions **qui composent** rendent des `Element` et ne sont pas
//!   testables autrement qu'à l'œil.
//!
//! # Trois honnêtetés
//!
//! **1. Il n'y a pas de latence par nœud.** [`DeviceStatus`] n'en porte
//! aucune, et le protocole n'en expose nulle part ailleurs. La colonne
//! « Latence » écrit donc la latence estimée **du moteur** — deux quanta,
//! `2 × quantum / fréquence` —, la même sur toutes les lignes, et un tiret
//! cadratin pour un périphérique suspendu, qui ne tourne pas. Une note sous le
//! tableau le dit à l'utilisateur (voir [`latence_du_noeud`]).
//!
//! **2. Le remplissage de tampon est en trames, pas en pourcentage.**
//! [`DeviceStatus::fill`] est un nombre de trames et le protocole n'annonce
//! aucune capacité : il n'existe aucun dénominateur. La colonne « Tampon »
//! écrit donc la valeur en trames, et sa barre est **relative au plus grand
//! remplissage du tableau** (voir [`part_de_remplissage`]) — ce qu'une note
//! dit également. Une barre qui laisserait croire à un pourcentage serait un
//! mensonge.
//!
//! **3. Le ratio est celui du rééchantillonneur.** Il vaut à peu de chose près
//! 1 : il s'écrit à la sixième décimale
//! ([`format::ratio_reechantillonnage`]), comme `conduitctl status` le fait
//! déjà, et non avec le « × » et les deux décimales de [`format::ratio`].
//!
//! # Le quantum et la fréquence sont en lecture seule
//!
//! Le protocole n'a ni `SetQuantum` ni `SetSampleRate` : les deux valeurs sont
//! fixées au démarrage du démon. Elles sont donc montrées dans la même boîte
//! que la liste déroulante du pilote, à la même typographie, mais **sans
//! chevron** — une liste déroulante inerte ferait croire à un réglage.

use std::path::PathBuf;
use std::time::SystemTime;

use conduit_protocol::{
    DeviceStatus, DriverChoice, EngineStatus, NodeDescriptor, NodeState, TimingSnapshot,
};
use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{button, column, container, pick_list, row, rule, scrollable, space, text};
use iced::{Element, Fill, Length, Padding, Right};

use crate::app::Message;
use crate::i18n::{self, Text};
use crate::model::Mirror;
use crate::theme::{
    CORPS_INTERFACE, CORPS_META, ESPACE_L, ESPACE_M, ESPACE_S, ESPACE_XL, ESPACE_XS, ESPACE_XXL,
    FILET,
};
use crate::{format, style, typo};

// --- Mesures de la maquette -------------------------------------------------

/// Corps de la valeur d'une tuile.
const CORPS_TUILE: f32 = 28.0;
/// Corps d'un surtitre, en petites capitales.
const CORPS_SURTITRE: f32 = 11.0;
/// Corps d'un en-tête de colonne, en petites capitales.
const CORPS_ENTETE: f32 = 11.0;
/// Corps de la description du pilote de plateforme, en texte courant.
const CORPS_DESCRIPTION: f32 = 15.0;
/// Interligne serré d'une valeur de tuile : un chiffre n'a pas besoin des 1,6
/// du texte courant.
const INTERLIGNE_SERRE: f32 = 1.1;
/// Padding d'une tuile : 16 en hauteur, 18 en largeur.
const PADDING_TUILE: Padding = Padding {
    top: 16.0,
    right: 18.0,
    bottom: 16.0,
    left: 18.0,
};
/// Hauteur d'une tuile : la même pour les quatre, que la tuile des xruns
/// porte son action de remise à zéro ou non.
const HAUTEUR_TUILE: f32 = 116.0;
/// Padding d'une boîte de valeur — liste déroulante et lecture seule.
const PADDING_BOITE: Padding = Padding {
    top: 8.0,
    right: 12.0,
    bottom: 8.0,
    left: 12.0,
};
/// Padding vertical d'une ligne du tableau.
const PADDING_LIGNE: Padding = Padding {
    top: 10.0,
    right: 0.0,
    bottom: 10.0,
    left: 0.0,
};
/// Largeur de la colonne « État ».
const COL_ETAT: f32 = 96.0;
/// Largeur de la colonne « Latence ».
const COL_LATENCE: f32 = 88.0;
/// Largeur de la colonne « Tampon ».
const COL_TAMPON: f32 = 132.0;
/// Largeur de la colonne « Ratio ».
const COL_RATIO: f32 = 88.0;
/// Écart entre deux colonnes du tableau.
const ECART_COLONNES: f32 = 16.0;
/// Largeur de la colonne « Moteur ».
const LARGEUR_MOTEUR: f32 = 260.0;
/// Écart entre le tableau et la colonne « Moteur ».
const ECART_MOTEUR: f32 = ESPACE_XXL;
/// Hauteur d'une barre de remplissage.
const HAUTEUR_BARRE: f32 = 4.0;
/// Nombre de parts d'une barre : `iced` ne sait partager une largeur qu'en
/// parts entières, mille suffisent au dixième de pour cent.
const PARTS_BARRE: u16 = 1_000;

/// Préfixe du fichier de rapport.
///
/// Ce n'est **pas** un texte d'interface : un nom de fichier ne se traduit
/// pas, sans quoi un rapport changerait de nom d'une langue à l'autre et les
/// utilisateurs ne se comprendraient plus en s'en envoyant.
const PREFIXE_RAPPORT: &str = "conduit-diagnostic-";
/// Extension du fichier de rapport : le démon rédige du texte brut.
const EXTENSION_RAPPORT: &str = ".txt";

// --- Décisions : les quatre tuiles ------------------------------------------

/// Une tuile de chiffre : un surtitre, une valeur, une note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tuile {
    /// Surtitre, en petites capitales.
    pub surtitre: Text,
    /// La valeur, déjà mise en forme.
    pub valeur: String,
    /// La note qui la qualifie.
    pub note: String,
}

/// Les quatre tuiles, dans l'ordre de lecture : xruns, latence estimée, temps
/// de cycle, charge CPU.
///
/// `xruns` est le compteur du miroir ([`Mirror::xruns`]) et non
/// [`EngineStatus::xruns`] : le premier suit les événements du fil audio reçus
/// depuis le chargement, le second date de la dernière lecture de `Status`.
pub fn tuiles(status: &EngineStatus, xruns: u64) -> [Tuile; 4] {
    let t = &status.timing;
    [
        Tuile {
            surtitre: Text::DiagXruns,
            valeur: format::entier(xruns),
            note: i18n::t(Text::DiagXrunsGoal).to_string(),
        },
        Tuile {
            surtitre: Text::DiagLatency,
            valeur: format::latence_estimee(status.quantum.get() as u32, status.sample_rate.hz()),
            note: i18n::t(Text::DiagLatencyPath).to_string(),
        },
        Tuile {
            surtitre: Text::DiagCycle,
            valeur: format::millisecondes(nanosecondes_en_ms(t.avg_ns)),
            note: i18n::cycle_note(
                &format::microsecondes(t.min_ns / 1_000),
                &format::microsecondes(t.max_ns / 1_000),
                &format::microsecondes(t.budget_ns / 1_000),
            ),
        },
        Tuile {
            surtitre: Text::DiagLoad,
            valeur: match charge_cpu(t) {
                Some(charge) => format::pourcentage(charge),
                None => inconnu(Text::UnitPercent),
            },
            note: i18n::charge_note(status.nodes),
        },
    ]
}

/// Une valeur que le démon ne donne pas : un tiret cadratin et son unité.
fn inconnu(unite: Text) -> String {
    format!(
        "{}{}{}",
        i18n::t(Text::Inconnu),
        format::INSECABLE,
        i18n::t(unite)
    )
}

/// Une durée en nanosecondes, en millisecondes.
fn nanosecondes_en_ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

/// La charge d'un cœur : le temps de cycle moyen rapporté à son budget.
///
/// `None` quand le budget est nul — un moteur qui n'a pas encore tourné : il
/// n'y a alors rien à diviser, et la tuile écrit un tiret plutôt qu'un zéro
/// qui passerait pour une mesure.
///
/// La valeur **n'est pas plafonnée à 1** : un moteur qui dépasse son budget
/// doit pouvoir dire 130 %, c'est même toute l'information de la tuile. Elle
/// est en revanche bornée en bas à 0 — une durée est positive, mais la borne
/// tient la promesse du type.
pub fn charge_cpu(timing: &TimingSnapshot) -> Option<f32> {
    if timing.budget_ns == 0 {
        return None;
    }
    Some((timing.avg_ns as f32 / timing.budget_ns as f32).max(0.0))
}

// --- Décisions : le tableau des nœuds ---------------------------------------

/// Une ligne du tableau des nœuds.
#[derive(Debug, Clone, PartialEq)]
pub struct Ligne {
    /// Libellé du nœud correspondant, ou l'identifiant du périphérique si le
    /// nœud a disparu du miroir.
    pub libelle: String,
    /// État du périphérique.
    pub etat: Text,
    /// Latence écrite dans la colonne (voir [`latence_du_noeud`]).
    pub latence: String,
    /// Remplissage du tampon, en trames.
    pub tampon: String,
    /// Part de la barre, de 0 à 1, relative au plus grand remplissage du
    /// tableau (voir [`part_de_remplissage`]).
    pub part: f32,
    /// Ratio du rééchantillonneur, à la sixième décimale.
    pub ratio: String,
}

/// Une ligne par périphérique de `status`, dans l'ordre où le démon les donne.
///
/// Le libellé se prend sur le nœud correspondant (`DeviceStatus::node` →
/// `NodeDescriptor::label`) ; un nœud absent du miroir ne fait pas paniquer,
/// la ligne se nomme alors par son identifiant de périphérique.
pub fn lignes(status: &EngineStatus, nodes: &[NodeDescriptor]) -> Vec<Ligne> {
    let plus_grand = plus_grand_remplissage(&status.devices);
    status
        .devices
        .iter()
        .map(|device| Ligne {
            libelle: nodes
                .iter()
                .find(|n| n.id == device.node)
                .map_or_else(|| device.id.to_string(), |n| n.label.clone()),
            etat: libelle_d_etat(device.state),
            latence: latence_du_noeud(device.state, status),
            tampon: format::trames(device.fill),
            part: part_de_remplissage(device.fill, plus_grand),
            ratio: format::ratio_reechantillonnage(device.ratio),
        })
        .collect()
}

/// Le libellé de l'état d'un périphérique.
pub fn libelle_d_etat(etat: NodeState) -> Text {
    match etat {
        NodeState::Internal => Text::NodeStateInternal,
        NodeState::Active => Text::NodeStateActive,
        NodeState::Driver => Text::NodeStateDriver,
        NodeState::Suspended => Text::NodeStateSuspended,
    }
}

/// La latence à écrire pour un périphérique dans cet état.
///
/// **Le protocole ne mesure aucune latence par nœud** : [`DeviceStatus`] n'en
/// porte pas. Ce qui est écrit ici est la latence estimée **du moteur** —
/// deux quanta —, la même sur toutes les lignes ; un périphérique suspendu, qui
/// ne tourne pas, n'en a aucune et prend le tiret cadratin. Inventer une
/// latence par nœud à partir de son remplissage de tampon serait une mesure
/// imaginaire.
pub fn latence_du_noeud(etat: NodeState, status: &EngineStatus) -> String {
    match etat {
        NodeState::Active | NodeState::Driver => {
            format::latence_estimee(status.quantum.get() as u32, status.sample_rate.hz())
        }
        NodeState::Internal | NodeState::Suspended => inconnu(Text::UnitMs),
    }
}

/// Le plus grand remplissage du tableau, qui sert d'échelle aux barres.
pub fn plus_grand_remplissage(devices: &[DeviceStatus]) -> u32 {
    devices.iter().map(|d| d.fill).max().unwrap_or(0)
}

/// La part d'une barre de remplissage : `fill` rapporté au plus grand
/// remplissage du tableau, de 0 à 1.
///
/// **Ce n'est pas un pourcentage de tampon** : le protocole n'expose aucune
/// capacité, il n'existe donc aucun dénominateur absolu. La barre ne compare
/// que les périphériques entre eux, ce que dit la note du tableau. Un tableau
/// dont tous les remplissages sont nuls n'a aucune barre.
pub fn part_de_remplissage(fill: u32, plus_grand: u32) -> f32 {
    if plus_grand == 0 {
        return 0.0;
    }
    (fill as f32 / plus_grand as f32).clamp(0.0, 1.0)
}

// --- Décisions : le choix du pilote -----------------------------------------

/// Un choix de pilote de graphe, tel que la liste déroulante l'écrit.
///
/// L'enveloppe existe pour porter le `Display` qu'`iced` exige d'une option de
/// `pick_list` : [`DriverChoice`] est un type du protocole, il n'a pas à
/// connaître les mots de l'interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoixPilote(pub DriverChoice);

impl std::fmt::Display for ChoixPilote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            DriverChoice::Auto => f.write_str(i18n::t(Text::DriverAuto)),
            DriverChoice::Internal => f.write_str(i18n::t(Text::DriverInternal)),
            // Un identifiant de périphérique est une donnée du système, pas
            // un texte d'interface : il s'écrit tel quel.
            DriverChoice::Device { id } => write!(f, "{id}"),
        }
    }
}

/// Les choix offerts par la liste déroulante : automatique, horloge interne,
/// puis un périphérique par périphérique ouvert du moteur.
///
/// La liste vient de `status.devices` et de rien d'autre : proposer un
/// périphérique que le moteur n'a pas ouvert serait proposer un pilote que le
/// démon refuserait.
pub fn choix_de_pilote(status: &EngineStatus) -> Vec<ChoixPilote> {
    let mut choix = vec![
        ChoixPilote(DriverChoice::Auto),
        ChoixPilote(DriverChoice::Internal),
    ];
    choix.extend(
        status
            .devices
            .iter()
            .map(|d| ChoixPilote(DriverChoice::Device { id: d.id.clone() })),
    );
    choix
}

/// Le pilote de plateforme décrit en une phrase, selon le système compilé.
///
/// Windows a un pilote noyau, macOS un plugin HAL, Linux ni l'un ni l'autre —
/// comme [`crate::shell::pilote_plateforme`], dont ceci est la version longue.
pub fn description_de_plateforme() -> &'static str {
    i18n::t(if cfg!(target_os = "windows") {
        Text::DiagPlatformWindows
    } else if cfg!(target_os = "macos") {
        Text::DiagPlatformMacos
    } else {
        Text::DiagPlatformLinux
    })
}

// --- Décisions : le fichier de rapport --------------------------------------

/// Nombre de secondes dans un jour.
const SECONDES_PAR_JOUR: i64 = 86_400;

/// La date civile UTC d'un instant : `(année, mois, jour)`.
///
/// Le dépôt n'a ni `chrono` ni `jiff`, et n'en veut pas pour dater un nom de
/// fichier : la conversion est l'algorithme du jour julien proleptique
/// grégorien (H. Hinnant, *`civil_from_days`*), qui vaut pour toute date, avant
/// comme après 1970, et traite les années bissextiles séculaires sans cas
/// particulier.
///
/// En **UTC**, sans exception : le fuseau de l'utilisateur n'est pas accessible
/// sans dépendance, et un rapport daté d'un fuseau tu serait pire qu'un rapport
/// daté d'UTC.
pub fn date_utc(instant: SystemTime) -> (i64, u32, u32) {
    let secondes = match instant.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(depuis) => depuis.as_secs() as i64,
        // Un instant antérieur à 1970 : l'écart est rendu positif par
        // `duration()`, la division euclidienne fait le reste.
        Err(avant) => -(avant.duration().as_secs() as i64),
    };
    jour_civil(secondes.div_euclid(SECONDES_PAR_JOUR))
}

/// La date civile d'un nombre de jours depuis le 1er janvier 1970.
///
/// L'algorithme décale l'origine au 1er mars de l'an 0 : mars devient le
/// premier mois, ce qui met le 29 février en fin d'année et supprime tout cas
/// particulier de bissextile.
fn jour_civil(jours: i64) -> (i64, u32, u32) {
    // 719 468 = jours du 1er mars 0000 au 1er janvier 1970.
    let z = jours + 719_468;
    let ere = if z >= 0 { z } else { z - 146_096 } / 146_097;
    // Jour dans l'ère de 400 ans, qui compte 146 097 jours.
    let jour_d_ere = z - ere * 146_097;
    let an_d_ere =
        (jour_d_ere - jour_d_ere / 1_460 + jour_d_ere / 36_524 - jour_d_ere / 146_096) / 365;
    let annee = an_d_ere + ere * 400;
    let jour_d_an = jour_d_ere - (365 * an_d_ere + an_d_ere / 4 - an_d_ere / 100);
    let mois_decale = (5 * jour_d_an + 2) / 153;
    let jour = (jour_d_an - (153 * mois_decale + 2) / 5 + 1) as u32;
    let mois = if mois_decale < 10 {
        mois_decale + 3
    } else {
        mois_decale - 9
    } as u32;
    (if mois <= 2 { annee + 1 } else { annee }, mois, jour)
}

/// Nom du fichier de rapport pour un instant : `conduit-diagnostic-AAAA-MM-JJ.txt`.
///
/// Le nom ne se traduit pas — un rapport changerait sinon de nom d'une langue
/// à l'autre — et la date est en UTC (voir [`date_utc`]). Deux rapports du même jour se remplacent : c'est
/// voulu, un rapport est une photographie de l'instant, pas une archive.
pub fn nom_du_rapport(instant: SystemTime) -> String {
    let (annee, mois, jour) = date_utc(instant);
    format!("{PREFIXE_RAPPORT}{annee:04}-{mois:02}-{jour:02}{EXTENSION_RAPPORT}")
}

/// Répertoire où écrire le rapport : les **Documents** de l'utilisateur, à
/// défaut le répertoire de données de Conduit.
///
/// Le rapport est un fichier que l'utilisateur envoie ; il va donc là où il le
/// retrouvera. Un système sans répertoire Documents — un compte de service —
/// retombe sur le répertoire de données, et l'absence des deux est une erreur
/// annoncée, pas un silence.
pub fn repertoire_du_rapport() -> Option<PathBuf> {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.document_dir().map(PathBuf::from))
        .or_else(|| {
            directories::ProjectDirs::from("", "", "conduit")
                .map(|dirs| dirs.data_dir().to_path_buf())
        })
}

/// Chemin complet du rapport pour un instant donné.
pub fn chemin_du_rapport(instant: SystemTime) -> Option<PathBuf> {
    repertoire_du_rapport().map(|dir| dir.join(nom_du_rapport(instant)))
}

// --- Composition ------------------------------------------------------------

/// Compose la vue. `enabled` est faux tant que le démon ne répond pas : les
/// actions sont alors grisées plutôt que perdues.
///
/// `graisse` est la graisse du texte courant du mode
/// ([`crate::theme::Jetons::graisse_texte`]).
pub fn view<'a>(mirror: &'a Mirror, enabled: bool, graisse: Weight) -> Element<'a, Message> {
    let Some(status) = &mirror.status else {
        return attente(graisse);
    };
    column![
        tuiles_en_ligne(
            &tuiles(status, mirror.xruns),
            mirror.xruns,
            enabled,
            graisse
        ),
        space::vertical().height(ESPACE_XL),
        row![
            tableau(status, &mirror.nodes, graisse),
            space::horizontal().width(ECART_MOTEUR),
            moteur(status, enabled, graisse),
        ]
        .width(Fill)
        .height(Fill),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

/// Ce que la vue montre tant que le miroir n'est pas chargé.
fn attente<'a>(graisse: Weight) -> Element<'a, Message> {
    column![
        text(i18n::t(Text::Waiting))
            .size(CORPS_INTERFACE)
            .font(typo::texte_a(graisse, CORPS_INTERFACE))
            .style(style::texte_en(|j| j.texte_2)),
        space::vertical(),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

/// Les quatre tuiles, côte à côte et de largeur égale.
fn tuiles_en_ligne<'a>(
    tuiles: &[Tuile; 4],
    xruns: u64,
    enabled: bool,
    graisse: Weight,
) -> Element<'a, Message> {
    let mut ligne = row![].spacing(ESPACE_L).width(Fill);
    for (index, t) in tuiles.iter().enumerate() {
        // Seule la première tuile porte une action : remettre les compteurs à
        // zéro n'a de sens que s'il y a quelque chose à remettre.
        let action = index == 0 && xruns > 0;
        ligne = ligne.push(tuile(t, action, enabled, graisse));
    }
    ligne.into()
}

/// Une tuile : surtitre en petites capitales, valeur en display, note.
fn tuile<'a>(tuile: &Tuile, action: bool, enabled: bool, graisse: Weight) -> Element<'a, Message> {
    let mut contenu = column![
        container(typo::petites_capitales(
            i18n::t(tuile.surtitre),
            CORPS_SURTITRE
        ))
        .style(style::encre_de(|j| j.texte_2_carte)),
        space::vertical().height(ESPACE_S),
        text(tuile.valeur.clone())
            .size(CORPS_TUILE)
            .line_height(LineHeight::Relative(INTERLIGNE_SERRE))
            .font(typo::display(CORPS_TUILE))
            .style(style::texte_en(|j| j.titre)),
        space::vertical().height(ESPACE_XS),
        note(tuile.note.clone(), graisse),
    ]
    .width(Fill);
    if action {
        contenu = contenu.push(space::vertical().height(ESPACE_XS));
        contenu = contenu.push(
            button(
                text(i18n::t(Text::DiagXrunsReset))
                    .size(CORPS_META)
                    .font(typo::texte_a(graisse, CORPS_META)),
            )
            .padding(0)
            .on_press_maybe(enabled.then_some(Message::ReinitialiserXruns))
            .style(style::lien),
        );
    }
    container(contenu)
        .padding(PADDING_TUILE)
        .width(Fill)
        .height(HAUTEUR_TUILE)
        .style(style::carte)
        .into()
}

/// Une note de tuile : corps 12, texte secondaire **des surfaces posées** —
/// une tuile est une carte, le texte secondaire du fond n'y passerait que
/// 4,3:1 en mode clair.
fn note<'a>(contenu: String, graisse: Weight) -> iced::widget::Text<'a> {
    text(contenu)
        .size(CORPS_META)
        .font(typo::texte_a(graisse, CORPS_META))
        .style(style::texte_en(|j| j.texte_2_carte))
}

/// Une note posée sur le fond de la fenêtre : même corps, mais le texte
/// secondaire de la surface.
fn note_de_surface<'a>(contenu: String, graisse: Weight) -> iced::widget::Text<'a> {
    text(contenu)
        .size(CORPS_META)
        .font(typo::texte_a(graisse, CORPS_META))
        .style(style::texte_en(|j| j.texte_2))
}

/// Le tableau des nœuds : en-têtes, une ligne par périphérique, puis les deux
/// notes qui disent ce que les colonnes ne mesurent pas.
///
/// Les lignes sont séparées par un filet et **ne sont pas posées** sur une
/// surface, contrairement à la vue Câbles : c'est le dessin de la maquette.
fn tableau<'a>(
    status: &EngineStatus,
    nodes: &'a [NodeDescriptor],
    graisse: Weight,
) -> Element<'a, Message> {
    let table = lignes(status, nodes);
    let mut corps = column![].width(Fill);
    if table.is_empty() {
        corps = corps.push(
            container(
                text(i18n::t(Text::DiagNoDevice))
                    .size(CORPS_INTERFACE)
                    .font(typo::texte_a(graisse, CORPS_INTERFACE))
                    .style(style::texte_en(|j| j.texte_2)),
            )
            .padding(PADDING_LIGNE),
        );
    }
    for (index, ligne) in table.iter().enumerate() {
        if index > 0 {
            corps = corps.push(rule::horizontal(FILET).style(style::filet));
        }
        corps = corps.push(ligne_du_tableau(ligne, graisse));
    }
    column![
        entetes(),
        rule::horizontal(FILET).style(style::filet),
        scrollable(corps)
            .width(Fill)
            .height(Fill)
            .style(style::defilement),
        space::vertical().height(ESPACE_M),
        note_du_tableau(Text::DiagLatencyNote, graisse),
        space::vertical().height(ESPACE_XS),
        note_du_tableau(Text::DiagBufferNote, graisse),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

/// Une note sous le tableau, sur toute sa largeur.
fn note_du_tableau<'a>(quoi: Text, graisse: Weight) -> Element<'a, Message> {
    note_de_surface(i18n::t(quoi).to_string(), graisse)
        .width(Fill)
        .into()
}

/// La ligne d'en-têtes de colonnes, en petites capitales.
fn entetes<'a>() -> Element<'a, Message> {
    container(
        row![
            entete(Text::ColumnNode, Length::Fill),
            entete(Text::ColumnState, Length::Fixed(COL_ETAT)),
            entete(Text::ColumnLatency, Length::Fixed(COL_LATENCE)),
            entete(Text::ColumnBuffer, Length::Fixed(COL_TAMPON)),
            entete(Text::ColumnRatio, Length::Fixed(COL_RATIO)),
        ]
        .spacing(ECART_COLONNES)
        .width(Fill),
    )
    .padding(Padding::ZERO.bottom(ESPACE_S))
    .into()
}

/// Un en-tête de colonne : capitales espacées, corps 11, texte secondaire.
fn entete<'a>(libelle: Text, largeur: Length) -> Element<'a, Message> {
    container(typo::petites_capitales(i18n::t(libelle), CORPS_ENTETE))
        .width(largeur)
        .style(style::encre_de(|j| j.texte_2))
        .into()
}

/// Une ligne du tableau : les cinq colonnes, les trois dernières alignées à
/// droite comme toute colonne de chiffres (`docs/design-system.md` § 6.2).
fn ligne_du_tableau<'a>(ligne: &Ligne, graisse: Weight) -> Element<'a, Message> {
    let cellule = move |contenu: String, largeur: f32| {
        container(
            text(contenu)
                .size(CORPS_INTERFACE)
                .font(typo::interface_graisse(graisse))
                .style(style::texte_en(|j| j.texte))
                .width(Fill)
                .align_x(Right),
        )
        .width(largeur)
    };
    container(
        row![
            container(
                text(ligne.libelle.clone())
                    .size(CORPS_INTERFACE)
                    .font(typo::texte_a(graisse, CORPS_INTERFACE))
                    .style(style::texte_en(|j| j.titre))
            )
            .width(Fill),
            container(
                text(i18n::t(ligne.etat))
                    .size(CORPS_META)
                    .font(typo::texte_a(graisse, CORPS_META))
                    .style(style::texte_en(|j| j.texte_2))
            )
            .width(COL_ETAT),
            cellule(ligne.latence.clone(), COL_LATENCE),
            tampon(ligne, graisse),
            cellule(ligne.ratio.clone(), COL_RATIO),
        ]
        .spacing(ECART_COLONNES)
        .width(Fill),
    )
    .padding(PADDING_LIGNE)
    .width(Fill)
    .into()
}

/// La colonne « Tampon » : la valeur en trames et, dessous, la barre relative.
fn tampon<'a>(ligne: &Ligne, graisse: Weight) -> Element<'a, Message> {
    container(
        column![
            text(ligne.tampon.clone())
                .size(CORPS_INTERFACE)
                .font(typo::interface_graisse(graisse))
                .style(style::texte_en(|j| j.texte))
                .width(Fill)
                .align_x(Right),
            barre(ligne.part),
        ]
        .spacing(ESPACE_XS)
        .width(Fill),
    )
    .width(COL_TAMPON)
    .into()
}

/// Une barre : un rail creusé, rempli d'or sur la part donnée.
fn barre<'a>(part: f32) -> Element<'a, Message> {
    let rempli = (part.clamp(0.0, 1.0) * f32::from(PARTS_BARRE)) as u16;
    let mut rail = row![].width(Fill).height(Fill);
    if rempli > 0 {
        rail = rail.push(
            container(space::horizontal())
                .width(Length::FillPortion(rempli))
                .height(Fill)
                .style(style::barre_remplie),
        );
    }
    if rempli < PARTS_BARRE {
        rail = rail.push(space::horizontal().width(Length::FillPortion(PARTS_BARRE - rempli)));
    }
    container(rail)
        .width(Fill)
        .height(HAUTEUR_BARRE)
        .style(style::barre_rail)
        .into()
}

/// La colonne « Moteur » : le pilote de graphe, le quantum et la fréquence,
/// puis le pilote de plateforme sous un filet d'or.
fn moteur<'a>(status: &EngineStatus, enabled: bool, graisse: Weight) -> Element<'a, Message> {
    column![
        surtitre(Text::DiagEngine),
        space::vertical().height(ESPACE_M),
        liste_des_pilotes(status, enabled),
        space::vertical().height(ESPACE_XS),
        note_de_surface(i18n::t(Text::DiagDriverHot).to_string(), graisse),
        space::vertical().height(ESPACE_L),
        lecture_seule(
            Text::DiagQuantum,
            format::entier(status.quantum.get() as u64)
        ),
        space::vertical().height(ESPACE_M),
        lecture_seule(
            Text::DiagSampleRate,
            format::kilohertz(status.sample_rate.hz())
        ),
        space::vertical().height(ESPACE_XS),
        note_de_surface(i18n::t(Text::DiagReadOnly).to_string(), graisse),
        space::vertical().height(ESPACE_L),
        rule::horizontal(FILET).style(style::filet_or),
        space::vertical().height(ESPACE_L),
        surtitre(Text::DiagPlatform),
        space::vertical().height(ESPACE_S),
        text(description_de_plateforme())
            .size(CORPS_DESCRIPTION)
            .font(typo::texte_a(graisse, CORPS_DESCRIPTION))
            .style(style::texte_en(|j| j.texte)),
        space::vertical(),
    ]
    .width(LARGEUR_MOTEUR)
    .height(Fill)
    .into()
}

/// Un surtitre de section, en petites capitales sur le fond de la fenêtre.
fn surtitre<'a>(quoi: Text) -> Element<'a, Message> {
    container(typo::petites_capitales(i18n::t(quoi), CORPS_SURTITRE))
        .style(style::encre_de(|j| j.texte_2))
        .into()
}

/// Le libellé d'une ligne de la colonne « Moteur ».
fn libelle<'a>(quoi: Text) -> Element<'a, Message> {
    text(i18n::t(quoi))
        .size(CORPS_META)
        .font(typo::interface())
        .style(style::texte_en(|j| j.texte_2))
        .into()
}

/// La liste déroulante du pilote de graphe.
///
/// `menu_style` n'est pas facultatif : la liste **ouverte** est un `overlay`
/// distinct, qui garderait sinon le thème par défaut d'`iced`.
///
/// La sélection montrée est le **choix** configuré du démon
/// ([`EngineStatus::driver_choice`]), pas le pilote effectif : « automatique »
/// doit rester lisible comme tel même quand il retombe sur l'horloge interne.
/// Rien n'est appliqué localement au clic — l'interface envoie `SetDriver` et
/// attend la relecture d'état qui suit.
fn liste_des_pilotes<'a>(status: &EngineStatus, enabled: bool) -> Element<'a, Message> {
    // `pick_list` n'a pas d'état désactivé : privée de ses options, elle ne
    // s'ouvre plus et garde sa valeur courante affichée, ce qui est
    // exactement ce qu'il faut quand le démon ne répond pas.
    let choix = if enabled {
        choix_de_pilote(status)
    } else {
        Vec::new()
    };
    let courant = ChoixPilote(status.driver_choice.clone());
    let liste = pick_list(choix, Some(courant), |choix: ChoixPilote| {
        Message::ChoisirPilote(choix.0)
    })
    .width(Fill)
    .padding(PADDING_BOITE)
    .text_size(CORPS_INTERFACE)
    .font(typo::interface())
    .style(style::liste_deroulante)
    .menu_style(style::menu_deroulant);
    column![
        libelle(Text::GraphDriver),
        space::vertical().height(ESPACE_XS),
        liste
    ]
    .width(Fill)
    .into()
}

/// Une valeur en lecture seule : la même boîte que la liste déroulante, la
/// même typographie, **sans chevron**.
///
/// Le protocole n'a ni `SetQuantum` ni `SetSampleRate` : une liste déroulante
/// inerte ferait croire à un réglage possible.
fn lecture_seule<'a>(quoi: Text, valeur: String) -> Element<'a, Message> {
    column![
        libelle(quoi),
        space::vertical().height(ESPACE_XS),
        container(
            text(valeur)
                .size(CORPS_INTERFACE)
                .font(typo::interface())
                .style(style::texte_en(|j| j.texte))
                .width(Fill),
        )
        .padding(PADDING_BOITE)
        .width(Fill)
        .style(style::carte),
    ]
    .width(Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use conduit_core::types::{Quantum, SampleRate};

    use crate::model::fixtures::{device, loaded, node, status};

    /// Un état de moteur représentatif : quatre périphériques, des temps de
    /// cycle plausibles du backend `null`.
    fn plein() -> EngineStatus {
        let mut s = status();
        s.nodes = 7;
        s.timing = TimingSnapshot {
            count: 1_000,
            min_ns: 511_000,
            avg_ns: 1_854_000,
            max_ns: 3_704_000,
            last_ns: 1_700_000,
            budget_ns: 5_333_000,
            overruns: 2,
        };
        s.devices = vec![
            DeviceStatus {
                fill: 831,
                ratio: 0.999_821,
                ..device(5, "null:haut-parleurs", NodeState::Driver)
            },
            DeviceStatus {
                fill: 960,
                ratio: 1.000_018,
                ..device(2, "null:conduit-1", NodeState::Active)
            },
            DeviceStatus {
                fill: 0,
                ratio: 1.0,
                ..device(9, "null:disparu", NodeState::Suspended)
            },
        ];
        s
    }

    /// Chaque tuile dit sa valeur et sa note, à partir du seul `EngineStatus`.
    #[test]
    fn les_quatre_tuiles_disent_ce_que_le_statut_porte() {
        let [xruns, latence, cycle, charge] = tuiles(&plein(), 3);

        assert_eq!(xruns.surtitre, Text::DiagXruns);
        assert_eq!(xruns.valeur, "3");
        assert_eq!(xruns.note, "Objectif : 0");

        assert_eq!(latence.surtitre, Text::DiagLatency);
        assert_eq!(latence.valeur, "10,7\u{a0}ms");
        assert_eq!(latence.note, "Câble → carte son");

        assert_eq!(cycle.surtitre, Text::DiagCycle);
        assert_eq!(cycle.valeur, "1,9\u{a0}ms");
        assert_eq!(
            cycle.note,
            "min 511\u{a0}µs · max 3\u{202f}704\u{a0}µs · budget 5\u{202f}333\u{a0}µs"
        );

        assert_eq!(charge.surtitre, Text::DiagLoad);
        assert_eq!(charge.valeur, "35\u{a0}%");
        assert_eq!(charge.note, "7 nœuds · un cœur");
    }

    /// Le compteur affiché est celui du miroir, pas celui du dernier `Status`.
    #[test]
    fn la_tuile_des_xruns_suit_le_compteur_du_miroir() {
        let mut s = plein();
        s.xruns = 3;
        assert_eq!(tuiles(&s, 12)[0].valeur, "12");
        assert_eq!(tuiles(&s, 0)[0].valeur, "0");
    }

    /// La charge est le temps moyen sur le budget, sans plafond à 100 %, et
    /// n'existe pas tant que le budget est nul.
    #[test]
    fn la_charge_cpu_rapporte_le_temps_moyen_au_budget() {
        let mut t = TimingSnapshot {
            budget_ns: 1_000,
            avg_ns: 250,
            ..TimingSnapshot::default()
        };
        assert_eq!(charge_cpu(&t), Some(0.25));
        t.avg_ns = 0;
        assert_eq!(charge_cpu(&t), Some(0.0));
        // Un moteur en dépassement doit pouvoir le dire.
        t.avg_ns = 1_300;
        assert_eq!(charge_cpu(&t), Some(1.3));
        assert_eq!(format::pourcentage(charge_cpu(&t).unwrap()), "130\u{a0}%");
        // Budget nul : rien à diviser, la tuile écrit un tiret.
        t.budget_ns = 0;
        assert_eq!(charge_cpu(&t), None);
        let mut s = plein();
        s.timing = t;
        assert_eq!(tuiles(&s, 0)[3].valeur, "—\u{a0}%");
    }

    /// Une ligne par périphérique, nommée par son nœud.
    #[test]
    fn les_lignes_joignent_les_peripheriques_a_leurs_noeuds() {
        let nodes = vec![node(5, "Haut-parleurs"), node(2, "Conduit 1")];
        let table = lignes(&plein(), &nodes);
        assert_eq!(table.len(), 3);
        assert_eq!(table[0].libelle, "Haut-parleurs");
        assert_eq!(table[1].libelle, "Conduit 1");
        // Nœud absent du miroir : la ligne se nomme par son périphérique, et
        // rien ne panique.
        assert_eq!(table[2].libelle, "null:disparu");
        assert_eq!(table[0].etat, Text::NodeStateDriver);
        assert_eq!(table[1].etat, Text::NodeStateActive);
        assert_eq!(table[2].etat, Text::NodeStateSuspended);
        // Sans aucun nœud connu, toutes les lignes se nomment ainsi.
        let orphelines = lignes(&plein(), &[]);
        assert_eq!(orphelines[0].libelle, "null:haut-parleurs");
    }

    /// Le remplissage s'écrit en trames et le ratio à la sixième décimale.
    #[test]
    fn les_colonnes_ecrivent_des_trames_et_six_decimales() {
        let table = lignes(&plein(), &[]);
        assert_eq!(table[0].tampon, "831\u{a0}trames");
        assert_eq!(table[0].ratio, "0,999821");
        assert_eq!(table[1].ratio, "1,000018");
        assert_eq!(table[2].tampon, "0\u{a0}trame");
        for ligne in &table {
            assert!(!ligne.tampon.contains('%'), "un pourcentage inventé");
        }
    }

    /// La latence est celle du moteur, la même partout ; un périphérique
    /// suspendu n'en a aucune.
    #[test]
    fn la_latence_est_celle_du_moteur_et_non_du_noeud() {
        let s = plein();
        let table = lignes(&s, &[]);
        assert_eq!(table[0].latence, "10,7\u{a0}ms");
        assert_eq!(table[1].latence, "10,7\u{a0}ms");
        assert_eq!(table[2].latence, "—\u{a0}ms");
        assert_eq!(table[0].latence, table[1].latence);
        // Elle suit le quantum du moteur, seule grandeur qui la détermine.
        let mut court = s.clone();
        court.quantum = Quantum::new(128).expect("quantum valide");
        assert_eq!(lignes(&court, &[])[0].latence, "5,3\u{a0}ms");
        assert_eq!(
            latence_du_noeud(NodeState::Internal, &s),
            latence_du_noeud(NodeState::Suspended, &s)
        );
    }

    /// La barre est relative au plus grand remplissage du tableau.
    #[test]
    fn la_barre_est_relative_au_plus_grand_remplissage() {
        let table = lignes(&plein(), &[]);
        // 960 est le plus grand : sa barre est pleine.
        assert_eq!(table[1].part, 1.0);
        assert!((table[0].part - 831.0 / 960.0).abs() < 1e-6);
        assert_eq!(table[2].part, 0.0);
        assert_eq!(part_de_remplissage(0, 0), 0.0);
        assert_eq!(part_de_remplissage(480, 480), 1.0);
        // Tous les remplissages nuls : aucune barre, et pas de division par
        // zéro.
        let mut vides = plein();
        for d in &mut vides.devices {
            d.fill = 0;
        }
        assert_eq!(plus_grand_remplissage(&vides.devices), 0);
        assert!(lignes(&vides, &[]).iter().all(|l| l.part == 0.0));
    }

    /// Les choix de pilote se construisent depuis les périphériques du moteur.
    #[test]
    fn les_choix_de_pilote_viennent_des_peripheriques() {
        let choix = choix_de_pilote(&plein());
        assert_eq!(choix.len(), 5);
        assert_eq!(choix[0], ChoixPilote(DriverChoice::Auto));
        assert_eq!(choix[1], ChoixPilote(DriverChoice::Internal));
        assert_eq!(
            choix[2..]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["null:haut-parleurs", "null:conduit-1", "null:disparu"]
        );
        assert_eq!(choix[0].to_string(), "automatique");
        assert_eq!(choix[1].to_string(), "horloge interne");
        // Sans périphérique, les deux choix intemporels restent.
        let mut sans = plein();
        sans.devices.clear();
        assert_eq!(choix_de_pilote(&sans).len(), 2);
    }

    /// La description du pilote de plateforme est celle du système compilé.
    #[test]
    fn la_description_de_plateforme_suit_le_systeme() {
        let attendue = description_de_plateforme();
        if cfg!(target_os = "windows") {
            assert!(attendue.contains("pilote noyau"));
        } else if cfg!(target_os = "macos") {
            assert!(attendue.contains("HAL"));
        } else {
            assert!(attendue.contains("PipeWire"));
        }
    }

    /// L'instant se convertit en date civile UTC, bissextiles comprises.
    #[test]
    fn la_date_civile_se_calcule_sans_dependance() {
        let a = |secondes: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(secondes);
        assert_eq!(date_utc(SystemTime::UNIX_EPOCH), (1970, 1, 1));
        // 2024-02-29 : une bissextile ordinaire.
        assert_eq!(date_utc(a(1_709_164_800)), (2024, 2, 29));
        // 2024-12-31, à la dernière seconde de l'année.
        assert_eq!(date_utc(a(1_735_689_599)), (2024, 12, 31));
        assert_eq!(date_utc(a(1_735_689_600)), (2025, 1, 1));
        // 2000-02-29 : séculaire **et** bissextile (divisible par 400).
        assert_eq!(date_utc(a(951_782_400)), (2000, 2, 29));
        // 1900 ne l'était pas : le 1900-02-28 est suivi du 1900-03-01.
        assert_eq!(
            date_utc(SystemTime::UNIX_EPOCH - Duration::from_secs(2_203_977_600)),
            (1900, 2, 28)
        );
        assert_eq!(
            date_utc(SystemTime::UNIX_EPOCH - Duration::from_secs(2_203_891_200)),
            (1900, 3, 1)
        );
        // Le jour de ce commit.
        assert_eq!(date_utc(a(1_788_652_800)), (2026, 9, 6));
    }

    /// Le nom du fichier porte la date, sur deux chiffres, et ne se traduit
    /// pas.
    #[test]
    fn le_nom_du_rapport_est_date() {
        let a = |secondes: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(secondes);
        assert_eq!(
            nom_du_rapport(a(1_788_652_800)),
            "conduit-diagnostic-2026-09-06.txt"
        );
        assert_eq!(
            nom_du_rapport(a(1_709_164_800)),
            "conduit-diagnostic-2024-02-29.txt"
        );
        assert_eq!(
            nom_du_rapport(SystemTime::UNIX_EPOCH),
            "conduit-diagnostic-1970-01-01.txt"
        );
        // Le chemin, s'il existe, se termine par ce nom.
        if let Some(chemin) = chemin_du_rapport(a(1_788_652_800)) {
            assert!(chemin.ends_with("conduit-diagnostic-2026-09-06.txt"));
        }
    }

    /// La vue se compose sans miroir chargé : elle dit l'attente et ne
    /// panique pas.
    #[test]
    fn la_vue_ne_panique_pas_sans_miroir() {
        let vide = Mirror::default();
        let _ = view(&vide, false, Weight::Normal);
        // Un miroir chargé, mais sans périphérique ni temps de cycle : le cas
        // du démon qui vient de démarrer.
        let charge = loaded();
        let _ = view(&charge, true, Weight::Normal);
        let s = charge.status.as_ref().expect("miroir chargé");
        assert!(lignes(s, &charge.nodes).is_empty());
        assert_eq!(charge_cpu(&s.timing), None);
    }

    /// Le libellé d'état couvre les quatre cas du protocole.
    #[test]
    fn les_quatre_etats_ont_leur_libelle() {
        let cas = [
            (NodeState::Internal, "Interne"),
            (NodeState::Active, "Actif"),
            (NodeState::Driver, "Pilote"),
            (NodeState::Suspended, "Suspendu"),
        ];
        for (etat, attendu) in cas {
            assert_eq!(i18n::t(libelle_d_etat(etat)), attendu);
        }
    }

    /// La fréquence d'échantillonnage se lit dans la colonne « Moteur ».
    #[test]
    fn le_quantum_et_la_frequence_sont_ceux_du_moteur() {
        let mut s = plein();
        s.sample_rate = SampleRate::HZ_44100;
        assert_eq!(format::kilohertz(s.sample_rate.hz()), "44,1\u{a0}kHz");
        assert_eq!(format::entier(s.quantum.get() as u64), "256");
    }
}
