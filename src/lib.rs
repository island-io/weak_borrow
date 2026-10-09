//! This crate is intended to enable reference-counted borrows across threads
//! for `!Send` types.  `WeakLender` owns `T` and creates `WeakBorrow`s that are
//! `Send`.  Unlike `Arc`/`Weak`, `WeakBorrow` doesn't allow ownership transfer,
//! which makes it `Send` even for `!Send` types.
//!
//! Author's motivating case is weakly sharing an FFI handle which it is safe to
//! use from any thread (`Sync`), but whose destructor must run on the thread
//! that created it, so the handle is `!Send`.
//!
//! Assuming `T: !Send`, [`WeakLender<T>`] is the owner and stays on the
//! creating thread (`!Send`).  [`WeakLender::downgrade`] hands out
//! [`WeakBorrow<T>`], which *is* `Send`, and other threads use
//! [`WeakBorrow::upgrade`] to obtain `&T` via a guard object.  Dropping the
//! `WeakLender` blocks until every other strong reference is gone, then
//! destroys `T`.
//!
//! If `T` is `Send`, or if you don't need cross-thread sharing, this crate
//! isn't very useful.
//!
//! ```
//! use weak_borrow::WeakLender;
//!
//! struct FFISession {
//!     raw: *const (),
//!     owner: std::thread::ThreadId,
//! }
//!
//! // SAFETY: the imaginary library locks its own runtime state, so concurrent
//! // calls through `&FFISession` are fine.
//! unsafe impl Sync for FFISession {}
//!
//! impl FFISession {
//!     fn open() -> Self {
//!         Self {
//!             raw: std::ptr::null(),
//!             owner: std::thread::current().id(),
//!         }
//!     }
//!     fn something(&self) {
//!         // use session.
//!     }
//! }
//! impl Drop for FFISession {
//!     fn drop(&mut self) {
//!         assert_eq!(self.owner, std::thread::current().id());
//!         // destroy session (must be on original thread).
//!     }
//! }
//!
//! let session = WeakLender::new(FFISession::open());
//!
//! let weak = WeakLender::downgrade(&session);
//! let worker = std::thread::spawn(move || {
//!     for _ in 0..2 {
//!         match weak.upgrade() {
//!             Some(guard) => guard.something(), // guard derefs to &FFISession
//!             None => return,                   // owner gone or shutting down
//!         };
//!     }
//! });
//!
//! drop(session); // waits for guards, then closes the session on this thread
//! ```
//!
//! ## Deadlocks to avoid
//!
//! Since dropping the owner blocks, a guard must always be released
//! eventually. Some ways to get stuck:
//!
//! - Holding a guard while waiting on the owning thread for something else.
//! - Upgrading on the owning thread and then dropping the `WeakLender` while
//!   that guard is still alive. Use plain `Deref` on WeakLender itself instead.
//! - Leaking or `mem::forget`ing the guard, obviously.
//! - Using it in async context, since a drop could block the executor.

use std::{
    fmt,
    mem::ManuallyDrop,
    ops::Deref,
    ptr,
    sync::{Arc, Condvar, Mutex, Weak, atomic, atomic::AtomicBool},
};

/// Owns `T` and weakly lends it out to other threads.  The value is destroyed on the
/// thread that holds the lender.
///
/// To pass a weak `&T` to other threads, go through
/// [`downgrade`](Self::downgrade).  On the owning thread just use `Deref`, no
/// borrow needed.
///
/// Dropping this blocks until every [`BorrowGuard`] created by
/// [`WeakBorrow::upgrade`] has been dropped, so see the crate-level deadlock
/// notes.  To make that wait explicit, or to keep the value, use
/// [`into_inner`](Self::into_inner) or
/// [`try_into_inner`](Self::try_into_inner).
pub struct WeakLender<T> {
    // `inner` must be dropped only when it is the last reference, so we wrap it
    // in ManuallyDrop to precisely control the drop.
    inner: ManuallyDrop<Arc<T>>,
    sync: Arc<WeakLenderSync>,
}

#[derive(Default)]
struct WeakLenderSync {
    // Used to block creation of new strong references (Relaxed), to help us
    // drain, and to tell dropping guards that the lender may be waiting for a
    // notify (ordered by SeqCst fences).  Other memory ordering provided by
    // `Arc`.
    lender_dropping: AtomicBool,

    // Used to wait for existing strong references to drop.
    dropping_mutex: Mutex<()>,
    dropping_cv: Condvar,
}

impl<T> WeakLender<T> {
    /// Takes ownership of `inner`.
    pub fn new(inner: T) -> Self {
        Self {
            inner: ManuallyDrop::new(Arc::new(inner)),
            sync: Default::default(),
        }
    }

    /// Creates a `Send` [`WeakBorrow`].
    pub fn downgrade(lender: &Self) -> WeakBorrow<T> {
        WeakBorrow {
            inner: Arc::downgrade(&lender.inner),
            sync: lender.sync.clone(),
        }
    }

    /// Extracts the inner `T`.  Blocks until no [`BorrowGuard`] is alive, so
    /// see the crate-level deadlock notes.
    pub fn into_inner(lender: Self) -> T {
        let (inner, sync) = lender.into_parts();
        Self::wait_unwrap(inner, &sync)
    }

    /// Returns `T` if no [`BorrowGuard`] is alive, otherwise gives the lender
    /// back.  Never blocks.
    pub fn try_into_inner(lender: Self) -> Result<T, Self> {
        let (inner, sync) = lender.into_parts();
        Self::try_unwrap(inner).map_err(|inner| Self { inner, sync })
    }

    fn into_parts(self) -> (ManuallyDrop<Arc<T>>, Arc<WeakLenderSync>) {
        let this = ManuallyDrop::new(self);
        // Exhaustive, so new fields fail to compile.
        let Self { inner, sync } = &*this;
        unsafe {
            // SAFETY: `this` is never used or dropped again, so each field is
            // moved out exactly once.
            (ptr::read(inner), ptr::read(sync))
        }
    }

    /// Blocks until it succeeds to unwrap `inner`.
    fn wait_unwrap(mut inner: ManuallyDrop<Arc<T>>, sync: &WeakLenderSync) -> T {
        sync.lender_dropping.store(true, atomic::Ordering::Relaxed);

        // Pairs with the fence in `BorrowGuard::drop`.  See comment over there.
        atomic::fence(atomic::Ordering::SeqCst);

        // This is effectively cv.wait_while() but allows us to return in the
        // middle.  Regarding mutex unwraps: Nothing that locks the mutex can
        // panic.
        let mut guard = sync.dropping_mutex.lock().unwrap();
        loop {
            match Self::try_unwrap(inner) {
                Ok(value) => return value,
                Err(arc) => inner = arc,
            }
            guard = sync.dropping_cv.wait(guard).unwrap();
        }
    }

    /// Unwraps `inner` if it is the last strong reference, otherwise gives it
    /// back.
    fn try_unwrap(inner: ManuallyDrop<Arc<T>>) -> Result<T, ManuallyDrop<Arc<T>>> {
        // This doesn't panic so there is no risk of dropping the in-flight
        // `Arc<T>`.
        Arc::try_unwrap(ManuallyDrop::into_inner(inner)).map_err(ManuallyDrop::new)
    }
}

impl<T> Drop for WeakLender<T> {
    fn drop(&mut self) {
        let inner = unsafe {
            // SAFETY: `self` is being dropped, so `inner` is never touched
            // again.  Being `ManuallyDrop`, it won't be dropped twice either.
            ptr::read(&self.inner)
        };
        drop(Self::wait_unwrap(inner, &self.sync));
    }
}

impl<T> Deref for WeakLender<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// SAFETY: The reason `Sync` isn't implemented automatically is because a `T:
// !Send` makes `Arc<T>` non-`Send` but this crate doesn't expose the `Arc`.
unsafe impl<T: Sync> Sync for WeakLender<T> {}

impl<T: fmt::Debug> fmt::Debug for WeakLender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakLender")
            .field("value", &**self)
            .finish()
    }
}

/// Sendable weak pointer to a [`WeakLender<T>`], obtained from
/// [`WeakLender::downgrade`].
///
/// Holding one doesn't keep the value alive on its own.  Obtain a strong
/// reference with [`upgrade`](Self::upgrade).
pub struct WeakBorrow<T> {
    inner: Weak<T>,
    sync: Arc<WeakLenderSync>,
}

// Hand-written rather than derived: `#[derive(Clone)]` would demand `T: Clone`,
// which cloning a weak pointer doesn't need.  `std`'s `Weak` does the same.
impl<T> Clone for WeakBorrow<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            sync: self.sync.clone(),
        }
    }
}

// SAFETY: We never let the other thread get ownership of T, only a
// reference.  WeakLender is the only entity that gets to destroy T.
unsafe impl<T: Sync> Send for WeakBorrow<T> {}

// SAFETY: The reason `Sync` isn't implemented automatically is because a `T:
// !Send` makes `Weak<T>` non-`Send` but this crate doesn't expose the `Weak`.
unsafe impl<T: Sync> Sync for WeakBorrow<T> {}

impl<T> fmt::Debug for WeakBorrow<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakBorrow").finish_non_exhaustive()
    }
}

impl<T> WeakBorrow<T> {
    /// Borrows the value for the lifetime of the returned guard, or returns
    /// `None` if the lender is gone or has started going away.
    ///
    /// Drop the guard promptly, since the lender's drop waits on it.
    pub fn upgrade(&self) -> Option<BorrowGuard<'_, T>> {
        if self.sync.lender_dropping.load(atomic::Ordering::Relaxed) {
            return None;
        }

        let inner = self.inner.upgrade()?;
        Some(BorrowGuard {
            inner: ManuallyDrop::new(inner),
            sync: &self.sync,
        })
    }
}

/// Returned by [`WeakBorrow::upgrade`] and gives access to `&T` via Deref.
pub struct BorrowGuard<'a, T> {
    // Important to never give it as an Arc outside of the module!  It's
    // ManuallyDrop so that we can drop it before signaling WeakLender.
    inner: ManuallyDrop<Arc<T>>,
    sync: &'a WeakLenderSync,
}

impl<T> Drop for BorrowGuard<'_, T> {
    fn drop(&mut self) {
        // This drop can't panic because Arc::drop itself doesn't panic and
        // T::drop won't be called since the WeakLender holds a strong reference
        // right now.  This means we're guaranteed to reach the signaling code.
        unsafe {
            // SAFETY: inner won't be accessed again.
            ManuallyDrop::drop(&mut self.inner);
        }

        // Pairs with the fence in `wait_unwrap`.  Wow, actual usecase for
        // SeqCst!  We don't want to lock and signal the Condvar unless the
        // lender could be waiting.  The atomic ops:
        //
        // - guard (this): decrement refcount (in `Arc::drop`), [fence], load
        // `lender_dropping`, if true notify under mutex.
        //
        // - lender: store `lender_dropping`, [fence], lock the mutex, read
        // refcount (in `Arc::try_unwrap`), if > 1 wait.
        //
        // Invariant: each guard's decrement is either seen by the lender's
        // first refcount read, or followed by a notify.  So the lender can't
        // miss a notification for the last decrement.  Per guard:
        //
        // - Guard's fence is first in the SeqCst order: lender loads refcount
        // after its fence sees the guard's decrement (or a later value).
        //
        // - Lender's fence is first: Guard loads `lender_dropping` as true,
        // notifies lender.
        //
        // With weaker fences, both loads may be stale (`lender_dropping ==
        // false` here, refcount 2 there), and the lender waits for a notify
        // that never comes.
        //
        // This is a variant of the store-buffering pattern (Dekker's algorithm
        // and such), which needs SeqCst.
        atomic::fence(atomic::Ordering::SeqCst);

        if self.sync.lender_dropping.load(atomic::Ordering::Relaxed) {
            let _guard = self.sync.dropping_mutex.lock().unwrap();
            // This doesn't panic, so guard won't be poisoned.
            self.sync.dropping_cv.notify_one();
        }
    }
}

impl<T> Deref for BorrowGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// SAFETY: A guard is never the last strong reference so it can be dropped on
// any thread.  Beyond dropping, a guard only yields `&T`, which means it can be
// shared and moved between threads when `T: Sync`.
unsafe impl<T: Sync> Send for BorrowGuard<'_, T> {}
unsafe impl<T: Sync> Sync for BorrowGuard<'_, T> {}

impl<T: fmt::Debug> fmt::Debug for BorrowGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// Doctests for the Send/Sync hard invariants:
///
/// Borrow: Send when T: Sync.
///
/// ```
/// fn assert_send<T: Send>() {}
/// // `MutexGuard` is `Sync` but not `Send`.
/// assert_send::<weak_borrow::WeakBorrow<std::sync::MutexGuard<'static, u32>>>();
/// ```
///
/// Borrow: !Send when T: !Sync.
///
/// ```compile_fail,E0277
/// fn assert_send<T: Send>() {}
/// // `Cell` is `Send` but not `Sync`.
/// assert_send::<weak_borrow::WeakBorrow<std::cell::Cell<u32>>>();
/// ```
///
/// BorrowGuard: !Send when T: !Sync.
///
/// ```compile_fail,E0277
/// fn assert_send<T: Send>() {}
/// // `Cell` is `Send` but not `Sync`.
/// assert_send::<weak_borrow::BorrowGuard<std::cell::Cell<u32>>>();
/// ```
///
/// Lender: !Send when T: !Send.
///
/// ```compile_fail,E0277
/// fn assert_send<T: Send>() {}
/// // `MutexGuard` is `Sync` but not `Send`.
/// assert_send::<weak_borrow::WeakLender<std::sync::MutexGuard<'static, u32>>>();
/// ```
#[cfg(doctest)]
pub mod send_bounds {}
