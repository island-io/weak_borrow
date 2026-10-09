use std::{
    panic::{self, AssertUnwindSafe},
    sync::{
        Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    thread::{self, ThreadId},
    time::Duration,
};

use weak_borrow::WeakLender;

// We assume that this duration is enough to let all other threads in this test
// run until blocking.
const SLEEP: Duration = Duration::from_millis(100);

/// Sync + !Send.
struct Probe<'a> {
    owner: ThreadId,
    dropped: &'a AtomicBool,
    counter: AtomicUsize,
    _raw: *const (),
}
unsafe impl Sync for Probe<'_> {}

impl<'a> Probe<'a> {
    fn new(dropped: &'a AtomicBool) -> Self {
        Self {
            owner: thread::current().id(),
            dropped,
            counter: AtomicUsize::default(),
            _raw: std::ptr::null(),
        }
    }

    fn ensure_alive(&self) {
        // To make sure the object is alive, in Miri.
        self.counter.fetch_add(1, Relaxed);
    }
}

impl Drop for Probe<'_> {
    fn drop(&mut self) {
        assert_eq!(
            thread::current().id(),
            self.owner,
            "destroyed off the owning thread"
        );
        assert!(!self.dropped.swap(true, Relaxed), "destroyed twice");
    }
}

fn value_is_dropped_on_the_owning_thread(destroy: fn(WeakLender<Probe>)) {
    let dropped = AtomicBool::new(false);
    let barrier = Barrier::new(2);

    let lender = WeakLender::new(Probe::new(&dropped));
    let weak = WeakLender::downgrade(&lender);

    thread::scope(|s| {
        s.spawn({
            let barrier = &barrier;
            move || {
                let guard = weak.upgrade().expect("lender is still alive");

                barrier.wait();
                // The lender will start destruction before the guard is returned.
                thread::sleep(SLEEP);
                guard.ensure_alive();
                drop(guard);
            }
        });

        barrier.wait();
        destroy(lender);
        assert!(dropped.load(Relaxed), "value was never destroyed");
    });
}

#[test]
fn drop_waits_for_guards() {
    value_is_dropped_on_the_owning_thread(|lender| drop(lender));
}

#[test]
fn into_inner_waits_for_guards() {
    value_is_dropped_on_the_owning_thread(|lender| drop(WeakLender::into_inner(lender)));
}

#[test]
fn value_is_dropped_in_stress() {
    let dropped = AtomicBool::new(false);
    let barrier = Barrier::new(4 + 1);

    let lender = WeakLender::new(Probe::new(&dropped));
    let weak = WeakLender::downgrade(&lender);

    thread::scope(|s| {
        for _ in 0..4 {
            s.spawn({
                let weak = weak.clone();
                let barrier = &barrier;
                move || {
                    let guard = weak.upgrade().expect("lender is alive");
                    barrier.wait();
                    drop(guard);
                    for _ in 0..64 {
                        if let Some(guard) = weak.upgrade() {
                            guard.ensure_alive();
                        }
                    }
                }
            });
        }

        barrier.wait();
        // Dropped while workers are stressing, so the drop hopefully
        // races against both live guards and fresh upgrade attempts.
        drop(lender);
        assert!(dropped.load(Relaxed), "value was never destroyed");
    });
}

#[test]
fn upgrade_fails_after_the_lender_is_gone() {
    let dropped = AtomicBool::new(false);

    let lender = WeakLender::new(Probe::new(&dropped));
    let weak = WeakLender::downgrade(&lender);

    drop(lender);
    assert!(dropped.load(Relaxed), "value was never destroyed");

    assert!(weak.upgrade().is_none());
}

#[test]
fn upgrade_fails_when_the_lender_starts_dropping() {
    let dropped = AtomicBool::new(false);
    let barrier = Barrier::new(2);

    let lender = WeakLender::new(Probe::new(&dropped));
    let weak = WeakLender::downgrade(&lender);

    thread::scope(|s| {
        s.spawn({
            let barrier = &barrier;
            move || {
                let guard = weak.upgrade().expect("lender is alive");
                barrier.wait();
                // Even though the guard is kept alive, new upgrades need to
                // eventually start failing once lender starts dropping.
                while let Some(guard) = weak.upgrade() {
                    guard.ensure_alive();
                    thread::yield_now();
                }
                drop(guard);
            }
        });

        barrier.wait();
        drop(lender);
        assert!(dropped.load(Relaxed), "value was never destroyed");
    });
}

#[test]
fn guard_dropped_when_its_holder_panics() {
    let dropped = AtomicBool::new(false);
    let barrier = Barrier::new(2);

    let lender = WeakLender::new(Probe::new(&dropped));
    let weak = WeakLender::downgrade(&lender);

    thread::scope(|s| {
        let worker = s.spawn({
            let barrier = &barrier;
            move || {
                let _guard = weak.upgrade().expect("lender is still alive");
                barrier.wait();
                panic!("worker exploded while holding a guard");
            }
        });

        barrier.wait();
        // Might hang here if unwinding skipped the guard's signalling path.
        drop(lender);
        assert!(dropped.load(Relaxed), "value was never destroyed");

        let panicked = worker.join().expect_err("worker was supposed to panic");
        assert_eq!(
            panicked.downcast_ref::<&str>().copied(),
            Some("worker exploded while holding a guard")
        );
    });
}

#[test]
fn panicking_drop_propagates() {
    struct PanicOnDrop;
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("drop exploded");
        }
    }

    let barrier = Barrier::new(2);

    let lender = WeakLender::new(PanicOnDrop);
    let weak = WeakLender::downgrade(&lender);

    let panicked = thread::scope(|s| {
        s.spawn({
            let barrier = &barrier;
            move || {
                let guard = weak.upgrade().expect("lender is still alive");

                barrier.wait();
                // The lender will start dropping before the guard is returned.
                thread::sleep(SLEEP);
                drop(guard);
            }
        });

        barrier.wait();
        panic::catch_unwind(AssertUnwindSafe(|| drop(lender)))
            .expect_err("the drop's panic should have propagated")
    });

    assert_eq!(
        panicked.downcast_ref::<&str>().copied(),
        Some("drop exploded")
    );
}

#[test]
fn try_into_inner_fails_while_a_guard_is_alive() {
    let dropped = AtomicBool::new(false);

    let lender = WeakLender::new(Probe::new(&dropped));
    let weak = WeakLender::downgrade(&lender);

    let guard = weak.upgrade().expect("lender is still alive");
    let Err(lender) = WeakLender::try_into_inner(lender) else {
        panic!("try_into_inner succeeded despite guard");
    };

    // Ensure lender usable.
    lender.ensure_alive();
    drop(guard);
    assert!(weak.upgrade().is_some());

    let Ok(probe) = WeakLender::try_into_inner(lender) else {
        panic!("try_into_inner failed");
    };
    assert!(
        !dropped.load(Relaxed),
        "value was destroyed while unwrapping?"
    );
    assert!(weak.upgrade().is_none());
    drop(probe);
}
