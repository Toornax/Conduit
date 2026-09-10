//! La mise en forme d'une [`Reponse`] pour l'écran.
//!
//! **Pur et portable** : la fonction prend une réponse et rend du texte, sans rien lire
//! du système. C'est ce qui permet de vérifier sans Windows ce qu'un utilisateur verra —
//! y compris dans les cas qu'on espère ne jamais voir, comme un pilote absent.

use conduit_backend::CableId;
use conduit_kmd_core::config::{CableFormat, CABLE_MAX};
use conduit_kmd_core::ring::SampleFormat;

use crate::protocole::{Reponse, Requete, Statut};

/// Un format en français, l'encodage brut entre parenthèses.
///
/// L'encodage est gardé parce que c'est **lui** qu'on retrouve dans `regedit` et dans le
/// journal du service : afficher l'un sans l'autre obligerait à traduire de tête pour
/// vérifier ce qui est écrit dans la clé.
#[must_use]
fn format_lisible(format: &CableFormat, brut: u32) -> String {
    let profondeur = match format.depth {
        SampleFormat::I16 => "PCM 16 bits".to_owned(),
        SampleFormat::Pcm24 => "PCM 24 bits".to_owned(),
        SampleFormat::F32 => "float 32 bits".to_owned(),
        // `SampleFormat` est `#[non_exhaustive]` : une quatrième profondeur ajoutée au
        // contrat ne doit pas empêcher ce crate de compiler, ni faire afficher un nom
        // faux. On dit alors ce qu'on sait — le nombre de bits, que le contrat rend pour
        // toutes ses variantes — plutôt que de deviner un libellé.
        autre => format!("{} bits", autre.bits_per_sample()),
    };
    format!(
        "{} Hz, {profondeur}, {} canaux ({brut:#010x})",
        format.sample_rate, format.channels
    )
}

/// Met en forme la réponse à `requete`.
///
/// Trois parties, dans cet ordre : ce que l'ordre a donné, l'état des câbles, et — quand
/// il y en a un — le renseignement chiffré. La liste des câbles n'est affichée que
/// lorsqu'on en connaît au moins un : une liste vide de seize lignes « absent » serait
/// du bruit devant le vrai message, « le pilote n'est pas là ».
#[must_use]
pub fn rendre(requete: &Requete, reponse: &Reponse) -> String {
    let mut texte = String::new();
    let ordre = requete.label();
    if reponse.statut.succes() {
        texte.push_str(&format!("{ordre} : succès\n"));
    } else {
        texte.push_str(&format!("{ordre} : {}\n", reponse.statut));
    }

    // `version` porte la version du **protocole** dans `detail` : c'est le seul ordre où
    // ce champ veut dire cela, et le seul dont c'est la raison d'être.
    if matches!(requete, Requete::Version) && reponse.statut.succes() {
        texte.push_str(&format!(
            "  protocole du service : version {}\n",
            reponse.detail
        ));
    }

    if let Some(precision) = precision(reponse) {
        texte.push_str(&format!("  {precision}\n"));
    }

    if reponse.presents == 0 {
        if reponse.statut.succes() {
            texte
                .push_str("  aucun câble Conduit sur cette machine : le pilote n'est pas chargé\n");
        }
        return texte;
    }

    texte.push_str(&format!(
        "  contrat KS servi par le pilote : version {}\n",
        reponse.version_ks
    ));
    texte.push_str("  câbles :\n");
    for numero in 1..=CABLE_MAX {
        let cable = CableId(numero);
        if !reponse.est_present(cable) {
            continue;
        }
        let etat = if reponse.est_actif(cable) {
            "connecté"
        } else {
            "déconnecté"
        };
        texte.push_str(&format!("    Conduit {numero} : {etat}\n"));
    }
    texte
}

/// Le renseignement chiffré qui accompagne certains statuts, en français.
///
/// Chaque statut décide lui-même de ce que `detail` veut dire : un code Win32, une
/// version, un nombre de canaux. C'est ici, et nulle part ailleurs, que la
/// correspondance se lit.
#[must_use]
pub fn precision(reponse: &Reponse) -> Option<String> {
    match reponse.statut {
        Statut::ErreurSysteme => Some(format!(
            "code rendu par le système : {} (à lire tel quel)",
            reponse.detail
        )),
        Statut::VersionInconnue => Some(format!(
            "le service sert la version {} de ce protocole",
            reponse.detail
        )),
        // Le renseignement qui manquait au défaut de frontière de M1b-05 : **quelle
        // valeur** le pilote attendait. Sans lui, l'utilisateur ne lisait que « refus du
        // système, code 87 ».
        Statut::CanauxNonApplicables => Some(format!(
            "ce câble sert {} canaux : c'est la valeur que la requête doit porter, et en \
             changer demande d'écrire le format du câble dans la clé matérielle du \
             périphérique puis de redémarrer celui-ci",
            reponse.detail
        )),
        Statut::PrivilegeAbsent => Some(
            "le service ne tourne pas en LocalSystem : réinstallez-le par \
             « conduit-helper installer »"
                .to_owned(),
        ),
        Statut::PiloteAbsent => Some(
            "le pilote Conduit n'est pas chargé, ou ce câble n'est pas enregistré \
             (voir docs/driver-dev.md)"
                .to_owned(),
        ),
        // Un format servi qu'on sait lire s'affiche **lisiblement**, pas en hexadécimal :
        // « 48000 Hz, float 32 bits, 2 canaux » se comprend, `0x00020302` non. L'encodage
        // brut reste dans le journal du service, où il sert à retrouver la valeur dans
        // `regedit`.
        Statut::Succes if reponse.format != 0 => Some(match CableFormat::decode(reponse.format) {
            Ok(format) => format!(
                "format du câble visé : {}",
                format_lisible(&format, reponse.format)
            ),
            // Le parseur des réponses refuse déjà les encodages illisibles ; ce bras
            // n'est atteignable que par un appel direct, et il ne ment pas pour autant.
            Err(cause) => format!(
                "format du câble visé illisible ({:#010x}) : {cause}",
                reponse.format
            ),
        }),
        Statut::Succes if reponse.canaux != 0 => {
            Some(format!("canaux du câble visé : {}", reponse.canaux))
        }
        // M1b-05. Le refus **redit la séquence** : le message du statut la porte déjà, et
        // la précision nomme le câble qu'il faut désactiver.
        Statut::CableActif => Some(
            "désactivez le câble, réglez son format, puis réactivez-le — le format d'un \
             endpoint est figé à sa création"
                .to_owned(),
        ),
        Statut::FormatInvalide => Some(format!(
            "encodage refusé : {:#010x} (fréquence, profondeur et canaux sur un octet \
             chacun, poids fort nul)",
            reponse.detail
        )),
        // 14 et 15 disent des choses **opposées**, et c'est tout leur intérêt : ici rien
        // n'a bougé, là-bas la valeur est posée.
        Statut::FormatNonEcrit => Some(format!(
            "rien n'a été écrit : le câble sert toujours son ancien format (code du \
             système : {})",
            reponse.detail
        )),
        Statut::RedemarrageEchoue => Some(format!(
            "le format est écrit mais pas appliqué : il prendra effet au prochain \
             démarrage du périphérique, au redémarrage de la machine au pire (code du \
             système : {})",
            reponse.detail
        )),
        // M1b-21. Le message du statut dit déjà quoi faire (« activez le câble, puis
        // renommez ») ; `detail` n'y ajoute que le nombre de côtés trouvés, qui distingue
        // « rien de publié » de « un endpoint publié à moitié ».
        Statut::EndpointAbsent => Some(format!(
            "{} côté(s) de ce câble trouvé(s) sur 2 dans le registre",
            reponse.detail
        )),
        Statut::Succes
        | Statut::TrameInvalide
        | Statut::OrdreInconnu
        | Statut::CableInconnu
        // Le message du statut porte déjà la règle du nom en entier.
        | Statut::NomInvalide
        | Statut::CanauxInvalides => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Une réponse de succès avec trois câbles présents, dont deux connectés.
    fn peuplee() -> Reponse {
        Reponse {
            ordre: crate::protocole::ORDRE_LISTER,
            statut: Statut::Succes,
            detail: 0,
            presents: 0b0111,
            actifs: 0b0011,
            version_ks: 1,
            canaux: 0,
            format: 0,
            formats: [0; CABLE_MAX as usize],
        }
    }

    /// La liste nomme chaque câble présent, avec son état, et rien d'autre.
    #[test]
    fn la_liste_nomme_les_cables_presents() {
        let texte = rendre(&Requete::Lister, &peuplee());
        assert!(texte.contains("lister : succès"), "{texte}");
        assert!(texte.contains("Conduit 1 : connecté"), "{texte}");
        assert!(texte.contains("Conduit 2 : connecté"), "{texte}");
        assert!(texte.contains("Conduit 3 : déconnecté"), "{texte}");
        // Les câbles absents ne sont pas affichés.
        assert!(!texte.contains("Conduit 4"), "{texte}");
        assert!(texte.contains("version 1"), "{texte}");
        // Trois lignes de câble, pas seize.
        assert_eq!(
            texte.lines().filter(|l| l.contains("Conduit ")).count(),
            3,
            "{texte}"
        );
    }

    /// Sans le pilote, on le dit — au lieu d'aligner seize lignes vides.
    #[test]
    fn sans_pilote_on_le_dit() {
        let vide = Reponse {
            presents: 0,
            actifs: 0,
            version_ks: 0,
            ..peuplee()
        };
        let texte = rendre(&Requete::Lister, &vide);
        assert!(texte.contains("le pilote n'est pas chargé"), "{texte}");
        // Aucune ligne de câble : la phrase ci-dessus est le message, pas un en-tête de
        // liste vide.
        assert!(!texte.contains("Conduit 1 :"), "{texte}");
        assert!(!texte.contains("câbles :"), "{texte}");

        // Un refus « pilote absent » a sa propre précision, qui renvoie à la
        // documentation.
        let refus = Reponse::refus(crate::protocole::ORDRE_ACTIVER, Statut::PiloteAbsent);
        let texte = rendre(&Requete::Activer(CableId(1)), &refus);
        assert!(texte.contains("activer :"), "{texte}");
        assert!(texte.contains("driver-dev.md"), "{texte}");
    }

    /// Table des précisions : chaque statut dit ce que son détail signifie.
    #[test]
    fn precision_table() {
        let cas: [(Statut, u32, Option<&str>); 12] = [
            (Statut::Succes, 0, None),
            (Statut::TrameInvalide, 0, None),
            (Statut::OrdreInconnu, 0, None),
            (Statut::CableInconnu, 0, None),
            (Statut::ErreurSysteme, 1314, Some("1314")),
            (Statut::VersionInconnue, 1, Some("version 1")),
            // Le détail est le compte **servi par le câble**, et il doit se lire tel quel.
            (Statut::CanauxNonApplicables, 6, Some("6 canaux")),
            (Statut::PrivilegeAbsent, 0, Some("LocalSystem")),
            // Les quatre de M1b-05.
            (Statut::CableActif, 0, Some("désactivez")),
            // Le détail est le `u32` refusé, relisible en hexadécimal.
            (Statut::FormatInvalide, 0xFF02_0302, Some("0xff020302")),
            (Statut::FormatNonEcrit, 5, Some("rien n'a été écrit")),
            (
                Statut::RedemarrageEchoue,
                23,
                Some("écrit mais pas appliqué"),
            ),
        ];
        for (statut, detail, attendu) in cas {
            let reponse = Reponse::refus_detaille(0, statut, detail);
            let precision = precision(&reponse);
            match attendu {
                None => assert_eq!(precision, None, "{statut:?}"),
                Some(fragment) => {
                    let texte = precision.unwrap_or_default();
                    assert!(texte.contains(fragment), "{statut:?} : {texte}");
                }
            }
        }
        // Les douze cas couvrent les seize statuts moins ceux dont le message se suffit.
        assert_eq!(cas.len(), 12);

        // **Le refus « câble actif » redit la séquence en entier.**
        let actif = precision(&Reponse::refus(0, Statut::CableActif)).unwrap_or_default();
        assert!(actif.contains("désactivez"), "{actif}");
        assert!(actif.contains("réactivez"), "{actif}");

        // **14 et 15 disent des choses opposées** : rien n'a bougé contre la valeur est
        // posée. Les confondre coûterait à l'utilisateur la mauvaise réparation.
        let non_ecrit =
            precision(&Reponse::refus_detaille(0, Statut::FormatNonEcrit, 5)).unwrap_or_default();
        assert!(non_ecrit.contains("rien n'a été écrit"), "{non_ecrit}");
        assert!(non_ecrit.contains("ancien format"), "{non_ecrit}");
        let echoue = precision(&Reponse::refus_detaille(0, Statut::RedemarrageEchoue, 23))
            .unwrap_or_default();
        assert!(echoue.contains("écrit"), "{echoue}");
        assert!(echoue.contains("prochain démarrage"), "{echoue}");
        assert!(echoue.contains("23"), "{echoue}");

        // Un succès qui porte un nombre de canaux le dit.
        let avec_canaux = Reponse {
            canaux: 2,
            ..peuplee()
        };
        let texte = precision(&avec_canaux).unwrap_or_default();
        assert!(texte.contains('2'), "{texte}");
    }

    /// **Un format servi s'affiche en français**, pas en hexadécimal nu.
    ///
    /// L'encodage reste là, entre parenthèses : c'est lui qu'on retrouve dans `regedit` et
    /// dans le journal du service, et devoir le traduire de tête pour vérifier ce qui est
    /// écrit dans la clé serait une corvée gratuite.
    #[test]
    fn un_succes_affiche_le_format_lisiblement() {
        let cas: [(u32, &str, &str, &str); 3] = [
            (0x0002_0302, "48000 Hz", "float 32 bits", "2 canaux"),
            (0x0006_0103, "96000 Hz", "PCM 16 bits", "6 canaux"),
            (0x0001_0201, "44100 Hz", "PCM 24 bits", "1 canaux"),
        ];
        for (brut, frequence, profondeur, canaux) in cas {
            let reponse = Reponse {
                format: brut,
                ..peuplee()
            };
            let texte = precision(&reponse).unwrap_or_default();
            assert!(texte.contains(frequence), "{brut:#010x} : {texte}");
            assert!(texte.contains(profondeur), "{brut:#010x} : {texte}");
            assert!(texte.contains(canaux), "{brut:#010x} : {texte}");
            // L'encodage brut est là aussi, sous la forme qu'on lit dans `regedit`.
            assert!(
                texte.contains(&format!("{brut:#010x}")),
                "{brut:#010x} : {texte}"
            );
            // Et il apparaît dans le rendu complet, sur l'ordre qui vise un câble.
            let rendu = rendre(&Requete::Activer(CableId(1)), &reponse);
            assert!(rendu.contains(frequence), "{rendu}");
        }

        // Sans format connu, on n'invente rien : c'est le nombre de canaux qui parle, et
        // faute de canaux, rien du tout.
        let sans = Reponse {
            format: 0,
            canaux: 0,
            ..peuplee()
        };
        assert_eq!(precision(&sans), None);
    }

    /// L'ordre `version` affiche la version du protocole, et lui seul.
    ///
    /// Le champ `detail` veut dire autre chose pour chaque statut ; l'afficher comme une
    /// version partout donnerait « protocole version 1314 » sur un refus du pilote.
    #[test]
    fn seul_l_ordre_version_affiche_la_version_du_protocole() {
        let reponse = Reponse {
            ordre: crate::protocole::ORDRE_VERSION,
            detail: u32::from(crate::protocole::PROTOCOLE_VERSION),
            ..peuplee()
        };
        let texte = rendre(&Requete::Version, &reponse);
        // La version affichée est celle du protocole, pas un chiffre recopié : le test
        // suit `PROTOCOLE_VERSION` quand elle bouge (2 depuis M1b-21).
        assert!(
            texte.contains(&format!(
                "protocole du service : version {}",
                crate::protocole::PROTOCOLE_VERSION
            )),
            "{texte}"
        );

        // Le même champ sur un autre ordre ne s'affiche pas comme une version.
        let texte = rendre(&Requete::Lister, &reponse);
        assert!(!texte.contains("protocole du service"), "{texte}");

        // Ni sur un refus de `version`.
        let refus =
            Reponse::refus_detaille(crate::protocole::ORDRE_VERSION, Statut::ErreurSysteme, 1314);
        let texte = rendre(&Requete::Version, &refus);
        assert!(!texte.contains("protocole du service"), "{texte}");
        assert!(texte.contains("1314"), "{texte}");
    }

    /// Le code rendu par le système est affiché **tel quel** : c'est le seul
    /// renseignement qui distingue les refus entre eux.
    #[test]
    fn le_code_du_systeme_est_affiche_tel_quel() {
        for code in [1314, 5, 87, 2] {
            let reponse = Reponse::refus_detaille(
                crate::protocole::ORDRE_ACTIVER,
                Statut::ErreurSysteme,
                code,
            );
            let texte = rendre(&Requete::Activer(CableId(1)), &reponse);
            assert!(texte.contains(&code.to_string()), "code {code} : {texte}");
        }
    }
}
