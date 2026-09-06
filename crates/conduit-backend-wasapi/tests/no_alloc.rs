//! Preuve que la boucle du fil d'un flux WASAPI n'alloue pas (dev-guide §2).
//!
//! Le `GuardAllocator` de `conduit-testing` arme un mode **par fil**, le temps d'une
//! fermeture : il ne peut pas surveiller, depuis le fil de test, une boucle qui
//! tourne sur le fil du flux. Ce binaire installe donc son propre allocateur
//! global : il compte chaque allocation, réallocation et libération faite par le
//! fil dont l'identifiant Win32 est **armé**. Le rappel arme le fil du flux à son
//! deuxième passage (le premier peut encore payer des mises en place paresseuses
//! de la bibliothèque standard, comme `thread::current()`), le test désarme avant
//! `stop()` : entre les deux, réveils, `GetBuffer`, rappel et publication de
//! l'horloge ne doivent rien demander à l'allocateur Rust.
//!
//! Ce que Windows alloue de son côté (`HeapAlloc` dans le moteur audio) est hors
//! de portée : seul le code Rust du backend est prouvé.
//!
//! Le cas **exclusif** (M1b-32) est couvert aussi : le rappel y écrit dans le
//! tampon intermédiaire `f32`, que le fil convertit ensuite vers le format
//! matériel (souvent du PCM 24 bits) — cette conversion supplémentaire ne doit pas
//! davantage toucher à l'allocateur. Le test se saute si la carte par défaut
//! refuse l'exclusif.

#![cfg(windows)]

use core::alloc::{GlobalAlloc, Layout};
use std::alloc::System;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use conduit_backend::{Backend, DeviceDirection, DeviceHandle, StreamFormat};
use conduit_backend_wasapi::{ExclusivePolicy, ShareMode, WasapiBackend};
use conduit_core::types::SampleRate;
use windows::Win32::System::Threading::GetCurrentThreadId;

/// Les deux tests partagent les compteurs : un à la fois.
static SERIAL: Mutex<()> = Mutex::new(());
/// Identifiant Win32 du fil surveillé ; 0 = personne.
static ARMED_THREAD: AtomicU32 = AtomicU32::new(0);
/// Allocations, réallocations et libérations vues sur le fil surveillé.
static VIOLATIONS: AtomicUsize = AtomicUsize::new(0);

/// Allocateur global : délègue à `System`, compte ce que fait le fil armé.
struct ThreadCountingAllocator;

#[inline]
fn check() {
    let armed = ARMED_THREAD.load(Ordering::Relaxed);
    // SAFETY: `GetCurrentThreadId` n'a ni précondition ni effet de bord.
    if armed != 0 && unsafe { GetCurrentThreadId() } == armed {
        VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: chaque méthode délègue à `System` avec les mêmes arguments ; `check`
// n'alloue pas (deux atomiques et un appel Win32 sans allocation).
unsafe impl GlobalAlloc for ThreadCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        check();
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        check();
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check();
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        check();
        // SAFETY: contrat identique à celui reçu.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: ThreadCountingAllocator = ThreadCountingAllocator;

/// Rappel de test : silence, et armement du fil courant au deuxième passage.
fn arming_callback(calls: Arc<AtomicUsize>) -> conduit_backend::AudioCallback {
    Box::new(move |io, _| {
        io.silence_output();
        let n = calls.fetch_add(1, Ordering::Relaxed);
        if n == 1 {
            // SAFETY: voir `check`.
            ARMED_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        }
    })
}

fn run_one_second(direction: DeviceDirection, policy: ExclusivePolicy) {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut backend = WasapiBackend::new().expect("WasapiBackend::new");
    backend.set_exclusive_policy(policy);
    let Some(id) = backend.default_device(direction) else {
        eprintln!("test sauté : aucun périphérique de {direction} par défaut");
        return;
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let format = StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels: 2,
        block_frames: 480,
    };
    let mut handle = backend
        .open_handle(&id, format, arming_callback(Arc::clone(&calls)))
        .expect("ouverture");
    let mode = handle.share_mode();
    if policy != ExclusivePolicy::Never && mode != ShareMode::Exclusive {
        eprintln!(
            "test sauté : le rendu par défaut refuse le mode exclusif ({})",
            handle.exclusive_refusal().unwrap_or("sans raison donnée")
        );
        return;
    }
    eprintln!(
        "{direction} en mode {mode}, {} octets par échantillon ({})",
        handle.sample_type().bytes(),
        handle.sample_type()
    );
    VIOLATIONS.store(0, Ordering::Relaxed);
    handle.start().expect("démarrage");
    std::thread::sleep(Duration::from_secs(1));
    ARMED_THREAD.store(0, Ordering::Relaxed);
    let violations = VIOLATIONS.load(Ordering::Relaxed);
    let n = calls.load(Ordering::Relaxed);
    handle.stop().expect("arrêt");
    assert!(n >= 10, "seulement {n} rappels en 1 s : rien à prouver");
    assert_eq!(
        violations, 0,
        "{violations} allocation(s) ou libération(s) sur le fil du flux ({direction}, \
         mode {mode}) pendant {n} rappels"
    );
}

#[test]
fn render_loop_does_not_allocate() {
    run_one_second(DeviceDirection::Render, ExclusivePolicy::Never);
}

#[test]
fn capture_loop_does_not_allocate() {
    run_one_second(DeviceDirection::Capture, ExclusivePolicy::Never);
}

/// M1b-32 : en mode exclusif, la conversion `f32` → format matériel a lieu **dans**
/// le rappel, sur le fil du flux. Elle ne doit rien allouer non plus. Le test se
/// saute si la carte par défaut refuse l'exclusif.
#[test]
fn exclusive_render_loop_does_not_allocate_either() {
    run_one_second(DeviceDirection::Render, ExclusivePolicy::Preferred);
}
