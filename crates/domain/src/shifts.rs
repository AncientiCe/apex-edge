//! Till & Shift state machine.
//!
//! A shift opens when cash is counted into the drawer (`opening_float`). Orders and
//! returns created during the shift accumulate totals; paid-in/paid-out/no-sale movements
//! are recorded. When closed, the counted cash is compared to the expected cash and the
//! variance is recorded.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShiftState {
    Open,
    Closed,
}

impl ShiftState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShiftError {
    #[error("shift is already closed")]
    AlreadyClosed,
    #[error("variance {0} exceeds threshold; supervisor approval required")]
    VarianceRequiresApproval(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CashMovementKind {
    PaidIn,
    PaidOut,
    NoSale,
}

impl CashMovementKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PaidIn => "paid_in",
            Self::PaidOut => "paid_out",
            Self::NoSale => "no_sale",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "paid_in" => Some(Self::PaidIn),
            "paid_out" => Some(Self::PaidOut),
            "no_sale" => Some(Self::NoSale),
            _ => None,
        }
    }
    /// Signed cash-drawer impact of this movement type.
    pub fn signed_effect(self) -> i64 {
        match self {
            Self::PaidIn => 1,
            Self::PaidOut => -1,
            Self::NoSale => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CashMovement {
    pub id: Uuid,
    pub kind: CashMovementKind,
    pub amount_cents: u64,
    pub reason: Option<String>,
    pub approval_id: Option<Uuid>,
}

/// Compute the cash drawer's expected balance at close.
/// `opening_float_cents + cash_sales_cents - cash_refunds_cents + sum(paid_in) - sum(paid_out)`
pub fn expected_cash_cents(
    opening_float_cents: u64,
    cash_sales_cents: u64,
    cash_refunds_cents: u64,
    movements: &[CashMovement],
) -> i64 {
    let mut total = opening_float_cents as i64;
    total += cash_sales_cents as i64;
    total -= cash_refunds_cents as i64;
    for m in movements {
        total += m.kind.signed_effect() * (m.amount_cents as i64);
    }
    total
}

/// Compute variance = counted - expected.
pub fn variance_cents(counted_cents: i64, expected_cents: i64) -> i64 {
    counted_cents - expected_cents
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(kind: CashMovementKind, amount: u64) -> CashMovement {
        CashMovement {
            id: Uuid::new_v4(),
            kind,
            amount_cents: amount,
            reason: None,
            approval_id: None,
        }
    }

    #[test]
    fn expected_is_opening_plus_sales_minus_refunds_plus_movements() {
        let movements = vec![
            mv(CashMovementKind::PaidIn, 500),
            mv(CashMovementKind::PaidOut, 200),
            mv(CashMovementKind::NoSale, 0),
        ];
        let e = expected_cash_cents(10_000, 25_000, 3_000, &movements);
        // 10_000 + 25_000 - 3_000 + 500 - 200 = 32_300
        assert_eq!(e, 32_300);
    }

    #[test]
    fn variance_is_counted_minus_expected() {
        assert_eq!(variance_cents(32_000, 32_300), -300);
        assert_eq!(variance_cents(32_500, 32_300), 200);
        assert_eq!(variance_cents(32_300, 32_300), 0);
    }

    #[test]
    fn cash_movement_kind_parses_and_stringifies() {
        assert_eq!(
            CashMovementKind::parse("paid_in"),
            Some(CashMovementKind::PaidIn)
        );
        assert_eq!(CashMovementKind::NoSale.as_str(), "no_sale");
        assert_eq!(CashMovementKind::parse("invalid"), None);
    }
}
