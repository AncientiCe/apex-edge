//! Tax provider adapter trait and reference implementations.

use apex_edge_contracts::{TaxBreakdown, TaxQuote, TaxQuoteLine, TaxRule};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxQuoteRequest {
    pub currency: String,
    pub lines: Vec<TaxQuoteLine>,
    pub destination: Option<TaxDestination>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxDestination {
    pub country: String,
    pub region: Option<String>,
    pub postal_code: Option<String>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum TaxProviderError {
    #[error("tax quote requires at least one line")]
    EmptyQuote,
    #[error("tax provider {provider} is not configured")]
    NotConfigured { provider: String },
}

pub trait TaxProvider: Send + Sync {
    fn provider_code(&self) -> &'static str;

    fn quote(&self, request: TaxQuoteRequest) -> Result<TaxQuote, TaxProviderError>;
}

#[derive(Debug, Clone)]
pub struct InternalTaxProvider {
    rules: Vec<TaxRule>,
}

impl InternalTaxProvider {
    pub fn new(rules: Vec<TaxRule>) -> Self {
        Self { rules }
    }
}

impl TaxProvider for InternalTaxProvider {
    fn provider_code(&self) -> &'static str {
        "internal"
    }

    fn quote(&self, request: TaxQuoteRequest) -> Result<TaxQuote, TaxProviderError> {
        if request.lines.is_empty() {
            return Err(TaxProviderError::EmptyQuote);
        }

        let mut breakdown = Vec::new();
        for line in &request.lines {
            for rule in self
                .rules
                .iter()
                .filter(|rule| rule.tax_category_id == line.tax_category_id)
            {
                breakdown.push(TaxBreakdown {
                    line_id: line.line_id,
                    jurisdiction: rule.name.clone(),
                    rate_bps: rule.rate_bps,
                    amount_cents: apply_tax(
                        line.taxable_amount_cents,
                        rule.rate_bps,
                        rule.inclusive,
                    ),
                    inclusive: rule.inclusive,
                });
            }
        }

        Ok(TaxQuote {
            provider: self.provider_code().into(),
            currency: request.currency,
            total_tax_cents: breakdown.iter().map(|line| line.amount_cents).sum(),
            breakdown,
        })
    }
}

#[derive(Debug, Clone)]
pub struct HostedTaxProvider {
    provider_code: &'static str,
    configured: bool,
}

impl HostedTaxProvider {
    pub fn avalara(configured: bool) -> Self {
        Self {
            provider_code: "avalara",
            configured,
        }
    }

    pub fn stripe_tax(configured: bool) -> Self {
        Self {
            provider_code: "stripe_tax",
            configured,
        }
    }
}

impl TaxProvider for HostedTaxProvider {
    fn provider_code(&self) -> &'static str {
        self.provider_code
    }

    fn quote(&self, request: TaxQuoteRequest) -> Result<TaxQuote, TaxProviderError> {
        if request.lines.is_empty() {
            return Err(TaxProviderError::EmptyQuote);
        }
        if !self.configured {
            return Err(TaxProviderError::NotConfigured {
                provider: self.provider_code.into(),
            });
        }

        Ok(TaxQuote {
            provider: self.provider_code.into(),
            currency: request.currency,
            total_tax_cents: 0,
            breakdown: Vec::new(),
        })
    }
}

fn apply_tax(amount_cents: u64, rate_bps: u32, inclusive: bool) -> u64 {
    if inclusive {
        amount_cents - (amount_cents * 10000 / (10000 + rate_bps as u64))
    } else {
        amount_cents * rate_bps as u64 / 10000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn line(line_id: Uuid, tax_category_id: Uuid, amount_cents: u64) -> TaxQuoteLine {
        TaxQuoteLine {
            line_id,
            tax_category_id,
            taxable_amount_cents: amount_cents,
        }
    }

    #[test]
    fn internal_provider_stacks_us_destination_style_rates() {
        let tax_category_id = Uuid::new_v4();
        let line_id = Uuid::new_v4();
        let provider = InternalTaxProvider::new(vec![
            TaxRule {
                id: Uuid::new_v4(),
                tax_category_id,
                rate_bps: 625,
                name: "state".into(),
                inclusive: false,
                version: 1,
            },
            TaxRule {
                id: Uuid::new_v4(),
                tax_category_id,
                rate_bps: 125,
                name: "county".into(),
                inclusive: false,
                version: 1,
            },
        ]);

        let quote = provider
            .quote(TaxQuoteRequest {
                currency: "USD".into(),
                lines: vec![line(line_id, tax_category_id, 10_000)],
                destination: None,
            })
            .expect("tax quote");

        assert_eq!(quote.total_tax_cents, 750);
        assert_eq!(quote.breakdown.len(), 2);
    }

    #[test]
    fn internal_provider_handles_eu_inclusive_vat() {
        let tax_category_id = Uuid::new_v4();
        let line_id = Uuid::new_v4();
        let provider = InternalTaxProvider::new(vec![TaxRule {
            id: Uuid::new_v4(),
            tax_category_id,
            rate_bps: 2000,
            name: "vat_de".into(),
            inclusive: true,
            version: 1,
        }]);

        let quote = provider
            .quote(TaxQuoteRequest {
                currency: "EUR".into(),
                lines: vec![line(line_id, tax_category_id, 12_000)],
                destination: None,
            })
            .expect("tax quote");

        assert_eq!(quote.total_tax_cents, 2_000);
        assert!(quote.breakdown[0].inclusive);
    }

    #[test]
    fn hosted_tax_provider_fails_closed_when_not_configured() {
        let provider = HostedTaxProvider::avalara(false);
        let err = provider
            .quote(TaxQuoteRequest {
                currency: "USD".into(),
                lines: vec![line(Uuid::new_v4(), Uuid::new_v4(), 100)],
                destination: None,
            })
            .expect_err("unconfigured tax provider must fail");

        assert_eq!(
            err,
            TaxProviderError::NotConfigured {
                provider: "avalara".into()
            }
        );
    }
}
