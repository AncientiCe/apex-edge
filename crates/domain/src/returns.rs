//! Returns & Refunds state machine.
//!
//! States:
//! ```text
//! Open  -(return_line_item)->  Items
//! Items -(refund_tender)----->  Tendered
//! Tendered -(refund_tender)-->  Tendered   (accumulates until refunded == total)
//! Tendered -(refund_tender)-->  Paid       (when sum(refunds) >= total)
//! Items/Tendered/Paid -(finalize_return)-> Finalized
//! Open/Items/Tendered/Paid -(void_return)-> Voided
//! ```
//!
//! Receipted returns (with an `original_order_id`) enforce per-line quantity limits;
//! blind returns (no original) require supervisor approval captured at `start_return`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnState {
    Open,
    Items,
    Tendered,
    Paid,
    Finalized,
    Voided,
}

impl ReturnState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Items => "items",
            Self::Tendered => "tendered",
            Self::Paid => "paid",
            Self::Finalized => "finalized",
            Self::Voided => "voided",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "items" => Some(Self::Items),
            "tendered" => Some(Self::Tendered),
            "paid" => Some(Self::Paid),
            "finalized" => Some(Self::Finalized),
            "voided" => Some(Self::Voided),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReturnError {
    #[error("return is not in a state that allows this transition: {0:?}")]
    InvalidState(ReturnState),
    #[error("line quantity {requested} exceeds returnable quantity {allowed}")]
    QuantityExceeded { requested: u32, allowed: u32 },
    #[error("refund tender {0} cents exceeds return total")]
    OverRefund(u64),
    #[error("blind return requires supervisor approval")]
    ApprovalRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnLineSnapshot {
    pub line_id: Uuid,
    pub original_line_id: Option<Uuid>,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub tax_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundSnapshot {
    pub refund_id: Uuid,
    pub tender_type: String,
    pub amount_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnSnapshot {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub original_order_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub state: ReturnState,
    pub total_cents: u64,
    pub tax_cents: u64,
    pub refunded_cents: u64,
    pub approval_id: Option<Uuid>,
    pub lines: Vec<ReturnLineSnapshot>,
    pub refunds: Vec<RefundSnapshot>,
}

impl ReturnSnapshot {
    pub fn is_blind(&self) -> bool {
        self.original_order_id.is_none()
    }

    /// Add a line; updates state Open -> Items when first line is added.
    pub fn add_line(
        &mut self,
        line: ReturnLineSnapshot,
        max_allowed: Option<u32>,
    ) -> Result<(), ReturnError> {
        if !matches!(self.state, ReturnState::Open | ReturnState::Items) {
            return Err(ReturnError::InvalidState(self.state));
        }
        if let Some(max) = max_allowed {
            let already: u32 = self
                .lines
                .iter()
                .filter(|l| l.original_line_id == line.original_line_id)
                .map(|l| l.quantity)
                .sum();
            if already + line.quantity > max {
                return Err(ReturnError::QuantityExceeded {
                    requested: already + line.quantity,
                    allowed: max,
                });
            }
        }
        self.total_cents = self
            .total_cents
            .saturating_add(line.line_total_cents.saturating_add(line.tax_cents));
        self.tax_cents = self.tax_cents.saturating_add(line.tax_cents);
        self.lines.push(line);
        self.state = ReturnState::Items;
        Ok(())
    }

    /// Apply a refund tender; advances to Tendered, and to Paid when refunded sum meets total.
    pub fn apply_refund(&mut self, refund: RefundSnapshot) -> Result<(), ReturnError> {
        if !matches!(self.state, ReturnState::Items | ReturnState::Tendered) {
            return Err(ReturnError::InvalidState(self.state));
        }
        let new_refunded = self.refunded_cents.saturating_add(refund.amount_cents);
        if new_refunded > self.total_cents {
            return Err(ReturnError::OverRefund(refund.amount_cents));
        }
        self.refunded_cents = new_refunded;
        self.refunds.push(refund);
        self.state = if self.refunded_cents >= self.total_cents {
            ReturnState::Paid
        } else {
            ReturnState::Tendered
        };
        Ok(())
    }

    pub fn can_finalize(&self) -> bool {
        matches!(self.state, ReturnState::Paid)
            || (matches!(self.state, ReturnState::Items) && self.total_cents == 0)
    }

    pub fn finalize(&mut self) -> Result<(), ReturnError> {
        if !self.can_finalize() {
            return Err(ReturnError::InvalidState(self.state));
        }
        self.state = ReturnState::Finalized;
        Ok(())
    }

    pub fn void(&mut self) -> Result<(), ReturnError> {
        if matches!(self.state, ReturnState::Finalized) {
            return Err(ReturnError::InvalidState(self.state));
        }
        self.state = ReturnState::Voided;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_return() -> ReturnSnapshot {
        ReturnSnapshot {
            id: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            shift_id: None,
            original_order_id: Some(Uuid::new_v4()),
            reason_code: Some("damaged".into()),
            state: ReturnState::Open,
            total_cents: 0,
            tax_cents: 0,
            refunded_cents: 0,
            approval_id: None,
            lines: vec![],
            refunds: vec![],
        }
    }

    fn line(qty: u32, price: u64, tax: u64) -> ReturnLineSnapshot {
        ReturnLineSnapshot {
            line_id: Uuid::new_v4(),
            original_line_id: Some(Uuid::nil()),
            sku: "SKU-1".into(),
            name: "Widget".into(),
            quantity: qty,
            unit_price_cents: price,
            line_total_cents: price * qty as u64,
            tax_cents: tax,
        }
    }

    #[test]
    fn add_line_moves_open_to_items_and_sums_total() {
        let mut r = empty_return();
        r.add_line(line(2, 500, 100), Some(5)).unwrap();
        assert_eq!(r.state, ReturnState::Items);
        assert_eq!(r.total_cents, 1100); // 2*500 + 100 tax
    }

    #[test]
    fn over_quantity_is_rejected_for_receipted_returns() {
        let mut r = empty_return();
        let err = r.add_line(line(10, 500, 0), Some(3)).unwrap_err();
        assert!(matches!(err, ReturnError::QuantityExceeded { .. }));
    }

    #[test]
    fn refunds_advance_to_tendered_then_paid() {
        let mut r = empty_return();
        r.add_line(line(2, 1000, 0), None).unwrap();
        assert_eq!(r.total_cents, 2000);

        r.apply_refund(RefundSnapshot {
            refund_id: Uuid::new_v4(),
            tender_type: "cash".into(),
            amount_cents: 500,
        })
        .unwrap();
        assert_eq!(r.state, ReturnState::Tendered);

        r.apply_refund(RefundSnapshot {
            refund_id: Uuid::new_v4(),
            tender_type: "cash".into(),
            amount_cents: 1500,
        })
        .unwrap();
        assert_eq!(r.state, ReturnState::Paid);
        assert_eq!(r.refunded_cents, 2000);
    }

    #[test]
    fn over_refund_is_rejected() {
        let mut r = empty_return();
        r.add_line(line(1, 1000, 0), None).unwrap();
        let err = r
            .apply_refund(RefundSnapshot {
                refund_id: Uuid::new_v4(),
                tender_type: "cash".into(),
                amount_cents: 2000,
            })
            .unwrap_err();
        assert!(matches!(err, ReturnError::OverRefund(2000)));
    }

    #[test]
    fn cannot_finalize_before_refund() {
        let mut r = empty_return();
        r.add_line(line(1, 1000, 0), None).unwrap();
        assert!(r.finalize().is_err());
    }

    #[test]
    fn can_finalize_after_full_refund() {
        let mut r = empty_return();
        r.add_line(line(1, 1000, 0), None).unwrap();
        r.apply_refund(RefundSnapshot {
            refund_id: Uuid::new_v4(),
            tender_type: "cash".into(),
            amount_cents: 1000,
        })
        .unwrap();
        r.finalize().unwrap();
        assert_eq!(r.state, ReturnState::Finalized);
    }

    #[test]
    fn void_from_open_is_allowed() {
        let mut r = empty_return();
        r.void().unwrap();
        assert_eq!(r.state, ReturnState::Voided);
    }

    #[test]
    fn cannot_void_after_finalize() {
        let mut r = empty_return();
        r.add_line(line(1, 0, 0), None).unwrap();
        r.finalize().unwrap();
        assert!(r.void().is_err());
    }

    #[test]
    fn blind_return_snapshot_reports_is_blind() {
        let mut r = empty_return();
        r.original_order_id = None;
        assert!(r.is_blind());
    }
}
