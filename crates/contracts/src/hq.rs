//! ApexEdge -> HQ contract: order submission payload (perfect-world).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::version::ContractVersion;
use crate::PaymentEntryMethod;

/// Build submission envelope with checksum (deterministic for idempotency).
pub fn build_submission_envelope(
    submission_id: Uuid,
    store_id: Uuid,
    register_id: Uuid,
    sequence_number: u64,
    order: HqOrderPayload,
) -> HqOrderSubmissionEnvelope {
    let submitted_at = Utc::now();
    let payload_json = serde_json::to_string(&order).unwrap_or_default();
    let checksum = format!(
        "{:x}",
        md5::compute(format!(
            "{}:{}:{}:{}:{}",
            submission_id, store_id, register_id, sequence_number, payload_json
        ))
    );
    HqOrderSubmissionEnvelope {
        version: ContractVersion::V1_0_0,
        submission_id,
        store_id,
        register_id,
        sequence_number,
        order,
        checksum,
        submitted_at,
    }
}

/// Order submission envelope to HQ (idempotent; same key => same result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqOrderSubmissionEnvelope {
    pub version: ContractVersion,
    pub submission_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub sequence_number: u64,
    pub order: HqOrderPayload,
    pub checksum: String,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqOrderPayload {
    pub order_id: Uuid,
    pub cart_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub lines: Vec<HqOrderLine>,
    pub subtotal_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub total_cents: u64,
    pub payments: Vec<HqPayment>,
    pub applied_promo_ids: Vec<Uuid>,
    pub applied_coupons: Vec<HqAppliedCoupon>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqOrderLine {
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
pub struct HqPayment {
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
pub struct HqAppliedCoupon {
    pub coupon_id: Uuid,
    pub code: String,
    pub discount_cents: u64,
}

/// HQ ingest response (contractual).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqOrderSubmissionResponse {
    pub accepted: bool,
    pub submission_id: Uuid,
    pub order_id: Uuid,
    pub hq_order_ref: Option<String>,
    pub errors: Vec<HqError>,
}

impl Default for HqOrderSubmissionResponse {
    fn default() -> Self {
        Self {
            accepted: false,
            submission_id: Uuid::nil(),
            order_id: Uuid::nil(),
            hq_order_ref: None,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqError {
    pub code: String,
    pub message: String,
}

// ---------- Returns submission ----------

pub fn build_return_submission_envelope(
    submission_id: Uuid,
    store_id: Uuid,
    register_id: Uuid,
    sequence_number: u64,
    ret: HqReturnPayload,
) -> HqReturnSubmissionEnvelope {
    let submitted_at = Utc::now();
    let payload_json = serde_json::to_string(&ret).unwrap_or_default();
    let checksum = format!(
        "{:x}",
        md5::compute(format!(
            "{}:{}:{}:{}:{}",
            submission_id, store_id, register_id, sequence_number, payload_json
        ))
    );
    HqReturnSubmissionEnvelope {
        version: ContractVersion::V1_0_0,
        submission_id,
        store_id,
        register_id,
        sequence_number,
        ret,
        checksum,
        submitted_at,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqReturnSubmissionEnvelope {
    pub version: ContractVersion,
    pub submission_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub sequence_number: u64,
    pub ret: HqReturnPayload,
    pub checksum: String,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqReturnPayload {
    pub return_id: Uuid,
    pub original_order_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub approval_id: Option<Uuid>,
    pub shift_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub lines: Vec<HqReturnLine>,
    pub refunds: Vec<HqRefund>,
    pub total_cents: u64,
    pub tax_cents: u64,
    pub refunded_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqReturnLine {
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
pub struct HqRefund {
    pub refund_id: Uuid,
    pub tender_type: String,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

// ---------- Shift submission (Z report) ----------

pub fn build_shift_submission_envelope(
    submission_id: Uuid,
    store_id: Uuid,
    register_id: Uuid,
    sequence_number: u64,
    shift: HqShiftPayload,
) -> HqShiftSubmissionEnvelope {
    let submitted_at = Utc::now();
    let payload_json = serde_json::to_string(&shift).unwrap_or_default();
    let checksum = format!(
        "{:x}",
        md5::compute(format!(
            "{}:{}:{}:{}:{}",
            submission_id, store_id, register_id, sequence_number, payload_json
        ))
    );
    HqShiftSubmissionEnvelope {
        version: ContractVersion::V1_0_0,
        submission_id,
        store_id,
        register_id,
        sequence_number,
        shift,
        checksum,
        submitted_at,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqShiftSubmissionEnvelope {
    pub version: ContractVersion,
    pub submission_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub sequence_number: u64,
    pub shift: HqShiftPayload,
    pub checksum: String,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqShiftPayload {
    pub shift_id: Uuid,
    pub associate_id: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub opening_float_cents: u64,
    pub expected_cents: i64,
    pub counted_cents: i64,
    pub variance_cents: i64,
    pub cash_sales_cents: u64,
    pub cash_refunds_cents: u64,
    pub movements: Vec<HqShiftMovement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HqShiftMovement {
    pub id: Uuid,
    pub kind: String,
    pub amount_cents: u64,
    pub reason: Option<String>,
}
