//! The session model: ordered actions with first-class ordering.
//!
//! Ordering contract (SPEC decision 2): actions are sorted by
//! `(timestamp, agent lane [main first], source line number)`; equal-timestamp
//! cross-agent pairs are order-uncertain and excluded from strict
//! before/after findings. `Idx` is the stable handle every finding uses as
//! evidence and every payload exposes for `evidence()` dereferencing.

use serde::{Deserialize, Serialize};

/// Stable index of an action in the session's total order.
///
/// Monotonic within a session; findings cite `Idx` values as evidence and
/// the `evidence()` MCP tool dereferences them back to raw actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Idx(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idx_is_ordered_and_serializes_transparently() {
        assert!(Idx(2) > Idx(1));
        assert_eq!(serde_json::to_string(&Idx(102)).unwrap(), "102");
        assert_eq!(serde_json::from_str::<Idx>("102").unwrap(), Idx(102));
    }
}
