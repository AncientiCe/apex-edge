//! Fiscal provider trait with NoOp and DE-TSE reference implementations.

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiscalReceiptRequest {
    pub order_id: Uuid,
    pub total_cents: u64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiscalReceipt {
    pub provider: String,
    pub fiscal_id: Option<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum FiscalError {
    #[error("fiscal provider {provider} is not configured")]
    NotConfigured { provider: String },
    #[error("fiscal receipt total must be greater than zero")]
    InvalidTotal,
}

pub trait FiscalProvider: Send + Sync {
    fn provider_code(&self) -> &'static str;
    fn sign_receipt(&self, request: FiscalReceiptRequest) -> Result<FiscalReceipt, FiscalError>;
}

#[derive(Debug, Clone, Default)]
pub struct NoOpFiscalProvider;

impl FiscalProvider for NoOpFiscalProvider {
    fn provider_code(&self) -> &'static str {
        "noop"
    }

    fn sign_receipt(&self, request: FiscalReceiptRequest) -> Result<FiscalReceipt, FiscalError> {
        validate_total(request.total_cents)?;
        Ok(FiscalReceipt {
            provider: self.provider_code().into(),
            fiscal_id: None,
            signature: None,
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

impl FiscalProvider for DeTseFiscalProvider {
    fn provider_code(&self) -> &'static str {
        "de_tse"
    }

    fn sign_receipt(&self, request: FiscalReceiptRequest) -> Result<FiscalReceipt, FiscalError> {
        if !self.configured {
            return Err(FiscalError::NotConfigured {
                provider: self.provider_code().into(),
            });
        }
        validate_total(request.total_cents)?;
        Ok(FiscalReceipt {
            provider: self.provider_code().into(),
            fiscal_id: Some(format!("tse_{}", request.order_id)),
            signature: Some(format!("sig_{}_{}", request.order_id, request.total_cents)),
        })
    }
}

fn validate_total(total_cents: u64) -> Result<(), FiscalError> {
    if total_cents == 0 {
        return Err(FiscalError::InvalidTotal);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_fiscal_provider_accepts_positive_receipts() {
        let provider = NoOpFiscalProvider;
        let receipt = provider
            .sign_receipt(FiscalReceiptRequest {
                order_id: Uuid::new_v4(),
                total_cents: 100,
                currency: "USD".into(),
            })
            .expect("sign receipt");

        assert_eq!(receipt.provider, "noop");
        assert!(receipt.signature.is_none());
    }

    #[test]
    fn de_tse_reference_adapter_fails_closed_when_unconfigured() {
        let provider = DeTseFiscalProvider::new(false);
        let err = provider
            .sign_receipt(FiscalReceiptRequest {
                order_id: Uuid::new_v4(),
                total_cents: 100,
                currency: "EUR".into(),
            })
            .expect_err("unconfigured provider");

        assert_eq!(
            err,
            FiscalError::NotConfigured {
                provider: "de_tse".into()
            }
        );
    }
}
