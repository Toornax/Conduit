//! Le binaire du service d'assistance (M1b-20).
//!
//! Un seul exécutable pour trois rôles : il **est** le service (`service`), il
//! l'installe et le désinstalle (`installer`, `desinstaller`, `demarrer`, `etat`), et il
//! sait lui parler en client (`version`, `lister`, `activer`, `desactiver`, `canaux`).
//!
//! Le troisième rôle est ce qui rend le critère de M1b-20 vérifiable tout de suite :
//! depuis une **session ordinaire non élevée**, `conduit-helper activer 1` fait passer
//! l'ordre par le canal nommé, le service l'exécute en `LocalSystem`, et l'endpoint
//! apparaît. `conduitd` fera exactement la même chose en M1b-34.
//!
//! # Codes de sortie
//!
//! | Code | Sens |
//! |---|---|
//! | 0 | l'ordre a abouti |
//! | 1 | le service ou le système a refusé |
//! | 2 | ce binaire ne tourne pas sous Windows |
//!
//! Un ordre **refusé par le pilote** sort en 1 comme un canal absent : de l'extérieur,
//! les deux veulent dire « ça n'a pas marché », et le message dit lequel des deux.

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    {
        windows::executer()
    }
    #[cfg(not(windows))]
    {
        eprintln!(
            "conduit-helper est le service d'assistance Windows de Conduit : il écrit la \
             configuration des câbles dans le pilote noyau, qui n'existe que sous Windows. \
             Rien à exécuter sur cette plateforme."
        );
        std::process::ExitCode::from(2)
    }
}

/// Tout ce qui a besoin de Windows.
#[cfg(windows)]
mod windows {
    use clap::Parser as _;
    use conduit_backend::CableId;
    use conduit_helper::cli::{Args, Commande};
    use conduit_helper::protocole::Requete;
    use conduit_helper::{rapport, scm, tube};
    use std::process::ExitCode;

    /// Analyse la ligne de commande et exécute la sous-commande.
    pub fn executer() -> ExitCode {
        let args = Args::parse();
        let resultat = match args.commande {
            Commande::Installer { demarrer } => installer(demarrer),
            Commande::Desinstaller => desinstaller(),
            Commande::Demarrer => scm::demarrer()
                .map(|()| println!("service {} démarré", scm::NOM_SERVICE))
                .map_err(|e| e.to_string()),
            Commande::Etat => etat(),
            // Ne rend la main qu'à l'arrêt du service.
            Commande::Service => scm::lancer_dispatcher().map_err(|e| e.to_string()),
            Commande::Console { verbeux } => console(verbeux),
            Commande::Version => client(Requete::Version),
            Commande::Lister => client(Requete::Lister),
            Commande::Activer { cable } => client(Requete::Activer(CableId(cable))),
            Commande::Desactiver { cable } => client(Requete::Desactiver(CableId(cable))),
            Commande::Canaux { cable, canaux } => client(Requete::Canaux {
                cable: CableId(cable),
                canaux,
            }),
        };
        match resultat {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("conduit-helper : {message}");
                ExitCode::from(1)
            }
        }
    }

    /// `installer [--demarrer]`.
    fn installer(demarrer: bool) -> Result<(), String> {
        scm::installer().map_err(|e| e.to_string())?;
        println!(
            "service {} installé (LocalSystem, démarrage automatique)",
            scm::NOM_SERVICE
        );
        if demarrer {
            scm::demarrer().map_err(|e| e.to_string())?;
            println!("service démarré");
        } else {
            println!("démarrez-le par « conduit-helper demarrer » ou au prochain redémarrage");
        }
        Ok(())
    }

    /// `desinstaller`.
    fn desinstaller() -> Result<(), String> {
        scm::desinstaller().map_err(|e| e.to_string())?;
        println!("service {} arrêté et supprimé", scm::NOM_SERVICE);
        Ok(())
    }

    /// `etat` : ce que le contrôleur de services dit, **et** ce que le canal répond.
    ///
    /// Les deux, parce qu'ils peuvent diverger : un service « en marche » dont le canal
    /// ne répond pas est un cas qu'il faut savoir distinguer d'un service arrêté, et
    /// c'est l'inverse qui indique un squattage du nom de canal.
    fn etat() -> Result<(), String> {
        let etat = scm::etat().map_err(|e| e.to_string())?;
        println!("service {} : {etat}", scm::NOM_SERVICE);
        let repond = tube::repond();
        println!(
            "canal {} : {}",
            conduit_helper::protocole::NOM_TUBE,
            if repond { "répond" } else { "ne répond pas" }
        );
        if matches!(etat, scm::EtatService::EnMarche) && !repond {
            println!(
                "  le service est démarré mais son canal ne répond pas : consultez {}",
                conduit_helper::journal::chemin_par_defaut().display()
            );
        }
        Ok(())
    }

    /// `console [--verbeux]` : sert en avant-plan jusqu'à `Ctrl+C`.
    fn console(verbeux: bool) -> Result<(), String> {
        println!(
            "service en avant-plan sur {} — Ctrl+C pour arrêter",
            conduit_helper::protocole::NOM_TUBE
        );
        scm::servir_en_console(verbeux).map_err(|e| e.to_string())
    }

    /// Les sous-commandes clientes : envoyer un ordre, afficher la réponse.
    ///
    /// Le code de sortie suit le **statut** de la réponse, pas le fait d'avoir été
    /// servi : un ordre refusé par le pilote est un échec du point de vue de qui l'a
    /// tapé, même si l'échange s'est parfaitement déroulé.
    fn client(requete: Requete) -> Result<(), String> {
        let reponse = tube::demander(requete).map_err(|e| e.to_string())?;
        print!("{}", rapport::rendre(requete, &reponse));
        if reponse.statut.succes() {
            Ok(())
        } else {
            Err(format!("ordre refusé : {}", reponse.statut))
        }
    }
}
