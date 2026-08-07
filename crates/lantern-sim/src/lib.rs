//! Development-only simulator boundary.

#![forbid(unsafe_code)]

/// Reports that the simulator crate is linked only by development tooling.
#[must_use]
pub const fn is_development_only() -> bool {
    true
}
