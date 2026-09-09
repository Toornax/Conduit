//! Le protocole **interne** du canal nommé, entre le démon et le service (M1b-20).
//!
//! Fixe, versionné, à trames bornées et longueur préfixée. Tout ce fichier est **pur** :
//! aucun appel au système, aucune allocation hors des `Vec` de sortie, aucune panique.
//! C'est la partie qui se vérifie sans machine virtuelle, et celle que M1b-08 voudra
//! fuzzer.
//!
//! # Ce n'est pas le protocole de `conduit-protocol`
//!
//! `conduit-protocol` est le protocole **utilisateur**, entre le démon et `conduitctl` :
//! MessagePack, extensible, avec négociation de capacités. Celui-ci est autre chose —
//! sept ordres, deux structures presque toujours de taille fixe, aucune extensibilité,
//! et un serveur qui tourne en `LocalSystem`. Les mélanger ferait entrer un décodeur
//! générique dans le processus le plus privilégié du produit ; on écrit donc le strict
//! nécessaire, et on le refuse dès qu'il n'a pas exactement la forme attendue.
//!
//! # Boutisme : petit-boutiste, et c'est un choix
//!
//! [`conduit_kmd_core::config::CableState`] est en boutisme **natif** parce qu'elle ne
//! traverse qu'une frontière mémoire (KS recopie un tampon d'un processus à l'autre sur
//! la même machine). Les trames de ce module traversent une frontière de **format** —
//! un canal, un flux d'octets, quelque chose qu'on peut relire dans un journal ou
//! rejouer — donc petit-boutiste explicite, comme le `u32` d'en-tête de
//! `conduit_protocol::framing`.
//!
//! # La règle du parseur : la longueur est exacte, les champs inutilisés sont nuls
//!
//! Une requête fait [`TAILLE_REQUETE`] octets, une réponse [`TAILLE_REPONSE`], et **rien
//! d'autre n'est accepté** : ni plus court, ni plus long, ni un préfixe valide suivi
//! d'octets en trop. C'est la règle de [`conduit_kmd_core::config::CableState::from_bytes`],
//! pour les mêmes raisons — accepter un préfixe rend le format non extensible et c'est le
//! premier écart qu'un fuzzer trouve.
//!
//! Elle va plus loin ici : les champs qu'un ordre **n'utilise pas** sont exigés nuls
//! ([`ErreurRequete::ChampParasite`]). Un `Lister` qui porterait un numéro de câble est
//! un client qui s'est trompé d'ordre ou un octet retourné en transit ; le laisser passer
//! reviendrait à servir une requête qu'on n'a pas comprise.
//!
//! # L'exception : `renommer` porte une chaîne, et c'est la seule (M1b-21)
//!
//! [`Requete::Renommer`] est le **premier** ordre de ce protocole à porter une charge de
//! longueur variable. La règle ne s'assouplit pas pour autant, elle se précise :
//!
//! - la longueur exigée devient *par ordre* — [`TAILLE_REQUETE`] pile pour les six
//!   autres, `TAILLE_REQUETE + n` pour `renommer`, `n` dans `1..=`[`MAX_NOM_OCTETS`] ;
//! - `n` est un **nombre d'octets**, borné par la borne du dépôt
//!   ([`conduit_backend::cable::MAX_CABLE_NAME_LEN`] caractères, donc quatre fois plus
//!   d'octets au pire en UTF-8) ; la longueur annoncée est confrontée à
//!   [`MAX_TRAME_OCTETS`] **avant** toute allocation, comme avant ;
//! - les octets du nom doivent être de l'UTF-8 valide, sans quoi la trame est refusée —
//!   on ne remplace jamais un octet douteux par `U+FFFD`, ce qui reviendrait à écrire
//!   dans le registre un nom que le client n'a pas demandé ;
//! - les octets **en trop après un nom valide** n'existent pas : tout ce qui suit
//!   l'en-tête *est* le nom, et sa longueur est celle de la trame. Un client qui ajoute
//!   un `\0` de fin voit donc ce `\0` refusé par [`valider_nom`], caractère de contrôle.
//!
//! Les **règles** du nom (longueur en caractères, caractères de contrôle, `/\:*?"<>|`)
//! ne sont pas réécrites ici : c'est [`conduit_backend::cable::validate_cable_name`] qui
//! tranche, le même juge que `conduitctl` et que `CableSpec`. Ce module ne fait
//! qu'appliquer les bornes du **format**.
//!
//! # Ce module alloue, désormais, et exactement une fois
//!
//! Un nom est possédé ([`Requete::Renommer::nom`]) : analyser une trame `renommer`
//! alloue une `String` d'au plus [`MAX_NOM_OCTETS`] octets, après que la borne a été
//! vérifiée. C'est la seule allocation d'analyse du module, et [`Requete`] n'est donc
//! plus `Copy`.

use conduit_backend::cable::{validate_cable_name, MAX_CABLE_NAME_LEN};
use conduit_backend::CableId;
use conduit_kmd_core::config::CABLE_MAX;
use conduit_kmd_core::params::{DEFAULT_CHANNELS, MAX_CHANNELS, MIN_CHANNELS};
use core::fmt;

// ---------------------------------------------------------------------------------
// Les constantes du format.
// ---------------------------------------------------------------------------------

/// Version de **ce** protocole (démon ↔ service), à ne pas confondre avec
/// [`conduit_kmd_core::config::CONFIG_VERSION`], celle du contrat KS (service ↔ pilote).
///
/// Les deux se déplacent indépendamment : on peut changer la forme d'une trame sans
/// toucher au pilote, et l'inverse. Un client d'une autre version reçoit
/// [`Statut::VersionInconnue`] et la version servie dans [`Reponse::detail`] — un refus
/// qui **dit quoi faire**, là où une trame mal comprise donnerait un octet aberrant.
///
/// # Pourquoi 2
///
/// La version 1 servait cinq ordres et n'acceptait qu'une requête de huit octets.
/// **M1b-21** en ajoute deux ([`ORDRE_RENOMMER`], [`ORDRE_NOM_DEFAUT`]) et ouvre la
/// requête à une charge de longueur variable : les deux changements sont observables par
/// un client, et l'en-tête de module de la version 1 disait déjà « cinq ordres, et jamais
/// un sixième sans changer `PROTOCOLE_VERSION` ».
///
/// Le service et le démon sont livrés ensemble et se déplacent ensemble : un démon v1
/// devant un service v2 reçoit [`Statut::VersionInconnue`] et un message qui dit de les
/// réinstaller tous les deux, ce qui vaut mieux qu'un `renommer` compris de travers.
pub const PROTOCOLE_VERSION: u8 = 2;

/// Taille de l'en-tête de trame : un `u32` petit-boutiste, la longueur de la charge.
pub const EN_TETE_OCTETS: usize = 4;

/// Taille de l'**en-tête** d'une requête : 8 octets, et la requête entière pour les six
/// ordres qui ne portent pas de nom.
///
/// Seul [`Requete::Renommer`] fait suivre ces huit octets d'une charge : voir
/// [`MAX_NOM_OCTETS`] et [`TAILLE_REQUETE_MAX`].
pub const TAILLE_REQUETE: usize = 8;

/// Longueur maximale, en **octets**, du nom que porte [`Requete::Renommer`].
///
/// Le domaine vient de [`conduit_backend::cable::MAX_CABLE_NAME_LEN`], qui compte des
/// **caractères** ; un point de code UTF-8 en coûte jusqu'à quatre, d'où le facteur. La
/// borne du format est donc légèrement plus large que la règle du nom, et c'est voulu :
/// un nom de 64 caractères accentués doit passer le cadrage pour que
/// [`validate_cable_name`] puisse le juger sur ses mérites, plutôt que d'être refusé
/// pour une raison — sa taille en octets — que l'utilisateur ne peut pas voir.
pub const MAX_NOM_OCTETS: usize = MAX_CABLE_NAME_LEN * 4;

/// Taille de la plus longue requête : l'en-tête plus le plus long nom.
pub const TAILLE_REQUETE_MAX: usize = TAILLE_REQUETE + MAX_NOM_OCTETS;

/// Taille d'une réponse : **28 octets**, toujours.
///
/// Une réponse de taille fixe est la propriété de sécurité la moins chère qui soit : le
/// serveur n'a jamais à décider d'une longueur à partir de ce qu'il a reçu.
pub const TAILLE_REPONSE: usize = 28;

/// Charge utile maximale acceptée avant toute allocation : la plus longue requête.
///
/// **Calculée, pas choisie** : c'est exactement [`TAILLE_REQUETE_MAX`], donc la borne
/// suit `MAX_CABLE_NAME_LEN` si celle-ci bouge, et un client hostile ne peut faire
/// réserver au service `LocalSystem` que quelques centaines d'octets. La longueur
/// annoncée est confrontée à cette borne **avant** que le moindre tampon ne soit alloué
/// ([`longueur_annoncee`]).
pub const MAX_TRAME_OCTETS: usize = TAILLE_REQUETE_MAX;

// Les deux structures tiennent dans la borne, avec de la marge.
const _: () = assert!(TAILLE_REQUETE <= MAX_TRAME_OCTETS);
const _: () = assert!(TAILLE_REPONSE <= MAX_TRAME_OCTETS);
// La borne du format laisse passer le plus long nom que la règle du dépôt accepte.
const _: () = assert!(MAX_NOM_OCTETS >= MAX_CABLE_NAME_LEN);

/// Nom du canal nommé du service.
///
/// Fixe et sans numéro de version : c'est le protocole qui porte la sienne, et un client
/// d'une autre version doit pouvoir obtenir un refus explicite plutôt qu'un
/// `ERROR_FILE_NOT_FOUND` qui ressemblerait à « le service n'est pas installé ».
pub const NOM_TUBE: &str = r"\\.\pipe\conduit-helper";

// ---------------------------------------------------------------------------------
// Les ordres.
// ---------------------------------------------------------------------------------

/// Code de l'ordre `version`, dans la trame.
pub const ORDRE_VERSION: u8 = 0;
/// Code de l'ordre `lister`.
pub const ORDRE_LISTER: u8 = 1;
/// Code de l'ordre `activer`.
pub const ORDRE_ACTIVER: u8 = 2;
/// Code de l'ordre `désactiver`.
pub const ORDRE_DESACTIVER: u8 = 3;
/// Code de l'ordre `canaux`.
pub const ORDRE_CANAUX: u8 = 4;
/// Code de l'ordre `renommer` (M1b-21).
pub const ORDRE_RENOMMER: u8 = 5;
/// Code de l'ordre `nom-defaut` (M1b-21) : efface le nom personnalisé.
pub const ORDRE_NOM_DEFAUT: u8 = 6;

/// Le plus grand code d'ordre servi, pour les messages de refus.
pub const ORDRE_MAX: u8 = ORDRE_NOM_DEFAUT;

/// Une requête du démon au service : l'interface **fixe** du canal nommé.
///
/// Sept ordres, et jamais un huitième sans changer [`PROTOCOLE_VERSION`]. Deux ne
/// modifient rien ([`Self::Version`], [`Self::Lister`]) ; les cinq autres écrivent — dans
/// le pilote pour trois d'entre eux, dans le registre pour les deux de M1b-21 — et sont
/// donc journalisés avec l'identité de l'appelant.
///
/// **Non `Copy`** : [`Self::Renommer`] possède son nom. Voir l'en-tête de module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Requete {
    /// La version de ce protocole et celle du contrat KS servi par le pilote.
    ///
    /// C'est l'ordre qu'un client émet en premier : il ne touche à rien, ne demande
    /// aucun câble, et dit si la suite a un sens.
    Version,
    /// L'état de tous les câbles : lesquels existent, lesquels sont connectés.
    Lister,
    /// Connecte le câble : ses deux endpoints apparaissent.
    Activer(CableId),
    /// Déconnecte le câble : ses deux endpoints se rangent sous « Périphériques
    /// déconnectés ».
    Desactiver(CableId),
    /// Règle le nombre de canaux du câble.
    ///
    /// **Validé ici, pas appliqué par le pilote** : voir [`Statut::CanauxNonApplicables`].
    Canaux {
        /// Le câble visé.
        cable: CableId,
        /// Le nombre de canaux voulu, dans `MIN_CHANNELS..=MAX_CHANNELS`.
        canaux: u32,
    },
    /// Donne au câble le nom `nom` dans les réglages Son (M1b-21).
    ///
    /// **N'atteint pas le pilote** : le nom d'un endpoint audio vit dans le registre, et
    /// c'est le service qui l'y écrit parce que la clé est sous `HKLM`. Voir
    /// [`crate::registre`].
    Renommer {
        /// Le câble visé.
        cable: CableId,
        /// Le nom voulu, déjà validé par
        /// [`conduit_backend::cable::validate_cable_name`].
        nom: String,
    },
    /// Rend au câble son nom d'origine, `Conduit N`, en **effaçant** le nom personnalisé
    /// (M1b-21, F-52).
    ///
    /// C'est le pendant obligatoire de [`Self::Renommer`] : les clés MMDevices ne sont
    /// pas des `HKR` du périphérique et survivent au retrait du pilote, donc Conduit doit
    /// savoir défaire ce qu'il a écrit.
    NomDefaut(CableId),
}

impl Requete {
    /// Le code d'ordre de cette requête, tel qu'il voyage dans la trame.
    #[must_use]
    pub const fn code(&self) -> u8 {
        match self {
            Self::Version => ORDRE_VERSION,
            Self::Lister => ORDRE_LISTER,
            Self::Activer(_) => ORDRE_ACTIVER,
            Self::Desactiver(_) => ORDRE_DESACTIVER,
            Self::Canaux { .. } => ORDRE_CANAUX,
            Self::Renommer { .. } => ORDRE_RENOMMER,
            Self::NomDefaut(_) => ORDRE_NOM_DEFAUT,
        }
    }

    /// Le nom de l'ordre, en français, pour le journal.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Lister => "lister",
            Self::Activer(_) => "activer",
            Self::Desactiver(_) => "désactiver",
            Self::Canaux { .. } => "canaux",
            Self::Renommer { .. } => "renommer",
            Self::NomDefaut(_) => "nom par défaut",
        }
    }

    /// Cette requête **modifie-t-elle** l'état de la machine ?
    ///
    /// C'est ce prédicat qui décide de journaliser ou non l'identité de l'appelant : un
    /// ordre qui change ce que l'utilisateur voit doit laisser une trace nominative, une
    /// lecture non.
    ///
    /// Il ne décide **pas** de l'armement de `SeLoadDriverPrivilege` : celui-ci est armé
    /// par `crate::cables::ecrire`, sur le seul chemin qui parle au pilote, et pour la
    /// durée de cette écriture-là. Les deux ordres de M1b-21 modifient bel et bien la
    /// machine sans jamais toucher au pilote — ils écrivent dans `HKLM`, ce que
    /// `LocalSystem` fait de plein droit ([`Self::touche_le_pilote`]).
    #[must_use]
    pub const fn modifie(&self) -> bool {
        match self {
            Self::Version | Self::Lister => false,
            Self::Activer(_)
            | Self::Desactiver(_)
            | Self::Canaux { .. }
            | Self::Renommer { .. }
            | Self::NomDefaut(_) => true,
        }
    }

    /// Cet ordre passe-t-il par le jeu de propriétés KS du pilote ?
    ///
    /// Sépare les deux natures d'écriture du service : celles qui exigent
    /// `SeLoadDriverPrivilege` armé (le pilote), et celles qui n'exigent que d'être
    /// `LocalSystem` (le registre `HKLM`, M1b-21). Un privilège armé pour une écriture
    /// de registre serait une surface offerte pour rien.
    #[must_use]
    pub const fn touche_le_pilote(&self) -> bool {
        match self {
            Self::Activer(_) | Self::Desactiver(_) | Self::Canaux { .. } => true,
            Self::Version | Self::Lister | Self::Renommer { .. } | Self::NomDefaut(_) => false,
        }
    }

    /// Le câble visé, s'il y en a un.
    #[must_use]
    pub const fn cable(&self) -> Option<CableId> {
        match self {
            Self::Version | Self::Lister => None,
            Self::Activer(cable) | Self::Desactiver(cable) | Self::NomDefaut(cable) => Some(*cable),
            Self::Canaux { cable, .. } | Self::Renommer { cable, .. } => Some(*cable),
        }
    }

    /// Le nom que porte cet ordre, s'il en porte un.
    #[must_use]
    pub fn nom(&self) -> Option<&str> {
        match self {
            Self::Renommer { nom, .. } => Some(nom),
            _ => None,
        }
    }

    /// Sérialise la requête : [`TAILLE_REQUETE`] octets, suivis du nom pour
    /// [`Self::Renommer`].
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(TAILLE_REQUETE + self.nom().map_or(0, str::len));
        out.extend_from_slice(&self.en_tete());
        if let Some(nom) = self.nom() {
            out.extend_from_slice(nom.as_bytes());
        }
        out
    }

    /// L'en-tête de [`TAILLE_REQUETE`] octets, commun aux sept ordres.
    #[must_use]
    pub const fn en_tete(&self) -> [u8; TAILLE_REQUETE] {
        let (cable, canaux) = match self {
            Self::Version | Self::Lister => (0_u16, 0_u16),
            Self::Activer(c) | Self::Desactiver(c) | Self::NomDefaut(c) => (c.0 as u16, 0),
            Self::Canaux { cable, canaux } => (cable.0 as u16, *canaux as u16),
            // Le nom ne voyage pas dans l'en-tête : sa longueur est celle de la trame.
            Self::Renommer { cable, .. } => (cable.0 as u16, 0),
        };
        let cable = cable.to_le_bytes();
        let canaux = canaux.to_le_bytes();
        [
            PROTOCOLE_VERSION,
            self.code(),
            cable[0],
            cable[1],
            canaux[0],
            canaux[1],
            0,
            0,
        ]
    }

    /// Sérialise la requête, en-tête de trame compris.
    #[must_use]
    pub fn encadrer(&self) -> Vec<u8> {
        encadrer(&self.to_bytes())
    }

    /// **Le** parseur des requêtes : des octets hostiles vers un ordre valide, ou une
    /// cause de refus.
    ///
    /// L'ordre des contrôles est celui des champs — l'en-tête doit d'abord être là,
    /// puis la version, le champ réservé, l'ordre, et enfin la longueur et les champs
    /// **que cet ordre exige** — pour que la cause nommée soit la première anomalie et
    /// non une arbitraire.
    ///
    /// La longueur est vérifiée en deux temps depuis M1b-21 : le minimum
    /// ([`TAILLE_REQUETE`]) avant de lire quoi que ce soit, puis la longueur exacte de
    /// l'ordre reconnu. Les six ordres sans nom exigent toujours [`TAILLE_REQUETE`]
    /// pile ; `renommer` exige l'en-tête plus 1 à [`MAX_NOM_OCTETS`] octets d'UTF-8
    /// valide.
    ///
    /// # Erreurs
    ///
    /// [`ErreurRequete`], dont chaque variante porte ce qui a été trouvé.
    pub fn from_bytes(octets: &[u8]) -> Result<Self, ErreurRequete> {
        let Some(tete) = octets.get(..TAILLE_REQUETE) else {
            return Err(ErreurRequete::Longueur {
                recus: octets.len(),
            });
        };
        let brut: [u8; TAILLE_REQUETE] = tete.try_into().unwrap_or([0; TAILLE_REQUETE]);
        let suite = octets.get(TAILLE_REQUETE..).unwrap_or(&[]);
        let version = brut[0];
        if version != PROTOCOLE_VERSION {
            return Err(ErreurRequete::Version { trouvee: version });
        }
        let ordre = brut[1];
        let cable = u16::from_le_bytes([brut[2], brut[3]]);
        let canaux = u16::from_le_bytes([brut[4], brut[5]]);
        let reserve = u16::from_le_bytes([brut[6], brut[7]]);
        if reserve != 0 {
            return Err(ErreurRequete::Reserve { brut: reserve });
        }
        // Le code d'ordre est reconnu **avant** la longueur exacte : c'est lui qui dit
        // quelle longueur exiger. Un ordre inconnu est donc signalé comme tel, quelle
        // que soit la taille de la trame.
        if !matches!(
            ordre,
            ORDRE_VERSION
                | ORDRE_LISTER
                | ORDRE_ACTIVER
                | ORDRE_DESACTIVER
                | ORDRE_CANAUX
                | ORDRE_RENOMMER
                | ORDRE_NOM_DEFAUT
        ) {
            return Err(ErreurRequete::Ordre { code: ordre });
        }
        // Les six ordres sans nom n'acceptent **rien** après l'en-tête : un préfixe
        // valide suivi d'octets en trop reste une trame refusée.
        if ordre != ORDRE_RENOMMER && !suite.is_empty() {
            return Err(ErreurRequete::Longueur {
                recus: octets.len(),
            });
        }
        match ordre {
            ORDRE_VERSION | ORDRE_LISTER => {
                // Aucun champ n'est utilisé : les deux sont exigés nuls.
                exiger_nul("câble", cable)?;
                exiger_nul("canaux", canaux)?;
                Ok(if ordre == ORDRE_VERSION {
                    Self::Version
                } else {
                    Self::Lister
                })
            }
            ORDRE_ACTIVER | ORDRE_DESACTIVER => {
                let cable = valider_cable(cable)?;
                exiger_nul("canaux", canaux)?;
                Ok(if ordre == ORDRE_ACTIVER {
                    Self::Activer(cable)
                } else {
                    Self::Desactiver(cable)
                })
            }
            ORDRE_CANAUX => {
                let cable = valider_cable(cable)?;
                let canaux = valider_canaux(canaux)?;
                Ok(Self::Canaux { cable, canaux })
            }
            ORDRE_NOM_DEFAUT => {
                let cable = valider_cable(cable)?;
                exiger_nul("canaux", canaux)?;
                Ok(Self::NomDefaut(cable))
            }
            // `ORDRE_RENOMMER` : le seul ordre dont la trame est plus longue que
            // l'en-tête, et le seul qui alloue.
            _ => {
                let cable = valider_cable(cable)?;
                exiger_nul("canaux", canaux)?;
                let nom = valider_nom(suite)?;
                Ok(Self::Renommer { cable, nom })
            }
        }
    }
}

/// Refuse un champ que l'ordre n'utilise pas et qui n'est pas nul.
const fn exiger_nul(champ: &'static str, brut: u16) -> Result<(), ErreurRequete> {
    if brut == 0 {
        Ok(())
    } else {
        Err(ErreurRequete::ChampParasite { champ, brut })
    }
}

/// Valide un numéro de câble **affiché** : 1 à [`CABLE_MAX`].
///
/// Le domaine vient du contrat partagé avec le pilote, pas d'un 16 recopié. Le décalage
/// de un (« Conduit 1 » = index pilote 0) ne se fait **pas** ici : il appartient à
/// `conduit_backend_wasapi::cable::driver_index`, seul endroit du dépôt qui le connaisse.
const fn valider_cable(brut: u16) -> Result<CableId, ErreurRequete> {
    let numero = brut as u32;
    if numero == 0 || numero > CABLE_MAX {
        return Err(ErreurRequete::Cable { brut });
    }
    Ok(CableId(numero))
}

/// Valide le nom que porte un `renommer` : bornes du **format**, puis règle du dépôt.
///
/// Trois contrôles, dans cet ordre, parce que chacun rend le suivant possible :
///
/// 1. la charge n'est pas vide — un `renommer` sans nom n'existe pas, c'est
///    [`Requete::NomDefaut`] qu'il fallait envoyer, et le dire vaut mieux qu'écrire une
///    chaîne vide dans le registre ;
/// 2. elle tient dans [`MAX_NOM_OCTETS`] — vérifié avant l'allocation de la `String` ;
/// 3. c'est de l'UTF-8 valide — `from_utf8` et non `from_utf8_lossy` : un octet douteux
///    remplacé par `U+FFFD` deviendrait un nom que personne n'a demandé, écrit dans une
///    clé `HKLM` ;
///
/// puis la **règle du dépôt** : [`validate_cable_name`], le même juge que `conduitctl`
/// et `CableSpec`. Elle n'est pas réécrite ici, et sa cause exacte n'est pas recopiée
/// non plus — le message de refus est celui qu'elle rend, rendu par le service dans son
/// journal ; la trame, elle, ne transporte que « ce nom est refusé ».
fn valider_nom(octets: &[u8]) -> Result<String, ErreurRequete> {
    if octets.is_empty() {
        return Err(ErreurRequete::NomVide);
    }
    if octets.len() > MAX_NOM_OCTETS {
        return Err(ErreurRequete::NomTropLong {
            octets: octets.len(),
        });
    }
    let nom = core::str::from_utf8(octets).map_err(|_| ErreurRequete::NomNonUtf8)?;
    validate_cable_name(nom).map_err(|_| ErreurRequete::NomRefuse)?;
    Ok(nom.to_owned())
}

/// Valide un nombre de canaux contre les bornes de [`conduit_kmd_core::params`].
const fn valider_canaux(brut: u16) -> Result<u32, ErreurRequete> {
    let canaux = brut as u32;
    if canaux < MIN_CHANNELS || canaux > MAX_CHANNELS {
        return Err(ErreurRequete::Canaux { brut });
    }
    Ok(canaux)
}

/// Ce qui a fait refuser une requête : un cas, une cause, une ligne de journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErreurRequete {
    /// Longueur différente de [`TAILLE_REQUETE`] — plus courte **ou** plus longue.
    Longueur {
        /// Octets reçus.
        recus: usize,
    },
    /// Version de protocole que ce service ne sert pas.
    Version {
        /// La version annoncée par le client.
        trouvee: u8,
    },
    /// Code d'ordre inconnu.
    Ordre {
        /// Le code reçu.
        code: u8,
    },
    /// Numéro de câble hors de 1 à [`CABLE_MAX`].
    Cable {
        /// Le numéro reçu.
        brut: u16,
    },
    /// Nombre de canaux hors des bornes du pilote.
    Canaux {
        /// Le nombre reçu.
        brut: u16,
    },
    /// Un champ que cet ordre n'utilise pas, et qui n'est pas nul.
    ChampParasite {
        /// Le nom du champ, en français.
        champ: &'static str,
        /// La valeur trouvée.
        brut: u16,
    },
    /// Le champ réservé de la requête n'est pas nul.
    Reserve {
        /// La valeur trouvée.
        brut: u16,
    },
    /// `renommer` sans nom : c'est `nom-defaut` qu'il fallait envoyer.
    NomVide,
    /// Le nom dépasse [`MAX_NOM_OCTETS`] octets.
    NomTropLong {
        /// Les octets reçus après l'en-tête.
        octets: usize,
    },
    /// Les octets du nom ne sont pas de l'UTF-8 valide.
    NomNonUtf8,
    /// Le nom est bien formé mais refusé par
    /// [`conduit_backend::cable::validate_cable_name`].
    NomRefuse,
}

impl ErreurRequete {
    /// Le statut à renvoyer au client pour cette cause.
    #[must_use]
    pub const fn statut(&self) -> Statut {
        match self {
            Self::Version { .. } => Statut::VersionInconnue,
            Self::Ordre { .. } => Statut::OrdreInconnu,
            Self::Cable { .. } => Statut::CableInconnu,
            Self::Canaux { .. } => Statut::CanauxInvalides,
            // Les quatre causes de nom disent la même chose au client — « ce nom-là ne
            // sera pas écrit » — et appellent la même conduite : en donner un autre.
            Self::NomVide | Self::NomTropLong { .. } | Self::NomNonUtf8 | Self::NomRefuse => {
                Statut::NomInvalide
            }
            Self::Longueur { .. } | Self::ChampParasite { .. } | Self::Reserve { .. } => {
                Statut::TrameInvalide
            }
        }
    }
}

impl fmt::Display for ErreurRequete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Longueur { recus } => write!(
                f,
                "requête de {recus} octets, {TAILLE_REQUETE} attendus exactement"
            ),
            Self::Version { trouvee } => write!(
                f,
                "protocole version {trouvee} : ce service sert la version \
                 {PROTOCOLE_VERSION}"
            ),
            Self::Ordre { code } => write!(f, "ordre {code} inconnu (0 à {ORDRE_MAX})"),
            Self::Cable { brut } => {
                write!(
                    f,
                    "câble {brut} inconnu : les câbles vont de 1 à {CABLE_MAX}"
                )
            }
            Self::Canaux { brut } => {
                write!(f, "{brut} canaux hors de {MIN_CHANNELS} à {MAX_CHANNELS}")
            }
            Self::ChampParasite { champ, brut } => write!(
                f,
                "champ « {champ} » = {brut} alors que cet ordre ne l'utilise pas"
            ),
            Self::Reserve { brut } => write!(f, "champ réservé non nul ({brut:#06x})"),
            Self::NomVide => f.write_str(
                "« renommer » sans nom : pour rendre au câble son nom d'origine, c'est \
                 l'ordre « nom par défaut » qu'il faut envoyer",
            ),
            Self::NomTropLong { octets } => write!(
                f,
                "nom de {octets} octets : maximum {MAX_NOM_OCTETS} \
                 ({MAX_CABLE_NAME_LEN} caractères au plus)"
            ),
            Self::NomNonUtf8 => f.write_str("le nom n'est pas de l'UTF-8 valide"),
            Self::NomRefuse => write!(
                f,
                "nom refusé : au plus {MAX_CABLE_NAME_LEN} caractères, ni caractère de \
                 contrôle ni « /\\:*?\"<>| »"
            ),
        }
    }
}

impl std::error::Error for ErreurRequete {}

// ---------------------------------------------------------------------------------
// Les statuts.
// ---------------------------------------------------------------------------------

/// L'issue d'un ordre, telle qu'elle voyage dans [`Reponse::statut`].
///
/// Chaque variante correspond à **une conduite différente** pour le client : changer de
/// version, corriger sa trame, installer le pilote, changer de compte. « Erreur » sans
/// autre précision ne servirait à rien.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Statut {
    /// L'ordre a été exécuté.
    Succes,
    /// Le client parle une autre version du protocole ; [`Reponse::detail`] porte celle
    /// que ce service sert.
    VersionInconnue,
    /// La trame n'a pas la forme attendue : longueur, champ réservé, champ parasite.
    TrameInvalide,
    /// Code d'ordre inconnu.
    OrdreInconnu,
    /// Numéro de câble hors des câbles adressables.
    CableInconnu,
    /// Nombre de canaux hors des bornes du pilote.
    CanauxInvalides,
    /// Nombre de canaux valide, mais qui n'est pas celui que **ce câble** sert.
    ///
    /// [`Reponse::detail`] porte le nombre de canaux réellement servi, pour que le client
    /// n'ait pas à le deviner : c'est ce qui distingue ce refus d'un
    /// [`Self::ErreurSysteme`] portant le 87 du pilote, lequel ne dit pas quelle valeur
    /// était attendue.
    ///
    /// # Ce que ce statut veut dire depuis M1b-05
    ///
    /// Il a changé de raison sans changer de conclusion. Avant M1b-05, le pilote ne
    /// *savait* pas servir autre chose que deux canaux : la valeur était scellée dans ses
    /// tables KS. Depuis, il sert 1 à 8 — mais **pas en changer à chaud** : les tables KS
    /// sont immuables et PortCls en retient les pointeurs pour toute la vie du filtre.
    ///
    /// Changer le format d'un câble demande donc deux gestes qu'aucun ordre de ce
    /// protocole ne fait : écrire `CableFormat<n>` dans la clé matérielle du périphérique
    /// ([`conduit_kmd_core::config::CABLE_FORMAT_VALUE_NAMES`]) **et** redémarrer le
    /// devnode. Répondre `Succes` à un `canaux` qui n'agirait sur rien avant le prochain
    /// démarrage serait pire qu'un refus, et c'est pourquoi ce statut reste.
    ///
    /// # Il ne répond plus au seul ordre `canaux`
    ///
    /// [`conduit_kmd_core::config::CableState::channels`] est un **écho vérifié** : tout
    /// `SET` en porte un, y compris un `activer`. Le service le juge donc sur les trois
    /// ordres qui parlent au pilote, et ce statut peut désormais répondre à n'importe
    /// lequel — c'est ce qui a manqué au défaut de frontière de M1b-05, où un `activer` sur
    /// un câble en six canaux ne rendait qu'« erreur Win32 87 ».
    CanauxNonApplicables,
    /// Aucune interface `KSCATEGORY_TOPOLOGY` de ce câble : le pilote Conduit n'est pas
    /// chargé, ou ce câble n'est pas enregistré.
    PiloteAbsent,
    /// `SeLoadDriverPrivilege` n'a pas pu être armé dans le jeton du service.
    ///
    /// Ne devrait pas arriver en `LocalSystem`, qui le détient : c'est le signe que le
    /// service tourne sous un autre compte.
    PrivilegeAbsent,
    /// Le système a refusé ; [`Reponse::detail`] porte le code Win32 **tel quel**.
    ErreurSysteme,
    /// Le nom demandé est refusé par la règle du dépôt
    /// ([`conduit_backend::cable::validate_cable_name`]) ou par les bornes du format.
    ///
    /// Distinct de [`Self::TrameInvalide`] : la trame était bien formée, c'est le nom
    /// qu'elle portait qui ne peut pas être écrit. La conduite est d'en donner un autre.
    NomInvalide,
    /// Aucun endpoint MMDevices ne correspond à ce câble : rien à renommer (M1b-21).
    ///
    /// Le cas courant et bénin : le câble est **déconnecté**, donc Windows n'a pas
    /// publié ses endpoints et la clé de registre à écrire n'existe pas. La conduite est
    /// d'activer le câble d'abord. [`Reponse::detail`] porte le nombre de côtés trouvés,
    /// 0 ou 1 — un seul côté trouvé sur deux est le signe d'un endpoint à moitié publié,
    /// et ce n'est pas la même panne qu'aucun.
    EndpointAbsent,
}

impl Statut {
    /// Tous les statuts, dans l'ordre de leur code.
    pub const ALL: [Self; 12] = [
        Self::Succes,
        Self::VersionInconnue,
        Self::TrameInvalide,
        Self::OrdreInconnu,
        Self::CableInconnu,
        Self::CanauxInvalides,
        Self::CanauxNonApplicables,
        Self::PiloteAbsent,
        Self::PrivilegeAbsent,
        Self::ErreurSysteme,
        Self::NomInvalide,
        Self::EndpointAbsent,
    ];

    /// Le code de ce statut, tel qu'il voyage dans la trame.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Succes => 0,
            Self::VersionInconnue => 1,
            Self::TrameInvalide => 2,
            Self::OrdreInconnu => 3,
            Self::CableInconnu => 4,
            Self::CanauxInvalides => 5,
            Self::CanauxNonApplicables => 6,
            Self::PiloteAbsent => 7,
            Self::PrivilegeAbsent => 8,
            Self::ErreurSysteme => 9,
            Self::NomInvalide => 10,
            Self::EndpointAbsent => 11,
        }
    }

    /// Le statut de code `code`, ou `None` : un code inconnu n'est **pas** replié sur
    /// « erreur », sinon un service d'une version future ferait croire à un échec
    /// générique là où il y a une inadéquation de version.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Succes,
            1 => Self::VersionInconnue,
            2 => Self::TrameInvalide,
            3 => Self::OrdreInconnu,
            4 => Self::CableInconnu,
            5 => Self::CanauxInvalides,
            6 => Self::CanauxNonApplicables,
            7 => Self::PiloteAbsent,
            8 => Self::PrivilegeAbsent,
            9 => Self::ErreurSysteme,
            10 => Self::NomInvalide,
            11 => Self::EndpointAbsent,
            _ => return None,
        })
    }

    /// L'ordre a-t-il abouti ?
    #[must_use]
    pub const fn succes(self) -> bool {
        matches!(self, Self::Succes)
    }
}

impl fmt::Display for Statut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let texte = match self {
            Self::Succes => "succès",
            Self::VersionInconnue => "version de protocole non servie par ce service",
            Self::TrameInvalide => "trame invalide",
            Self::OrdreInconnu => "ordre inconnu",
            Self::CableInconnu => "câble inconnu",
            Self::CanauxInvalides => "nombre de canaux hors bornes",
            Self::CanauxNonApplicables => {
                "nombre de canaux valide, mais ce n'est pas celui que ce câble sert : le \
                 format d'un câble ne change qu'au redémarrage du périphérique"
            }
            Self::PiloteAbsent => "pilote Conduit absent ou câble non enregistré",
            Self::PrivilegeAbsent => {
                "SeLoadDriverPrivilege impossible à armer : le service ne tourne pas en \
                 LocalSystem"
            }
            Self::ErreurSysteme => "refus du système",
            // La borne vient du dépôt, pas d'un 64 recopié.
            Self::NomInvalide => {
                return write!(
                    f,
                    "nom refusé : au plus {MAX_CABLE_NAME_LEN} caractères, ni caractère \
                     de contrôle ni « /\\:*?\"<>| »"
                )
            }
            Self::EndpointAbsent => {
                "aucun endpoint audio pour ce câble : Windows ne les publie que lorsque \
                 le câble est connecté — activez-le, puis renommez"
            }
        };
        f.write_str(texte)
    }
}

// ---------------------------------------------------------------------------------
// Les réponses.
// ---------------------------------------------------------------------------------

/// La réponse du service : **toujours** [`TAILLE_REPONSE`] octets, quel que soit l'ordre
/// et quelle que soit l'issue.
///
/// Elle porte l'état complet des câbles même sur un refus, ce qui évite au client de
/// devoir enchaîner un `lister` derrière chaque `activer` — et rend le journal du client
/// lisible sans corrélation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reponse {
    /// Écho du code d'ordre **reçu**, tel quel — y compris quand il est inconnu.
    pub ordre: u8,
    /// L'issue.
    pub statut: Statut,
    /// Le renseignement chiffré qui accompagne le statut : code Win32 pour
    /// [`Statut::ErreurSysteme`], version servie pour [`Statut::VersionInconnue`], 0
    /// sinon.
    pub detail: u32,
    /// Masque des câbles dont le filtre de topologie **existe** sur la machine (bit `n`
    /// = « Conduit *n+1* »), même disposition que
    /// [`conduit_kmd_core::config::ACTIVE_CABLES_MASK`].
    pub presents: u32,
    /// Masque des câbles **connectés**.
    pub actifs: u32,
    /// Version du contrat KS servie par le pilote
    /// ([`conduit_kmd_core::config::CONFIG_VERSION`] côté service), 0 si elle n'a pas pu
    /// être lue.
    pub version_ks: u32,
    /// Nombre de canaux du câble visé, 0 quand l'ordre ne vise aucun câble.
    pub canaux: u32,
}

impl Reponse {
    /// Une réponse vide, de statut `statut`, en écho à l'ordre `ordre`.
    #[must_use]
    pub const fn refus(ordre: u8, statut: Statut) -> Self {
        Self {
            ordre,
            statut,
            detail: 0,
            presents: 0,
            actifs: 0,
            version_ks: 0,
            canaux: 0,
        }
    }

    /// La même, avec un détail chiffré.
    #[must_use]
    pub const fn refus_detaille(ordre: u8, statut: Statut, detail: u32) -> Self {
        Self {
            detail,
            ..Self::refus(ordre, statut)
        }
    }

    /// Le câble `cable` (numéro **affiché**) est-il connecté d'après [`Self::actifs`] ?
    ///
    /// Rend `false` hors des câbles adressables : un numéro qui n'existe pas n'est
    /// jamais connecté. Le décalage de un est fait ici pour que le client n'ait pas à
    /// le refaire.
    #[must_use]
    pub const fn est_actif(&self, cable: CableId) -> bool {
        conduit_kmd_core::config::is_active(self.actifs, cable.0.wrapping_sub(1))
    }

    /// Le câble `cable` est-il enregistré par le pilote sur cette machine ?
    #[must_use]
    pub const fn est_present(&self, cable: CableId) -> bool {
        conduit_kmd_core::config::is_active(self.presents, cable.0.wrapping_sub(1))
    }

    /// Sérialise la réponse en [`TAILLE_REPONSE`] octets.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; TAILLE_REPONSE] {
        let detail = self.detail.to_le_bytes();
        let presents = self.presents.to_le_bytes();
        let actifs = self.actifs.to_le_bytes();
        let version_ks = self.version_ks.to_le_bytes();
        let canaux = self.canaux.to_le_bytes();
        [
            PROTOCOLE_VERSION,
            self.ordre,
            self.statut.code(),
            0,
            detail[0],
            detail[1],
            detail[2],
            detail[3],
            presents[0],
            presents[1],
            presents[2],
            presents[3],
            actifs[0],
            actifs[1],
            actifs[2],
            actifs[3],
            version_ks[0],
            version_ks[1],
            version_ks[2],
            version_ks[3],
            canaux[0],
            canaux[1],
            canaux[2],
            canaux[3],
            0,
            0,
            0,
            0,
        ]
    }

    /// Sérialise la réponse, en-tête de trame compris.
    #[must_use]
    pub fn encadrer(&self) -> Vec<u8> {
        encadrer(&self.to_bytes())
    }

    /// **Le** parseur des réponses, côté client : mêmes règles qu'à l'aller.
    ///
    /// Le client d'un service `LocalSystem` n'a pas de raison de se méfier autant que le
    /// serveur — mais rien ne garantit que le canal soit tenu par notre service (un
    /// canal nommé se squatte, c'est même la raison du descripteur de sécurité du module
    /// `securite`). La réponse est donc analysée avec la même sévérité.
    ///
    /// # Erreurs
    ///
    /// [`ErreurReponse`], dont chaque variante porte ce qui a été trouvé.
    pub fn from_bytes(octets: &[u8]) -> Result<Self, ErreurReponse> {
        let brut: [u8; TAILLE_REPONSE] =
            octets.try_into().map_err(|_| ErreurReponse::Longueur {
                recus: octets.len(),
            })?;
        let version = brut[0];
        if version != PROTOCOLE_VERSION {
            return Err(ErreurReponse::Version { trouvee: version });
        }
        if brut[3] != 0 {
            return Err(ErreurReponse::Reserve { brut: brut[3] });
        }
        let statut = Statut::from_code(brut[2]).ok_or(ErreurReponse::Statut { code: brut[2] })?;
        let reserve2 = mot(&brut, 24);
        if reserve2 != 0 {
            return Err(ErreurReponse::Reserve2 { brut: reserve2 });
        }
        Ok(Self {
            ordre: brut[1],
            statut,
            detail: mot(&brut, 4),
            presents: mot(&brut, 8),
            actifs: mot(&brut, 12),
            version_ks: mot(&brut, 16),
            canaux: mot(&brut, 20),
        })
    }
}

/// Lit un `u32` petit-boutiste au décalage `offset` d'une réponse.
///
/// Les quatre octets sont **recopiés** puis interprétés, jamais transtypés ; `offset`
/// est toujours un décalage nommé de ce fichier, donc dans les bornes, et le repli à
/// zéro n'est là que pour interdire la panique.
const fn mot(brut: &[u8; TAILLE_REPONSE], offset: usize) -> u32 {
    match offset.checked_add(4) {
        Some(fin) if fin <= TAILLE_REPONSE => u32::from_le_bytes([
            brut[offset],
            brut[offset.wrapping_add(1)],
            brut[offset.wrapping_add(2)],
            brut[offset.wrapping_add(3)],
        ]),
        _ => 0,
    }
}

/// Ce qui a fait refuser une réponse, côté client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErreurReponse {
    /// Longueur différente de [`TAILLE_REPONSE`].
    Longueur {
        /// Octets reçus.
        recus: usize,
    },
    /// Version de protocole inattendue.
    Version {
        /// La version annoncée.
        trouvee: u8,
    },
    /// Code de statut inconnu.
    Statut {
        /// Le code reçu.
        code: u8,
    },
    /// Le premier champ réservé n'est pas nul.
    Reserve {
        /// La valeur trouvée.
        brut: u8,
    },
    /// Le second champ réservé n'est pas nul.
    Reserve2 {
        /// La valeur trouvée.
        brut: u32,
    },
}

impl fmt::Display for ErreurReponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Longueur { recus } => write!(
                f,
                "réponse de {recus} octets, {TAILLE_REPONSE} attendus exactement"
            ),
            Self::Version { trouvee } => write!(
                f,
                "réponse en protocole version {trouvee}, {PROTOCOLE_VERSION} attendue"
            ),
            Self::Statut { code } => write!(f, "statut {code} inconnu"),
            Self::Reserve { brut } => write!(f, "champ réservé non nul ({brut:#04x})"),
            Self::Reserve2 { brut } => write!(f, "second champ réservé non nul ({brut:#010x})"),
        }
    }
}

impl std::error::Error for ErreurReponse {}

// ---------------------------------------------------------------------------------
// Le cadrage.
// ---------------------------------------------------------------------------------

/// Ce qui a fait refuser un en-tête de trame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErreurTrame {
    /// Longueur annoncée au-delà de [`MAX_TRAME_OCTETS`] : refusée **avant** toute
    /// allocation.
    TropGrande {
        /// La longueur annoncée.
        annoncee: usize,
        /// La borne.
        maximum: usize,
    },
    /// Longueur annoncée nulle : une trame vide n'existe pas dans ce protocole.
    Vide,
}

impl fmt::Display for ErreurTrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TropGrande { annoncee, maximum } => {
                write!(f, "trame de {annoncee} octets annoncée : maximum {maximum}")
            }
            Self::Vide => f.write_str("trame de longueur nulle"),
        }
    }
}

impl std::error::Error for ErreurTrame {}

/// Préfixe `charge` de sa longueur (`u32` petit-boutiste).
///
/// La charge vient toujours d'un `to_bytes` de ce module, donc sa longueur est connue à
/// la compilation et tient dans un `u32` : la conversion ne peut pas échouer, et le
/// repli sur `u32::MAX` n'est là que pour interdire la panique.
#[must_use]
pub fn encadrer(charge: &[u8]) -> Vec<u8> {
    let longueur = u32::try_from(charge.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(EN_TETE_OCTETS.saturating_add(charge.len()));
    out.extend_from_slice(&longueur.to_le_bytes());
    out.extend_from_slice(charge);
    out
}

/// Décode un en-tête de trame et rend la longueur de la charge qui suit, **bornée**.
///
/// C'est le seul endroit où un nombre venu du réseau devient une taille de tampon. La
/// borne est vérifiée ici, avant l'allocation, et non après la lecture : un serveur qui
/// réserverait d'abord et vérifierait ensuite offrirait à tout membre du groupe
/// `INTERACTIVE` de faire réserver 4 Gio au processus `LocalSystem` avec quatre octets.
///
/// # Erreurs
///
/// [`ErreurTrame::TropGrande`] ou [`ErreurTrame::Vide`].
pub const fn longueur_annoncee(en_tete: [u8; EN_TETE_OCTETS]) -> Result<usize, ErreurTrame> {
    let annoncee = u32::from_le_bytes(en_tete) as usize;
    if annoncee == 0 {
        return Err(ErreurTrame::Vide);
    }
    if annoncee > MAX_TRAME_OCTETS {
        return Err(ErreurTrame::TropGrande {
            annoncee,
            maximum: MAX_TRAME_OCTETS,
        });
    }
    Ok(annoncee)
}

/// Découpe la **première** trame complète de `tampon` : rend la charge et le nombre
/// d'octets consommés, ou `None` s'il en manque encore.
///
/// Fonction pure et sans allocation, séparée de toute entrée-sortie : c'est elle que les
/// tables de cas éprouvent, et c'est elle que M1b-08 fuzzera. Le serveur, lui, connaît la
/// taille de ce qu'il attend et lit en deux temps ([`longueur_annoncee`]).
///
/// # Erreurs
///
/// [`ErreurTrame`] si l'en-tête annonce une longueur nulle ou hors borne.
pub fn decouper(tampon: &[u8]) -> Result<Option<(&[u8], usize)>, ErreurTrame> {
    let Some(en_tete) = tampon.get(..EN_TETE_OCTETS) else {
        return Ok(None);
    };
    let en_tete: [u8; EN_TETE_OCTETS] = en_tete.try_into().unwrap_or([0; EN_TETE_OCTETS]);
    let longueur = longueur_annoncee(en_tete)?;
    let fin = EN_TETE_OCTETS.saturating_add(longueur);
    match tampon.get(EN_TETE_OCTETS..fin) {
        Some(charge) => Ok(Some((charge, fin))),
        None => Ok(None),
    }
}

/// Le nombre de canaux d'un câble **neuf**, celui que l'INF écrit et sur lequel le pilote
/// se replie.
///
/// Ce n'est **plus** la seule valeur applicable, contrairement à `CANAUX_APPLICABLES` de
/// M1b-20 : depuis M1b-05, chaque câble sert le nombre de canaux que son `CableFormat<n>`
/// fixe, entre 1 et 8, et [`Statut::CanauxNonApplicables`] se décide contre **cette**
/// valeur-là, relue sur le câble visé. La constante ne sert plus qu'à dire ce qu'un poste
/// fraîchement installé sert.
///
/// Ré-exportée depuis le contrat partagé plutôt que recopiée : une seule source.
pub const CANAUX_PAR_DEFAUT: u32 = DEFAULT_CHANNELS;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Une requête brute, champ par champ, sans passer par le sérialiseur : c'est ce qui
    /// permet de fabriquer des trames que `Requete::to_bytes` ne produirait jamais.
    fn brut(version: u8, ordre: u8, cable: u16, canaux: u16, reserve: u16) -> [u8; 8] {
        let c = cable.to_le_bytes();
        let n = canaux.to_le_bytes();
        let r = reserve.to_le_bytes();
        [version, ordre, c[0], c[1], n[0], n[1], r[0], r[1]]
    }

    /// Les sept ordres font l'aller-retour, et le code d'ordre est celui qu'on a gravé.
    #[test]
    fn les_sept_ordres_font_l_aller_retour() {
        let cas = [
            (Requete::Version, ORDRE_VERSION),
            (Requete::Lister, ORDRE_LISTER),
            (Requete::Activer(CableId(1)), ORDRE_ACTIVER),
            (Requete::Desactiver(CableId(16)), ORDRE_DESACTIVER),
            (
                Requete::Canaux {
                    cable: CableId(3),
                    canaux: 2,
                },
                ORDRE_CANAUX,
            ),
            (
                Requete::Renommer {
                    cable: CableId(2),
                    nom: "Musique".to_owned(),
                },
                ORDRE_RENOMMER,
            ),
            (Requete::NomDefaut(CableId(7)), ORDRE_NOM_DEFAUT),
        ];
        for (requete, code) in &cas {
            let octets = requete.to_bytes();
            // Seul `renommer` dépasse l'en-tête, et exactement de la taille du nom.
            assert_eq!(
                octets.len(),
                TAILLE_REQUETE + requete.nom().map_or(0, str::len),
                "{requete:?}"
            );
            assert_eq!(octets[0], PROTOCOLE_VERSION, "{requete:?}");
            assert_eq!(octets[1], *code, "{requete:?}");
            assert_eq!(requete.code(), *code);
            assert_eq!(
                Requete::from_bytes(&octets),
                Ok(requete.clone()),
                "{requete:?}"
            );
        }
        // Les codes sont deux à deux distincts et contigus depuis 0.
        let codes: Vec<u8> = cas.iter().map(|(_, c)| *c).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(ORDRE_MAX, 6);

        // Le nom voyage **après** l'en-tête, tel quel, et nulle part ailleurs.
        let octets = Requete::Renommer {
            cable: CableId(2),
            nom: "Musique".to_owned(),
        }
        .to_bytes();
        assert_eq!(&octets[TAILLE_REQUETE..], "Musique".as_bytes());
        // Les champs « canaux » et « réservé » de l'en-tête restent nuls.
        assert_eq!(&octets[4..8], &[0, 0, 0, 0]);
    }

    /// **Les deux ordres de M1b-21 ne touchent pas au pilote**, et les trois autres
    /// ordres modifiants si.
    ///
    /// C'est ce prédicat qui décide d'armer ou non `SeLoadDriverPrivilege` : le laisser
    /// glisser vers « tout ce qui modifie » armerait un privilège pour une écriture de
    /// registre.
    #[test]
    fn ce_qui_touche_le_pilote_et_ce_qui_modifie() {
        let cas: [(Requete, bool, bool); 7] = [
            (Requete::Version, false, false),
            (Requete::Lister, false, false),
            (Requete::Activer(CableId(1)), true, true),
            (Requete::Desactiver(CableId(1)), true, true),
            (
                Requete::Canaux {
                    cable: CableId(1),
                    canaux: 2,
                },
                true,
                true,
            ),
            (
                Requete::Renommer {
                    cable: CableId(1),
                    nom: "Musique".to_owned(),
                },
                true,
                false,
            ),
            (Requete::NomDefaut(CableId(1)), true, false),
        ];
        for (requete, modifie, pilote) in &cas {
            assert_eq!(requete.modifie(), *modifie, "{requete:?}");
            assert_eq!(requete.touche_le_pilote(), *pilote, "{requete:?}");
            // Tout ce qui touche le pilote modifie ; l'inverse n'est plus vrai.
            assert!(
                !requete.touche_le_pilote() || requete.modifie(),
                "{requete:?}"
            );
        }
    }

    /// Table du nom : ce qui passe, ce qui ne passe pas, et pourquoi.
    ///
    /// La règle du nom n'est pas réécrite ici — c'est
    /// `conduit_backend::cable::validate_cable_name` qui tranche — mais elle doit être
    /// **appliquée** par le parseur, sans quoi un nom impossible arriverait jusqu'au
    /// registre.
    #[test]
    fn nom_table() {
        /// Une trame `renommer` portant `nom` en octets bruts.
        fn trame_renommer(octets: &[u8]) -> Vec<u8> {
            let mut brut = brut(PROTOCOLE_VERSION, ORDRE_RENOMMER, 1, 0, 0).to_vec();
            brut.extend_from_slice(octets);
            brut
        }

        // Ce qui passe.
        for nom in [
            "Musique",
            "M",
            "Jeu vidéo — sortie",
            "Conduit 1",
            &"é".repeat(64),
        ] {
            assert_eq!(
                Requete::from_bytes(&trame_renommer(nom.as_bytes())),
                Ok(Requete::Renommer {
                    cable: CableId(1),
                    nom: nom.to_owned()
                }),
                "« {nom} »"
            );
        }

        // Vide : c'est `nom-defaut` qu'il fallait envoyer, et le message le dit.
        assert_eq!(
            Requete::from_bytes(&trame_renommer(b"")),
            Err(ErreurRequete::NomVide)
        );
        assert!(ErreurRequete::NomVide
            .to_string()
            .contains("nom par défaut"));

        // Trop long **en octets** : refusé avant toute allocation.
        let trop = vec![b'x'; MAX_NOM_OCTETS + 1];
        assert_eq!(
            Requete::from_bytes(&trame_renommer(&trop)),
            Err(ErreurRequete::NomTropLong {
                octets: MAX_NOM_OCTETS + 1
            })
        );
        // Exactement la borne du format : c'est la règle du nom qui tranche alors, et
        // 256 « x » font 256 caractères, donc trop.
        let borne = vec![b'x'; MAX_NOM_OCTETS];
        assert_eq!(
            Requete::from_bytes(&trame_renommer(&borne)),
            Err(ErreurRequete::NomRefuse)
        );

        // UTF-8 invalide : refusé, jamais remplacé par U+FFFD.
        assert_eq!(
            Requete::from_bytes(&trame_renommer(&[0xFF, 0xFE])),
            Err(ErreurRequete::NomNonUtf8)
        );

        // La règle du dépôt, appliquée et non recopiée : les caractères interdits, les
        // caractères de contrôle, et le nom qui n'est que des espaces.
        for refuse in [
            "a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a\nb", " ", "   ",
        ] {
            assert_eq!(
                Requete::from_bytes(&trame_renommer(refuse.as_bytes())),
                Err(ErreurRequete::NomRefuse),
                "« {refuse} »"
            );
            // Et le juge est bien celui du dépôt.
            assert!(
                conduit_backend::cable::validate_cable_name(refuse).is_err(),
                "« {refuse} »"
            );
        }

        // Les quatre causes mènent au même statut : « donnez-en un autre ».
        for cause in [
            ErreurRequete::NomVide,
            ErreurRequete::NomTropLong { octets: 999 },
            ErreurRequete::NomNonUtf8,
            ErreurRequete::NomRefuse,
        ] {
            assert_eq!(cause.statut(), Statut::NomInvalide, "{cause:?}");
            assert!(!cause.to_string().is_empty(), "{cause:?}");
        }
    }

    /// Les octets en trop restent refusés — pour les six ordres qui n'ont pas de nom.
    ///
    /// C'est la règle que M1b-21 aurait pu relâcher par accident : ouvrir la requête à
    /// une charge variable ne doit l'ouvrir que pour `renommer`.
    #[test]
    fn seul_renommer_accepte_des_octets_apres_l_en_tete() {
        let sans_nom = [
            (ORDRE_VERSION, 0u16),
            (ORDRE_LISTER, 0),
            (ORDRE_ACTIVER, 1),
            (ORDRE_DESACTIVER, 1),
            (ORDRE_NOM_DEFAUT, 1),
        ];
        for (ordre, cable) in sans_nom {
            let mut trop = brut(PROTOCOLE_VERSION, ordre, cable, 0, 0).to_vec();
            trop.push(b'x');
            assert_eq!(
                Requete::from_bytes(&trop),
                Err(ErreurRequete::Longueur { recus: 9 }),
                "ordre {ordre}"
            );
        }
        // `canaux` aussi, avec un nombre de canaux valide.
        let mut trop = brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, 2, 0).to_vec();
        trop.push(b'x');
        assert_eq!(
            Requete::from_bytes(&trop),
            Err(ErreurRequete::Longueur { recus: 9 })
        );
    }

    /// Table des longueurs : exacte, tronquée, allongée, vide.
    ///
    /// L'allongement est le cas qui compte — un préfixe parfaitement valide suivi d'un
    /// octet en trop doit être refusé, sinon le format n'est plus extensible.
    #[test]
    fn longueur_table() {
        let complet = Requete::Lister.to_bytes();
        assert!(Requete::from_bytes(&complet).is_ok());

        assert_eq!(
            Requete::from_bytes(&complet[..7]),
            Err(ErreurRequete::Longueur { recus: 7 })
        );
        let mut trop_long = complet.to_vec();
        trop_long.push(0);
        assert_eq!(
            Requete::from_bytes(&trop_long),
            Err(ErreurRequete::Longueur { recus: 9 })
        );
        // L'octet en trop n'a même pas besoin d'être nul.
        let mut trop_long = complet.to_vec();
        trop_long.push(0xFF);
        assert_eq!(
            Requete::from_bytes(&trop_long),
            Err(ErreurRequete::Longueur { recus: 9 })
        );

        // Toutes les longueurs de 0 à 24 sont refusées, mais **pas pour la même
        // raison** depuis M1b-21 : en dessous de l'en-tête c'est la longueur, au-dessus
        // c'est le premier champ lisible — ici la version, qui vaut 0 dans un tampon nul.
        // Le format n'étant plus de taille fixe, la longueur ne peut plus être jugée
        // avant de savoir de quel ordre il s'agit.
        for taille in 0..=24usize {
            let tampon = vec![0u8; taille];
            let resultat = Requete::from_bytes(&tampon);
            if taille < TAILLE_REQUETE {
                assert_eq!(
                    resultat,
                    Err(ErreurRequete::Longueur { recus: taille }),
                    "taille {taille}"
                );
            } else {
                assert_eq!(
                    resultat,
                    Err(ErreurRequete::Version { trouvee: 0 }),
                    "taille {taille}"
                );
            }
        }
    }

    /// Table de la version : la nôtre passe, toutes les autres sont refusées, et le
    /// refus est le **premier** contrôle après la longueur.
    #[test]
    fn version_table() {
        for version in 0..=u8::MAX {
            let trame = brut(version, ORDRE_LISTER, 0, 0, 0);
            let resultat = Requete::from_bytes(&trame);
            if version == PROTOCOLE_VERSION {
                assert_eq!(resultat, Ok(Requete::Lister));
            } else {
                assert_eq!(resultat, Err(ErreurRequete::Version { trouvee: version }));
            }
        }
        // Une version étrangère est signalée même quand tout le reste est aberrant :
        // c'est le renseignement le plus utile au client.
        let trame = brut(9, 200, 999, 999, 7);
        assert_eq!(
            Requete::from_bytes(&trame),
            Err(ErreurRequete::Version { trouvee: 9 })
        );
    }

    /// Table des codes d'ordre : les sept connus, tous les autres refusés.
    #[test]
    fn ordre_table() {
        for code in 0..=u8::MAX {
            let trame = brut(PROTOCOLE_VERSION, code, 0, 0, 0);
            let resultat = Requete::from_bytes(&trame);
            match code {
                ORDRE_VERSION => assert_eq!(resultat, Ok(Requete::Version)),
                ORDRE_LISTER => assert_eq!(resultat, Ok(Requete::Lister)),
                // Les ordres qui visent un câble refusent le câble 0, pas l'ordre.
                ORDRE_ACTIVER | ORDRE_DESACTIVER | ORDRE_CANAUX | ORDRE_RENOMMER
                | ORDRE_NOM_DEFAUT => {
                    assert_eq!(
                        resultat,
                        Err(ErreurRequete::Cable { brut: 0 }),
                        "code {code}"
                    );
                }
                _ => assert_eq!(resultat, Err(ErreurRequete::Ordre { code }), "code {code}"),
            }
        }
        // Un ordre inconnu est signalé comme tel **même** avec des octets en trop : le
        // code décide de la longueur à exiger, donc il est reconnu d'abord.
        let mut trop = brut(PROTOCOLE_VERSION, 200, 0, 0, 0).to_vec();
        trop.extend_from_slice(b"Musique");
        assert_eq!(
            Requete::from_bytes(&trop),
            Err(ErreurRequete::Ordre { code: 200 })
        );
    }

    /// Table du numéro de câble : 1 à `CABLE_MAX`, et rien d'autre. Le domaine vient du
    /// contrat, pas d'un 16 recopié.
    #[test]
    fn cable_table() {
        for ordre in [ORDRE_ACTIVER, ORDRE_DESACTIVER] {
            for numero in 0..=(CABLE_MAX as u16).saturating_add(2) {
                let trame = brut(PROTOCOLE_VERSION, ordre, numero, 0, 0);
                let resultat = Requete::from_bytes(&trame);
                if numero >= 1 && u32::from(numero) <= CABLE_MAX {
                    assert_eq!(
                        resultat.map(|r| r.cable()),
                        Ok(Some(CableId(u32::from(numero)))),
                        "câble {numero}"
                    );
                } else {
                    assert_eq!(
                        resultat,
                        Err(ErreurRequete::Cable { brut: numero }),
                        "câble {numero}"
                    );
                }
            }
        }
        // Les extrêmes du champ, qui ne peuvent pas venir d'un client honnête.
        for numero in [17, 100, u16::MAX] {
            let trame = brut(PROTOCOLE_VERSION, ORDRE_ACTIVER, numero, 0, 0);
            assert_eq!(
                Requete::from_bytes(&trame),
                Err(ErreurRequete::Cable { brut: numero })
            );
        }
    }

    /// Table des canaux : les bornes de `params`, ni plus ni moins.
    #[test]
    fn canaux_table() {
        for canaux in 0..=(MAX_CHANNELS as u16).saturating_add(2) {
            let trame = brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, canaux, 0);
            let resultat = Requete::from_bytes(&trame);
            let attendu = u32::from(canaux) >= MIN_CHANNELS && u32::from(canaux) <= MAX_CHANNELS;
            if attendu {
                assert_eq!(
                    resultat,
                    Ok(Requete::Canaux {
                        cable: CableId(1),
                        canaux: u32::from(canaux)
                    }),
                    "canaux {canaux}"
                );
            } else {
                assert_eq!(
                    resultat,
                    Err(ErreurRequete::Canaux { brut: canaux }),
                    "canaux {canaux}"
                );
            }
        }
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, u16::MAX, 0)),
            Err(ErreurRequete::Canaux { brut: u16::MAX })
        );
    }

    /// Les champs qu'un ordre n'utilise pas sont exigés **nuls**.
    ///
    /// C'est la règle qui distingue « j'ai compris la requête » de « j'ai compris un
    /// préfixe de la requête ».
    #[test]
    fn les_champs_inutilises_sont_exiges_nuls() {
        // `version` et `lister` n'utilisent ni le câble ni les canaux.
        for ordre in [ORDRE_VERSION, ORDRE_LISTER] {
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 1, 0, 0)),
                Err(ErreurRequete::ChampParasite {
                    champ: "câble",
                    brut: 1
                }),
                "ordre {ordre}"
            );
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 0, 2, 0)),
                Err(ErreurRequete::ChampParasite {
                    champ: "canaux",
                    brut: 2
                }),
                "ordre {ordre}"
            );
        }
        // `activer` et `désactiver` n'utilisent pas les canaux.
        for ordre in [ORDRE_ACTIVER, ORDRE_DESACTIVER] {
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 1, 2, 0)),
                Err(ErreurRequete::ChampParasite {
                    champ: "canaux",
                    brut: 2
                }),
                "ordre {ordre}"
            );
        }
        // `renommer` et `nom par défaut` n'utilisent pas les canaux non plus.
        for ordre in [ORDRE_RENOMMER, ORDRE_NOM_DEFAUT] {
            let mut trame = brut(PROTOCOLE_VERSION, ordre, 1, 2, 0).to_vec();
            if ordre == ORDRE_RENOMMER {
                trame.extend_from_slice(b"Musique");
            }
            assert_eq!(
                Requete::from_bytes(&trame),
                Err(ErreurRequete::ChampParasite {
                    champ: "canaux",
                    brut: 2
                }),
                "ordre {ordre}"
            );
        }
        // Le champ réservé est refusé pour tous les ordres, y compris valides.
        for ordre in [
            ORDRE_VERSION,
            ORDRE_LISTER,
            ORDRE_ACTIVER,
            ORDRE_DESACTIVER,
            ORDRE_CANAUX,
            ORDRE_RENOMMER,
            ORDRE_NOM_DEFAUT,
        ] {
            assert_eq!(
                Requete::from_bytes(&brut(PROTOCOLE_VERSION, ordre, 1, 2, 0xBEEF)),
                Err(ErreurRequete::Reserve { brut: 0xBEEF }),
                "ordre {ordre}"
            );
        }
    }

    /// L'ordre des contrôles suit celui des champs : la cause nommée est la **première**
    /// anomalie, pas une arbitraire.
    #[test]
    fn la_cause_nommee_est_la_premiere_anomalie() {
        // Tout est faux à la fois : c'est la version qui est signalée.
        assert_eq!(
            Requete::from_bytes(&brut(0xFF, 0xFF, 0xFFFF, 0xFFFF, 0xFFFF)),
            Err(ErreurRequete::Version { trouvee: 0xFF })
        );
        // Version réparée : le champ réservé passe avant l'ordre, parce qu'il condamne
        // la trame entière quel que soit l'ordre.
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, 0xFF, 0xFFFF, 0xFFFF, 0xFFFF)),
            Err(ErreurRequete::Reserve { brut: 0xFFFF })
        );
        // Puis l'ordre, puis le câble, puis les canaux.
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, 0xFF, 0xFFFF, 0xFFFF, 0)),
            Err(ErreurRequete::Ordre { code: 0xFF })
        );
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 0xFFFF, 0xFFFF, 0)),
            Err(ErreurRequete::Cable { brut: 0xFFFF })
        );
        assert_eq!(
            Requete::from_bytes(&brut(PROTOCOLE_VERSION, ORDRE_CANAUX, 1, 0xFFFF, 0)),
            Err(ErreurRequete::Canaux { brut: 0xFFFF })
        );
    }

    /// Chaque cause de refus a le statut qui dit au client quoi faire.
    #[test]
    fn chaque_cause_a_son_statut() {
        let cas = [
            (ErreurRequete::Longueur { recus: 7 }, Statut::TrameInvalide),
            (
                ErreurRequete::Version { trouvee: 2 },
                Statut::VersionInconnue,
            ),
            (ErreurRequete::Ordre { code: 9 }, Statut::OrdreInconnu),
            (ErreurRequete::Cable { brut: 0 }, Statut::CableInconnu),
            (ErreurRequete::Canaux { brut: 99 }, Statut::CanauxInvalides),
            (
                ErreurRequete::ChampParasite {
                    champ: "câble",
                    brut: 1,
                },
                Statut::TrameInvalide,
            ),
            (ErreurRequete::Reserve { brut: 1 }, Statut::TrameInvalide),
        ];
        for (erreur, statut) in cas {
            assert_eq!(erreur.statut(), statut, "{erreur:?}");
            // Le message nomme la valeur trouvée, pas seulement le fait qu'on a refusé.
            assert!(!erreur.to_string().is_empty(), "{erreur:?}");
        }
    }

    /// Les messages nomment le champ, la valeur et le domaine — pas « requête
    /// invalide ».
    #[test]
    fn les_messages_nomment_le_champ_et_sa_valeur() {
        let ligne = ErreurRequete::Longueur { recus: 9 }.to_string();
        assert!(ligne.contains('9'), "{ligne}");
        assert!(ligne.contains('8'), "{ligne}");

        let ligne = ErreurRequete::Cable { brut: 99 }.to_string();
        assert!(ligne.contains("99"), "{ligne}");
        assert!(ligne.contains("câble"), "{ligne}");
        assert!(ligne.contains("16"), "{ligne}");

        let ligne = ErreurRequete::Canaux { brut: 12 }.to_string();
        assert!(ligne.contains("12"), "{ligne}");
        assert!(ligne.contains("canaux"), "{ligne}");

        let ligne = ErreurRequete::ChampParasite {
            champ: "canaux",
            brut: 4,
        }
        .to_string();
        assert!(ligne.contains("canaux"), "{ligne}");
        assert!(ligne.contains('4'), "{ligne}");

        // Le statut des canaux non applicables dit **ce que le câble sert** et ce qu'il
        // faudrait faire pour en changer ; le compte, lui, voyage dans `detail`. Un
        // message qui ne dirait que « refusé » laisserait chercher un bogue là où il n'y a
        // qu'un format.
        let ligne = Statut::CanauxNonApplicables.to_string();
        assert!(ligne.contains("ce câble sert"), "{ligne}");
        assert!(ligne.contains("redémarrage"), "{ligne}");
    }

    /// Les codes de statut sont stables, deux à deux distincts, et l'aller-retour est
    /// fidèle.
    #[test]
    fn statut_table() {
        let attendus: [(Statut, u8); 12] = [
            (Statut::Succes, 0),
            (Statut::VersionInconnue, 1),
            (Statut::TrameInvalide, 2),
            (Statut::OrdreInconnu, 3),
            (Statut::CableInconnu, 4),
            (Statut::CanauxInvalides, 5),
            (Statut::CanauxNonApplicables, 6),
            (Statut::PiloteAbsent, 7),
            (Statut::PrivilegeAbsent, 8),
            (Statut::ErreurSysteme, 9),
            (Statut::NomInvalide, 10),
            (Statut::EndpointAbsent, 11),
        ];
        for (statut, code) in attendus {
            assert_eq!(statut.code(), code, "{statut:?}");
            assert_eq!(Statut::from_code(code), Some(statut));
        }
        assert_eq!(Statut::ALL.len(), attendus.len());
        // Un code inconnu n'est pas replié sur une erreur générique.
        for code in 12..=u8::MAX {
            assert_eq!(Statut::from_code(code), None, "code {code}");
        }
        assert!(Statut::Succes.succes());
        assert!(!Statut::ErreurSysteme.succes());
    }

    /// La réponse fait l'aller-retour, et les masques se relisent par câble affiché.
    #[test]
    fn la_reponse_fait_l_aller_retour() {
        let reponse = Reponse {
            ordre: ORDRE_ACTIVER,
            statut: Statut::Succes,
            detail: 0,
            presents: 0xFFFF,
            actifs: 0b0000_0101,
            version_ks: 1,
            canaux: 2,
        };
        let octets = reponse.to_bytes();
        assert_eq!(octets.len(), TAILLE_REPONSE);
        assert_eq!(octets[0], PROTOCOLE_VERSION);
        assert_eq!(Reponse::from_bytes(&octets), Ok(reponse));

        // Le décalage de un est fait par la réponse : bit 0 = « Conduit 1 ».
        assert!(reponse.est_actif(CableId(1)));
        assert!(!reponse.est_actif(CableId(2)));
        assert!(reponse.est_actif(CableId(3)));
        assert!(!reponse.est_actif(CableId(0)));
        assert!(!reponse.est_actif(CableId(17)));
        assert!(reponse.est_present(CableId(16)));
        assert!(!reponse.est_present(CableId(17)));
    }

    /// Table des refus de réponse : longueur, version, statut inconnu, réservés.
    #[test]
    fn reponse_refus_table() {
        let valide = Reponse::refus(ORDRE_LISTER, Statut::PiloteAbsent).to_bytes();
        assert!(Reponse::from_bytes(&valide).is_ok());

        assert_eq!(
            Reponse::from_bytes(&valide[..27]),
            Err(ErreurReponse::Longueur { recus: 27 })
        );
        let mut trop_long = valide.to_vec();
        trop_long.push(0);
        assert_eq!(
            Reponse::from_bytes(&trop_long),
            Err(ErreurReponse::Longueur { recus: 29 })
        );

        // Une version qui n'est pas la nôtre — dite en fonction de la nôtre, pour que ce
        // test ne tombe pas au prochain incrément de `PROTOCOLE_VERSION`.
        let etrangere = PROTOCOLE_VERSION.wrapping_add(1);
        let mut mauvaise_version = valide;
        mauvaise_version[0] = etrangere;
        assert_eq!(
            Reponse::from_bytes(&mauvaise_version),
            Err(ErreurReponse::Version { trouvee: etrangere })
        );

        let mut reserve = valide;
        reserve[3] = 1;
        assert_eq!(
            Reponse::from_bytes(&reserve),
            Err(ErreurReponse::Reserve { brut: 1 })
        );

        let mut statut = valide;
        statut[2] = 200;
        assert_eq!(
            Reponse::from_bytes(&statut),
            Err(ErreurReponse::Statut { code: 200 })
        );

        let mut reserve2 = valide;
        reserve2[24] = 0xFF;
        assert_eq!(
            Reponse::from_bytes(&reserve2),
            Err(ErreurReponse::Reserve2 { brut: 0xFF })
        );
    }

    /// Le cadrage : longueur préfixée, bornée **avant** l'allocation.
    #[test]
    fn cadrage_table() {
        let charge = Requete::Lister.to_bytes();
        let trame = encadrer(&charge);
        assert_eq!(trame.len(), EN_TETE_OCTETS + TAILLE_REQUETE);
        assert_eq!(&trame[..4], &(TAILLE_REQUETE as u32).to_le_bytes());
        assert_eq!(Requete::Lister.encadrer(), trame);

        // Découpage : la charge et le nombre d'octets consommés.
        let (lue, consommes) = decouper(&trame).unwrap().expect("trame complète");
        assert_eq!(lue, &charge[..]);
        assert_eq!(consommes, trame.len());

        // Incomplet : on attend, on ne se trompe pas.
        for coupe in 0..trame.len() {
            assert_eq!(decouper(&trame[..coupe]), Ok(None), "coupé à {coupe}");
        }

        // Deux trames à la suite : la première est rendue, le reste attend son tour.
        let mut deux = trame.clone();
        deux.extend_from_slice(&trame);
        let (_, consommes) = decouper(&deux).unwrap().expect("première trame");
        assert_eq!(consommes, trame.len());
        assert!(decouper(&deux[consommes..]).unwrap().is_some());

        // Longueur hors borne : refusée sans qu'aucun tampon ne soit réservé.
        for annoncee in [MAX_TRAME_OCTETS as u32 + 1, 1 << 20, u32::MAX] {
            let mut hostile = annoncee.to_le_bytes().to_vec();
            hostile.extend_from_slice(&charge);
            assert_eq!(
                decouper(&hostile),
                Err(ErreurTrame::TropGrande {
                    annoncee: annoncee as usize,
                    maximum: MAX_TRAME_OCTETS
                }),
                "longueur {annoncee}"
            );
        }
        // Exactement la borne : acceptée (c'est une borne, pas un seuil).
        let en_tete = (MAX_TRAME_OCTETS as u32).to_le_bytes();
        assert_eq!(longueur_annoncee(en_tete), Ok(MAX_TRAME_OCTETS));
        // Nulle : refusée.
        assert_eq!(longueur_annoncee([0; 4]), Err(ErreurTrame::Vide));
        assert_eq!(decouper(&[0, 0, 0, 0]), Err(ErreurTrame::Vide));
        // En-tête incomplet : on attend.
        for coupe in 0..EN_TETE_OCTETS {
            assert_eq!(decouper(&trame[..coupe]), Ok(None));
        }
    }

    /// Le nom du canal est bien un canal local, et il est gravé.
    #[test]
    fn le_nom_du_canal_est_local() {
        assert_eq!(NOM_TUBE, r"\\.\pipe\conduit-helper");
        assert!(NOM_TUBE.starts_with(r"\\.\pipe\"));
        // Pas de canal distant : `\\serveur\pipe\…` serait une tout autre surface.
        assert!(!NOM_TUBE.starts_with(r"\\serveur"));
    }

    /// Les canaux applicables sont ceux **du câble visé**, et non plus une constante.
    ///
    /// C'est le rappel que M1b-05 avait mis en place et qu'elle vient de consommer :
    /// `channels_applicables()` a laissé la place à `channels_appliquables(courants)`, et
    /// le prédicat suit désormais le format de chaque câble.
    #[test]
    fn les_canaux_applicables_sont_ceux_du_cable() {
        assert_eq!(CANAUX_PAR_DEFAUT, DEFAULT_CHANNELS);
        const _: () =
            assert!(MIN_CHANNELS <= CANAUX_PAR_DEFAUT && CANAUX_PAR_DEFAUT <= MAX_CHANNELS);
        // Sur chaque câble possible, le prédicat du contrat accepte exactement le nombre
        // de canaux que ce câble sert — ni plus (un réglage sans effet), ni moins (un
        // refus d'une valeur légitime).
        for servis in MIN_CHANNELS..=MAX_CHANNELS {
            for demandes in MIN_CHANNELS..=MAX_CHANNELS {
                let etat = conduit_kmd_core::config::CableState {
                    cable: 0,
                    connected: 1,
                    channels: demandes,
                    reserved: 0,
                };
                assert_eq!(
                    etat.channels_appliquables(servis),
                    demandes == servis,
                    "{demandes} canaux demandés sur un câble à {servis}"
                );
            }
        }
    }

    proptest! {
        /// Le critère de M1b-08 : **quelle que soit l'entrée**, le parseur ne panique
        /// pas, et toute requête acceptée se resérialise à l'identique.
        #[test]
        fn aucune_requete_ne_panique_et_l_aller_retour_est_fidele(
            octets in proptest::collection::vec(any::<u8>(), 0..40),
        ) {
            match Requete::from_bytes(&octets) {
                Ok(requete) => {
                    // La longueur acceptée est celle que l'ordre exige, et rien d'autre.
                    let attendue = TAILLE_REQUETE + requete.nom().map_or(0, str::len);
                    prop_assert_eq!(octets.len(), attendue);
                    prop_assert_eq!(&requete.to_bytes()[..], &octets[..]);
                    prop_assert_eq!(
                        Requete::from_bytes(&requete.to_bytes()),
                        Ok(requete.clone())
                    );
                    // Toute sortie acceptée est dans les domaines annoncés.
                    if let Some(cable) = requete.cable() {
                        prop_assert!(cable.0 >= 1 && cable.0 <= CABLE_MAX);
                    }
                    if let Requete::Canaux { canaux, .. } = requete {
                        prop_assert!((MIN_CHANNELS..=MAX_CHANNELS).contains(&canaux));
                    }
                    // Un nom accepté est **toujours** un nom que la règle du dépôt
                    // accepte : c'est la propriété qui garantit qu'aucune chaîne
                    // impossible n'atteindra le registre.
                    if let Some(nom) = requete.nom() {
                        prop_assert!(!nom.is_empty());
                        prop_assert!(nom.len() <= MAX_NOM_OCTETS);
                        prop_assert!(conduit_backend::cable::validate_cable_name(nom).is_ok());
                    }
                }
                Err(err) => {
                    // Une longueur inférieure à l'en-tête est **toujours** signalée
                    // comme telle ; au-delà, la cause dépend de l'ordre.
                    let longueur = matches!(err, ErreurRequete::Longueur { .. });
                    if octets.len() < TAILLE_REQUETE {
                        prop_assert!(longueur);
                    }
                }
            }
        }

        /// Idem côté réponse.
        #[test]
        fn aucune_reponse_ne_panique(octets in proptest::collection::vec(any::<u8>(), 0..64)) {
            match Reponse::from_bytes(&octets) {
                Ok(reponse) => {
                    prop_assert_eq!(octets.len(), TAILLE_REPONSE);
                    prop_assert_eq!(&reponse.to_bytes()[..], &octets[..]);
                }
                Err(err) => {
                    let longueur = matches!(err, ErreurReponse::Longueur { .. });
                    prop_assert_eq!(longueur, octets.len() != TAILLE_REPONSE);
                }
            }
        }

        /// Le découpage ne panique jamais et ne rend jamais plus d'octets que le tampon
        /// n'en contient — c'est la propriété qui garde le serveur d'un débordement.
        #[test]
        fn le_decoupage_ne_deborde_jamais(
            octets in proptest::collection::vec(any::<u8>(), 0..200),
        ) {
            match decouper(&octets) {
                Ok(Some((charge, consommes))) => {
                    prop_assert!(consommes <= octets.len());
                    prop_assert_eq!(charge.len(), consommes - EN_TETE_OCTETS);
                    prop_assert!(charge.len() <= MAX_TRAME_OCTETS);
                    prop_assert!(!charge.is_empty());
                }
                Ok(None) => {}
                Err(ErreurTrame::TropGrande { annoncee, .. }) => {
                    prop_assert!(annoncee > MAX_TRAME_OCTETS);
                }
                Err(ErreurTrame::Vide) => {
                    prop_assert!(octets.len() >= EN_TETE_OCTETS);
                    prop_assert_eq!(&octets[..EN_TETE_OCTETS], &[0, 0, 0, 0]);
                }
            }
        }

        /// Une requête quelconque des sept ordres survit au cadrage et au découpage.
        ///
        /// Le nom est engendré dans les caractères que la règle du dépôt accepte, et
        /// jusqu'à la borne du format : c'est la trame la plus longue que le protocole
        /// laisse passer, et donc celle qui éprouve `MAX_TRAME_OCTETS`.
        #[test]
        fn le_cadrage_est_reversible(
            ordre in 0_u8..7,
            cable in 1_u32..=CABLE_MAX,
            canaux in MIN_CHANNELS..=MAX_CHANNELS,
            nom in "[a-zA-Zé0-9 _.-]{1,64}",
        ) {
            let requete = match ordre {
                0 => Requete::Version,
                1 => Requete::Lister,
                2 => Requete::Activer(CableId(cable)),
                3 => Requete::Desactiver(CableId(cable)),
                4 => Requete::Canaux { cable: CableId(cable), canaux },
                5 => Requete::Renommer { cable: CableId(cable), nom: nom.trim().to_owned() },
                _ => Requete::NomDefaut(CableId(cable)),
            };
            // Un nom qui n'est que des espaces est refusé par la règle du dépôt : la
            // propriété ne porte que sur les requêtes que le protocole peut porter.
            prop_assume!(requete.nom().is_none_or(|n| !n.is_empty()));
            let trame = requete.encadrer();
            prop_assert!(trame.len() <= EN_TETE_OCTETS + MAX_TRAME_OCTETS);
            let (charge, consommes) = decouper(&trame)
                .map_err(|e| TestCaseError::fail(e.to_string()))?
                .ok_or_else(|| TestCaseError::fail("trame incomplète"))?;
            prop_assert_eq!(consommes, trame.len());
            prop_assert_eq!(Requete::from_bytes(charge), Ok(requete));
        }
    }
}
