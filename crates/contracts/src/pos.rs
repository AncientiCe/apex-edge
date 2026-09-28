//! POS <-> ApexEdge contract: cart commands, checkout, payment events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::payments::PaymentEntryMethod;
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
    // --- v0.9.0 In-store operations ---
    ParkCart(ParkCartPayload),
    RecallCart(RecallCartPayload),
    ListParkedCarts(ListParkedCartsPayload),
    ClockIn(ClockInPayload),
    ClockOut(ClockOutPayload),
    // --- v0.10.0 Stock operations ---
    ReceiveStock(StockMovementPayload),
    TransferStock(StockMovementPayload),
    AdjustStock(StockMovementPayload),
    // --- v1.2.0 Gift cards ---
    IssueGiftCard(IssueGiftCardPayload),
    ActivateGiftCard(ActivateGiftCardPayload),
    ReloadGiftCard(ReloadGiftCardPayload),
    RedeemGiftCard(RedeemGiftCardPayload),
    // --- v1.2.0 Loyalty ---
    EarnLoyaltyPoints(EarnLoyaltyPointsPayload),
    RedeemLoyaltyPoints(RedeemLoyaltyPointsPayload),
    // --- v2.0.0 Direct printing ---
    PrintDocument(PrintDocumentPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkCartPayload {
    pub cart_id: Uuid,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallCartPayload {
    pub parked_cart_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListParkedCartsPayload {
    pub register_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockInPayload {
    pub associate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockOutPayload {
    pub associate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockMovementPayload {
    pub item_id: Uuid,
    pub quantity_delta: i64,
    pub reason: String,
    pub reference: Option<String>,
}

// --- Gift card payloads ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueGiftCardPayload {
    /// If omitted, a code is generated server-side and returned in `GiftCardInfo`.
    pub code: Option<String>,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivateGiftCardPayload {
    pub code: String,
    pub opening_balance_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadGiftCardPayload {
    pub code: String,
    pub amount_cents: u64,
}

/// Redeem a gift card as a tender against an open cart, e.g. at checkout. This both
/// debits the card and records a payment on the cart in one command, the same way a
/// terminal-reported card payment is recorded via `AddPayment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemGiftCardPayload {
    pub cart_id: Uuid,
    pub tender_id: Uuid,
    pub code: String,
    pub amount_cents: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GiftCardStateKind {
    Issued,
    Active,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiftCardInfo {
    pub gift_card_id: Uuid,
    pub code: String,
    pub balance_cents: u64,
    pub currency: String,
    pub state: GiftCardStateKind,
}

// --- Loyalty payloads ---

/// Manually credit points to a customer's loyalty account (e.g. goodwill points, or
/// backfilling a sale not captured by auto-earn). `FinalizeOrder` also auto-earns points
/// for carts with an attached customer, without a separate command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarnLoyaltyPointsPayload {
    pub customer_id: Uuid,
    pub spend_cents: u64,
}

/// Redeem loyalty points as a tender against an open cart, mirroring `RedeemGiftCard`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemLoyaltyPointsPayload {
    pub cart_id: Uuid,
    pub tender_id: Uuid,
    pub customer_id: Uuid,
    pub points: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoyaltyAccountInfo {
    pub customer_id: Uuid,
    pub points: u64,
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
    /// True when `tax_cents` is already inside `unit_price_cents` (VAT-inclusive), so the
    /// refund is the price alone.
    #[serde(default)]
    pub tax_inclusive: bool,
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
    #[serde(default)]
    pub tip_amount_cents: u64,
    pub external_reference: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub provider_payment_id: Option<String>,
    #[serde(default)]
    pub entry_method: Option<PaymentEntryMethod>,
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

/// Print a document the hub has already generated on the hub's own printer.
///
/// The existing contract — the POS fetches a document and prints it itself — is
/// unchanged. This is for hubs with a printer attached, and for reprints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintDocumentPayload {
    pub document_id: Uuid,
    /// Open the cash drawer as well. Used for a manager's till-open, and ignored when
    /// the drawer policy is `never`.
    #[serde(default)]
    pub open_drawer: bool,
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
    /// True when `tax_cents` is contained in the price (VAT-inclusive) rather than added to it.
    #[serde(default)]
    pub tax_inclusive: bool,
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
    /// Set when the sale completed but the hub's printer refused the receipt. The sale
    /// is still final — the money is taken and the order is durable — so this is
    /// reported rather than raised as a command failure, and the POS can reprint from
    /// `print_job_ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_error: Option<String>,
}
