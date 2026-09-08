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

use clap::{Parser, Subcommand};
use conduit_kmd_core::config::CABLE_MAX;
use conduit_kmd_core::params::{MAX_CHANNELS, MIN_CHANNELS};

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
    format!("câbles : 1 à {CABLE_MAX} ; canaux : {MIN_CHANNELS} à {MAX_CHANNELS}")
}

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
        let cas: [(&[&str], Commande); 11] = [
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
