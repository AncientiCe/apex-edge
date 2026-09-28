//! Fiscal provider selection and the sign-later sweeper.
//!
//! `apex-edge-adapters-fiscal` owns the transaction contract and the signers. This
//! module turns a paid cart (or a finalized return) into that transaction, calls the
//! configured signer, and — when the signer is briefly down and the jurisdiction
//! allows it — queues the unsigned receipt so the till can keep selling.

use std::sync::Arc;
use std::time::Instant;

use apex_edge_adapters_fiscal::export::{
    cii_factur_x, dsfinvk_files, ubl_xrechnung, CashPointClosing,
};
use apex_edge_adapters_fiscal::{
    CashPoint, FiscalError, FiscalLine, FiscalProvider, FiscalSignature, FiscalTender,
    FiscalTenderType, FiscalTransaction, FiscalTransactionKind, NoOpFiscalProvider, OfflinePolicy,
};
use apex_edge_domain::{Order, ReturnSnapshot};
use apex_edge_storage::{
    apply_order_fiscal_receipt, apply_return_fiscal_receipt, count_fiscal_queue, enqueue_document,
    enqueue_fiscal_signing, fetch_due_fiscal_signings, fetch_order_ledger_entry, get_catalog_item,
    get_catalog_item_by_sku, list_order_ledger_entries, list_tax_rules, mark_fiscal_dead_letter,
    mark_fiscal_signed, mark_generated, schedule_fiscal_retry, FiscalQueueStatus,
    FiscalReceiptUpdate, NewFiscalQueueEntry, ShiftRow,
};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Clone)]
pub struct FiscalSettings {
    pub provider: Arc<dyn FiscalProvider + Send + Sync>,
    pub currency: String,
}

impl std::fmt::Debug for FiscalSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FiscalSettings")
            .field("provider", &self.provider.provider_code())
            .field("currency", &self.currency)
            .finish()
    }
}

impl Default for FiscalSettings {
    fn default() -> Self {
        Self {
            provider: Arc::new(NoOpFiscalProvider),
            currency: "USD".into(),
        }
    }
}

/// How finalize should persist the fiscal result.
pub enum FiscalOutcome {
    Signed(FiscalSignature),
    Queued,
}

/// Columns written onto the order or return after a sign attempt.
pub struct FiscalPersistFields {
    pub provider: Option<String>,
    pub fiscal_id: Option<String>,
    pub signature: Option<String>,
    pub qr_payload: Option<String>,
    pub signed_at: Option<chrono::DateTime<Utc>>,
    pub pending: bool,
}

/// Build a sale transaction from a paid order, looking up tax rates from the catalog.
pub async fn sale_transaction(
    pool: &SqlitePool,
    order: &Order,
    store_id: Uuid,
    register_id: Uuid,
    shift_id: Option<Uuid>,
    currency: &str,
) -> FiscalTransaction {
    let rules = list_tax_rules(pool, store_id).await.unwrap_or_default();
    let mut lines = Vec::with_capacity(order.lines.len());
    for line in &order.lines {
        let (tax_rate_bps, tax_category) =
            tax_info_for_item(pool, store_id, line.item_id, &rules).await;
        // The line records how its tax was priced; the synced rule may have changed since.
        let tax_inclusive = line.tax_inclusive;
        let tax_cents = line.tax_cents as i64;
        let net_cents = net_of(
            line.line_total_cents.saturating_sub(line.discount_cents) as i64,
            tax_cents,
            tax_inclusive,
        );
        lines.push(FiscalLine {
            line_id: line.line_id,
            sku: line.sku.clone(),
            description: line.name.clone(),
            quantity: line.quantity,
            unit_price_cents: line.unit_price_cents as i64,
            net_cents,
            tax_cents,
            gross_cents: net_cents.saturating_add(tax_cents),
            discount_cents: line.discount_cents as i64,
            tax_rate_bps,
            tax_category,
            tax_inclusive,
        });
    }
    allocate_basket_discount(
        &mut lines,
        order.applied_coupons.iter().map(|c| c.2).sum::<u64>() as i64,
    );

    let net_cents: i64 = lines.iter().map(|l| l.net_cents).sum();
    let tax_cents: i64 = lines.iter().map(|l| l.tax_cents).sum();
    let gross_cents: i64 = lines.iter().map(|l| l.gross_cents).sum();

    FiscalTransaction {
        transaction_id: order.order_id,
        kind: FiscalTransactionKind::Sale,
        reference_transaction_id: None,
        cash_point: CashPoint {
            store_id,
            register_id,
            shift_id,
            operator_id: None,
        },
        occurred_at: order.created_at,
        currency: currency.to_ascii_uppercase(),
        lines,
        tenders: {
            let mut tenders: Vec<FiscalTender> = order
                .payments
                .iter()
                .map(|payment| FiscalTender {
                    tender_id: payment.tender_id,
                    tender_type: tender_type_from_label(
                        payment.external_reference.as_deref(),
                        payment.provider.as_deref(),
                    ),
                    amount_cents: payment.amount_cents as i64,
                    tip_cents: payment.tip_amount_cents as i64,
                    provider: payment.provider.clone(),
                    provider_reference: payment.provider_payment_id.clone(),
                })
                .collect();
            fit_tenders_to_gross(&mut tenders, gross_cents);
            tenders
        },
        net_cents,
        tax_cents,
        gross_cents,
    }
}

/// Build a refund transaction. Amounts are negative so a tax authority can tell
/// them apart from sales without reading the kind flag.
pub async fn refund_transaction(
    pool: &SqlitePool,
    snapshot: &ReturnSnapshot,
    currency: &str,
) -> FiscalTransaction {
    let rules = list_tax_rules(pool, snapshot.store_id)
        .await
        .unwrap_or_default();
    let mut lines = Vec::with_capacity(snapshot.lines.len());
    for line in &snapshot.lines {
        let (tax_rate_bps, tax_category) =
            tax_info_for_sku(pool, snapshot.store_id, &line.sku, &rules).await;
        let tax_inclusive = line.tax_inclusive;
        let tax_cents = -(line.tax_cents as i64);
        let net_cents = net_of(-(line.line_total_cents as i64), tax_cents, tax_inclusive);
        lines.push(FiscalLine {
            line_id: line.line_id,
            sku: line.sku.clone(),
            description: line.name.clone(),
            quantity: line.quantity,
            unit_price_cents: line.unit_price_cents as i64,
            net_cents,
            tax_cents,
            gross_cents: net_cents.saturating_add(tax_cents),
            discount_cents: 0,
            tax_rate_bps,
            tax_category,
            tax_inclusive,
        });
    }
    let net_cents: i64 = lines.iter().map(|l| l.net_cents).sum();
    let tax_cents: i64 = lines.iter().map(|l| l.tax_cents).sum();
    let gross_cents: i64 = lines.iter().map(|l| l.gross_cents).sum();

    FiscalTransaction {
        transaction_id: snapshot.id,
        kind: FiscalTransactionKind::Refund,
        reference_transaction_id: snapshot.original_order_id,
        cash_point: CashPoint {
            store_id: snapshot.store_id,
            register_id: snapshot.register_id,
            shift_id: snapshot.shift_id,
            operator_id: None,
        },
        occurred_at: Utc::now(),
        currency: currency.to_ascii_uppercase(),
        lines,
        tenders: snapshot
            .refunds
            .iter()
            .map(|refund| FiscalTender {
                tender_id: refund.refund_id,
                tender_type: tender_type_from_label(Some(&refund.tender_type), None),
                amount_cents: -(refund.amount_cents as i64),
                tip_cents: 0,
                provider: None,
                provider_reference: None,
            })
            .collect(),
        net_cents,
        tax_cents,
        gross_cents,
    }
}

/// Sign now, or — when the signer is down and the jurisdiction allows it — queue.
pub async fn sign_or_queue(
    app: &AppState,
    subject_kind: &str,
    subject_id: Uuid,
    transaction: &FiscalTransaction,
) -> Result<FiscalOutcome, FiscalError> {
    let provider_code = app.fiscal.provider.provider_code();
    let started = Instant::now();
    let result = app.fiscal.provider.sign(transaction).await;
    metrics::histogram!(apex_edge_metrics::FISCAL_RECEIPT_DURATION_SECONDS, "provider" => provider_code)
        .record(started.elapsed().as_secs_f64());

    match result {
        Ok(signature) => {
            metrics::counter!(apex_edge_metrics::FISCAL_RECEIPTS_TOTAL, "provider" => signature.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_SUCCESS).increment(1);
            Ok(FiscalOutcome::Signed(signature))
        }
        Err(error)
            if error.is_transient()
                && app.fiscal.provider.offline_policy() == OfflinePolicy::SignLater =>
        {
            let payload_json = serde_json::to_string(transaction).map_err(|e| {
                FiscalError::InvalidTransaction {
                    reason: e.to_string(),
                }
            })?;
            enqueue_fiscal_signing(
                &app.pool,
                NewFiscalQueueEntry {
                    subject_kind: subject_kind.into(),
                    subject_id,
                    provider: provider_code.into(),
                    payload_json,
                },
            )
            .await
            .map_err(|e| FiscalError::Rejected {
                provider: provider_code.into(),
                code: "queue_write_failed".into(),
                detail: e.to_string(),
            })?;
            metrics::counter!(apex_edge_metrics::FISCAL_RECEIPTS_TOTAL, "provider" => provider_code, "outcome" => apex_edge_metrics::OUTCOME_QUEUED).increment(1);
            Ok(FiscalOutcome::Queued)
        }
        Err(error) => {
            metrics::counter!(apex_edge_metrics::FISCAL_RECEIPTS_TOTAL, "provider" => provider_code, "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
            Err(error)
        }
    }
}

pub fn fields_from_outcome(provider_code: &str, outcome: FiscalOutcome) -> FiscalPersistFields {
    match outcome {
        FiscalOutcome::Signed(signature) => FiscalPersistFields {
            provider: Some(signature.provider),
            fiscal_id: signature.fiscal_id,
            signature: signature.signature,
            qr_payload: signature.qr_payload,
            signed_at: Some(signature.signed_at),
            pending: false,
        },
        FiscalOutcome::Queued => FiscalPersistFields {
            provider: Some(provider_code.into()),
            fiscal_id: None,
            signature: None,
            qr_payload: None,
            signed_at: None,
            pending: true,
        },
    }
}

/// Retry unsigned transactions. Permanent rejections go to the dead-letter queue;
/// outages are scheduled for another attempt.
pub async fn run_fiscal_signing_sweep(
    pool: &SqlitePool,
    fiscal: &FiscalSettings,
) -> Result<usize, apex_edge_storage::pool::PoolError> {
    let due = fetch_due_fiscal_signings(pool, 20).await?;
    let mut signed = 0usize;

    for row in due {
        let transaction: FiscalTransaction = match serde_json::from_str(&row.payload_json) {
            Ok(tx) => tx,
            Err(e) => {
                mark_fiscal_dead_letter(pool, row.id, &format!("payload unreadable: {e}")).await?;
                metrics::counter!(apex_edge_metrics::FISCAL_SIGN_LATER_TOTAL, "provider" => row.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                continue;
            }
        };

        match fiscal.provider.sign(&transaction).await {
            Ok(signature) => {
                let update = FiscalReceiptUpdate {
                    provider: signature.provider.clone(),
                    fiscal_id: signature.fiscal_id,
                    signature: signature.signature,
                    qr_payload: signature.qr_payload,
                    signed_at: Some(signature.signed_at),
                    pending: false,
                };
                if row.subject_kind == "return" {
                    apply_return_fiscal_receipt(pool, row.subject_id, &update).await?;
                } else {
                    apply_order_fiscal_receipt(pool, row.subject_id, &update).await?;
                }
                mark_fiscal_signed(pool, row.id).await?;
                signed += 1;
                metrics::counter!(apex_edge_metrics::FISCAL_SIGN_LATER_TOTAL, "provider" => row.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_SUCCESS).increment(1);
            }
            Err(error) if error.is_transient() => {
                let delay = backoff_delay_seconds(row.attempts.max(0) as u32);
                schedule_fiscal_retry(pool, row.id, delay, &error.to_string()).await?;
                metrics::counter!(apex_edge_metrics::FISCAL_SIGN_LATER_TOTAL, "provider" => row.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
            }
            Err(error) => {
                mark_fiscal_dead_letter(pool, row.id, &error.to_string()).await?;
                metrics::counter!(apex_edge_metrics::FISCAL_SIGN_LATER_TOTAL, "provider" => row.provider.clone(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
            }
        }
    }

    report_queue_depth(pool).await?;
    Ok(signed)
}

pub async fn run_fiscal_signing_loop(
    pool: SqlitePool,
    fiscal: FiscalSettings,
    interval: std::time::Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(e) = run_fiscal_signing_sweep(&pool, &fiscal).await {
            tracing::error!(error = %e, "fiscal sign-later sweep failed");
        }
    }
}

async fn report_queue_depth(pool: &SqlitePool) -> Result<(), apex_edge_storage::pool::PoolError> {
    let pending = count_fiscal_queue(pool, FiscalQueueStatus::Pending).await?;
    let dead = count_fiscal_queue(pool, FiscalQueueStatus::DeadLetter).await?;
    metrics::gauge!(apex_edge_metrics::FISCAL_QUEUE_DEPTH, "status" => "pending")
        .set(pending as f64);
    metrics::gauge!(apex_edge_metrics::FISCAL_QUEUE_DEPTH, "status" => "dead_letter")
        .set(dead as f64);
    Ok(())
}

fn backoff_delay_seconds(attempts: u32) -> i64 {
    let exp = attempts.min(6);
    (15i64.saturating_mul(1i64 << exp)).min(3600)
}

fn tender_type_from_label(reference: Option<&str>, provider: Option<&str>) -> FiscalTenderType {
    let label = reference.unwrap_or("").trim().to_ascii_lowercase();
    if label == "cash" {
        FiscalTenderType::Cash
    } else if label.starts_with("gift_card") {
        FiscalTenderType::GiftCard
    } else if label == "voucher" {
        FiscalTenderType::Voucher
    } else if label == "card"
        || label == "external"
        || provider.is_some_and(|p| !p.is_empty() && p != "manual")
    {
        FiscalTenderType::Card
    } else {
        FiscalTenderType::Other
    }
}

/// Net amount of a line priced at `price_cents`: an inclusive price already contains the tax.
fn net_of(price_cents: i64, tax_cents: i64, tax_inclusive: bool) -> i64 {
    if tax_inclusive {
        price_cents - tax_cents
    } else {
        price_cents
    }
}

async fn tax_info_for_item(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
    rules: &[apex_edge_contracts::TaxRule],
) -> (u32, String) {
    let tax_category_id = match get_catalog_item(pool, store_id, item_id).await {
        Ok(Some(item)) => item.tax_category_id,
        _ => return (0, "untaxed".into()),
    };
    match rules.iter().find(|r| r.tax_category_id == tax_category_id) {
        Some(rule) => (rule.rate_bps, rule.name.clone()),
        None => (0, "untaxed".into()),
    }
}

async fn tax_info_for_sku(
    pool: &SqlitePool,
    store_id: Uuid,
    sku: &str,
    rules: &[apex_edge_contracts::TaxRule],
) -> (u32, String) {
    let tax_category_id = match get_catalog_item_by_sku(pool, store_id, sku).await {
        Ok(Some(item)) => item.tax_category_id,
        _ => return (0, "untaxed".into()),
    };
    match rules.iter().find(|r| r.tax_category_id == tax_category_id) {
        Some(rule) => (rule.rate_bps, rule.name.clone()),
        None => (0, "untaxed".into()),
    }
}

fn allocate_basket_discount(lines: &mut [FiscalLine], discount: i64) {
    if discount <= 0 || lines.is_empty() {
        return;
    }
    let total_net: i64 = lines.iter().map(|l| l.net_cents).sum();
    if total_net <= 0 {
        return;
    }
    let mut remaining = discount;
    let last = lines.len() - 1;
    for (index, line) in lines.iter_mut().enumerate() {
        let share = if index == last {
            remaining
        } else {
            discount.saturating_mul(line.net_cents) / total_net
        };
        let share = share.min(line.net_cents).min(remaining).max(0);
        line.net_cents -= share;
        line.discount_cents = line.discount_cents.saturating_add(share);
        line.gross_cents = line.net_cents.saturating_add(line.tax_cents);
        remaining -= share;
    }
}

/// Cash often tenders more than the sale (the rest is change). Fiscalise the sale
/// amount, not the notes the customer handed over.
fn fit_tenders_to_gross(tenders: &mut [FiscalTender], gross: i64) {
    let sum: i64 = tenders.iter().map(|t| t.amount_cents).sum();
    if tenders.is_empty() || sum == gross {
        return;
    }
    if gross > 0 && sum > gross {
        let extra = sum - gross;
        if let Some(tender) = tenders.iter_mut().rev().find(|t| t.amount_cents >= extra) {
            tender.amount_cents -= extra;
        }
    }
}

/// Build a DSFinV-K closing (plus per-sale EN 16931 invoices) and persist it as a document.
/// Best-effort: a closing that cannot be exported must not reopen the till.
pub async fn persist_cash_point_closing(
    app: &AppState,
    shift: &ShiftRow,
    counted_cents: i64,
    expected_cents: i64,
) -> Option<Uuid> {
    let started = Instant::now();
    let summaries = list_order_ledger_entries(&app.pool, shift.store_id, Some(shift.id))
        .await
        .unwrap_or_default();
    let mut transactions = Vec::new();
    for summary in summaries {
        if let Ok(Some(order)) = fetch_order_ledger_entry(&app.pool, summary.order_id).await {
            transactions.push(transaction_from_ledger(&order, &app.fiscal.currency));
        }
    }
    let opened_at = DateTime::parse_from_rfc3339(&shift.opened_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let closing = CashPointClosing {
        shift_id: shift.id,
        store_id: shift.store_id,
        register_id: shift.register_id,
        opened_at,
        closed_at: Utc::now(),
        opening_float_cents: shift.opening_float_cents as i64,
        counted_cents,
        expected_cents,
        currency: app.fiscal.currency.clone(),
        tax_id: std::env::var("APEX_EDGE_FISKALY_TAX_ID").unwrap_or_default(),
        transactions: transactions.clone(),
    };
    let mut files: serde_json::Map<String, serde_json::Value> = dsfinvk_files(&closing)
        .into_iter()
        .map(|(name, body)| (name, serde_json::Value::String(body)))
        .collect();
    for tx in &transactions {
        files.insert(
            format!("invoices/{}.xrechnung.xml", tx.transaction_id),
            serde_json::Value::String(ubl_xrechnung(tx, &closing.tax_id, "ApexEdge store")),
        );
        files.insert(
            format!("invoices/{}.factur-x.xml", tx.transaction_id),
            serde_json::Value::String(cii_factur_x(tx, &closing.tax_id, "ApexEdge store")),
        );
    }
    let payload = serde_json::Value::Object(files);
    let content = payload.to_string();
    let doc_id = Uuid::new_v4();
    if enqueue_document(
        &app.pool,
        doc_id,
        "dsfinvk_closing",
        None,
        None,
        Uuid::nil(),
        &content,
    )
    .await
    .is_err()
    {
        metrics::counter!(apex_edge_metrics::FISCAL_EXPORTS_TOTAL, "kind" => "dsfinvk", "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
        return None;
    }
    if mark_generated(&app.pool, doc_id, "application/json", &content)
        .await
        .is_err()
    {
        metrics::counter!(apex_edge_metrics::FISCAL_EXPORTS_TOTAL, "kind" => "dsfinvk", "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
        return None;
    }
    metrics::counter!(apex_edge_metrics::FISCAL_EXPORTS_TOTAL, "kind" => "dsfinvk", "outcome" => apex_edge_metrics::OUTCOME_SUCCESS).increment(1);
    metrics::histogram!(apex_edge_metrics::FISCAL_RECEIPT_DURATION_SECONDS, "provider" => "export")
        .record(started.elapsed().as_secs_f64());
    Some(doc_id)
}

fn transaction_from_ledger(
    order: &apex_edge_storage::OrderLedgerEntry,
    currency: &str,
) -> FiscalTransaction {
    let lines: Vec<FiscalLine> = order
        .lines
        .iter()
        .map(|line| {
            let tax_cents = line.tax_cents as i64;
            let net_cents = net_of(
                line.line_total_cents.saturating_sub(line.discount_cents) as i64,
                tax_cents,
                line.tax_inclusive,
            );
            let tax_rate_bps = if net_cents.abs() > 0 {
                ((tax_cents.abs().saturating_mul(10_000)) / net_cents.abs()) as u32
            } else {
                0
            };
            FiscalLine {
                line_id: line.line_id,
                sku: line.sku.clone(),
                description: line.name.clone(),
                quantity: line.quantity.max(1),
                unit_price_cents: line.unit_price_cents as i64,
                net_cents,
                tax_cents,
                gross_cents: net_cents.saturating_add(tax_cents),
                discount_cents: line.discount_cents as i64,
                tax_rate_bps,
                tax_category: "standard".into(),
                tax_inclusive: line.tax_inclusive,
            }
        })
        .collect();
    let mut tenders: Vec<FiscalTender> = order
        .payments
        .iter()
        .map(|payment| FiscalTender {
            tender_id: payment.tender_id,
            tender_type: tender_type_from_label(
                Some(&payment.tender_type),
                payment.provider.as_deref(),
            ),
            amount_cents: payment.amount_cents as i64,
            tip_cents: payment.tip_amount_cents as i64,
            provider: payment.provider.clone(),
            provider_reference: payment.provider_payment_id.clone(),
        })
        .collect();
    let net_cents: i64 = lines.iter().map(|l| l.net_cents).sum();
    let tax_cents: i64 = lines.iter().map(|l| l.tax_cents).sum();
    let gross_cents: i64 = lines.iter().map(|l| l.gross_cents).sum();
    fit_tenders_to_gross(&mut tenders, gross_cents);
    FiscalTransaction {
        transaction_id: order.order_id,
        kind: FiscalTransactionKind::Sale,
        reference_transaction_id: None,
        cash_point: CashPoint {
            store_id: order.store_id,
            register_id: order.register_id,
            shift_id: order.shift_id,
            operator_id: None,
        },
        occurred_at: order.finalized_at,
        currency: currency.to_ascii_uppercase(),
        lines,
        tenders,
        net_cents,
        tax_cents,
        gross_cents,
    }
}
