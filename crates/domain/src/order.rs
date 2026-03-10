//! Finalized order (for HQ submission).

use apex_edge_contracts::{HqAppliedCoupon, HqOrderLine, HqOrderPayload, HqPayment};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLine {
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub modifier_option_ids: Vec<Uuid>,
    pub notes: Option<String>,
}

impl OrderLine {
    pub fn to_hq_line(&self) -> HqOrderLine {
        HqOrderLine {
            line_id: self.line_id,
            item_id: self.item_id,
            sku: self.sku.clone(),
            name: self.name.clone(),
            quantity: self.quantity,
            unit_price_cents: self.unit_price_cents,
            line_total_cents: self.line_total_cents,
            discount_cents: self.discount_cents,
            tax_cents: self.tax_cents,
            modifier_option_ids: self.modifier_option_ids.clone(),
            notes: self.notes.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: Uuid,
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub lines: Vec<OrderLine>,
    pub subtotal_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub total_cents: u64,
    pub payments: Vec<(Uuid, u64, Option<String>)>,
    pub applied_promo_ids: Vec<Uuid>,
    pub applied_coupons: Vec<(Uuid, String, u64)>,
    pub created_at: DateTime<Utc>,
}

impl Order {
    pub fn to_hq_payload(&self) -> HqOrderPayload {
        HqOrderPayload {
            order_id: self.order_id,
            cart_id: self.cart_id,
            created_at: self.created_at,
            lines: self.lines.iter().map(|l| l.to_hq_line()).collect(),
            subtotal_cents: self.subtotal_cents,
            discount_cents: self.discount_cents,
            tax_cents: self.tax_cents,
            total_cents: self.total_cents,
            payments: self
                .payments
                .iter()
                .map(|(tender_id, amount_cents, ext)| HqPayment {
                    tender_id: *tender_id,
                    amount_cents: *amount_cents,
                    external_reference: ext.clone(),
                })
                .collect(),
            applied_promo_ids: self.applied_promo_ids.clone(),
            applied_coupons: self
                .applied_coupons
                .iter()
                .map(|(id, code, discount)| HqAppliedCoupon {
                    coupon_id: *id,
                    code: code.clone(),
                    discount_cents: *discount,
                })
                .collect(),
            metadata: None,
        }
    }
}
