//! Named threads.
//!
//! Every long-lived std thread in efr has a name, so `top -H`, a panic message and a
//! debugger say which screen or reader it is. Screens need std threads because
//! libghostty-vt types are neither `Send` nor `Sync`, and each sets a small stack
//! because its loop is shallow.

use std::thread::{Builder, JoinHandle};

use crate::StdxError;

/// Starts a thread named `name`, with a stack of `stack_size` bytes, that runs `f`.
///
/// Linux shows at most 15 bytes of the name; Rust keeps all of it for panic messages.
/// A name with a NUL byte is an error here, where `std::thread::Builder` would panic.
///
/// ```
/// let handle = efr_stdx::thread::spawn_named("screen-0a1b", 512 * 1024, || 2 + 2)?;
/// assert_eq!(handle.join().ok(), Some(4));
/// # Ok::<(), efr_stdx::StdxError>(())
/// ```
pub fn spawn_named<F, T>(
    name: impl Into<String>,
    stack_size: usize,
    f: F,
) -> Result<JoinHandle<T>, StdxError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let name = name.into();
    if name.contains('\0') {
        return Err(StdxError::InvalidThreadName { name });
    }
    Builder::new()
        .name(name.clone())
        .stack_size(stack_size)
        .spawn(f)
        .map_err(|source| StdxError::SpawnThread { name, source })
}

#[cfg(test)]
mod tests;
