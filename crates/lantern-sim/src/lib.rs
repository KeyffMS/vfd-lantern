//! Development-only deterministic VFD simulator and PTY/RTU test harness.

#![forbid(unsafe_code)]

mod error;
mod identify;
mod pty;
mod runtime;
mod scenario;
mod service;
mod wire;

pub use error::*;
pub use identify::*;
pub use pty::*;
pub use runtime::*;
pub use scenario::*;
pub use service::*;
pub use wire::*;

/// Reports that this crate is excluded from user packages.
#[must_use]
pub const fn is_development_only() -> bool {
    true
}
