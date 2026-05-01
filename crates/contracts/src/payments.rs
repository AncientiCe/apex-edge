//! Payment provider contracts shared by POS, domain, and cloud submission paths.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentEntryMethod {
    Cash,
    Manual,
    Swipe,
    Dip,
    Contactless,
    Online,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddPaymentInput {
    pub tender_id: Uuid,
    pub amount_cents: u64,
    pub tip_amount_cents: u64,
    pub external_reference: Option<String>,
    pub provider: Option<String>,
    pub provider_payment_id: Option<String>,
    pub entry_method: Option<PaymentEntryMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentProviderReceipt {
    pub provider: String,
    pub provider_payment_id: String,
    pub entry_method: Option<PaymentEntryMethod>,
    pub last4: Option<String>,
    pub aid: Option<String>,
    pub authorization_code: Option<String>,
}
