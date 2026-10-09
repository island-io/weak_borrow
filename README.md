# weak_borrow

[![crates.io](https://img.shields.io/crates/v/weak_borrow.svg)](https://crates.io/crates/weak_borrow)
[![docs.rs](https://img.shields.io/docsrs/weak_borrow)](https://docs.rs/weak_borrow)
[![CI](https://github.com/island-io/weak_borrow/actions/workflows/ci.yml/badge.svg)](https://github.com/island-io/weak_borrow/actions/workflows/ci.yml)

Excerpt from module docs (for more, see [source](./src/lib.rs) or
[docs.rs](https://docs.rs/weak_borrow)):

This crate is intended to enable reference-counted borrows across threads for
`!Send` types.  `WeakLender` owns `T` and creates `WeakBorrow`s that are `Send`.
Unlike `Arc`/`Weak`, `WeakBorrow` doesn't allow ownership transfer, which makes
it `Send` even for `!Send` types.

Author's motivating case is weakly sharing an FFI handle which it is safe to use
from any thread (`Sync`), but whose destructor must run on the thread that
created it, so the handle is `!Send`.

Assuming `T: !Send`, `WeakLender<T>` is the owner and stays on the creating
thread (`!Send`).  `WeakLender::downgrade` hands out `WeakBorrow<T>`, which *is*
`Send`, and other threads use `WeakBorrow::upgrade` to obtain `&T` via a guard
object.  Dropping the `WeakLender` blocks until every other strong reference is
gone, then destroys `T`.

If `T` is `Send`, or if you don't need cross-thread sharing, this crate isn't
very useful.

```
use weak_borrow::WeakLender;

struct FFISession {
    raw: *const (),
    owner: std::thread::ThreadId,
}

// SAFETY: the imaginary library locks its own runtime state, so concurrent
// calls through `&FFISession` are fine.
unsafe impl Sync for FFISession {}

impl FFISession {
    fn open() -> Self {
        Self {
            raw: std::ptr::null(),
            owner: std::thread::current().id(),
        }
    }
    fn something(&self) {
        // use session.
    }
}
impl Drop for FFISession {
    fn drop(&mut self) {
        assert_eq!(self.owner, std::thread::current().id());
        // destroy session (must be on original thread).
    }
}

let session = WeakLender::new(FFISession::open());

let weak = WeakLender::downgrade(&session);
let worker = std::thread::spawn(move || {
    for _ in 0..2 {
        match weak.upgrade() {
            Some(guard) => guard.something(), // guard derefs to &FFISession
            None => return,                   // owner gone or shutting down
        };
    }
});

drop(session); // waits for guards, then closes the session on this thread
```


## Minimum supported Rust version

1.85 (edition 2024). Bumping the MSRV is a minor version change.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
