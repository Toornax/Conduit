//! Le journal que la GUI annonce est bien celui où le démon écrit (M2-13).
//!
//! ADR-010 interdit à la GUI de dépendre de l'engine, et `conduitd` en
//! dépend : la fenêtre ne peut pas importer `conduitd::paths::Paths` pour
//! afficher le chemin du journal, elle le recalcule
//! ([`conduit_gui::demarrage::repertoire_du_journal`]). Ici, en **test**, le
//! démon est une dépendance de développement : les deux règles se comparent,
//! et ne peuvent donc pas diverger en silence.

use conduit_gui::demarrage;
use conduitd::paths::Paths;

#[test]
fn le_journal_annonce_est_celui_du_demon() {
    // Sans répertoires standard — un système sans HOME —, ni l'un ni l'autre
    // n'a de chemin à donner, et il n'y a rien à comparer.
    let Some(paths) = Paths::standard() else {
        assert_eq!(demarrage::repertoire_du_journal(), None);
        return;
    };
    assert_eq!(
        demarrage::repertoire_du_journal(),
        Some(paths.log_dir),
        "la GUI annoncerait un journal que le démon n'écrit pas"
    );
}
