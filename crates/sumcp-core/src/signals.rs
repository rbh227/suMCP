//! Signals: pure functions `&Session -> Vec<Finding>`, one module per group.
//!
//! No I/O, no state — a signal only reads the session and returns findings,
//! each carrying the action `idxs` that prove it. This purity is what makes
//! the tool's output auditable and its tests trivial.

pub mod comprehension;
pub mod dynamics;
pub mod edit_shape;
pub mod failures;

pub use comprehension::comprehension;
pub use dynamics::dynamics;
pub use edit_shape::edit_shape;
pub use failures::failures;
