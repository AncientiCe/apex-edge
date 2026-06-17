//! Pure availability math for the real-time inventory ledger.
//!
//! The edge tracks four quantities per item and derives "available to sell" from them.
//! HQ remains authoritative for on-hand stock (`hq_baseline_qty`); the edge layers
//! real-time reservations, committed sales, and locally-applied deltas on top so that
//! concurrent registers cannot oversell between periodic HQ syncs.

/// The four ledger quantities that determine how many units a store can still sell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryLedgerState {
    /// On-hand quantity as of the last HQ inventory sync (authoritative baseline).
    pub hq_baseline_qty: i64,
    /// Units currently held by open carts (reserved, not yet sold).
    pub reserved_qty: i64,
    /// Units sold locally since the last HQ baseline was applied.
    pub sold_since_sync_qty: i64,
    /// Net local stock delta since the baseline (receive/adjust/transfer/returns).
    pub local_adjust_qty: i64,
}

impl InventoryLedgerState {
    /// Units the store can still sell right now, clamped at zero.
    ///
    /// `available = baseline + local_adjust - reserved - sold_since_sync`
    pub fn available_to_sell(&self) -> i64 {
        let raw = self.hq_baseline_qty + self.local_adjust_qty
            - self.reserved_qty
            - self.sold_since_sync_qty;
        raw.max(0)
    }

    /// Whether `qty` additional units can be reserved without overselling.
    pub fn can_reserve(&self, qty: i64) -> bool {
        qty > 0 && self.available_to_sell() >= qty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(baseline: i64, reserved: i64, sold: i64, adjust: i64) -> InventoryLedgerState {
        InventoryLedgerState {
            hq_baseline_qty: baseline,
            reserved_qty: reserved,
            sold_since_sync_qty: sold,
            local_adjust_qty: adjust,
        }
    }

    #[test]
    fn available_is_baseline_when_no_activity() {
        assert_eq!(state(10, 0, 0, 0).available_to_sell(), 10);
    }

    #[test]
    fn reservations_and_sales_reduce_availability() {
        assert_eq!(state(10, 3, 2, 0).available_to_sell(), 5);
    }

    #[test]
    fn local_received_stock_increases_availability() {
        assert_eq!(state(10, 0, 0, 5).available_to_sell(), 15);
    }

    #[test]
    fn local_shrinkage_reduces_availability() {
        assert_eq!(state(10, 0, 0, -4).available_to_sell(), 6);
    }

    #[test]
    fn availability_never_goes_negative() {
        assert_eq!(state(2, 5, 3, 0).available_to_sell(), 0);
    }

    #[test]
    fn can_reserve_respects_availability() {
        let s = state(10, 3, 2, 0); // available = 5
        assert!(s.can_reserve(5));
        assert!(!s.can_reserve(6));
    }

    #[test]
    fn cannot_reserve_zero_or_negative() {
        let s = state(10, 0, 0, 0);
        assert!(!s.can_reserve(0));
        assert!(!s.can_reserve(-1));
    }
}
