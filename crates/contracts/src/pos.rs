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
    ApplyManualDiscount(ApplyManualDiscountPayload),
    SetTendering(SetTenderingPayload),
    AddPayment(AddPaymentPayload),
    FinalizeOrder(FinalizeOrderPayload),
    VoidCart(VoidCartPayload),
    // --- v0.6.0 Returns & Refunds ---
    StartReturn(StartReturnPayload),
    ReturnLineItem(ReturnLineItemPayload),
    RefundTender(RefundTenderPayload),
    FinalizeReturn(FinalizeReturnPayload),
    VoidReturn(VoidReturnPayload),
    // --- v0.6.0 Till & Shift ---
    OpenTill(OpenTillPayload),
    PaidIn(PaidInPayload),
    PaidOut(PaidOutPayload),
    NoSale(NoSalePayload),
    CashCount(CashCountPayload),
    GetXReport(GetXReportPayload),
    CloseTill(CloseTillPayload),
}

// --- Returns & Refunds payloads ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartReturnPayload {
    pub return_id: Option<Uuid>,
    pub original_order_id: Option<Uuid>,
    pub reason_code: Option<String>,
    /// If the return is blind (no original_order_id), a prior approval id gated on a
    /// supervisor grant must be provided.
    pub approval_id: Option<Uuid>,
    pub shift_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnLineItemPayload {
    pub return_id: Uuid,
    pub sku: String,
    pub name: Option<String>,
    pub quantity: u32,
    /// Unit price in cents to credit back to the customer; for receipted returns this is
    /// typically the original line's unit price.
    pub unit_price_cents: u64,
    pub tax_cents: u64,
    /// Optional link to the original order line (receipted returns).
    pub original_line_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundTenderPayload {
    pub return_id: Uuid,
    pub tender_type: String,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeReturnPayload {
    pub return_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoidReturnPayload {
    pub return_id: Uuid,
    pub reason: Option<String>,
}

// --- Till & Shift payloads ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenTillPayload {
    pub register_id: Option<Uuid>,
    pub associate_id: Option<String>,
    pub opening_float_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaidInPayload {
    pub shift_id: Uuid,
    pub amount_cents: u64,
    pub reason: String,
    pub approval_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaidOutPayload {
    pub shift_id: Uuid,
    pub amount_cents: u64,
    pub reason: String,
    pub approval_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoSalePayload {
    pub shift_id: Uuid,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CashCountPayload {
    pub shift_id: Uuid,
    pub counted_cents: u64,
    pub denominations: std::collections::BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetXReportPayload {
    pub shift_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseTillPayload {
    pub shift_id: Uuid,
    pub counted_cents: u64,
    pub approval_id: Option<Uuid>,
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
    /// If set and > 0, overrides catalog price for this line (cents per unit).
    pub unit_price_override_cents: Option<u64>,
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

/// Manual discount: requires a reason (mandatory). Applied after promos.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyManualDiscountPayload {
    pub cart_id: Uuid,
    /// Mandatory reason for audit.
    pub reason: String,
    pub kind: ManualDiscountKind,
    /// For PercentCart/PercentItem: basis points (100 = 1%). For FixedCart/FixedItem: amount in cents.
    pub value: u64,
    /// Required for PercentItem and FixedItem; ignored for cart-level.
    pub line_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualDiscountKind {
    PercentCart,
    PercentItem,
    FixedCart,
    FixedItem,
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
    pub customer_name: Option<String>,
    pub customer_code: Option<String>,
    pub state: CartStateKind,
    pub lines: Vec<CartLine>,
    pub applied_promos: Vec<AppliedPromoInfo>,
    pub applied_coupons: Vec<AppliedCouponInfo>,
    /// Manual discounts (reason required); included in discount_cents.
    pub manual_discounts: Vec<ManualDiscountInfo>,
    pub subtotal_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub total_cents: u64,
    pub tendered_cents: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedPromoInfo {
    pub promo_id: Uuid,
    pub name: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualDiscountInfo {
    pub reason: String,
    pub amount_cents: u64,
    pub line_id: Option<Uuid>,
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
