//! Horloge interne : cadence le graphe quand aucun périphérique ne le pilote (F-22).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use conduit_backend::rt::{promote_current_thread, RtOutcome};
use conduit_core::executor::Executor;
use conduit_core::types::SampleRate;

use crate::stats::{CycleTiming, EngineEvent, EventProducer};

/// Statistiques de cadence de l'horloge interne.
#[derive(Debug, Default)]
pub struct ClockStats {
    ticks: std::sync::atomic::AtomicU64,
    /// Écart absolu maximal entre l'instant prévu et l'instant réel d'un tick (ns).
    max_jitter_ns: std::sync::atomic::AtomicU64,
    rt: Mutex<Option<RtOutcome>>,
}

impl ClockStats {
    /// Ticks exécutés.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
    /// Gigue maximale observée (ns).
    pub fn max_jitter_ns(&self) -> u64 {
        self.max_jitter_ns.load(Ordering::Relaxed)
    }
    /// Résultat de la promotion temps réel du fil.
    pub fn rt_outcome(&self) -> Option<RtOutcome> {
        self.rt.lock().unwrap().clone()
    }
}

/// Fil cadençant l'exécuteur à `quantum / sample_rate`.
pub struct InternalClock {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    stats: Arc<ClockStats>,
}

impl core::fmt::Debug for InternalClock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InternalClock")
            .field("ticks", &self.stats.ticks())
            .finish()
    }
}

impl InternalClock {
    /// Démarre le fil. Chaque tick verrouille l'exécuteur (`try_lock` : un tick est
    /// sauté si un autre pilote l'occupe) et exécute un cycle de `quantum` trames.
    pub fn start(
        executor: Arc<Mutex<Executor>>,
        sample_rate: SampleRate,
        quantum: usize,
        timing: Arc<CycleTiming>,
        mut events: EventProducer,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(ClockStats::default());
        let period = Duration::from_secs_f64(quantum as f64 / sample_rate.as_f64());
        let stop2 = Arc::clone(&stop);
        let stats2 = Arc::clone(&stats);
        let handle = std::thread::Builder::new()
            .name("conduit-internal-clock".into())
            .spawn(move || {
                *stats2.rt.lock().unwrap() = Some(promote_current_thread());
                let start = Instant::now();
                let mut next = start;
                let mut first = true;
                while !stop2.load(Ordering::Relaxed) {
                    next += period;
                    sleep_until(next);
                    let now = Instant::now();
                    let jitter = now
                        .saturating_duration_since(next)
                        .max(next.saturating_duration_since(now));
                    stats2
                        .max_jitter_ns
                        .fetch_max(jitter.as_nanos() as u64, Ordering::Relaxed);
                    match executor.try_lock() {
                        Ok(mut ex) => {
                            if first {
                                events.push(EngineEvent::DriverStarted { device: None });
                                first = false;
                            }
                            let t0 = Instant::now();
                            let report = ex.run(quantum);
                            let dur = t0.elapsed().as_nanos() as u64;
                            if timing.record(dur) {
                                events.push(EngineEvent::CycleOverrun {
                                    cycle: ex.cycle(),
                                    duration_ns: dur,
                                });
                            }
                            let _ = report;
                        }
                        Err(_) => events.push(EngineEvent::ExecutorBusy),
                    }
                    stats2.ticks.fetch_add(1, Ordering::Relaxed);
                    // Si on a pris trop de retard (machine suspendue), on resynchronise.
                    if Instant::now() > next + period * 4 {
                        next = Instant::now();
                    }
                }
            })
            .expect("fil horloge interne");
        Self {
            stop,
            handle: Some(handle),
            stats,
        }
    }

    /// Statistiques.
    pub fn stats(&self) -> Arc<ClockStats> {
        Arc::clone(&self.stats)
    }

    /// Arrête le fil (attend sa fin).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for InternalClock {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Dort jusqu'à `deadline` : sommeil grossier puis attente active courte pour la
/// précision (< 100 µs typiquement).
fn sleep_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let remaining = deadline - now;
        if remaining > Duration::from_micros(300) {
            std::thread::sleep(remaining - Duration::from_micros(250));
        } else {
            std::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::EventQueue;
    use conduit_core::graph::GraphBuilder;
    use conduit_core::nodes::SineNode;
    use conduit_core::slot::GraphSlot;
    use conduit_core::types::Quantum;

    #[test]
    fn internal_clock_runs_cycles_with_bounded_jitter() {
        let quantum = 256;
        let (mut slot, ex) = GraphSlot::new();
        let mut b = GraphBuilder::new(SampleRate::HZ_48000, Quantum::new(quantum).unwrap());
        b.add_node(Box::new(SineNode::new(440.0, 0.5, 2)), "s");
        slot.publish(&mut b).unwrap();
        let executor = Arc::new(Mutex::new(ex));
        let timing = Arc::new(CycleTiming::new(5_333_000));
        let (producer, mut consumer) = EventQueue::with_capacity(64);
        let mut clock = InternalClock::start(
            Arc::clone(&executor),
            SampleRate::HZ_48000,
            quantum,
            timing.clone(),
            producer,
        );
        let t0 = Instant::now();
        std::thread::sleep(Duration::from_millis(500));
        clock.stop();
        let elapsed = t0.elapsed();
        let stats = clock.stats();
        let expected = elapsed.as_secs_f64() * 48_000.0 / quantum as f64;
        let ticks = stats.ticks() as f64;
        assert!(
            (ticks - expected).abs() < expected * 0.1 + 3.0,
            "ticks {ticks} vs {expected}"
        );
        let ex = executor.lock().unwrap();
        assert_eq!(ex.cycle(), stats.ticks());
        assert!(ex.position() > 0);
        // Le critère F-22 (gigue < 10 % du quantum sur 60 s) est vérifié par le test
        // `jitter_60s` (ignoré par défaut) ; ici, sous charge de CI, on vérifie
        // seulement qu'aucun tick n'a dérapé d'une période entière.
        let jitter_ms = stats.max_jitter_ns() as f64 / 1e6;
        assert!(jitter_ms < 5.3, "gigue max {jitter_ms} ms");
        assert!(stats.rt_outcome().is_some());
        assert_eq!(
            consumer.pop(),
            Some(EngineEvent::DriverStarted { device: None })
        );
        assert!(timing.snapshot().count > 0);
        assert!(format!("{clock:?}").contains("ticks"));
    }

    #[test]
    #[ignore = "60 s en temps réel : critère F-22, lancer sur une machine calme"]
    fn jitter_60s() {
        let quantum = 256;
        let (_slot, ex) = GraphSlot::new();
        let executor = Arc::new(Mutex::new(ex));
        let (producer, _consumer) = EventQueue::with_capacity(64);
        let mut clock = InternalClock::start(
            executor,
            SampleRate::HZ_48000,
            quantum,
            Arc::new(CycleTiming::new(0)),
            producer,
        );
        std::thread::sleep(Duration::from_secs(60));
        clock.stop();
        let jitter_ms = clock.stats().max_jitter_ns() as f64 / 1e6;
        assert!(
            jitter_ms < 0.533,
            "gigue max {jitter_ms} ms > 10 % du quantum"
        );
    }

    #[test]
    fn busy_executor_skips_ticks() {
        let quantum = 64;
        let (_slot, ex) = GraphSlot::new();
        let executor = Arc::new(Mutex::new(ex));
        let timing = Arc::new(CycleTiming::new(0));
        let (producer, mut consumer) = EventQueue::with_capacity(64);
        let guard = executor.lock().unwrap();
        let mut clock = InternalClock::start(
            Arc::clone(&executor),
            SampleRate::HZ_48000,
            quantum,
            timing,
            producer,
        );
        std::thread::sleep(Duration::from_millis(30));
        drop(guard);
        std::thread::sleep(Duration::from_millis(30));
        clock.stop();
        let mut busy = 0;
        while let Some(e) = consumer.pop() {
            if e == EngineEvent::ExecutorBusy {
                busy += 1;
            }
        }
        assert!(busy > 0);
        assert!(clock.stats().ticks() > busy);
    }
}
