//! Plan de copie de la boucle locale rendu → capture (`docs/driver-design.md` §5.3,
//! étapes 1 à 4).
//!
//! À chaque tick du timer du câble (1 ms), le pilote lit la position absolue en trames
//! de chaque flux en `RUN` ([`StreamView`]) et demande à [`Loopback::plan`] quoi
//! écrire dans le tampon de capture : une copie depuis le tampon de rendu
//! ([`CopyOp`]), du silence ([`SilenceOp`]), ou rien. Le pilote exécute ensuite le plan
//! avec [`crate::ring::copy_frames`] et [`crate::ring::silence`]. Toute l'arithmétique
//! est ici, testable sans noyau ; le pilote ne fait que lire des positions et déplacer
//! des octets.
//!
//! # Fenêtres sûres : écrire et lire *en avance* des positions
//!
//! Le moteur audio écrit le tampon de rendu **devant** la position de lecture `R`
//! (SPEC : le lecteur écrit) et, réveillé par notre notification à une frontière de
//! période `R_b`, réécrit aussitôt la moitié qu'il vient de jouer, c'est-à-dire les
//! emplacements des trames `[R_b − période, R_b)`. Copier derrière `R` avec une marge
//! (`R − 1 ms`) laisserait donc, à chaque frontière, la dernière milliseconde jouée être
//! écrasée avant sa copie. Symétriquement, le moteur lit le tampon de capture
//! **derrière** la position `C` : une trame écrite sous `C` arrive trop tard.
//!
//! La copie lit et écrit donc **en avance** : à chaque tick, les trames de capture
//! `[curseur, C + avance)` sont écrites, depuis les trames de rendu correspondantes
//! (jusqu'à `R + avance`), où `avance` ([`Loopback::lead_frames`], [`LEAD_MS`]) couvre
//! la période du tick et sa gigue. Ces trames ont été écrites par le lecteur au moins
//! une période plus tôt et ne seront réécrites qu'une fois jouées ; côté capture, elles
//! sont en place avant que `C` ne les atteigne et l'emplacement qu'elles occupent a été
//! lu une période plus tôt (tampon d'au moins deux périodes : c'est ce que le moteur
//! demande). Tant que `avance` reste inférieure à une période de notification, elle ne
//! touche jamais la moitié en cours d'écriture ou de lecture par le moteur.
//!
//! # Décalage fixe rendu ↔ capture
//!
//! Au premier tick où les deux flux sont en `RUN`, le plan mémorise les deux positions
//! (`R0`, `C0`) : la trame de capture `j` reçoit la trame de rendu `k = j − C0 + R0`,
//! « même instant virtuel ». Les deux positions avancent au même rythme (même compteur
//! de performance, même fréquence d'échantillonnage), la latence est donc constante et,
//! en positions, nulle : la trame est « capturée » à l'instant où elle est « jouée ».
//! Le lien est oublié dès que l'un des deux flux quitte `RUN` et rétabli au retour.
//!
//! # Débordements
//!
//! Si le tick a pris tant de retard que le bloc à écrire dépasse la taille du plus petit
//! tampon (`count > min(rendu, capture)`), les trames sont irrécupérables (déjà
//! réécrites d'un côté, déjà lues de l'autre) : le plan signale un débordement
//! ([`Plan::overrun`]), n'écrit rien et resynchronise le curseur sur `C + avance`. Le
//! **lien survit** au débordement : un tick en retard fait un trou dans la capture, il
//! ne décale pas l'alignement rendu ↔ capture des trames suivantes.
//!
//! # Un seul côté ouvert : deux régimes, et de quoi les mesurer
//!
//! Sans rendu en `RUN`, la capture reçoit du silence (SPEC §5.3 : « l'entrée sans
//! producteur lit du silence ») ; sans capture en `RUN`, rien n'est copié (« la sortie
//! sans lecteur est jetée »). Les deux comportements sont asymétriques, et la trace
//! qu'ils laissent l'était trop : rien ne permettait de les **constater** en machine.
//!
//! - Le silence a **deux causes** ([`SilenceCause`]) qu'une simple quantité de trames
//!   confond : l'absence de producteur, régime permanent qui dure tant que le rendu
//!   n'ouvre pas, et le rendu plus jeune que le lien, transitoire et borné par le
//!   décalage de ce lien. Un compteur unique montrerait la même chose dans les deux cas,
//!   et « la capture seule lit du silence » ne se distinguerait pas de « le rendu vient
//!   de démarrer ».
//! - Le rendu seul ne produit **rien du tout** : ni copie, ni silence, ni débordement.
//!   Sans témoin, ce cas serait indistinguable d'un câble au repos, et « aucune
//!   accumulation » ne se vérifierait que par l'absence d'une preuve. D'où
//!   [`Plan::discarded`], le seul champ du plan qui ne décrit pas des octets à écrire mais
//!   des octets qu'on a choisi de ne pas garder.
//!
//! Aucun des deux ne change ce que le tick écrit : ce sont des **témoins**, que
//! `conduit_kmd::cable::Counters` additionne séparément.
//!
//! # Une frontière certaine : [`StreamView::committed`]
//!
//! Tout ce qui précède raisonne sur une **supposition** : le lecteur a écrit devant `R`,
//! donc lire jusqu'à `R + avance` est sûr. Le mode paquets remplace cette supposition par
//! une frontière que le client **déclare** — `SetWritePacket` dit « j'ai écrit le paquet
//! *n* », donc les trames `[n × taille, (n+1) × taille)` et rien au-delà (voir
//! [`crate::packetnum`]).
//!
//! Quand cette frontière existe, **une** chose change, et une seule : la copie ne lit
//! jamais au-delà (`src_start + count ≤ committed`), quitte à tronquer le bloc du tick.
//! Lire plus loin serait lire un emplacement que le client n'a pas encore rempli —
//! précisément ce que l'avance suppose sans le savoir. La fenêtre **lue** s'arrête donc à
//! `min(R + avance, committed)` au lieu de `R + avance`.
//!
//! # Dissymétrie source ↔ cible : la certitude borne la lecture, pas l'écriture
//!
//! L'avance, elle, reste entière **côté capture** : la fenêtre écrite est
//! `[curseur, C + avance)`, bornée ou non. Il est tentant de la mettre aussi à zéro — une
//! marge de sécurité par-dessus une certitude n'est plus une marge, c'est un décalage —
//! mais ce raisonnement ne vaut que pour la source. `committed` ne dit rien de `C` : il
//! parle de ce que le client a écrit dans le tampon de **rendu**, pas de l'endroit où le
//! moteur en est dans le tampon de **capture**. Là, l'avertissement du début de module
//! tient toujours : une trame écrite sous `C` arrive trop tard, le moteur est déjà passé
//! dessus.
//!
//! Mesure du 2026-09-10 (mode paquets, exclusif servi, dix passes de huit secondes), avec
//! l'avance mise à zéro des **deux** côtés : huit passes sur dix, et deux passes avec
//! quatre puis huit sauts de phase **sub-trame** (~0,02 rad pour un seuil de 0,0173) —
//! zéro trou, amplitude intacte. La signature exacte d'une écriture qui atterrit sous le
//! curseur du lecteur et perd la course par instants. D'où la dissymétrie : avance nulle
//! côté source, où la frontière la remplace ; avance intacte côté cible, où rien ne la
//! remplace.
//!
//! Conséquence sur la correspondance : la cible garde son ancrage `C + avance`, donc la
//! source vise toujours `R + avance` — le décalage du lien est inchangé, c'est la même
//! géométrie que le chemin sans frontière. Seule la **fin** du bloc source peut être
//! rabotée par `committed` ; les trames au-delà n'existent pas encore, elles sont
//! simplement moins nombreuses et le silence comble comme pour toute sous-alimentation.
//!
//! Le curseur, le lien, les causes de silence, le témoin de rejet et le débordement sont
//! **inchangés** : la frontière borne ce qu'on lit, elle ne change pas la façon dont on
//! avance ni la façon dont on rattrape.
//!
//! IRQL : tout ici est appelable à `DISPATCH_LEVEL` : aucune allocation, aucune panique.

/// Avance de la copie sur les positions, en millisecondes : deux ticks du timer
/// (période 1 ms plus une gigue d'un tick).
pub const LEAD_MS: u32 = 2;

/// Un flux en `RUN` avec un tampon, vu à l'instant du tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamView {
    /// Position absolue en trames ([`crate::position::StreamPosition::frames_at`]).
    pub frames: u64,
    /// Taille du tampon cyclique en trames (≥ 1).
    pub buffer_frames: u64,
    /// Première trame absolue que le client **n'a pas** validée, `None` s'il ne valide
    /// rien (voir « Une frontière certaine » en tête de module).
    ///
    /// Renseignée uniquement sur la vue du **rendu**, et uniquement en mode paquets :
    /// c'est `(dernier paquet accepté + 1) × taille_de_paquet`, en trames absolues. Une
    /// capture ne reçoit jamais de `SetWritePacket` et garde donc `None`.
    ///
    /// Elle borne la fenêtre **lue** dans le tampon de rendu, et rien d'autre : l'avance
    /// ([`LEAD_MS`]) reste entière sur la position d'**écriture** dans la capture, où
    /// aucune déclaration du client ne la remplace (voir « Dissymétrie source ↔ cible »
    /// en tête de module, et la mesure du 2026-09-10 qui l'a établie).
    pub committed: Option<u64>,
}

impl StreamView {
    /// Vue d'un flux sans frontière validée : le régime ordinaire, où la copie s'appuie
    /// sur la position et sur l'avance.
    pub const fn new(frames: u64, buffer_frames: u64) -> Self {
        Self {
            frames,
            buffer_frames,
            committed: None,
        }
    }

    /// La même vue, bornée par les trames que le client a validées.
    pub const fn with_committed(self, committed: u64) -> Self {
        Self {
            committed: Some(committed),
            ..self
        }
    }
}

/// Copie de `count` trames du rendu (à partir de la trame absolue `src_start`) vers
/// la capture (à partir de la trame absolue `dst_start`), pour
/// [`crate::ring::copy_frames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyOp {
    /// Première trame absolue du rendu.
    pub src_start: u64,
    /// Première trame absolue de la capture.
    pub dst_start: u64,
    /// Nombre de trames (≥ 1, ≤ taille du plus petit tampon).
    pub count: u64,
}

/// Pourquoi un tick écrit du silence. Deux causes, un seul geste : `ring::silence` n'en
/// tient aucun compte, mais les compteurs du pilote les additionnent séparément (voir
/// l'en-tête de module).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SilenceCause {
    /// Aucun flux de rendu en `RUN` : « l'entrée sans producteur lit du silence »
    /// (SPEC §5.3). **Régime permanent** : tant que le rendu n'ouvre pas, chaque tick
    /// écrit son avance de silence, indéfiniment.
    NoRender,
    /// Un rendu est bien en `RUN`, mais les trames demandées sont antérieures à son
    /// départ (`k < 0`, voir « Décalage fixe rendu ↔ capture »). **Régime transitoire** :
    /// la quantité est bornée par le décalage du lien et ne se reproduit pas tant que ce
    /// lien tient.
    BeforeRenderStart,
}

/// `count` trames de silence dans la capture à partir de la trame absolue
/// `dst_start`, pour [`crate::ring::silence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilenceOp {
    /// Première trame absolue de la capture.
    pub dst_start: u64,
    /// Nombre de trames (≥ 1, ≤ taille du tampon de capture).
    pub count: u64,
    /// Pourquoi ces trames sont du silence plutôt qu'une copie.
    pub cause: SilenceCause,
}

/// Ce qu'un tick doit écrire dans le tampon de capture. Le silence, s'il y en a,
/// précède la copie (trames de rendu antérieures au début du flux).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Plan {
    /// Silence à écrire.
    pub silence: Option<SilenceOp>,
    /// Copie à faire.
    pub copy: Option<CopyOp>,
    /// Le tick était trop en retard : rien n'est écrit, le curseur est resynchronisé.
    pub overrun: bool,
    /// Un lien rendu ↔ capture vient d'être établi à ce tick.
    pub linked: bool,
    /// Un rendu tournait **sans capture** : ses trames sont jetées (SPEC §5.3, « la
    /// sortie sans lecteur est jetée »).
    ///
    /// Le seul champ qui ne demande rien : les quatre autres sont vides dans ce cas, et
    /// c'est précisément le problème qu'il résout — sans lui, « le rendu tournait seul »
    /// et « rien ne tournait » rendent le même plan vide, et la non-accumulation ne se
    /// constate pas, elle se suppose.
    pub discarded: bool,
}

/// Positions des deux flux au moment où le lien a été établi (même instant virtuel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link {
    render: u64,
    capture: u64,
}

/// État de la boucle locale d'un câble entre deux ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Loopback {
    /// Prochaine trame absolue de la capture à écrire ; `None` tant que la capture n'a
    /// pas été vue en `RUN`.
    cursor: Option<u64>,
    /// Lien rendu ↔ capture courant.
    link: Option<Link>,
}

impl Loopback {
    /// Boucle sans curseur ni lien.
    pub const fn new() -> Self {
        Self {
            cursor: None,
            link: None,
        }
    }

    /// Oublie curseur et lien (arrêt du périphérique, fermeture d'un flux).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Vrai si un lien rendu ↔ capture est établi.
    pub const fn is_linked(&self) -> bool {
        self.link.is_some()
    }

    /// Avance en trames pour `sample_rate` trames/s : [`LEAD_MS`] millisecondes, au
    /// moins une trame.
    pub const fn lead_frames(sample_rate: u32) -> u64 {
        // Diviseur constant non nul : `checked_div` ne rend jamais `None` ici.
        let per_ms = match sample_rate.checked_div(1000) {
            Some(per_ms) => per_ms,
            None => 0,
        };
        let lead = (per_ms as u64).saturating_mul(LEAD_MS as u64);
        if lead == 0 {
            1
        } else {
            lead
        }
    }

    /// Calcule le plan d'un tick : `render` et `capture` sont les flux en `RUN` avec un
    /// tampon (`None` sinon), `lead` l'avance en trames ([`Self::lead_frames`]).
    ///
    /// Invariants garantis (voir les tests) : les blocs écrits d'un tick à l'autre se
    /// suivent sans trou ni recouvrement tant qu'il n'y a pas de débordement ; la copie
    /// respecte le décalage du lien ; `count` ne dépasse jamais la taille du plus petit
    /// tampon concerné. Une position de capture qui recule (le flux a été arrêté et
    /// relancé entre deux ticks) repart d'un curseur neuf.
    ///
    /// IRQL : `<= DISPATCH_LEVEL`.
    pub fn plan(
        &mut self,
        render: Option<StreamView>,
        capture: Option<StreamView>,
        lead: u64,
    ) -> Plan {
        let mut plan = Plan::default();
        let Some(capture) = capture else {
            // Rendu seul : le curseur et le lien sont oubliés, aucun tampon n'est même
            // consulté, et le tick ne laisserait aucune trace sans ce témoin.
            self.reset();
            plan.discarded = render.is_some();
            return plan;
        };
        // La frontière validée par le client, s'il en déclare une. Elle borne ce qu'on
        // **lit** dans le rendu (plus bas, une fois `src_start` connu) et rien d'autre :
        // l'avance reste entière sur la cible, où elle ne couvre pas l'incertitude sur `R`
        // mais la course avec le lecteur de capture (voir « Dissymétrie source ↔ cible »
        // en tête de module).
        let committed = render.and_then(|r| r.committed);
        let target = capture.frames.saturating_add(lead);
        let mut start = match self.cursor {
            Some(cursor) if cursor <= target => cursor,
            _ => capture.frames,
        };
        let mut count = target.saturating_sub(start);
        let cap = render.map_or(capture.buffer_frames, |r| {
            r.buffer_frames.min(capture.buffer_frames)
        });
        if count > cap {
            plan.overrun = true;
            start = target;
            count = 0;
        }
        self.cursor = Some(target);

        let Some(render) = render else {
            self.link = None;
            if count > 0 {
                plan.silence = Some(SilenceOp {
                    dst_start: start,
                    count,
                    cause: SilenceCause::NoRender,
                });
            }
            return plan;
        };
        let link = match self.link {
            Some(link) => link,
            None => {
                let link = Link {
                    render: render.frames,
                    capture: capture.frames,
                };
                self.link = Some(link);
                plan.linked = true;
                link
            }
        };
        if count == 0 {
            return plan;
        }
        // k = j − C0 + R0, en i128 pour absorber un décalage négatif.
        let src_start = i128::from(start)
            .saturating_sub(i128::from(link.capture))
            .saturating_add(i128::from(link.render));
        let (src_start, dst_start, mut count) = if src_start < 0 {
            // Trames de rendu « d'avant le départ » du flux : silence, puis copie du
            // reste à partir de la trame 0.
            let missing = u64::try_from(src_start.saturating_neg())
                .unwrap_or(u64::MAX)
                .min(count);
            plan.silence = Some(SilenceOp {
                dst_start: start,
                count: missing,
                cause: SilenceCause::BeforeRenderStart,
            });
            (
                0,
                start.saturating_add(missing),
                count.saturating_sub(missing),
            )
        } else {
            (u64::try_from(src_start).unwrap_or(u64::MAX), start, count)
        };
        // La borne du client, appliquée en dernier : le bloc est tronqué à ce qu'il a
        // réellement écrit, jamais étendu.
        //
        // Le curseur, lui, a déjà avancé à `target` : une troncature laisse donc un trou,
        // comme un débordement. C'est délibéré et c'est le seul comportement honnête — les
        // trames manquantes n'existent pas encore côté rendu, et rien ne les fera exister
        // en les attendant. En régime normal la troncature ne se produit pas : le client
        // valide un paquet **d'avance** sur la position, et un paquet dure plus longtemps
        // que les deux millisecondes de l'avance, donc `committed` dépasse la fin du bloc,
        // qui s'arrête à `R + avance`.
        if let Some(committed) = committed {
            count = count.min(committed.saturating_sub(src_start));
        }
        if count > 0 {
            plan.copy = Some(CopyOp {
                src_start,
                dst_start,
                count,
            });
        }
        plan
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::vec::Vec;

    const LEAD: u64 = 96; // 2 ms à 48 kHz

    fn view(frames: u64, buffer_frames: u64) -> Option<StreamView> {
        Some(StreamView::new(frames, buffer_frames))
    }

    /// La même vue, bornée par ce que le client a validé (mode paquets).
    fn view_committed(frames: u64, buffer_frames: u64, committed: u64) -> Option<StreamView> {
        Some(StreamView::new(frames, buffer_frames).with_committed(committed))
    }

    /// Un plan qui ne demande rien et ne jette rien : le câble au repos.
    const REPOS: Plan = Plan {
        silence: None,
        copy: None,
        overrun: false,
        linked: false,
        discarded: false,
    };

    /// Le plan d'un tick où le rendu tourne sans capture : rien à écrire, mais un témoin.
    const JETE: Plan = Plan {
        silence: None,
        copy: None,
        overrun: false,
        linked: false,
        discarded: true,
    };

    #[test]
    fn l_avance_vaut_deux_millisecondes() {
        assert_eq!(Loopback::lead_frames(48_000), 96);
        assert_eq!(Loopback::lead_frames(44_100), 88);
        assert_eq!(Loopback::lead_frames(8_000), 16);
        assert_eq!(Loopback::lead_frames(0), 1);
        assert_eq!(Loopback::lead_frames(999), 1);
    }

    #[test]
    fn sans_capture_rien_n_est_ecrit_et_tout_est_oublie() {
        let mut lb = Loopback::new();
        assert!(lb.plan(view(1_000, 960), view(500, 960), LEAD).linked);
        assert!(lb.is_linked());
        // Le plan ne demande rien — mais il dit que le rendu tournait pour rien.
        assert_eq!(lb.plan(view(1_048, 960), None, LEAD), JETE);
        assert!(!lb.is_linked());
        assert_eq!(lb, Loopback::default());
        // Les deux côtés fermés : même plan vide, sans le témoin.
        assert_eq!(lb.plan(None, None, LEAD), REPOS);
    }

    #[test]
    fn la_capture_seule_recoit_du_silence_en_avance() {
        let mut lb = Loopback::new();
        // Premier tick : [C, C + avance).
        let p = lb.plan(None, view(500, 960), LEAD);
        assert_eq!(
            p,
            Plan {
                silence: Some(SilenceOp {
                    dst_start: 500,
                    count: LEAD,
                    cause: SilenceCause::NoRender
                }),
                ..Plan::default()
            }
        );
        // Tick suivant, 48 trames plus tard : [C_prev + avance, C + avance).
        let p = lb.plan(None, view(548, 960), LEAD);
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 596,
                count: 48,
                cause: SilenceCause::NoRender
            })
        );
        assert!(p.copy.is_none() && !p.overrun && !p.linked && !p.discarded);
        // Horloge immobile : rien.
        assert_eq!(lb.plan(None, view(548, 960), LEAD), REPOS);
    }

    #[test]
    fn le_lien_associe_le_meme_instant_virtuel() {
        let mut lb = Loopback::new();
        // Capture seule pendant deux ticks : silence [500, 596) puis [596, 644), le
        // curseur est donc à 644 quand le rendu arrive.
        lb.plan(None, view(500, 960), LEAD);
        lb.plan(None, view(548, 960), LEAD);
        let p = lb.plan(view(10_000, 480), view(596, 960), LEAD);
        assert!(p.linked && p.silence.is_none() && !p.overrun);
        // Lien R0 = 10 000, C0 = 596. Bloc capture [644, 692) (curseur → C + avance) ;
        // source k = 644 − 596 + 10 000 = 10 048, fin 10 096 = R + avance.
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 10_048,
                dst_start: 644,
                count: 48
            })
        );
        // Tick suivant : les deux avancent de 48 ; décalage inchangé, blocs contigus.
        let p = lb.plan(view(10_048, 480), view(644, 960), LEAD);
        assert!(!p.linked);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 10_096,
                dst_start: 692,
                count: 48
            })
        );
    }

    #[test]
    fn le_rendu_anterieur_a_son_origine_devient_du_silence() {
        let mut lb = Loopback::new();
        // Rendu à peine démarré (R = 10), capture loin devant : les trames de rendu
        // « négatives » deviennent du silence, le reste est copié depuis 0.
        let p = lb.plan(view(10, 480), view(5_000, 960), LEAD);
        assert!(p.linked);
        // Bloc capture [5 000, 5 096) ↔ rendu [10, 106) : tout positif ici.
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 10,
                dst_start: 5_000,
                count: 96
            })
        );
        // Cas négatif : curseur en retard sur C0 après un redémarrage de la capture.
        let mut lb = Loopback::new();
        // Curseur 196 ; puis la capture recule (arrêt/relance) à 20 : curseur neuf à
        // 20, cible 116.
        lb.plan(None, view(100, 960), LEAD);
        let p = lb.plan(view(5, 480), view(20, 960), LEAD);
        assert!(p.linked);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 5,
                dst_start: 20,
                count: 96
            })
        );
        // Lien C0 = 20, R0 = 5 ; forcer un bloc sous C0 − R0 est impossible par
        // construction (curseur ≥ C0) : vérifier tout de même le chemin via un lien
        // établi avec R0 < C0 puis un curseur réinitialisé plus bas.
        let mut lb = Loopback::new();
        // Lien R0 = 0, C0 = 50, curseur 146 ; puis recul de la capture à 10 : curseur
        // neuf à 10, k = 10 − 50 + 0 = −40.
        lb.plan(view(0, 480), view(50, 960), LEAD);
        let p = lb.plan(view(96, 480), view(10, 960), LEAD);
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 10,
                count: 40,
                cause: SilenceCause::BeforeRenderStart
            })
        );
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 0,
                dst_start: 50,
                count: 56
            })
        );
    }

    #[test]
    fn le_debordement_saute_et_resynchronise() {
        let mut lb = Loopback::new();
        // Curseur 596 ; puis 20 ms de retard : bloc de 960 + 96 > 480 (plus petit
        // tampon).
        lb.plan(view(1_000, 480), view(500, 960), LEAD);
        let p = lb.plan(view(1_960, 480), view(1_460, 960), LEAD);
        assert!(p.overrun && p.copy.is_none() && p.silence.is_none());
        // Le lien survit au débordement : un tick en retard ne décale pas
        // l'alignement rendu ↔ capture (lien R0 = 1 000, C0 = 500 conservé).
        assert!(!p.linked && lb.is_linked());
        // Resynchronisé : le tick suivant copie un bloc normal.
        let p = lb.plan(view(2_008, 480), view(1_508, 960), LEAD);
        assert!(!p.linked);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 2_056,
                dst_start: 1_556,
                count: 48
            })
        );
        // Sans rendu, la borne est le tampon de capture.
        let mut lb = Loopback::new();
        lb.plan(None, view(0, 960), LEAD);
        let p = lb.plan(None, view(2_000, 960), LEAD);
        assert!(p.overrun);
        let p = lb.plan(None, view(2_048, 960), LEAD);
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 2_096,
                count: 48,
                cause: SilenceCause::NoRender
            })
        );
    }

    #[test]
    fn l_arret_du_rendu_oublie_le_lien_et_fait_silence() {
        let mut lb = Loopback::new();
        lb.plan(view(1_000, 480), view(500, 960), LEAD);
        assert!(lb.is_linked());
        let p = lb.plan(None, view(548, 960), LEAD);
        assert!(!lb.is_linked());
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 596,
                count: 48,
                cause: SilenceCause::NoRender
            })
        );
        // Retour du rendu : nouveau lien.
        let p = lb.plan(view(7_000, 480), view(596, 960), LEAD);
        assert!(p.linked);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 7_048,
                dst_start: 644,
                count: 48
            })
        );
    }

    #[test]
    fn tout_sature_a_u64_max() {
        let mut lb = Loopback::new();
        let p = lb.plan(view(u64::MAX, 480), view(u64::MAX, 960), LEAD);
        assert!(p.linked);
        // cible = u64::MAX (saturée), curseur neuf à C = u64::MAX : rien à écrire.
        assert!(p.copy.is_none() && p.silence.is_none() && !p.overrun);
        let p = lb.plan(view(u64::MAX, 480), view(u64::MAX, 960), LEAD);
        assert_eq!(p, Plan::default());
    }

    // -----------------------------------------------------------------------------
    // M1b-07 : un seul côté ouvert.
    // -----------------------------------------------------------------------------

    /// **Le critère de M1b-07, en table** : les quatre combinaisons d'ouverture, chacune
    /// depuis une boucle neuve. Ce que le tick écrit, et la trace qu'il laisse.
    #[test]
    fn un_seul_cote_ouvert_table() {
        /// Une ligne : ce que le tick voit, le plan attendu, l'état du lien après.
        struct Cas {
            nom: &'static str,
            render: Option<StreamView>,
            capture: Option<StreamView>,
            attendu: Plan,
            lie: bool,
        }
        let cas = [
            Cas {
                nom: "les deux fermés : rien, et rien à signaler",
                render: None,
                capture: None,
                attendu: REPOS,
                lie: false,
            },
            Cas {
                nom: "capture seule : du silence, faute de producteur",
                render: None,
                capture: view(500, 960),
                attendu: Plan {
                    silence: Some(SilenceOp {
                        dst_start: 500,
                        count: LEAD,
                        cause: SilenceCause::NoRender,
                    }),
                    ..Plan::default()
                },
                lie: false,
            },
            Cas {
                nom: "rendu seul : rien n'est accumulé, et le tick le dit",
                render: view(1_000, 480),
                capture: None,
                attendu: JETE,
                lie: false,
            },
            Cas {
                nom: "les deux : lien et copie, aucun silence",
                render: view(1_000, 480),
                capture: view(500, 960),
                attendu: Plan {
                    copy: Some(CopyOp {
                        src_start: 1_000,
                        dst_start: 500,
                        count: LEAD,
                    }),
                    linked: true,
                    ..Plan::default()
                },
                lie: true,
            },
        ];
        for c in cas {
            let mut lb = Loopback::new();
            assert_eq!(lb.plan(c.render, c.capture, LEAD), c.attendu, "{}", c.nom);
            assert_eq!(lb.is_linked(), c.lie, "{}", c.nom);
        }
    }

    /// Ouverture puis fermeture d'un côté **pendant que l'autre tourne**, dans les deux
    /// sens. C'est la transition qui compte : le silence s'arrête exactement où la copie
    /// commence, et reprend au tick qui suit la fermeture, sans trou ni recouvrement.
    #[test]
    fn un_cote_s_ouvre_puis_se_ferme_pendant_que_l_autre_tourne() {
        // 1. La capture tourne sans interruption ; le rendu ouvre, puis ferme.
        let mut lb = Loopback::new();
        let p = lb.plan(None, view(500, 960), LEAD);
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 500,
                count: 96,
                cause: SilenceCause::NoRender
            })
        );
        let p = lb.plan(None, view(548, 960), LEAD);
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 596,
                count: 48,
                cause: SilenceCause::NoRender
            })
        );
        // Le rendu ouvre : lien, copie, et plus une trame de silence.
        let p = lb.plan(view(10_000, 480), view(596, 960), LEAD);
        assert!(p.linked && p.silence.is_none() && !p.discarded);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 10_048,
                dst_start: 644,
                count: 48
            })
        );
        // Le rendu ferme : le silence reprend là où la copie s'est arrêtée (692).
        let p = lb.plan(None, view(644, 960), LEAD);
        assert!(!lb.is_linked() && p.copy.is_none() && !p.discarded);
        assert_eq!(
            p.silence,
            Some(SilenceOp {
                dst_start: 692,
                count: 48,
                cause: SilenceCause::NoRender
            })
        );

        // 2. Le rendu tourne sans interruption ; la capture ouvre, puis ferme.
        let mut lb = Loopback::new();
        assert_eq!(lb.plan(view(2_000, 480), None, LEAD), JETE);
        // La capture ouvre : lien et copie dès ce tick, plus rien de jeté.
        let p = lb.plan(view(2_048, 480), view(0, 960), LEAD);
        assert!(p.linked && !p.discarded && p.silence.is_none());
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 2_048,
                dst_start: 0,
                count: 96
            })
        );
        // La capture ferme : retour au régime « jeté », et la boucle oublie tout.
        assert_eq!(lb.plan(view(2_144, 480), None, LEAD), JETE);
        assert_eq!(lb, Loopback::default());
    }

    /// **Aucune accumulation** : un rendu seul sur cent mille ticks ne demande jamais rien
    /// à écrire et ne laisse jamais grandir le moindre état de la boucle — pendant que le
    /// rendu, lui, avance. L'absence d'écriture n'est pas celle d'un flux immobile.
    #[test]
    fn le_rendu_seul_n_accumule_rien_sur_beaucoup_de_ticks() {
        const TICKS: u64 = 100_000; // 100 s à un tick par milliseconde.
        let mut lb = Loopback::new();
        let mut r = 1_000_u64;
        let mut jetes = 0_u64;
        for _ in 0..TICKS {
            let p = lb.plan(view(r, 480), None, LEAD);
            assert_eq!(p, JETE);
            jetes += 1;
            // La boucle est identique à une boucle neuve, à chaque tick.
            assert_eq!(lb, Loopback::default());
            r += 48;
        }
        assert_eq!(jetes, TICKS);
        assert_eq!(
            r,
            1_000 + 48 * TICKS,
            "le rendu a bien tourné pendant ce temps"
        );
    }

    /// **Silence continu** : une capture seule sur cent mille ticks écrit du silence à
    /// chaque tick, toujours pour la même cause, sans trou ni recouvrement, et pas une
    /// seule trame copiée.
    #[test]
    fn la_capture_seule_ecrit_du_silence_sans_trou_sur_beaucoup_de_ticks() {
        const TICKS: u64 = 100_000;
        let mut lb = Loopback::new();
        let mut c = 500_u64;
        let mut prochain = 500_u64;
        let mut total = 0_u64;
        for _ in 0..TICKS {
            let p = lb.plan(None, view(c, 960), LEAD);
            assert!(p.copy.is_none() && !p.overrun && !p.linked && !p.discarded);
            let s = p
                .silence
                .expect("un tick de capture seule écrit toujours du silence");
            assert_eq!(s.cause, SilenceCause::NoRender);
            assert_eq!(
                s.dst_start, prochain,
                "trou ou recouvrement dans la capture"
            );
            prochain = s.dst_start + s.count;
            total += s.count;
            c += 48;
        }
        // Les blocs pavent exactement `[500, dernier C + avance)`.
        assert_eq!(prochain, c - 48 + LEAD);
        assert_eq!(total, 48 * (TICKS - 1) + LEAD);
    }

    /// Fenêtre écrite par un plan : `[début, fin)` en trames de capture, silence puis copie.
    fn ecrit(plan: &Plan) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        if let Some(s) = plan.silence {
            v.push((s.dst_start, s.dst_start + s.count));
        }
        if let Some(c) = plan.copy {
            v.push((c.dst_start, c.dst_start + c.count));
        }
        v
    }

    // -----------------------------------------------------------------------------
    // Lot 3 : la frontière validée par le client (`SetWritePacket`).
    // -----------------------------------------------------------------------------

    /// **La borne ne coûte rien quand elle est large** : le client se met à valider en
    /// cours de route, avec un paquet d'avance sur la position ; ni la fenêtre écrite ni
    /// la fenêtre lue ne bougent d'une trame. La borne tronque, elle ne décale pas.
    #[test]
    fn la_borne_large_ne_change_rien() {
        let mut lb = Loopback::new();
        // Premier tick sans borne : régime ordinaire, la copie lit jusqu'à R + avance.
        let p = lb.plan(view(1_000, 480), view(500, 960), LEAD);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 1_000,
                dst_start: 500,
                count: LEAD
            })
        );
        // Le client se met à valider : la cible reste C + avance (644), le curseur laissé
        // à 596 enchaîne sans trou, et la source vise toujours R + avance.
        let p = lb.plan(view_committed(1_048, 480, 10_000), view(548, 960), LEAD);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 1_096,
                dst_start: 596,
                count: 48
            })
        );
        assert!(p.silence.is_none() && !p.overrun);
        // 1 096 + 48 = 1 144 = R + avance : la fenêtre lue est bien celle du chemin sans
        // borne, seulement plafonnée par 10 000, qui est loin.
        let p = lb.plan(view_committed(1_096, 480, 10_000), view(596, 960), LEAD);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 1_144,
                dst_start: 644,
                count: 48
            })
        );
    }

    /// **La dissymétrie source ↔ cible, mise face à face** : deux boucles menées sur la
    /// même séquence de positions, l'une bornée par un client qui valide un paquet
    /// d'avance, l'autre pas. Tant que la borne ne tronque rien, les plans sont
    /// *identiques* — donc en particulier la position d'écriture dans la capture, qui
    /// garde son avance des deux côtés. C'est le correctif du 2026-09-10 : mettre cette
    /// avance à zéro avec la borne faisait atterrir l'écriture sous le curseur du lecteur.
    #[test]
    fn la_borne_ne_deplace_pas_la_position_d_ecriture() {
        let mut sans_borne = Loopback::new();
        let mut avec_borne = Loopback::new();
        let (mut r, mut c) = (1_000_u64, 500_u64);
        for tick in 0..32 {
            let attendu = sans_borne.plan(view(r, 480), view(c, 960), LEAD);
            // Le client valide dix millisecondes d'avance : bien plus que l'avance de deux.
            let obtenu = avec_borne.plan(view_committed(r, 480, r + 480), view(c, 960), LEAD);
            assert_eq!(attendu, obtenu, "tick {tick} (R = {r}, C = {c})");
            let copy = obtenu.copy.expect("le régime nominal copie à chaque tick");
            assert_eq!(copy.dst_start + copy.count, c + LEAD, "cible sans avance");
            assert_eq!(copy.src_start + copy.count, r + LEAD, "source sans avance");
            r += 48;
            c += 48;
        }
    }

    /// La borne **tronque** : la copie ne lit jamais au-delà de ce que le client a validé,
    /// même quand le curseur demanderait davantage.
    #[test]
    fn la_borne_tronque_la_copie() {
        let mut lb = Loopback::new();
        // Un curseur pris d'avance par deux ticks sans rendu : 500 → 596 → 644.
        lb.plan(None, view(500, 960), LEAD);
        lb.plan(None, view(548, 960), LEAD);
        // Le rendu arrive, avec une borne serrée : lien R0 = 10 000, C0 = 596, curseur 644,
        // cible 692 (l'avance vaut toujours LEAD sur la capture). Bloc [644, 692) ↔ rendu
        // [10 048, 10 096) ; borne 10 050 : la copie s'arrête à deux trames au lieu de
        // quarante-huit.
        let p = lb.plan(view_committed(10_000, 480, 10_050), view(596, 960), LEAD);
        assert!(p.linked && p.silence.is_none() && !p.overrun);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 10_048,
                dst_start: 644,
                count: 2
            })
        );
        // Borne atteinte : plus rien à copier tant que le client ne valide pas. Le curseur,
        // lui, continue d'avancer — les trames sautées n'existent pas côté rendu.
        let p = lb.plan(view_committed(10_096, 480, 10_050), view(692, 960), LEAD);
        assert!(p.copy.is_none() && p.silence.is_none() && !p.overrun);
        let p = lb.plan(view_committed(10_144, 480, 10_050), view(740, 960), LEAD);
        assert!(p.copy.is_none() && p.silence.is_none() && !p.overrun);
        // Le client valide la suite : la copie repart du curseur, pas de la borne.
        let p = lb.plan(view_committed(10_192, 480, 11_000), view(788, 960), LEAD);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 10_240,
                dst_start: 836,
                count: 48
            })
        );
    }

    /// Une borne sur la **capture** n'existe pas : seule celle du rendu compte, parce que
    /// c'est le rendu qu'on lit. Une capture bornée par distraction ne doit rien changer.
    #[test]
    fn seule_la_borne_du_rendu_agit() {
        let mut lb = Loopback::new();
        let p = lb.plan(view(1_000, 480), view_committed(500, 960, 0), LEAD);
        assert_eq!(
            p.copy,
            Some(CopyOp {
                src_start: 1_000,
                dst_start: 500,
                count: LEAD
            })
        );
    }

    proptest! {
        /// **L'invariant du lot 3** : quelle que soit la séquence, la copie ne lit jamais
        /// au-delà de ce que le client a validé — `src_start + count ≤ committed`.
        #[test]
        fn la_copie_ne_depasse_jamais_la_borne(
            r0 in 0u64..1 << 40,
            c0 in 0u64..1 << 40,
            render_buf in 96u64..=4_800,
            capture_buf in 96u64..=4_800,
            lead in 1u64..=96,
            deltas in proptest::collection::vec(0u64..=200, 1..=60),
            avances in proptest::collection::vec(0u64..=600, 60),
        ) {
            let mut lb = Loopback::new();
            let (mut r, mut c) = (r0, c0);
            let mut committed = r0;
            for (i, &d) in deltas.iter().enumerate() {
                r += d;
                c += d;
                // Le client valide par à-coups, parfois en retard sur la position, parfois
                // en avance : les deux régimes doivent tenir l'invariant.
                committed = committed.saturating_add(avances[i]);
                let render = StreamView::new(r, render_buf).with_committed(committed);
                let plan = lb.plan(Some(render), view(c, capture_buf), lead);
                if let Some(copy) = plan.copy {
                    prop_assert!(
                        copy.src_start.saturating_add(copy.count) <= committed,
                        "copie {copy:?} au-delà de la borne {committed}"
                    );
                    prop_assert!(copy.count >= 1);
                    prop_assert!(copy.count <= render_buf.min(capture_buf));
                }
                // La borne ne fabrique ni silence sans cause ni débordement.
                if let Some(s) = plan.silence {
                    prop_assert_eq!(s.cause, SilenceCause::BeforeRenderStart);
                }
                prop_assert!(!plan.discarded);
            }
        }

        /// Régime normal (pas de retard supérieur au plus petit tampon) : les blocs
        /// écrits se suivent exactement, de `C0` à `C_n + avance`, sans trou ni
        /// recouvrement ; la copie garde un décalage constant et finit à `R + avance`.
        #[test]
        fn les_blocs_sont_contigus_et_le_decalage_constant(
            r0 in 0u64..1 << 40,
            c0 in 0u64..1 << 40,
            render_buf in 96u64..=4_800,
            capture_buf in 96u64..=4_800,
            lead in 1u64..=96,
            deltas in proptest::collection::vec(0u64..=200, 1..=60),
            render_present in proptest::collection::vec(any::<bool>(), 60),
        ) {
            let mut lb = Loopback::new();
            let (mut r, mut c) = (r0, c0);
            let mut expected_next = None::<u64>;
            let mut offset = None::<i128>;
            for (i, &d) in deltas.iter().enumerate() {
                r += d;
                c += d;
                let with_render = render_present[i];
                let cap = if with_render { render_buf.min(capture_buf) } else { capture_buf };
                let render = with_render.then_some(StreamView::new(r, render_buf));
                let plan = lb.plan(render, view(c, capture_buf), lead);
                let start = expected_next.unwrap_or(c);
                let count = c + lead - start;

                // Le lien est établi au premier tick avec rendu qui suit un tick sans
                // rendu, et **survit à un débordement** : un tick en retard ne décale
                // pas l'alignement rendu ↔ capture.
                if with_render {
                    if plan.linked {
                        offset = Some(i128::from(c) - i128::from(r));
                    }
                } else {
                    prop_assert!(!plan.linked);
                    prop_assert!(plan.copy.is_none());
                    offset = None;
                }
                expected_next = Some(c + lead);

                if count > cap {
                    prop_assert!(plan.overrun);
                    prop_assert!(ecrit(&plan).is_empty());
                    prop_assert_eq!(with_render, offset.is_some());
                    continue;
                }
                prop_assert!(!plan.overrun);
                let blocks = ecrit(&plan);
                if count == 0 {
                    prop_assert!(blocks.is_empty());
                } else {
                    prop_assert_eq!(blocks[0].0, start);
                    for w in blocks.windows(2) {
                        prop_assert_eq!(w[0].1, w[1].0);
                    }
                    prop_assert_eq!(blocks.last().unwrap().1, c + lead);
                }
                if with_render {
                    let off = offset.unwrap();
                    if let Some(copy) = plan.copy {
                        prop_assert_eq!(i128::from(copy.dst_start) - i128::from(copy.src_start), off);
                        prop_assert_eq!(copy.src_start + copy.count, r + lead);
                        prop_assert!(copy.count <= cap);
                    }
                    if let Some(s) = plan.silence {
                        // Silence seulement pour des trames de rendu négatives, et il le
                        // dit : avec un rendu en `RUN`, la cause ne peut pas être son
                        // absence.
                        prop_assert!(i128::from(s.dst_start) - off < 0);
                        prop_assert_eq!(s.cause, SilenceCause::BeforeRenderStart);
                    }
                } else if let Some(s) = plan.silence {
                    prop_assert_eq!(s.count, count);
                    prop_assert!(s.count <= capture_buf);
                    prop_assert_eq!(s.cause, SilenceCause::NoRender);
                }
                // La capture est toujours là : rien n'est jamais jeté ici.
                prop_assert!(!plan.discarded);
            }
        }

        /// Quoi qu'il arrive (retards, reculs, valeurs extrêmes) : jamais de panique,
        /// `count` borné par le plus petit tampon, blocs non vides et dans l'ordre.
        #[test]
        fn jamais_au_dela_des_tampons(
            render in proptest::option::of((any::<u64>(), 1u64..=100_000)),
            capture in proptest::option::of((any::<u64>(), 1u64..=100_000)),
            lead in 0u64..=10_000,
            cursor_seed in proptest::option::of(any::<u64>()),
        ) {
            let mut lb = Loopback::new();
            if let Some(seed) = cursor_seed {
                lb.plan(None, view(seed, 1), lead);
            }
            let rv = render.map(|(f, b)| StreamView::new(f, b));
            let cv = capture.map(|(f, b)| StreamView::new(f, b));
            let plan = lb.plan(rv, cv, lead);
            let cap = match (rv, cv) {
                (Some(r), Some(c)) => r.buffer_frames.min(c.buffer_frames),
                (None, Some(c)) => c.buffer_frames,
                _ => 0,
            };
            for (a, b) in ecrit(&plan) {
                prop_assert!(b > a);
                prop_assert!(b - a <= cap);
            }
            if let Some(c) = plan.copy { prop_assert!(c.count >= 1 && c.count <= cap); }
            if let Some(s) = plan.silence { prop_assert!(s.count >= 1 && s.count <= cap); }
            // Sans capture, le plan est vide **sauf** le témoin, qui distingue un rendu
            // qui tourne pour rien d'un câble au repos.
            if cv.is_none() {
                prop_assert_eq!(plan, Plan { discarded: rv.is_some(), ..Plan::default() });
            } else {
                prop_assert!(!plan.discarded);
            }
            // La cause du silence et la présence du rendu ne peuvent pas se contredire.
            if let Some(s) = plan.silence {
                prop_assert_eq!(s.cause == SilenceCause::NoRender, rv.is_none());
            }
        }
    }
}
