//! Compteurs temps réel et file d'événements RT → gestion (M0-66).

use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use conduit_backend::DeviceId;
use conduit_core::ring::{RingBuffer, RingConsumer, RingProducer};

/// Agrégat des temps de cycle, mis à jour par le fil audio, lu par la gestion.
#[derive(Debug, Default)]
pub struct CycleTiming {
    count: AtomicU64,
    sum_ns: AtomicU64,
    min_ns: AtomicU64,
    max_ns: AtomicU64,
    last_ns: AtomicU64,
    /// Cycles dont la durée a dépassé le budget (quantum).
    overruns: AtomicU64,
    budget_ns: AtomicU64,
}

/// Instantané des temps de cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TimingSnapshot {
    /// Cycles mesurés.
    pub count: u64,
    /// Durée minimale (ns).
    pub min_ns: u64,
    /// Durée moyenne (ns).
    pub avg_ns: u64,
    /// Durée maximale (ns).
    pub max_ns: u64,
    /// Dernière durée (ns).
    pub last_ns: u64,
    /// Budget d'un cycle (ns).
    pub budget_ns: u64,
    /// Cycles au-delà du budget.
    pub overruns: u64,
}

impl CycleTiming {
    /// Crée avec un budget par cycle.
    pub fn new(budget_ns: u64) -> Self {
        let t = Self::default();
        t.budget_ns.store(budget_ns, Ordering::Relaxed);
        t.min_ns.store(u64::MAX, Ordering::Relaxed);
        t
    }

    /// Change le budget.
    pub fn set_budget_ns(&self, budget_ns: u64) {
        self.budget_ns.store(budget_ns, Ordering::Relaxed);
    }

    /// Enregistre une durée. Temps réel : oui. Retourne `true` si le budget est dépassé.
    #[inline]
    pub fn record(&self, duration_ns: u64) -> bool {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_ns.fetch_add(duration_ns, Ordering::Relaxed);
        self.min_ns.fetch_min(duration_ns, Ordering::Relaxed);
        self.max_ns.fetch_max(duration_ns, Ordering::Relaxed);
        self.last_ns.store(duration_ns, Ordering::Relaxed);
        let budget = self.budget_ns.load(Ordering::Relaxed);
        if budget > 0 && duration_ns > budget {
            self.overruns.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Instantané.
    pub fn snapshot(&self) -> TimingSnapshot {
        let count = self.count.load(Ordering::Relaxed);
        let min = self.min_ns.load(Ordering::Relaxed);
        TimingSnapshot {
            count,
            min_ns: if count == 0 { 0 } else { min },
            avg_ns: self
                .sum_ns
                .load(Ordering::Relaxed)
                .checked_div(count)
                .unwrap_or(0),
            max_ns: self.max_ns.load(Ordering::Relaxed),
            last_ns: self.last_ns.load(Ordering::Relaxed),
            budget_ns: self.budget_ns.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
        }
    }

    /// Remet les statistiques à zéro (garde le budget).
    pub fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.sum_ns.store(0, Ordering::Relaxed);
        self.min_ns.store(u64::MAX, Ordering::Relaxed);
        self.max_ns.store(0, Ordering::Relaxed);
        self.last_ns.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
    }
}

/// Événement émis par le fil audio.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "type", rename_all = "snake_case")
)]
pub enum EngineEvent {
    /// Le cycle a dépassé son budget.
    CycleOverrun {
        /// Numéro du cycle.
        cycle: u64,
        /// Durée mesurée (ns).
        duration_ns: u64,
    },
    /// Xrun sur un périphérique asynchrone.
    DeviceXrun {
        /// Périphérique.
        device: DeviceId,
        /// Sous-alimentation (`true`) ou débordement.
        underrun: bool,
    },
    /// Le pilote de graphe a exécuté son premier cycle.
    DriverStarted {
        /// Périphérique pilote (`None` = horloge interne).
        device: Option<DeviceId>,
    },
    /// L'exécuteur était occupé (autre pilote en cours) : cycle sauté, silence.
    ExecutorBusy,
}

/// File d'événements RT → gestion.
#[derive(Debug)]
pub struct EventQueue;

impl EventQueue {
    /// Capacité par défaut.
    pub const DEFAULT_CAPACITY: usize = 256;

    /// Crée la paire producteur (fil audio) / consommateur (gestion).
    pub fn with_capacity(capacity: usize) -> (EventProducer, RingConsumer<EngineEvent>) {
        let (p, c) = RingBuffer::with_capacity(capacity);
        (
            EventProducer {
                inner: p,
                dropped: Arc::new(AtomicU64::new(0)),
            },
            c,
        )
    }
}

/// Producteur d'événements côté fil audio : ne bloque jamais, compte les pertes.
#[derive(Debug)]
pub struct EventProducer {
    inner: RingProducer<EngineEvent>,
    dropped: Arc<AtomicU64>,
}

impl EventProducer {
    /// Pousse un événement ; s'il n'y a pas de place, il est compté comme perdu.
    /// Temps réel : oui.
    #[inline]
    pub fn push(&mut self, event: EngineEvent) {
        if self.inner.push(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Compteur des événements perdus (partagé).
    pub fn dropped_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_aggregates_and_detects_overrun() {
        let t = CycleTiming::new(1000);
        assert_eq!(
            t.snapshot(),
            TimingSnapshot {
                budget_ns: 1000,
                ..Default::default()
            }
        );
        assert!(!t.record(100));
        assert!(!t.record(300));
        assert!(t.record(1500));
        let s = t.snapshot();
        assert_eq!(
            (s.count, s.min_ns, s.max_ns, s.last_ns, s.overruns),
            (3, 100, 1500, 1500, 1)
        );
        assert_eq!(s.avg_ns, (100 + 300 + 1500) / 3);
        t.reset();
        assert_eq!(t.snapshot().count, 0);
        assert_eq!(t.snapshot().min_ns, 0);
        assert_eq!(t.snapshot().budget_ns, 1000);
        t.set_budget_ns(0);
        assert!(!t.record(u64::MAX / 2), "budget 0 = pas de détection");
    }

    #[test]
    fn queue_never_blocks_and_counts_drops() {
        let (mut p, mut c) = EventQueue::with_capacity(2);
        let dropped = p.dropped_counter();
        p.push(EngineEvent::ExecutorBusy);
        p.push(EngineEvent::DriverStarted { device: None });
        p.push(EngineEvent::ExecutorBusy);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(c.pop(), Some(EngineEvent::ExecutorBusy));
        assert_eq!(c.pop(), Some(EngineEvent::DriverStarted { device: None }));
        assert_eq!(c.pop(), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn event_serde() {
        let e = EngineEvent::DeviceXrun {
            device: "d".into(),
            underrun: true,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(
            json,
            r#"{"type":"device_xrun","device":"d","underrun":true}"#
        );
    }
}
