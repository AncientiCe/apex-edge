//! Cart aggregate and state machine: Open -> Itemized -> Discounted -> Tendering -> Paid -> Finalized.

use apex_edge_contracts::{
    AppliedCouponInfo, AppliedPromoInfo, CartLine, CartState, CartStateKind, ManualDiscountInfo,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::DomainError;
use crate::order::{Order, OrderLine};
use crate::pricing::LinePriceResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cart {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub state: CartStateKind,
    pub lines: Vec<CartLineItem>,
    pub applied_promo_ids: Vec<Uuid>,
    pub applied_coupons: Vec<AppliedCouponRecord>,
    pub manual_discounts: Vec<ManualDiscountInfo>,
    pub payments: Vec<PaymentRecord>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for adding a line item to a cart (avoids too many arguments on add_line_item).
#[derive(Debug, Clone)]
pub struct AddLineItemInput {
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
    pub modifier_option_ids: Vec<Uuid>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CartLineItem {
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub modifier_option_ids: Vec<Uuid>,
    pub notes: Option<String>,
    /// Filled by pricing engine
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedCouponRecord {
    pub coupon_id: Uuid,
    pub code: String,
    pub discount_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRecord {
    pub tender_id: Uuid,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

impl Cart {
    pub fn new(id: Uuid, store_id: Uuid, register_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id,
            store_id,
            register_id,
            customer_id: None,
            state: CartStateKind::Open,
            lines: Vec::new(),
            applied_promo_ids: Vec::new(),
            applied_coupons: Vec::new(),
            manual_discounts: Vec::new(),
            payments: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn set_customer(&mut self, customer_id: Uuid) {
        self.customer_id = Some(customer_id);
        self.updated_at = Utc::now();
    }

    /// Add a line item (caller must run pricing pipeline and apply_pricing afterward).
    pub fn add_line_item(&mut self, input: AddLineItemInput) {
        let line_total_cents = input.unit_price_cents.saturating_mul(input.quantity as u64);
        self.lines.push(CartLineItem {
            line_id: input.line_id,
            item_id: input.item_id,
            sku: input.sku,
            name: input.name,
            quantity: input.quantity,
            modifier_option_ids: input.modifier_option_ids,
            notes: input.notes,
            unit_price_cents: input.unit_price_cents,
            line_total_cents,
            discount_cents: 0,
            tax_cents: 0,
        });
        self.updated_at = Utc::now();
        self.state = if self.lines.is_empty() {
            CartStateKind::Open
        } else {
            CartStateKind::Itemized
        };
    }

    pub fn ensure_can_edit(&self) -> Result<(), DomainError> {
        match self.state {
            CartStateKind::Open | CartStateKind::Itemized | CartStateKind::Discounted => Ok(()),
            CartStateKind::Tendering
            | CartStateKind::Paid
            | CartStateKind::Finalized
            | CartStateKind::Voided => Err(DomainError::InvalidTransition(format!(
                "cart in state {:?} cannot be edited",
                self.state
            ))),
        }
    }

    pub fn ensure_can_tender(&self) -> Result<(), DomainError> {
        match self.state {
            CartStateKind::Itemized | CartStateKind::Discounted => Ok(()),
            _ => Err(DomainError::InvalidTransition(
                "cart must be itemized or discounted to enter tendering".into(),
            )),
        }
    }

    pub fn ensure_can_finalize(&self) -> Result<(), DomainError> {
        if self.state != CartStateKind::Paid {
            return Err(DomainError::InvalidTransition(
                "cart must be paid to finalize".into(),
            ));
        }
        let total = self.total_cents();
        let tendered: u64 = self.payments.iter().map(|p| p.amount_cents).sum();
        if tendered < total {
            return Err(DomainError::PaymentExceedsTotal);
        }
        Ok(())
    }

    /// Total = subtotal - discounts + tax (coupon discounts applied at basket level).
    pub fn total_cents(&self) -> u64 {
        let lines_net: u64 = self
            .lines
            .iter()
            .map(|l| {
                l.line_total_cents
                    .saturating_sub(l.discount_cents)
                    .saturating_add(l.tax_cents)
            })
            .sum();
        let coupon_discount: u64 = self.applied_coupons.iter().map(|c| c.discount_cents).sum();
        lines_net.saturating_sub(coupon_discount)
    }

    pub fn subtotal_cents(&self) -> u64 {
        self.lines.iter().map(|l| l.line_total_cents).sum()
    }

    pub fn discount_cents(&self) -> u64 {
        let line_discounts: u64 = self.lines.iter().map(|l| l.discount_cents).sum();
        let coupon_discounts: u64 = self.applied_coupons.iter().map(|c| c.discount_cents).sum();
        line_discounts + coupon_discounts
    }

    pub fn tax_cents(&self) -> u64 {
        self.lines.iter().map(|l| l.tax_cents).sum()
    }

    pub fn tendered_cents(&self) -> u64 {
        self.payments.iter().map(|p| p.amount_cents).sum()
    }

    /// Apply pricing results from pipeline to lines and recompute state.
    pub fn apply_pricing(&mut self, line_results: Vec<LinePriceResult>) {
        for result in line_results {
            if let Some(line) = self.lines.iter_mut().find(|l| l.line_id == result.line_id) {
                line.unit_price_cents = result.unit_price_cents;
                line.line_total_cents = result.line_total_cents;
                line.discount_cents = result.discount_cents;
                line.tax_cents = result.tax_cents;
            }
        }
        self.updated_at = Utc::now();
        self.state = if self.lines.is_empty() {
            CartStateKind::Open
        } else {
            CartStateKind::Itemized
        };
    }

    /// Remove a line item by `line_id`. Returns `LineNotFound` if absent or `InvalidTransition`
    /// if the cart cannot be edited. Re-evaluates state after removal.
    pub fn remove_line_item(&mut self, line_id: Uuid) -> Result<(), DomainError> {
        self.ensure_can_edit()?;
        let pos = self
            .lines
            .iter()
            .position(|l| l.line_id == line_id)
            .ok_or(DomainError::LineNotFound(line_id))?;
        self.lines.remove(pos);
        self.updated_at = Utc::now();
        self.state = if self.lines.is_empty() {
            CartStateKind::Open
        } else {
            CartStateKind::Itemized
        };
        Ok(())
    }

    pub fn set_discounted(&mut self) {
        self.state = CartStateKind::Discounted;
        self.updated_at = Utc::now();
    }

    pub fn set_tendering(&mut self) {
        self.state = CartStateKind::Tendering;
        self.updated_at = Utc::now();
    }

    pub fn add_payment(
        &mut self,
        tender_id: Uuid,
        amount_cents: u64,
        external_reference: Option<String>,
    ) -> Result<(), DomainError> {
        if self.state != CartStateKind::Tendering && self.state != CartStateKind::Paid {
            return Err(DomainError::InvalidTransition(
                "cart must be in tendering to add payment".into(),
            ));
        }
        self.payments.push(PaymentRecord {
            tender_id,
            amount_cents,
            external_reference,
        });
        self.updated_at = Utc::now();
        let tendered = self.tendered_cents();
        let total = self.total_cents();
        if tendered >= total {
            self.state = CartStateKind::Paid;
        }
        Ok(())
    }

    pub fn set_finalized(&mut self) {
        self.state = CartStateKind::Finalized;
        self.updated_at = Utc::now();
    }

    pub fn set_voided(&mut self) {
        self.state = CartStateKind::Voided;
        self.updated_at = Utc::now();
    }

    pub fn to_cart_state(&self) -> CartState {
        CartState {
            cart_id: self.id,
            customer_id: self.customer_id,
            customer_name: None,
            customer_code: None,
            state: self.state.clone(),
            lines: self
                .lines
                .iter()
                .map(|l| CartLine {
                    line_id: l.line_id,
                    item_id: l.item_id,
                    sku: l.sku.clone(),
                    name: l.name.clone(),
                    quantity: l.quantity,
                    unit_price_cents: l.unit_price_cents,
                    line_total_cents: l.line_total_cents,
                    discount_cents: l.discount_cents,
                    tax_cents: l.tax_cents,
                    modifier_option_ids: l.modifier_option_ids.clone(),
                    notes: l.notes.clone(),
                })
                .collect(),
            applied_promos: self
                .applied_promo_ids
                .iter()
                .map(|promo_id| AppliedPromoInfo {
                    promo_id: *promo_id,
                    name: promo_id.to_string(),
                    code: None,
                })
                .collect(),
            applied_coupons: self
                .applied_coupons
                .iter()
                .map(|c| AppliedCouponInfo {
                    coupon_id: c.coupon_id,
                    code: c.code.clone(),
                    discount_cents: c.discount_cents,
                })
                .collect(),
            manual_discounts: self.manual_discounts.clone(),
            subtotal_cents: self.subtotal_cents(),
            discount_cents: self.discount_cents(),
            tax_cents: self.tax_cents(),
            total_cents: self.total_cents(),
            tendered_cents: self.tendered_cents(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    pub fn to_order_lines(&self) -> Vec<OrderLine> {
        self.lines
            .iter()
            .map(|l| OrderLine {
                line_id: l.line_id,
                item_id: l.item_id,
                sku: l.sku.clone(),
                name: l.name.clone(),
                quantity: l.quantity,
                unit_price_cents: l.unit_price_cents,
                line_total_cents: l.line_total_cents,
                discount_cents: l.discount_cents,
                tax_cents: l.tax_cents,
                modifier_option_ids: l.modifier_option_ids.clone(),
                notes: l.notes.clone(),
            })
            .collect()
    }

    /// Build an Order from this cart (must be in Paid state).
    pub fn to_order(&self, order_id: Uuid) -> Result<Order, DomainError> {
        self.ensure_can_finalize()?;
        let payments: Vec<(Uuid, u64, Option<String>)> = self
            .payments
            .iter()
            .map(|p| (p.tender_id, p.amount_cents, p.external_reference.clone()))
            .collect();
        let coupons: Vec<(Uuid, String, u64)> = self
            .applied_coupons
            .iter()
            .map(|c| (c.coupon_id, c.code.clone(), c.discount_cents))
            .collect();
        Ok(Order {
            order_id,
            cart_id: self.id,
            store_id: self.store_id,
            register_id: self.register_id,
            lines: self.to_order_lines(),
            subtotal_cents: self.subtotal_cents(),
            discount_cents: self.discount_cents(),
            tax_cents: self.tax_cents(),
            total_cents: self.total_cents(),
            payments,
            applied_promo_ids: self.applied_promo_ids.clone(),
            applied_coupons: coupons,
            manual_discounts: self.manual_discounts.clone(),
            created_at: self.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cart, CartLineItem};
    use crate::errors::DomainError;
    use apex_edge_contracts::CartStateKind;
    use uuid::Uuid;

    fn make_line(line_id: Uuid) -> CartLineItem {
        CartLineItem {
            line_id,
            item_id: Uuid::new_v4(),
            sku: "SKU-1".into(),
            name: "Test Item".into(),
            quantity: 1,
            modifier_option_ids: vec![],
            notes: None,
            unit_price_cents: 500,
            line_total_cents: 500,
            discount_cents: 0,
            tax_cents: 0,
        }
    }

    #[test]
    fn remove_line_item_removes_the_line() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let line_id = Uuid::new_v4();
        cart.lines.push(make_line(line_id));
        cart.state = CartStateKind::Itemized;

        cart.remove_line_item(line_id).expect("should remove");
        assert!(cart.lines.is_empty());
    }

    #[test]
    fn remove_line_item_transitions_to_open_when_last_line_removed() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let line_id = Uuid::new_v4();
        cart.lines.push(make_line(line_id));
        cart.state = CartStateKind::Itemized;

        cart.remove_line_item(line_id).expect("should remove");
        assert_eq!(cart.state, CartStateKind::Open);
    }

    #[test]
    fn remove_line_item_keeps_itemized_when_other_lines_remain() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let line_a = Uuid::new_v4();
        let line_b = Uuid::new_v4();
        cart.lines.push(make_line(line_a));
        cart.lines.push(make_line(line_b));
        cart.state = CartStateKind::Itemized;

        cart.remove_line_item(line_a).expect("should remove");
        assert_eq!(cart.lines.len(), 1);
        assert_eq!(cart.state, CartStateKind::Itemized);
    }

    #[test]
    fn remove_line_item_errors_on_unknown_line_id() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        cart.lines.push(make_line(Uuid::new_v4()));
        cart.state = CartStateKind::Itemized;

        let err = cart
            .remove_line_item(Uuid::new_v4())
            .expect_err("must reject unknown line");
        assert!(matches!(err, DomainError::LineNotFound(_)));
    }

    #[test]
    fn remove_line_item_rejected_when_cart_not_editable() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let line_id = Uuid::new_v4();
        cart.lines.push(make_line(line_id));
        cart.state = CartStateKind::Tendering;

        let err = cart
            .remove_line_item(line_id)
            .expect_err("must reject in tendering state");
        assert!(matches!(err, DomainError::InvalidTransition(_)));
    }

    #[test]
    fn cannot_finalize_when_not_paid() {
        let cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let err = cart.ensure_can_finalize().expect_err("must reject");
        match err {
            DomainError::InvalidTransition(_) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn add_payment_requires_tendering_or_paid_state() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let err = cart
            .add_payment(Uuid::new_v4(), 100, None)
            .expect_err("must reject payment in open state");
        match err {
            DomainError::InvalidTransition(_) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn payment_transitions_tendering_to_paid() {
        let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        cart.lines.push(CartLineItem {
            line_id: Uuid::new_v4(),
            item_id: Uuid::new_v4(),
            sku: "sku".into(),
            name: "item".into(),
            quantity: 1,
            modifier_option_ids: vec![],
            notes: None,
            unit_price_cents: 100,
            line_total_cents: 100,
            discount_cents: 0,
            tax_cents: 0,
        });
        cart.state = CartStateKind::Tendering;
        cart.add_payment(Uuid::new_v4(), 100, None)
            .expect("payment should succeed");
        assert_eq!(cart.state, CartStateKind::Paid);
    }
}
