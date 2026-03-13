//! Store, register, tender, and device config (HQ -> ApexEdge sync).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    pub store_id: Uuid,
    pub name: String,
    pub timezone: String,
    pub currency: String,
    pub default_tax_category_id: Uuid,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterConfig {
    pub register_id: Uuid,
    pub store_id: Uuid,
    pub name: String,
    pub tender_ids: Vec<Uuid>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenderType {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub requires_external_auth: bool,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintTemplateConfig {
    pub id: Uuid,
    pub document_type: DocumentType,
    pub template_body: String,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentType {
    CustomerReceipt,
    GiftReceipt,
    MerchantCopy,
    KitchenChit,
    Invoice,
    EndOfDayReport,
}

impl DocumentType {
    /// String used in API and storage (e.g. "customer_receipt", "gift_receipt").
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentType::CustomerReceipt => "customer_receipt",
            DocumentType::GiftReceipt => "gift_receipt",
            DocumentType::MerchantCopy => "merchant_copy",
            DocumentType::KitchenChit => "kitchen_chit",
            DocumentType::Invoice => "invoice",
            DocumentType::EndOfDayReport => "end_of_day_report",
        }
    }
}
