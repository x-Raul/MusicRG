//! Bloqueo de mutex sin `unwrap()`.
//!
//! Un mutex queda "envenenado" si un hilo entra en pánico mientras lo tiene.
//! Aquí todo lo que protegen es estado simple (listas, banderas, la carátula
//! en caché) que sigue siendo válido aunque eso pase, así que se sigue usando
//! en vez de propagar el pánico. En release, además, `panic = "abort"`.

use std::sync::{Mutex, MutexGuard, PoisonError};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
