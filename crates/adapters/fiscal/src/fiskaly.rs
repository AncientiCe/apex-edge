//! Fiskaly SIGN adapter: one HTTP client, seven European markets.
//!
//! The free sandbox (`kassensichv.io`) is enough to prove a German TSS signing flow
//! without a certified TSE. Austria and Spain still need their own QR on the paper
//! (RKSV, VeriFactu); those strings are built locally so a receipt is scannable even
//! when the cloud only confirms the signature.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::qr;
use crate::{
    FiscalError, FiscalProvider, FiscalSignature, FiscalTenderType, FiscalTransaction,
    OfflinePolicy,
};

const PROVIDER_CODE: &str = "fiskaly";
const DEFAULT_API_BASE: &str = "https://kassensichv.io/api/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiskalyMarket {
    De,
    At,
    Es,
    It,
    Fr,
    Pt,
    Se,
}

impl FiskalyMarket {
    pub const ALL: &'static [FiskalyMarket] = &[
        Self::De,
        Self::At,
        Self::Es,
        Self::It,
        Self::Fr,
        Self::Pt,
        Self::Se,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::De => "de",
            Self::At => "at",
            Self::Es => "es",
            Self::It => "it",
            Self::Fr => "fr",
            Self::Pt => "pt",
            Self::Se => "se",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "de" | "germany" => Some(Self::De),
            "at" | "austria" => Some(Self::At),
            "es" | "spain" => Some(Self::Es),
            "it" | "italy" => Some(Self::It),
            "fr" | "france" => Some(Self::Fr),
            "pt" | "portugal" => Some(Self::Pt),
            "se" | "sweden" => Some(Self::Se),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FiskalyConfig {
    pub api_key: String,
    pub api_secret: String,
    pub tss_id: String,
    pub client_id: String,
    pub tax_id: String,
    pub market: FiskalyMarket,
    pub api_base_url: String,
}

impl Default for FiskalyConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            api_secret: String::new(),
            tss_id: String::new(),
            client_id: String::new(),
            tax_id: String::new(),
            market: FiskalyMarket::De,
            api_base_url: DEFAULT_API_BASE.into(),
        }
    }
}

pub struct FiskalyProvider {
    config: FiskalyConfig,
    client: Client,
}

impl FiskalyProvider {
    pub fn new(config: FiskalyConfig) -> Result<Self, FiscalError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: e.to_string(),
            })?;
        Ok(Self { config, client })
    }

    fn is_configured(&self) -> bool {
        !self.config.api_key.trim().is_empty()
            && !self.config.api_secret.trim().is_empty()
            && !self.config.tss_id.trim().is_empty()
            && !self.config.client_id.trim().is_empty()
    }

    fn base(&self) -> String {
        self.config.api_base_url.trim_end_matches('/').to_string()
    }

    async fn authenticate(&self) -> Result<String, FiscalError> {
        let url = format!("{}/auth", self.base());
        let response = self
            .client
            .post(&url)
            .json(&json!({
                "api_key": self.config.api_key,
                "api_secret": self.config.api_secret,
            }))
            .send()
            .await
            .map_err(|e| FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: e.to_string(),
            })?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            body.get("access_token")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| FiscalError::Rejected {
                    provider: PROVIDER_CODE.into(),
                    code: "missing_token".into(),
                    detail: "auth response had no access_token".into(),
                })
        } else if status.is_server_error() {
            Err(FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: format!("auth {status}"),
            })
        } else {
            Err(FiscalError::Rejected {
                provider: PROVIDER_CODE.into(),
                code: status.as_u16().to_string(),
                detail: body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("authentication failed")
                    .into(),
            })
        }
    }

    async fn finish_transaction(
        &self,
        token: &str,
        transaction: &FiscalTransaction,
    ) -> Result<Value, FiscalError> {
        let tx_id = transaction.transaction_id;
        let url = format!("{}/tss/{}/tx/{}", self.base(), self.config.tss_id, tx_id);
        let put = self
            .client
            .put(&url)
            .bearer_auth(token)
            .json(&json!({
                "state": "ACTIVE",
                "client_id": self.config.client_id,
            }))
            .send()
            .await
            .map_err(|e| FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: e.to_string(),
            })?;
        if put.status().is_server_error() {
            return Err(FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: format!("open tx {}", put.status()),
            });
        }
        if !put.status().is_success() {
            return Err(FiscalError::Rejected {
                provider: PROVIDER_CODE.into(),
                code: put.status().as_u16().to_string(),
                detail: "could not open fiskaly transaction".into(),
            });
        }

        let receipt_type = match transaction.kind {
            crate::FiscalTransactionKind::Sale => "RECEIPT",
            crate::FiscalTransactionKind::Refund => "CANCELLATION",
            crate::FiscalTransactionKind::Void => "CANCELLATION",
        };
        let body = json!({
            "state": "FINISHED",
            "client_id": self.config.client_id,
            "schema": {
                "standard_v1": {
                    "receipt": {
                        "receipt_type": receipt_type,
                        "amounts_per_vat_rate": vat_rates(transaction),
                        "amounts_per_payment_type": payment_types(transaction),
                    }
                }
            }
        });
        let patch = self
            .client
            .patch(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: e.to_string(),
            })?;
        let status = patch.status();
        let payload: Value = patch.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(payload)
        } else if status.is_server_error() {
            Err(FiscalError::Unavailable {
                provider: PROVIDER_CODE.into(),
                detail: payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("tss timeout")
                    .into(),
            })
        } else {
            Err(FiscalError::Rejected {
                provider: PROVIDER_CODE.into(),
                code: status.as_u16().to_string(),
                detail: payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("fiskaly rejected the receipt")
                    .into(),
            })
        }
    }
}

#[async_trait]
impl FiscalProvider for FiskalyProvider {
    fn provider_code(&self) -> &'static str {
        PROVIDER_CODE
    }

    fn offline_policy(&self) -> OfflinePolicy {
        OfflinePolicy::SignLater
    }

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError> {
        transaction.validate()?;
        if !self.is_configured() {
            return Err(FiscalError::NotConfigured {
                provider: PROVIDER_CODE.into(),
            });
        }
        let token = self.authenticate().await?;
        let remote = self.finish_transaction(&token, transaction).await?;
        let fiscal_id = remote.get("number").and_then(|v| {
            v.as_i64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(ToOwned::to_owned))
        });
        let signature = remote
            .pointer("/log/signature/value")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let remote_qr = remote
            .get("qr_code_data")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let qr_payload = match self.config.market {
            FiskalyMarket::At => Some(qr::rksv_payload(transaction, &self.config.tax_id)),
            FiskalyMarket::Es => Some(qr::verifactu_payload(transaction, &self.config.tax_id)),
            _ => remote_qr.or_else(|| {
                Some(format!(
                    "V0;{};{};{}",
                    self.config.market.as_str(),
                    transaction.transaction_id,
                    fiscal_id.clone().unwrap_or_default()
                ))
            }),
        };
        Ok(FiscalSignature {
            provider: PROVIDER_CODE.into(),
            fiscal_id,
            signature,
            qr_payload,
            signed_at: chrono::Utc::now(),
        })
    }
}

fn vat_rates(transaction: &FiscalTransaction) -> Vec<Value> {
    transaction
        .tax_breakdown()
        .into_iter()
        .map(|entry| {
            json!({
                "vat_rate": vat_rate_name(entry.rate_bps),
                "amount": cents_to_fiskaly(entry.gross_cents),
            })
        })
        .collect()
}

fn payment_types(transaction: &FiscalTransaction) -> Vec<Value> {
    transaction
        .tenders
        .iter()
        .map(|tender| {
            json!({
                "payment_type": match tender.tender_type {
                    FiscalTenderType::Cash => "CASH",
                    _ => "NON_CASH",
                },
                "amount": cents_to_fiskaly(tender.amount_cents),
            })
        })
        .collect()
}

fn vat_rate_name(rate_bps: u32) -> &'static str {
    match rate_bps {
        0 => "NULL",
        700 | 1000 | 1300 => "REDUCED_1",
        500 => "REDUCED_2",
        _ => "NORMAL",
    }
}

fn cents_to_fiskaly(cents: i64) -> String {
    format!("{:.2}", cents as f64 / 100.0)
}
