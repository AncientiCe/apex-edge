//! POS <-> ApexEdge contract: cart commands, checkout, payment events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::version::ContractVersion;

/// All POS requests carry version and idempotency key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosRequestEnvelope<T> {
    pub version: ContractVersion,
    pub idempotency_key: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub payload: T,
}

impl<T> PosRequestEnvelope<T> {
    pub fn current() -> ContractVersion {
        ContractVersion::V1_0_0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PosCommand {
    CreateCart(CreateCartPayload),
    SetCustomer(SetCustomerPayload),
    AddLineItem(AddLineItemPayload),
    UpdateLineItem(UpdateLineItemPayload),
    RemoveLineItem(RemoveLineItemPayload),
    ApplyPromo(ApplyPromoPayload),
    RemovePromo(RemovePromoPayload),
    ApplyCoupon(ApplyCouponPayload),
    RemoveCoupon(RemoveCouponPayload),
    SetTendering(SetTenderingPayload),
    AddPayment(AddPaymentPayload),
    FinalizeOrder(FinalizeOrderPayload),
    VoidCart(VoidCartPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCartPayload {
    pub cart_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetCustomerPayload {
    pub cart_id: Uuid,
    pub customer_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddLineItemPayload {
    pub cart_id: Uuid,
    pub item_id: Uuid,
    pub modifier_option_ids: Vec<Uuid>,
    pub quantity: u32,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateLineItemPayload {
    pub cart_id: Uuid,
    pub line_id: Uuid,
    pub quantity: u32,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveLineItemPayload {
    pub cart_id: Uuid,
    pub line_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPromoPayload {
    pub cart_id: Uuid,
    pub promo_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovePromoPayload {
    pub cart_id: Uuid,
    pub promo_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyCouponPayload {
    pub cart_id: Uuid,
    pub coupon_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveCouponPayload {
    pub cart_id: Uuid,
    pub coupon_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetTenderingPayload {
    pub cart_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddPaymentPayload {
    pub cart_id: Uuid,
    pub tender_id: Uuid,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeOrderPayload {
    pub cart_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoidCartPayload {
    pub cart_id: Uuid,
    pub reason: Option<String>,
}

/// Response envelope for POS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosResponseEnvelope<T> {
    pub version: ContractVersion,
    pub success: bool,
    pub idempotency_key: Uuid,
    pub payload: Option<T>,
    pub errors: Vec<PosError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosError {
    pub code: String,
    pub message: String,
    pub field: Option<String>,
}

/// Cart state returned to POS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CartState {
    pub cart_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub state: CartStateKind,
    pub lines: Vec<CartLine>,
    pub applied_promos: Vec<Uuid>,
    pub applied_coupons: Vec<AppliedCouponInfo>,
    pub subtotal_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub total_cents: u64,
    pub tendered_cents: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CartStateKind {
    Open,
    Itemized,
    Discounted,
    Tendering,
    Paid,
    Finalized,
    Voided,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CartLine {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedCouponInfo {
    pub coupon_id: Uuid,
    pub code: String,
    pub discount_cents: u64,
}

/// Result of finalize: order id and print job ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeResult {
    pub order_id: Uuid,
    pub cart_id: Uuid,
    pub total_cents: u64,
    pub print_job_ids: Vec<Uuid>,
}
