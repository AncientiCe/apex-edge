//! Payment provider selection and the reversal sweeper.
//!
//! `apex-edge-adapters-payment` owns the provider implementations; this module decides
//! which one a POS command should use and owns the recovery path for money that was
//! taken for a sale that never completed.

use std::collections::BTreeMap;
use std::sync::Arc;

use apex_edge_adapters_payment::{
    CashPaymentProvider, PaymentProvider, SimulatedTerminalProvider, StripeTerminalConfig,
    StripeTerminalProvider, VoidRequest,
};
use apex_edge_storage::{
    fetch_payment_intents_awaiting_reversal, flag_stale_captured_payment_intents,
    mark_payment_intent_reversed, record_payment_intent_reversal_failure,
};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Provider code used when the POS records a tender the hub did not process itself
/// (cash counted into the drawer, a gift card balance, an externally captured card).
pub const MANUAL_PROVIDER: &str = "manual";

#[derive(Clone)]
pub struct PaymentSettings {
    providers: BTreeMap<String, Arc<dyn PaymentProvider>>,
    pub currency: String,
}

impl std::fmt::Debug for PaymentSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentSettings")
            .field("providers", &self.provider_codes())
            .field("currency", &self.currency)
            .finish()
    }
}

impl Default for PaymentSettings {
    /// Cash only. Card providers are opt-in so a misconfigured hub cannot quietly
    /// record card tenders it never actually took.
    fn default() -> Self {
        Self::new("USD")
    }
}

impl PaymentSettings {
    pub fn new(currency: impl Into<String>) -> Self {
        let mut settings = Self {
            providers: BTreeMap::new(),
            currency: currency.into(),
        };
        settings.register(Arc::new(CashPaymentProvider));
        settings
    }

    pub fn register(&mut self, provider: Arc<dyn PaymentProvider>) {
        self.providers
            .insert(provider.provider_code().to_string(), provider);
    }

    pub fn with_provider(mut self, provider: Arc<dyn PaymentProvider>) -> Self {
        self.register(provider);
        self
    }

    /// Resolve a provider code from a POS command. `None` means the POS is reporting a
    /// tender it settled itself, which is recorded without contacting any provider.
    pub fn resolve(&self, code: Option<&str>) -> ProviderChoice {
        match code {
            None => ProviderChoice::ManualTender,
            Some(code) if code == MANUAL_PROVIDER => ProviderChoice::ManualTender,
            Some(code) => match self.providers.get(code) {
                Some(provider) => ProviderChoice::Provider(provider.clone()),
                None => ProviderChoice::Unknown(code.to_string()),
            },
        }
    }

    pub fn provider_codes(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// Build from environment.
    ///
    /// - `APEX_EDGE_PAYMENT_SIMULATOR=1` registers the deterministic simulated terminal,
    ///   which is how the flow is exercised without hardware.
    /// - `APEX_EDGE_STRIPE_SECRET_KEY` + `APEX_EDGE_STRIPE_READER_ID` register Stripe
    ///   Terminal. Point the reader at a `simulated-wpe` reader in a sandbox to test it.
    pub fn from_env(currency: impl Into<String>) -> Self {
        let mut settings = Self::new(currency);

        if std::env::var("APEX_EDGE_PAYMENT_SIMULATOR")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            settings.register(Arc::new(SimulatedTerminalProvider::new()));
        }

        let stripe_key = std::env::var("APEX_EDGE_STRIPE_SECRET_KEY").unwrap_or_default();
        let stripe_reader = std::env::var("APEX_EDGE_STRIPE_READER_ID").unwrap_or_default();
        if !stripe_key.trim().is_empty() && !stripe_reader.trim().is_empty() {
            let config = StripeTerminalConfig {
                secret_key: stripe_key,
                reader_id: stripe_reader,
                api_base_url: std::env::var("APEX_EDGE_STRIPE_API_BASE_URL")
                    .unwrap_or_else(|_| "https://api.stripe.com".into()),
                ..StripeTerminalConfig::default()
            };
            match StripeTerminalProvider::new(config) {
                Ok(provider) => settings.register(Arc::new(provider)),
                Err(e) => tracing::error!(error = %e, "stripe terminal provider not registered"),
            }
        }

        settings
    }
}

pub enum ProviderChoice {
    /// The POS settled this tender itself; record it locally.
    ManualTender,
    Provider(Arc<dyn PaymentProvider>),
    /// A provider was named but is not configured on this hub.
    Unknown(String),
}

/// How a reversal sweep decides what to look at.
#[derive(Debug, Clone, Copy)]
pub struct ReversalSweepPolicy {
    /// Maximum intents handled per sweep.
    pub batch: i64,
    /// How long a captured payment may sit without a completed sale before it counts as
    /// orphaned. Long enough that a slow but healthy checkout is never reversed under the
    /// operator's hands.
    pub stale_capture_after: std::time::Duration,
}

impl Default for ReversalSweepPolicy {
    fn default() -> Self {
        Self {
            batch: 25,
            stale_capture_after: std::time::Duration::from_secs(300),
        }
    }
}

impl ReversalSweepPolicy {
    pub fn from_env() -> Self {
        let default = Self::default();
        Self {
            stale_capture_after: std::env::var("APEX_EDGE_PAYMENT_STALE_CAPTURE_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .map(std::time::Duration::from_secs)
                .unwrap_or(default.stale_capture_after),
            ..default
        }
    }
}

/// Void money that was captured for a sale which never completed.
///
/// This is the safety net behind the whole payment path: anything that captures funds and
/// then fails to produce a durable order leaves a `reversal_pending` intent, and this
/// sweep gives it back. A failed sweep leaves the row queued rather than dropping it,
/// because unreversed money owed to a customer must never be forgotten.
pub async fn run_payment_reversal_sweep(
    pool: &SqlitePool,
    payments: &PaymentSettings,
    policy: ReversalSweepPolicy,
) -> Result<usize, apex_edge_storage::pool::PoolError> {
    // Pick up captures orphaned by a crash, where no failure handler ever ran.
    let orphaned =
        flag_stale_captured_payment_intents(pool, policy.stale_capture_after.as_secs() as i64)
            .await?;
    if orphaned > 0 {
        tracing::warn!(
            orphaned,
            "found captured payments with no completed sale; queuing them for reversal"
        );
    }

    let owed = fetch_payment_intents_awaiting_reversal(pool, policy.batch).await?;
    let mut reversed = 0usize;

    for intent in owed {
        let Some(provider_payment_id) = intent.provider_payment_id.clone() else {
            // Nothing was ever captured against a provider, so there is nothing to give
            // back. Close the row out rather than retrying forever.
            mark_payment_intent_reversed(pool, intent.id).await?;
            continue;
        };

        let provider = match payments.resolve(Some(&intent.provider)) {
            ProviderChoice::Provider(provider) => provider,
            _ => {
                record_payment_intent_reversal_failure(
                    pool,
                    intent.id,
                    &format!("provider {} is not configured on this hub", intent.provider),
                )
                .await?;
                metrics::counter!(apex_edge_metrics::PAYMENT_REVERSALS_TOTAL, "provider" => intent.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                continue;
            }
        };

        let outcome = provider
            .void(VoidRequest {
                idempotency_key: intent.id,
                provider_payment_id,
                reason: Some("sale did not complete".into()),
            })
            .await;

        match outcome {
            Ok(()) => {
                mark_payment_intent_reversed(pool, intent.id).await?;
                reversed += 1;
                metrics::counter!(apex_edge_metrics::PAYMENT_REVERSALS_TOTAL, "provider" => intent.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_SUCCESS).increment(1);
                tracing::warn!(
                    intent_id = %intent.id,
                    amount_cents = intent.approved_cents,
                    "reversed a payment captured for a sale that never completed"
                );
            }
            Err(e) => {
                record_payment_intent_reversal_failure(pool, intent.id, &e.to_string()).await?;
                metrics::counter!(apex_edge_metrics::PAYMENT_REVERSALS_TOTAL, "provider" => intent.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                tracing::error!(intent_id = %intent.id, error = %e, "payment reversal failed; staying queued");
            }
        }
    }

    metrics::gauge!(apex_edge_metrics::PAYMENT_REVERSALS_PENDING).set(
        fetch_payment_intents_awaiting_reversal(pool, i64::from(u16::MAX))
            .await?
            .len() as f64,
    );

    Ok(reversed)
}

/// Background loop that keeps retrying owed reversals.
pub async fn run_payment_reversal_loop(
    pool: SqlitePool,
    payments: PaymentSettings,
    interval: std::time::Duration,
) {
    let policy = ReversalSweepPolicy::from_env();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(e) = run_payment_reversal_sweep(&pool, &payments, policy).await {
            tracing::error!(error = %e, "payment reversal sweep failed");
        }
    }
}

/// Stable, deterministic idempotency key for a provider call, derived from the POS
/// envelope key and the operation. A retried command reuses the same key so the provider
/// answers from its own idempotency cache instead of charging again.
pub fn provider_idempotency_key(envelope_key: Uuid, tender_id: Uuid, operation: &str) -> Uuid {
    let seed = format!("{envelope_key}:{tender_id}:{operation}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cash_is_always_available_and_unknown_providers_are_reported() {
        let settings = PaymentSettings::new("EUR");
        assert!(matches!(
            settings.resolve(Some("cash")),
            ProviderChoice::Provider(_)
        ));
        assert!(matches!(
            settings.resolve(None),
            ProviderChoice::ManualTender
        ));
        assert!(matches!(
            settings.resolve(Some(MANUAL_PROVIDER)),
            ProviderChoice::ManualTender
        ));
        match settings.resolve(Some("adyen_terminal")) {
            ProviderChoice::Unknown(code) => assert_eq!(code, "adyen_terminal"),
            _ => panic!("an unconfigured provider must not resolve"),
        }
    }

    #[test]
    fn registering_a_provider_makes_it_resolvable() {
        let settings =
            PaymentSettings::new("EUR").with_provider(Arc::new(SimulatedTerminalProvider::new()));
        assert!(settings.provider_codes().contains(&"simulated_terminal"));
        assert!(matches!(
            settings.resolve(Some("simulated_terminal")),
            ProviderChoice::Provider(_)
        ));
    }

    #[test]
    fn idempotency_keys_are_stable_per_operation() {
        let envelope = Uuid::new_v4();
        let tender = Uuid::new_v4();
        assert_eq!(
            provider_idempotency_key(envelope, tender, "authorize"),
            provider_idempotency_key(envelope, tender, "authorize")
        );
        assert_ne!(
            provider_idempotency_key(envelope, tender, "authorize"),
            provider_idempotency_key(envelope, tender, "capture")
        );
    }
}
