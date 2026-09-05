//! Tampon circulaire SPSC temps réel.
//!
//! Façade sur [`rtrb`] : un producteur et un consommateur, chacun utilisable depuis
//! un fil différent, lecture/écriture par blocs, sans allocation ni verrou après
//! construction.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Métadonnées partagées entre producteur et consommateur.
#[derive(Debug)]
struct Shared {
    capacity: usize,
    /// Nombre total d'éléments écrits (monotone), pour les statistiques.
    written: AtomicUsize,
    /// Nombre total d'éléments lus (monotone).
    read: AtomicUsize,
}

/// Côté écriture d'un [`RingBuffer`].
#[derive(Debug)]
pub struct RingProducer<T> {
    inner: rtrb::Producer<T>,
    shared: Arc<Shared>,
}

/// Côté lecture d'un [`RingBuffer`].
#[derive(Debug)]
pub struct RingConsumer<T> {
    inner: rtrb::Consumer<T>,
    shared: Arc<Shared>,
}

/// Constructeur de tampon circulaire SPSC.
#[derive(Debug)]
pub struct RingBuffer;

impl RingBuffer {
    /// Crée un tampon de `capacity` éléments et retourne les deux extrémités.
    pub fn with_capacity<T>(capacity: usize) -> (RingProducer<T>, RingConsumer<T>) {
        let (p, c) = rtrb::RingBuffer::new(capacity);
        let shared = Arc::new(Shared {
            capacity,
            written: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        });
        (
            RingProducer {
                inner: p,
                shared: Arc::clone(&shared),
            },
            RingConsumer { inner: c, shared },
        )
    }
}

impl<T> RingProducer<T> {
    /// Capacité totale.
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Nombre d'emplacements libres.
    ///
    /// Temps réel : oui.
    pub fn slots(&self) -> usize {
        self.inner.slots()
    }

    /// Nombre d'éléments actuellement dans le tampon (vu du producteur).
    ///
    /// Temps réel : oui.
    pub fn len(&self) -> usize {
        self.shared.capacity - self.inner.slots()
    }

    /// Vrai si vide.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Vrai si plein.
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// Vrai si le consommateur a été abandonné.
    pub fn is_abandoned(&self) -> bool {
        self.inner.is_abandoned()
    }

    /// Pousse un élément. Retourne `Err(value)` si plein.
    ///
    /// Temps réel : oui.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        match self.inner.push(value) {
            Ok(()) => {
                self.shared.written.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(rtrb::PushError::Full(v)) => Err(v),
        }
    }

    /// Total d'éléments écrits depuis la création.
    pub fn total_written(&self) -> usize {
        self.shared.written.load(Ordering::Relaxed)
    }
}

impl<T: Copy> RingProducer<T> {
    /// Écrit autant d'éléments de `src` que possible, retourne le nombre écrit.
    ///
    /// Temps réel : oui.
    pub fn write(&mut self, src: &[T]) -> usize {
        let n = src.len().min(self.inner.slots());
        if n == 0 {
            return 0;
        }
        let chunk = match self.inner.write_chunk_uninit(n) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        let written = chunk.fill_from_iter(src[..n].iter().copied());
        debug_assert_eq!(written, n);
        self.shared.written.fetch_add(written, Ordering::Relaxed);
        written
    }

    /// Écrit tout `src` ou rien. Retourne `false` s'il n'y a pas assez de place.
    ///
    /// Temps réel : oui.
    pub fn write_all(&mut self, src: &[T]) -> bool {
        if self.inner.slots() < src.len() {
            return false;
        }
        self.write(src) == src.len()
    }
}

impl<T> RingConsumer<T> {
    /// Capacité totale.
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Nombre d'éléments lisibles.
    ///
    /// Temps réel : oui.
    pub fn len(&self) -> usize {
        self.inner.slots()
    }

    /// Vrai si vide.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Vrai si le producteur a été abandonné.
    pub fn is_abandoned(&self) -> bool {
        self.inner.is_abandoned()
    }

    /// Retire un élément.
    ///
    /// Temps réel : oui.
    pub fn pop(&mut self) -> Option<T> {
        let v = self.inner.pop().ok()?;
        self.shared.read.fetch_add(1, Ordering::Relaxed);
        Some(v)
    }

    /// Total d'éléments lus depuis la création.
    pub fn total_read(&self) -> usize {
        self.shared.read.load(Ordering::Relaxed)
    }

    /// Jette jusqu'à `n` éléments, retourne le nombre jeté.
    ///
    /// Temps réel : oui.
    pub fn skip(&mut self, n: usize) -> usize {
        let n = n.min(self.inner.slots());
        if n == 0 {
            return 0;
        }
        match self.inner.read_chunk(n) {
            Ok(chunk) => {
                chunk.commit_all();
                self.shared.read.fetch_add(n, Ordering::Relaxed);
                n
            }
            Err(_) => 0,
        }
    }
}

impl<T: Copy> RingConsumer<T> {
    /// Lit autant d'éléments que possible dans `dst`, retourne le nombre lu.
    ///
    /// Temps réel : oui.
    pub fn read(&mut self, dst: &mut [T]) -> usize {
        let n = dst.len().min(self.inner.slots());
        if n == 0 {
            return 0;
        }
        let chunk = match self.inner.read_chunk(n) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        let (a, b) = chunk.as_slices();
        dst[..a.len()].copy_from_slice(a);
        dst[a.len()..a.len() + b.len()].copy_from_slice(b);
        chunk.commit_all();
        self.shared.read.fetch_add(n, Ordering::Relaxed);
        n
    }

    /// Lit exactement `dst.len()` éléments ou rien. Retourne `false` si pas assez.
    ///
    /// Temps réel : oui.
    pub fn read_exact(&mut self, dst: &mut [T]) -> bool {
        if self.inner.slots() < dst.len() {
            return false;
        }
        self.read(dst) == dst.len()
    }

    /// Regarde jusqu'à `dst.len()` éléments sans les consommer, retourne le nombre copié.
    ///
    /// Temps réel : oui.
    pub fn peek(&mut self, dst: &mut [T]) -> usize {
        let n = dst.len().min(self.inner.slots());
        if n == 0 {
            return 0;
        }
        let chunk = match self.inner.read_chunk(n) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        let (a, b) = chunk.as_slices();
        dst[..a.len()].copy_from_slice(a);
        dst[a.len()..a.len() + b.len()].copy_from_slice(b);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn push_pop_basic() {
        let (mut p, mut c) = RingBuffer::with_capacity::<u32>(4);
        assert_eq!(p.capacity(), 4);
        assert!(c.is_empty());
        assert!(p.push(1).is_ok());
        assert!(p.push(2).is_ok());
        assert_eq!(p.len(), 2);
        assert_eq!(c.len(), 2);
        assert_eq!(c.pop(), Some(1));
        assert_eq!(c.pop(), Some(2));
        assert_eq!(c.pop(), None);
        assert_eq!(p.total_written(), 2);
        assert_eq!(c.total_read(), 2);
    }

    #[test]
    fn full_rejects_push() {
        let (mut p, _c) = RingBuffer::with_capacity::<u8>(2);
        assert!(p.push(1).is_ok());
        assert!(p.push(2).is_ok());
        assert!(p.is_full());
        assert_eq!(p.push(3), Err(3));
    }

    #[test]
    fn block_write_read_wraps_around() {
        let (mut p, mut c) = RingBuffer::with_capacity::<f32>(8);
        assert_eq!(p.write(&[1.0; 6]), 6);
        let mut buf = [0.0; 5];
        assert_eq!(c.read(&mut buf), 5);
        assert_eq!(p.write(&[2.0; 7]), 7); // 1 + 7 = 8, wrap
        assert!(p.is_full());
        let mut buf = [0.0; 8];
        assert_eq!(c.read(&mut buf), 8);
        assert_eq!(buf[0], 1.0);
        assert!(buf[1..].iter().all(|&x| x == 2.0));
    }

    #[test]
    fn write_all_and_read_exact_are_atomic() {
        let (mut p, mut c) = RingBuffer::with_capacity::<u8>(4);
        assert!(!p.write_all(&[0; 5]));
        assert!(p.write_all(&[1, 2, 3]));
        let mut d = [0; 4];
        assert!(!c.read_exact(&mut d));
        assert_eq!(c.len(), 3);
        let mut d = [0; 3];
        assert!(c.read_exact(&mut d));
        assert_eq!(d, [1, 2, 3]);
    }

    #[test]
    fn peek_and_skip() {
        let (mut p, mut c) = RingBuffer::with_capacity::<u8>(4);
        p.write(&[1, 2, 3]);
        let mut d = [0; 2];
        assert_eq!(c.peek(&mut d), 2);
        assert_eq!(d, [1, 2]);
        assert_eq!(c.len(), 3);
        assert_eq!(c.skip(2), 2);
        assert_eq!(c.pop(), Some(3));
        assert_eq!(c.skip(5), 0);
    }

    #[test]
    fn abandoned_detection() {
        let (p, c) = RingBuffer::with_capacity::<u8>(1);
        drop(c);
        assert!(p.is_abandoned());
        let (p, c) = RingBuffer::with_capacity::<u8>(1);
        drop(p);
        assert!(c.is_abandoned());
    }

    #[test]
    fn multithreaded_stream_is_lossless_and_ordered() {
        const TOTAL: u32 = 200_000;
        let (mut p, mut c) = RingBuffer::with_capacity::<u32>(61);
        let writer = std::thread::spawn(move || {
            let mut next = 0u32;
            let mut chunk = [0u32; 17];
            while next < TOTAL {
                let n = (TOTAL - next).min(17) as usize;
                for (i, x) in chunk[..n].iter_mut().enumerate() {
                    *x = next + i as u32;
                }
                let w = p.write(&chunk[..n]);
                next += w as u32;
                if w == 0 {
                    std::thread::yield_now();
                }
            }
        });
        let mut expected = 0u32;
        let mut buf = [0u32; 23];
        while expected < TOTAL {
            let n = c.read(&mut buf);
            for &x in &buf[..n] {
                assert_eq!(x, expected);
                expected += 1;
            }
            if n == 0 {
                std::thread::yield_now();
            }
        }
        writer.join().unwrap();
        assert_eq!(c.total_read(), TOTAL as usize);
    }

    proptest! {
        /// Toute séquence d'écritures/lectures par blocs préserve l'ordre, sans perte
        /// ni duplication, et le niveau de remplissage reste cohérent.
        #[test]
        fn block_ops_preserve_order(
            cap in 1usize..64,
            ops in prop::collection::vec((any::<bool>(), 0usize..80), 0..200),
        ) {
            let (mut p, mut c) = RingBuffer::with_capacity::<u64>(cap);
            let mut next_write = 0u64;
            let mut next_read = 0u64;
            let mut scratch = vec![0u64; 80];
            for (is_write, n) in ops {
                if is_write {
                    for (i, s) in scratch[..n].iter_mut().enumerate() {
                        *s = next_write + i as u64;
                    }
                    let w = p.write(&scratch[..n]);
                    prop_assert!(w <= n);
                    prop_assert!(w == n || p.is_full());
                    next_write += w as u64;
                } else {
                    let r = c.read(&mut scratch[..n]);
                    prop_assert!(r <= n);
                    prop_assert!(r == n || c.is_empty());
                    for (i, &s) in scratch[..r].iter().enumerate() {
                        prop_assert_eq!(s, next_read + i as u64);
                    }
                    next_read += r as u64;
                }
                prop_assert_eq!(c.len() as u64, next_write - next_read);
                prop_assert_eq!(p.len(), c.len());
                prop_assert!(c.len() <= cap);
            }
        }
    }
}
