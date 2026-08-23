//! Fiscal provider contract: a full transaction, not just a total.
//!
//! A tax authority cares about what was sold, at which rate, how it was paid, on which
//! cash point, and when. [`FiscalProvider::sign`] takes that whole picture. When the
//! signer is briefly unreachable, [`OfflinePolicy::SignLater`] lets the till keep selling
//! and queue the transaction for a later attempt.

pub mod export;
mod fiskaly;
pub mod qr;

pub use export::{cii_factur_x, dsfinvk_files, ubl_xrechnung, CashPointClosing, En16931Profile};
pub use fiskaly::{FiskalyConfig, FiskalyMarket, FiskalyProvider};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiscalTransactionKind {
    Sale,
    Refund,
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiscalTenderType {
    Cash,
    Card,
    GiftCard,
    Voucher,
    Other,
}

/// What the till should do when the signer cannot be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflinePolicy {
    /// Refuse the sale. Use this when the jurisdiction forbids unsigned receipts.
    FailClosed,
    /// Persist the sale and retry signing in the background.
    SignLater,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashPoint {
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalLine {
    pub line_id: Uuid,
    pub sku: String,
    pub description: String,
    pub quantity: u32,
    pub unit_price_cents: i64,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub gross_cents: i64,
    pub discount_cents: i64,
    pub tax_rate_bps: u32,
    pub tax_category: String,
    pub tax_inclusive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalTender {
    pub tender_id: Uuid,
    pub tender_type: FiscalTenderType,
    pub amount_cents: i64,
    pub tip_cents: i64,
    pub provider: Option<String>,
    pub provider_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalTransaction {
    pub transaction_id: Uuid,
    pub kind: FiscalTransactionKind,
    pub reference_transaction_id: Option<Uuid>,
    pub cash_point: CashPoint,
    pub occurred_at: DateTime<Utc>,
    pub currency: String,
    pub lines: Vec<FiscalLine>,
    pub tenders: Vec<FiscalTender>,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub gross_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxBreakdownEntry {
    pub rate_bps: u32,
    pub tax_category: String,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub gross_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalSignature {
    pub provider: String,
    pub fiscal_id: Option<String>,
    pub signature: Option<String>,
    pub qr_payload: Option<String>,
    pub signed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum FiscalError {
    #[error("fiscal provider {provider} is not configured")]
    NotConfigured { provider: String },
    #[error("fiscal transaction is invalid: {reason}")]
    InvalidTransaction { reason: String },
    #[error("fiscal provider {provider} is unavailable: {detail}")]
    Unavailable { provider: String, detail: String },
    #[error("fiscal provider {provider} rejected the transaction ({code}): {detail}")]
    Rejected {
        provider: String,
        code: String,
        detail: String,
    },
}

impl FiscalError {
    /// Only an outage is worth retrying. Misconfiguration and a rejected payload will
    /// fail the same way on the next attempt.
    pub fn is_transient(&self) -> bool {
        matches!(self, FiscalError::Unavailable { .. })
    }
}

#[async_trait]
pub trait FiscalProvider: Send + Sync {
    fn provider_code(&self) -> &'static str;

    fn offline_policy(&self) -> OfflinePolicy;

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError>;
}

impl FiscalTransaction {
    pub fn validate(&self) -> Result<(), FiscalError> {
        if self.lines.is_empty() {
            return Err(invalid("a fiscal transaction must have at least one line"));
        }
        if !is_iso_currency(&self.currency) {
            return Err(invalid("currency must be a three-letter ISO 4217 code"));
        }

        let mut net = 0i64;
        let mut tax = 0i64;
        let mut gross = 0i64;
        for line in &self.lines {
            if line.quantity == 0 {
                return Err(invalid("a sold line always has a quantity"));
            }
            if line.tax_rate_bps > 10_000 {
                return Err(invalid("tax rate cannot exceed 100 percent"));
            }
            if line.gross_cents != line.net_cents.saturating_add(line.tax_cents) {
                return Err(invalid("line gross must equal net plus tax"));
            }
            net = net.saturating_add(line.net_cents);
            tax = tax.saturating_add(line.tax_cents);
            gross = gross.saturating_add(line.gross_cents);
        }
        if self.net_cents != net || self.tax_cents != tax || self.gross_cents != gross {
            return Err(invalid("header totals must equal the sum of the lines"));
        }

        let tendered: i64 = self.tenders.iter().map(|t| t.amount_cents).sum();
        if tendered != self.gross_cents {
            return Err(invalid(
                "the tender breakdown must account for the whole transaction",
            ));
        }

        match self.kind {
            FiscalTransactionKind::Sale if self.gross_cents <= 0 => {
                return Err(invalid("a sale must have a positive gross total"));
            }
            FiscalTransactionKind::Refund if self.gross_cents >= 0 => {
                return Err(invalid("a refund must have a negative gross total"));
            }
            FiscalTransactionKind::Void if self.reference_transaction_id.is_none() => {
                return Err(invalid("a void must reference the original transaction"));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn tax_breakdown(&self) -> Vec<TaxBreakdownEntry> {
        let mut grouped: Vec<TaxBreakdownEntry> = Vec::new();
        for line in &self.lines {
            if let Some(existing) = grouped.iter_mut().find(|e| e.rate_bps == line.tax_rate_bps) {
                existing.net_cents += line.net_cents;
                existing.tax_cents += line.tax_cents;
                existing.gross_cents += line.gross_cents;
            } else {
                grouped.push(TaxBreakdownEntry {
                    rate_bps: line.tax_rate_bps,
                    tax_category: line.tax_category.clone(),
                    net_cents: line.net_cents,
                    tax_cents: line.tax_cents,
                    gross_cents: line.gross_cents,
                });
            }
        }
        grouped.sort_by_key(|e| e.rate_bps);
        grouped
    }
}

fn invalid(reason: &str) -> FiscalError {
    FiscalError::InvalidTransaction {
        reason: reason.into(),
    }
}

fn is_iso_currency(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_alphabetic())
}

#[derive(Debug, Clone, Default)]
pub struct NoOpFiscalProvider;

#[async_trait]
impl FiscalProvider for NoOpFiscalProvider {
    fn provider_code(&self) -> &'static str {
        "noop"
    }

    fn offline_policy(&self) -> OfflinePolicy {
        OfflinePolicy::SignLater
    }

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError> {
        transaction.validate()?;
        Ok(FiscalSignature {
            provider: self.provider_code().into(),
            fiscal_id: None,
            signature: None,
            qr_payload: None,
            signed_at: Utc::now(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct DeTseFiscalProvider {
    configured: bool,
}

impl DeTseFiscalProvider {
    pub fn new(configured: bool) -> Self {
        Self { configured }
    }
}

#[async_trait]
impl FiscalProvider for DeTseFiscalProvider {
    fn provider_code(&self) -> &'static str {
        "de_tse"
    }

    fn offline_policy(&self) -> OfflinePolicy {
        OfflinePolicy::SignLater
    }

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError> {
        if !self.configured {
            return Err(FiscalError::NotConfigured {
                provider: self.provider_code().into(),
            });
        }
        transaction.validate()?;
        let fiscal_id = format!("tse_{}", transaction.transaction_id);
        let signature = format!(
            "sig_{}_{}",
            transaction.transaction_id, transaction.gross_cents
        );
        let qr_payload = format!(
            "V0;{};{};{};{}",
            transaction.transaction_id,
            transaction.gross_cents,
            signature,
            transaction.occurred_at.to_rfc3339()
        );
        Ok(FiscalSignature {
            provider: self.provider_code().into(),
            fiscal_id: Some(fiscal_id),
            signature: Some(signature),
            qr_payload: Some(qr_payload),
            signed_at: Utc::now(),
        })
    }
}
