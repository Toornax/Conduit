use conduit_testing::{assert_no_alloc, count_allocs, violations, GuardAllocator};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

#[test]
fn counting_mode_sees_allocations() {
    let (_, n) = count_allocs(|| {
        let v: Vec<u8> = Vec::with_capacity(16);
        drop(v);
    });
    assert_eq!(n, 2, "une allocation et une libération");
    let (_, n) = count_allocs(|| {
        let x = 1 + 1;
        std::hint::black_box(x)
    });
    assert_eq!(n, 0);
}

#[test]
fn forbid_mode_lets_alloc_free_code_through() {
    let mut buf = vec![0.0f32; 64];
    let sum = assert_no_alloc(|| {
        for (i, x) in buf.iter_mut().enumerate() {
            *x = i as f32;
        }
        buf.iter().sum::<f32>()
    });
    assert_eq!(sum, 2016.0);
}

#[test]
#[should_panic(expected = "allocation interdite")]
fn forbid_mode_panics_on_allocation() {
    assert_no_alloc(|| {
        let v: Vec<u8> = Vec::with_capacity(8);
        std::hint::black_box(v);
    });
}

#[test]
fn violations_are_per_thread() {
    let before = violations();
    std::thread::spawn(|| {
        let (_, n) = count_allocs(|| {
            let b = Box::new(1u32);
            std::hint::black_box(b);
        });
        assert_eq!(n, 2);
    })
    .join()
    .unwrap();
    assert_eq!(violations(), before);
}
