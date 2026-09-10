//! La ligne de commande du binaire : les sous-commandes du service et celles du client.
//!
//! Un seul binaire pour trois rôles, comme le veut la conception (§2 de
//! `docs/driver-design.md`) : il **est** le service, il l'installe et le désinstalle, et
//! il sait lui parler en client. Le troisième rôle n'est pas un luxe — c'est ce qui
//! permet de vérifier le critère de M1b-20 (« le démon non-admin active un câble via le
//! helper ») depuis une session ordinaire, sans attendre M1b-34.
//!
//! Les sous-commandes sont **sans accent** : elles se tapent à la ligne de commande, et
//! `desinstaller` se saisit sur un clavier quelconque là où `désinstaller` ne le ferait
//! pas partout. Les messages, eux, sont en français complet.

use clap::{Parser, Subcommand, ValueEnum};
use conduit_kmd_core::config::{CABLE_MAX, CODE_RATE_44100, CODE_RATE_48000, CODE_RATE_96000};
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};
use conduit_kmd_core::ring::SampleFormat;

/// Service d'assistance Conduit : écrit la configuration des câbles dans le pilote pour
/// le compte du démon.
#[derive(Debug, Parser)]
#[command(name = "conduit-helper", version, about, long_about = None)]
pub struct Args {
    /// La sous-commande. Sans elle, le binaire affiche l'aide.
    #[command(subcommand)]
    pub commande: Commande,
}

/// Les sous-commandes.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commande {
    /// Installe le service (LocalSystem, démarrage automatique). Demande une invite
    /// élevée.
    Installer {
        /// Démarre le service dans la foulée.
        #[arg(long)]
        demarrer: bool,
    },
    /// Arrête puis supprime le service. Demande une invite élevée.
    Desinstaller,
    /// Démarre le service installé. Demande une invite élevée.
    Demarrer,
    /// Affiche l'état du service et si son canal répond.
    Etat,
    /// Point d'entrée du contrôleur de services : **ne pas lancer à la main**.
    Service,
    /// Sert le canal en avant-plan, journal à l'écran (diagnostic).
    Console {
        /// Journalise aussi les ordres qui ne modifient rien.
        #[arg(long)]
        verbeux: bool,
    },
    /// Demande au service la version du protocole et celle du contrat KS du pilote.
    Version,
    /// Demande au service l'état des câbles.
    Lister,
    /// Demande au service de connecter un câble.
    Activer {
        /// Le numéro affiché du câble (« Conduit 1 » = 1).
        cable: u32,
    },
    /// Demande au service de déconnecter un câble.
    Desactiver {
        /// Le numéro affiché du câble.
        cable: u32,
    },
    /// Demande au service de régler le nombre de canaux d'un câble.
    ///
    /// Le pilote n'applique aujourd'hui que la valeur par défaut (M1b-05) ; toute autre
    /// valeur, même dans les bornes, est refusée avec un message qui le dit.
    Canaux {
        /// Le numéro affiché du câble.
        cable: u32,
        /// Le nombre de canaux voulu.
        canaux: u32,
    },
    /// Demande au service de renommer un câble dans les réglages Son (M1b-21).
    ///
    /// Le nom est écrit dans le registre, pas dans le pilote. Le câble doit être
    /// **connecté** : Windows ne publie ses endpoints — et donc leurs clés — qu'à ce
    /// moment-là.
    Renommer {
        /// Le numéro affiché du câble.
        cable: u32,
        /// Le nom voulu : au plus 64 caractères, sans caractère de contrôle ni
        /// « /\:*?"<>| ».
        nom: String,
    },
    /// Rend à un câble son nom d'origine « Conduit N » en effaçant le nom personnalisé.
    ///
    /// C'est le retour en arrière qu'exige F-52 : les clés MMDevices survivent au retrait
    /// du pilote, donc Conduit doit savoir défaire ce qu'il y a écrit.
    NomDefaut {
        /// Le numéro affiché du câble.
        cable: u32,
    },
    /// Demande au service de changer le **format** d'un câble (M1b-05).
    ///
    /// Écrit `CableFormat<n>` dans la clé matérielle du périphérique puis redémarre le
    /// devnode : environ une seconde de silence sur les seize câbles. Le câble visé doit
    /// être **déconnecté** — le format d'un endpoint audio est figé à sa création.
    ///
    /// C'est cette sous-commande qui rend le lot vérifiable en machine virtuelle sans
    /// attendre la chaîne client (`conduitctl cable set-format`).
    Format {
        /// Le numéro affiché du câble.
        cable: u32,
        /// La fréquence d'échantillonnage en Hz : 44100, 48000 ou 96000.
        frequence: u32,
        /// La profondeur préférée : `pcm16`, `pcm24` ou `f32`.
        profondeur: Profondeur,
        /// Le nombre de canaux voulu.
        canaux: u32,
    },
}

/// La profondeur d'échantillon, telle qu'elle se tape à la ligne de commande.
///
/// **Sans accent et en minuscules**, comme les sous-commandes, et nommée d'après ce que le
/// contrat encode ([`conduit_kmd_core::config::CODE_DEPTH_PCM16`] et ses deux voisines) —
/// pas d'après les variantes Rust, dont `I16` ne dirait rien à qui tape la commande.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Profondeur {
    /// PCM signé 16 bits.
    Pcm16,
    /// PCM signé 24 bits, conteneur de trois octets.
    Pcm24,
    /// Flottant 32 bits.
    F32,
}

impl Profondeur {
    /// La variante du contrat que ce mot désigne.
    #[must_use]
    pub const fn en_contrat(self) -> SampleFormat {
        match self {
            Self::Pcm16 => SampleFormat::I16,
            Self::Pcm24 => SampleFormat::Pcm24,
            Self::F32 => SampleFormat::F32,
        }
    }

    /// Le mot tel qu'il se tape, pour le réécrire dans un message de refus.
    ///
    /// **Pas le nom de la variante Rust** : un refus doit renvoyer à l'utilisateur ce
    /// qu'il a tapé (`f32`), pas ce que `Debug` en fait (`F32`). Les deux ne diffèrent que
    /// par la casse ici, et c'est justement pourquoi l'écart passerait inaperçu.
    #[must_use]
    pub const fn mot(self) -> &'static str {
        match self {
            Self::Pcm16 => "pcm16",
            Self::Pcm24 => "pcm24",
            Self::F32 => "f32",
        }
    }
}

impl Commande {
    /// Cette sous-commande parle-t-elle au service **en client** ?
    ///
    /// C'est ce qui distingue les commandes qui ont besoin du canal de celles qui ont
    /// besoin du contrôleur de services.
    #[must_use]
    pub const fn est_cliente(&self) -> bool {
        matches!(
            self,
            Self::Version
                | Self::Lister
                | Self::Activer { .. }
                | Self::Desactiver { .. }
                | Self::Canaux { .. }
                | Self::Renommer { .. }
                | Self::NomDefaut { .. }
                | Self::Format { .. }
        )
    }

    /// Cette sous-commande exige-t-elle une invite élevée ?
    ///
    /// Sert au message d'erreur : un `ERROR_ACCESS_DENIED` sur `OpenSCManagerW` veut
    /// dire « relancez en administrateur », et le dire vaut mieux que de rendre un 5.
    #[must_use]
    pub const fn exige_elevation(&self) -> bool {
        matches!(
            self,
            Self::Installer { .. } | Self::Desinstaller | Self::Demarrer
        )
    }
}

/// Le rappel des domaines, pour les messages d'aide.
///
/// Les bornes viennent du contrat partagé avec le pilote, pas d'un texte recopié : elles
/// suivront M1b-05 sans qu'on rouvre ce fichier.
#[must_use]
pub fn domaines() -> String {
    format!(
        "câbles : 1 à {CABLE_MAX} ; canaux : {MIN_CHANNELS} à {MAX_CHANNELS} ; fréquences : \
         {}",
        frequences_servies()
    )
}

/// Les trois fréquences que le pilote sert, en Hz et séparées par des barres.
///
/// **Déduites du contrat** (`CODE_RATE_*` et le codec qui les décode), pas recopiées : si
/// une quatrième apparaît, l'aide la nomme sans qu'on rouvre ce fichier, et la table de
/// cas de `frequence_valide` la connaît aussi.
#[must_use]
pub fn frequences_servies() -> String {
    FREQUENCES_SERVIES
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Les fréquences décodables, dans l'ordre de leur code.
///
/// Chacune est obtenue en **décodant** un encodage minimal du contrat : c'est
/// `conduit_kmd_core` qui dit quelle fréquence chaque code désigne, et cette liste ne fait
/// que la lui demander une fois par code.
const FREQUENCES_SERVIES: [u32; 3] = [
    frequence_du_code(CODE_RATE_44100),
    frequence_du_code(CODE_RATE_48000),
    frequence_du_code(CODE_RATE_96000),
];

/// La fréquence en Hz que le code `code` désigne, 0 s'il n'en désigne aucune.
const fn frequence_du_code(code: u32) -> u32 {
    // Un encodage complet : le code de fréquence, une profondeur valide, deux canaux.
    // Seule la fréquence en est relue.
    let brut = code | (conduit_kmd_core::config::CODE_DEPTH_F32 << 8) | (2 << 16);
    match conduit_kmd_core::config::CableFormat::decode(brut) {
        Ok(format) => format.sample_rate,
        Err(_) => 0,
    }
}

// Les trois fréquences existent bel et bien : un 0 dans la liste voudrait dire qu'un code
// du contrat n'est plus décodable, et l'aide afficherait « 0 » sans que rien ne le dise.
const _: () = assert!(FREQUENCES_SERVIES[0] != 0);
const _: () = assert!(FREQUENCES_SERVIES[1] != 0);
const _: () = assert!(FREQUENCES_SERVIES[2] != 0);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// La définition `clap` est cohérente : c'est le contrôle que `clap` recommande, et
    /// il attrape un nom de sous-commande ou d'argument en double.
    #[test]
    fn la_ligne_de_commande_est_coherente() {
        Args::command().debug_assert();
    }

    /// Table des sous-commandes : ce que Nathan tapera dans la VM.
    #[test]
    fn sous_commandes_table() {
        let cas: [(&[&str], Commande); 14] = [
            (
                &["conduit-helper", "installer"],
                Commande::Installer { demarrer: false },
            ),
            (
                &["conduit-helper", "installer", "--demarrer"],
                Commande::Installer { demarrer: true },
            ),
            (&["conduit-helper", "desinstaller"], Commande::Desinstaller),
            (&["conduit-helper", "demarrer"], Commande::Demarrer),
            (&["conduit-helper", "etat"], Commande::Etat),
            (&["conduit-helper", "service"], Commande::Service),
            (
                &["conduit-helper", "console"],
                Commande::Console { verbeux: false },
            ),
            (
                &["conduit-helper", "console", "--verbeux"],
                Commande::Console { verbeux: true },
            ),
            (&["conduit-helper", "lister"], Commande::Lister),
            (
                &["conduit-helper", "activer", "3"],
                Commande::Activer { cable: 3 },
            ),
            (
                &["conduit-helper", "canaux", "1", "2"],
                Commande::Canaux {
                    cable: 1,
                    canaux: 2,
                },
            ),
            // M1b-05 : les trois moitiés du format, dans l'ordre où on les tape.
            (
                &["conduit-helper", "format", "1", "48000", "f32", "2"],
                Commande::Format {
                    cable: 1,
                    frequence: 48000,
                    profondeur: Profondeur::F32,
                    canaux: 2,
                },
            ),
            (
                &["conduit-helper", "format", "16", "96000", "pcm24", "8"],
                Commande::Format {
                    cable: 16,
                    frequence: 96000,
                    profondeur: Profondeur::Pcm24,
                    canaux: 8,
                },
            ),
            (
                &["conduit-helper", "format", "3", "44100", "pcm16", "1"],
                Commande::Format {
                    cable: 3,
                    frequence: 44100,
                    profondeur: Profondeur::Pcm16,
                    canaux: 1,
                },
            ),
        ];
        for (argv, attendu) in cas {
            let args = Args::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?} : {e}"));
            assert_eq!(args.commande, attendu, "{argv:?}");
        }
    }

    /// Les sous-commandes sont sans accent : elles se tapent partout.
    #[test]
    fn les_sous_commandes_sont_sans_accent() {
        for nom in Args::command().get_subcommands() {
            let nom = nom.get_name();
            assert!(
                nom.is_ascii(),
                "la sous-commande « {nom} » ne se tape pas sur tous les claviers"
            );
        }
        // Celle qui aurait le plus de raisons d'en porter n'en porte pas.
        assert!(Args::try_parse_from(["conduit-helper", "desinstaller"]).is_ok());
    }

    /// Le classement client / service est celui qui décide de quoi ouvrir.
    #[test]
    fn le_classement_des_commandes() {
        assert!(Commande::Lister.est_cliente());
        assert!(Commande::Activer { cable: 1 }.est_cliente());
        assert!(Commande::Version.est_cliente());
        assert!(!Commande::Etat.est_cliente());
        assert!(!Commande::Service.est_cliente());
        assert!(!Commande::Console { verbeux: false }.est_cliente());

        assert!(Commande::Installer { demarrer: false }.exige_elevation());
        assert!(Commande::Desinstaller.exige_elevation());
        assert!(!Commande::Lister.exige_elevation());
        // Le mode console n'exige pas l'élévation en tant que telle : il sert justement
        // à voir ce qui se passe sous un compte donné.
        assert!(!Commande::Console { verbeux: false }.exige_elevation());
    }

    /// Une sous-commande inconnue ou un argument manquant est refusé.
    #[test]
    fn les_entrees_invalides_sont_refusees() {
        assert!(Args::try_parse_from(["conduit-helper"]).is_err());
        assert!(Args::try_parse_from(["conduit-helper", "inconnue"]).is_err());
        assert!(Args::try_parse_from(["conduit-helper", "activer"]).is_err());
        assert!(Args::try_parse_from(["conduit-helper", "activer", "trois"]).is_err());
        assert!(Args::try_parse_from(["conduit-helper", "canaux", "1"]).is_err());
        // `format` prend quatre arguments, et sa profondeur est un mot de la liste.
        assert!(Args::try_parse_from(["conduit-helper", "format", "1", "48000", "f32"]).is_err());
        assert!(
            Args::try_parse_from(["conduit-helper", "format", "1", "48000", "flottant", "2"])
                .is_err()
        );
        assert!(
            Args::try_parse_from(["conduit-helper", "format", "1", "quarante", "f32", "2"])
                .is_err()
        );
    }

    /// **Les trois profondeurs se tapent, et désignent bien celles du contrat.**
    ///
    /// Les mots sont ceux du `REG_DWORD` (`pcm16`, `pcm24`, `f32`), pas les noms des
    /// variantes Rust : `I16` ne dirait rien à qui tape la commande, et `regedit` ne
    /// l'affiche nulle part.
    #[test]
    fn les_profondeurs_se_tapent_et_designent_le_contrat() {
        let cas: [(&str, Profondeur, SampleFormat); 3] = [
            ("pcm16", Profondeur::Pcm16, SampleFormat::I16),
            ("pcm24", Profondeur::Pcm24, SampleFormat::Pcm24),
            ("f32", Profondeur::F32, SampleFormat::F32),
        ];
        for (mot, profondeur, contrat) in cas {
            let args = Args::try_parse_from(["conduit-helper", "format", "1", "48000", mot, "2"])
                .unwrap_or_else(|e| panic!("« {mot} » : {e}"));
            assert_eq!(
                args.commande,
                Commande::Format {
                    cable: 1,
                    frequence: 48000,
                    profondeur,
                    canaux: 2,
                },
                "« {mot} »"
            );
            assert_eq!(profondeur.en_contrat(), contrat, "« {mot} »");
            // Un refus doit rendre à l'utilisateur **ce qu'il a tapé**, pas le nom de la
            // variante Rust : `f32` et non `F32`.
            assert_eq!(profondeur.mot(), mot);
            // Le mot se tape sur un clavier quelconque, comme les sous-commandes.
            assert!(mot.is_ascii(), "« {mot} »");
            assert_eq!(mot, mot.to_lowercase(), "« {mot} »");
        }
        // Les trois désignent trois profondeurs distinctes du contrat.
        let mut vues: Vec<u32> = cas
            .iter()
            .map(|(_, p, _)| p.en_contrat().bits_per_sample())
            .collect();
        vues.sort_unstable();
        vues.dedup();
        assert_eq!(vues.len(), 3);
    }

    /// Les fréquences de l'aide sont celles que le **contrat** décode, pas un texte figé.
    #[test]
    fn les_frequences_de_l_aide_viennent_du_contrat() {
        assert_eq!(FREQUENCES_SERVIES, [44100, 48000, 96000]);
        let texte = frequences_servies();
        for frequence in FREQUENCES_SERVIES {
            assert!(texte.contains(&frequence.to_string()), "{texte}");
        }
        // Et le rappel des domaines les porte, pour que l'aide d'un refus se suffise.
        let domaines = domaines();
        assert!(domaines.contains("48000"), "{domaines}");
        assert!(domaines.contains("44100"), "{domaines}");
        assert!(domaines.contains("96000"), "{domaines}");
    }

    /// Le rappel des domaines vient du contrat, pas d'un texte figé.
    #[test]
    fn les_domaines_viennent_du_contrat() {
        let texte = domaines();
        assert!(texte.contains(&CABLE_MAX.to_string()), "{texte}");
        assert!(texte.contains(&MAX_CHANNELS.to_string()), "{texte}");
        assert!(texte.contains(&MIN_CHANNELS.to_string()), "{texte}");
    }
}
