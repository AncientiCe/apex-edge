use std::sync::Mutex;

use apex_edge_contracts::HqOrderSubmissionEnvelope;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct InsertOrderResult {
    pub inserted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredOrderSummary {
    pub submission_id: Uuid,
    pub order_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub sequence_number: u64,
    pub total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub line_count: u64,
    pub payment_summary: String,
    pub submitted_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredOrderDetail {
    pub submission_id: Uuid,
    pub order_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub sequence_number: u64,
    pub total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub line_count: u64,
    pub payment_summary: String,
    pub submitted_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub payload_json: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderPage {
    pub page: u64,
    pub per_page: u64,
    pub total: u64,
    pub items: Vec<StoredOrderSummary>,
}

pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("open sqlite at {path}: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn init_schema(&self) -> Result<(), String> {
        let guard = self.conn.lock().map_err(|_| "mutex poisoned".to_string())?;
        guard
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS received_orders (
                    submission_id   TEXT PRIMARY KEY,
                    order_id        TEXT NOT NULL,
                    store_id        TEXT NOT NULL,
                    register_id     TEXT NOT NULL,
                    sequence_number INTEGER NOT NULL,
                    total_cents     INTEGER NOT NULL,
                    discount_cents  INTEGER NOT NULL,
                    tax_cents       INTEGER NOT NULL,
                    line_count      INTEGER NOT NULL,
                    payment_summary TEXT NOT NULL,
                    submitted_at    TEXT NOT NULL,
                    received_at     TEXT NOT NULL,
                    payload_json    TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_received_orders_submitted_at
                    ON received_orders(submitted_at DESC, received_at DESC);
            "#,
            )
            .map_err(|e| format!("init schema: {e}"))?;
        Ok(())
    }

    pub fn insert_order(
        &self,
        envelope: &HqOrderSubmissionEnvelope,
    ) -> Result<InsertOrderResult, String> {
        let guard = self.conn.lock().map_err(|_| "mutex poisoned".to_string())?;
        let payload_json =
            serde_json::to_string(envelope).map_err(|e| format!("serialize envelope: {e}"))?;
        let payment_summary = envelope
            .order
            .payments
            .iter()
            .map(|p| {
                let reference = p
                    .external_reference
                    .clone()
                    .unwrap_or_else(|| "payment".to_string());
                format!("{reference} {}", format_money_cents(p.amount_cents))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let received_at = Utc::now().to_rfc3339();

        let changed = guard
            .execute(
                r#"
                INSERT INTO received_orders(
                    submission_id, order_id, store_id, register_id, sequence_number,
                    total_cents, discount_cents, tax_cents, line_count, payment_summary,
                    submitted_at, received_at, payload_json
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(submission_id) DO NOTHING
            "#,
                params![
                    envelope.submission_id.to_string(),
                    envelope.order.order_id.to_string(),
                    envelope.store_id.to_string(),
                    envelope.register_id.to_string(),
                    envelope.sequence_number as i64,
                    envelope.order.total_cents as i64,
                    envelope.order.discount_cents as i64,
                    envelope.order.tax_cents as i64,
                    envelope.order.lines.len() as i64,
                    payment_summary,
                    envelope.submitted_at.to_rfc3339(),
                    received_at,
                    payload_json,
                ],
            )
            .map_err(|e| format!("insert order: {e}"))?;

        Ok(InsertOrderResult {
            inserted: changed == 1,
        })
    }

    pub fn list_orders(&self, page: u64, per_page: u64) -> Result<OrderPage, String> {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 100);
        let offset = ((page - 1) * per_page) as i64;
        let limit = per_page as i64;
        let guard = self.conn.lock().map_err(|_| "mutex poisoned".to_string())?;

        let total: i64 = guard
            .query_row("SELECT COUNT(*) FROM received_orders", [], |row| row.get(0))
            .map_err(|e| format!("count orders: {e}"))?;

        let mut stmt = guard
            .prepare(
                r#"
                SELECT
                    submission_id, order_id, store_id, register_id, sequence_number,
                    total_cents, discount_cents, tax_cents, line_count, payment_summary,
                    submitted_at, received_at
                FROM received_orders
                ORDER BY submitted_at DESC, received_at DESC
                LIMIT ?1 OFFSET ?2
            "#,
            )
            .map_err(|e| format!("prepare list orders: {e}"))?;
        let rows = stmt
            .query_map(params![limit, offset], |row| {
                let submission_id: String = row.get(0)?;
                let order_id: String = row.get(1)?;
                let store_id: String = row.get(2)?;
                let register_id: String = row.get(3)?;
                let sequence_number: i64 = row.get(4)?;
                let total_cents: i64 = row.get(5)?;
                let discount_cents: i64 = row.get(6)?;
                let tax_cents: i64 = row.get(7)?;
                let line_count: i64 = row.get(8)?;
                let payment_summary: String = row.get(9)?;
                let submitted_at: String = row.get(10)?;
                let received_at: String = row.get(11)?;
                Ok((
                    submission_id,
                    order_id,
                    store_id,
                    register_id,
                    sequence_number,
                    total_cents,
                    discount_cents,
                    tax_cents,
                    line_count,
                    payment_summary,
                    submitted_at,
                    received_at,
                ))
            })
            .map_err(|e| format!("query list orders: {e}"))?;

        let mut items = Vec::new();
        for row in rows {
            let (
                submission_id,
                order_id,
                store_id,
                register_id,
                sequence_number,
                total_cents,
                discount_cents,
                tax_cents,
                line_count,
                payment_summary,
                submitted_at,
                received_at,
            ) = row.map_err(|e| format!("read list order row: {e}"))?;

            items.push(StoredOrderSummary {
                submission_id: parse_uuid(&submission_id)?,
                order_id: parse_uuid(&order_id)?,
                store_id: parse_uuid(&store_id)?,
                register_id: parse_uuid(&register_id)?,
                sequence_number: sequence_number as u64,
                total_cents: total_cents as u64,
                discount_cents: discount_cents as u64,
                tax_cents: tax_cents as u64,
                line_count: line_count as u64,
                payment_summary,
                submitted_at: parse_time(&submitted_at)?,
                received_at: parse_time(&received_at)?,
            });
        }

        Ok(OrderPage {
            page,
            per_page,
            total: total as u64,
            items,
        })
    }

    pub fn get_order(&self, submission_id: Uuid) -> Result<Option<StoredOrderDetail>, String> {
        let guard = self.conn.lock().map_err(|_| "mutex poisoned".to_string())?;
        let mut stmt = guard
            .prepare(
                r#"
                SELECT
                    submission_id, order_id, store_id, register_id, sequence_number,
                    total_cents, discount_cents, tax_cents, line_count, payment_summary,
                    submitted_at, received_at, payload_json
                FROM received_orders
                WHERE submission_id = ?1
            "#,
            )
            .map_err(|e| format!("prepare get order: {e}"))?;

        let row = stmt
            .query_row(params![submission_id.to_string()], |row| {
                let submission_id: String = row.get(0)?;
                let order_id: String = row.get(1)?;
                let store_id: String = row.get(2)?;
                let register_id: String = row.get(3)?;
                let sequence_number: i64 = row.get(4)?;
                let total_cents: i64 = row.get(5)?;
                let discount_cents: i64 = row.get(6)?;
                let tax_cents: i64 = row.get(7)?;
                let line_count: i64 = row.get(8)?;
                let payment_summary: String = row.get(9)?;
                let submitted_at: String = row.get(10)?;
                let received_at: String = row.get(11)?;
                let payload_json: String = row.get(12)?;
                Ok((
                    submission_id,
                    order_id,
                    store_id,
                    register_id,
                    sequence_number,
                    total_cents,
                    discount_cents,
                    tax_cents,
                    line_count,
                    payment_summary,
                    submitted_at,
                    received_at,
                    payload_json,
                ))
            })
            .optional()
            .map_err(|e| format!("query get order: {e}"))?;

        let Some((
            submission_id,
            order_id,
            store_id,
            register_id,
            sequence_number,
            total_cents,
            discount_cents,
            tax_cents,
            line_count,
            payment_summary,
            submitted_at,
            received_at,
            payload_json,
        )) = row
        else {
            return Ok(None);
        };

        Ok(Some(StoredOrderDetail {
            submission_id: parse_uuid(&submission_id)?,
            order_id: parse_uuid(&order_id)?,
            store_id: parse_uuid(&store_id)?,
            register_id: parse_uuid(&register_id)?,
            sequence_number: sequence_number as u64,
            total_cents: total_cents as u64,
            discount_cents: discount_cents as u64,
            tax_cents: tax_cents as u64,
            line_count: line_count as u64,
            payment_summary,
            submitted_at: parse_time(&submitted_at)?,
            received_at: parse_time(&received_at)?,
            payload_json,
        }))
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, String> {
    Uuid::parse_str(value).map_err(|e| format!("invalid UUID in db ({value}): {e}"))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| format!("invalid datetime in db ({value}): {e}"))
}

fn format_money_cents(value: u64) -> String {
    let dollars = value / 100;
    let cents = value % 100;
    format!("${dollars}.{cents:02}")
}
