//! Catalog endpoints for POS: product search/list by SKU, name, description; categories; pagination.
//! Also exposes availability (is_active, available_qty, image_urls) from synced inventory levels.

use apex_edge_metrics::{
    CATALOG_PRICES_TOTAL, CATALOG_PRODUCT_BY_ID_TOTAL, OUTCOME_ERROR, OUTCOME_HIT,
    OUTCOME_NOT_FOUND,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::de::{self, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt;
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Debug, Deserialize)]
pub struct ProductSearchQuery {
    /// Exact SKU lookup (returns at most one; backward compatible).
    pub sku: Option<String>,
    /// Search/list: match SKU, name, or description; use with category_id, page, per_page.
    pub q: Option<String>,
    pub category_id: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    24
}

#[derive(Debug, serde::Serialize)]
pub struct ProductSearchResult {
    pub id: Uuid,
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub title: Option<String>,
    pub brand: Option<String>,
    pub caption: Option<String>,
    pub category_id: Uuid,
    pub tax_category_id: Uuid,
    pub description: Option<String>,
    pub is_active: bool,
    pub is_available: bool,
    pub is_preorder: bool,
    pub online_from: Option<String>,
    pub serialized_inventory: bool,
    pub external_identifiers: Option<apex_edge_contracts::ExternalIdentifiers>,
    pub images: Vec<apex_edge_contracts::ProductImage>,
    pub extended_attributes: Vec<apex_edge_contracts::ExtendedAttribute>,
    pub variations: Option<serde_json::Value>,
    pub variation_attributes: Option<serde_json::Value>,
    /// `None` when inventory has not been synced (stock untracked).
    pub available_qty: Option<i64>,
    pub image_urls: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProductListResponse {
    pub items: Vec<ProductSearchResult>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

#[derive(Debug, Deserialize)]
pub struct PriceQuery {
    #[serde(
        default,
        alias = "productId",
        alias = "product_id",
        deserialize_with = "deserialize_one_or_many"
    )]
    pub product_id: Vec<String>,
}

fn deserialize_one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct OneOrManyVisitor;

    impl<'de> Visitor<'de> for OneOrManyVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string or a sequence of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_string()])
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value])
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut out = Vec::new();
            while let Some(value) = seq.next_element::<String>()? {
                out.push(value);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(OneOrManyVisitor)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceItem {
    pub product_id: Uuid,
    pub value: f64,
    pub currency_code: String,
}

#[derive(Debug, serde::Serialize)]
pub struct PriceListResponse {
    pub items: Vec<PriceItem>,
}

fn to_product_result(r: apex_edge_storage::CatalogItemRow) -> ProductSearchResult {
    let fallback_name = r.name.clone();
    let synced_item = r
        .raw_product_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<apex_edge_contracts::CatalogItem>(raw).ok());
    ProductSearchResult {
        id: r.id,
        product_id: r.id,
        sku: r.sku,
        name: r.name,
        title: synced_item
            .as_ref()
            .and_then(|item| item.title.clone())
            .or(Some(fallback_name)),
        brand: synced_item.as_ref().and_then(|item| item.brand.clone()),
        caption: synced_item.as_ref().and_then(|item| item.caption.clone()),
        category_id: r.category_id,
        tax_category_id: r.tax_category_id,
        description: synced_item
            .as_ref()
            .and_then(|item| item.description.clone())
            .or(r.description),
        is_active: r.is_active,
        is_available: r.is_available.unwrap_or(true),
        is_preorder: synced_item
            .as_ref()
            .and_then(|item| item.is_preorder)
            .unwrap_or(false),
        online_from: synced_item
            .as_ref()
            .and_then(|item| item.online_from.map(|datetime| datetime.to_rfc3339())),
        serialized_inventory: synced_item
            .as_ref()
            .and_then(|item| item.serialized_inventory)
            .unwrap_or(false),
        external_identifiers: synced_item
            .as_ref()
            .and_then(|item| item.external_identifiers.clone()),
        images: synced_item
            .as_ref()
            .and_then(|item| item.images.clone())
            .unwrap_or_default(),
        extended_attributes: synced_item
            .as_ref()
            .and_then(|item| item.extended_attributes.clone())
            .unwrap_or_default(),
        variations: synced_item
            .as_ref()
            .and_then(|item| item.variations.clone()),
        variation_attributes: synced_item
            .as_ref()
            .and_then(|item| item.variation_attributes.clone()),
        available_qty: r.available_qty,
        image_urls: r.image_urls,
    }
}

pub async fn get_prices(
    State(state): State<AppState>,
    Query(query): Query<PriceQuery>,
) -> Result<Json<PriceListResponse>, StatusCode> {
    let requested_ids: Vec<Uuid> = query
        .product_id
        .iter()
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect();
    if requested_ids.is_empty() {
        metrics::counter!(CATALOG_PRICES_TOTAL, 1u64, "outcome" => "empty");
        return Ok(Json(PriceListResponse { items: vec![] }));
    }

    let requested_set: HashSet<Uuid> = requested_ids.iter().copied().collect();
    let entries = apex_edge_storage::list_price_book_entries(&state.pool, state.store_id)
        .await
        .map_err(|_| {
            metrics::counter!(CATALOG_PRICES_TOTAL, 1u64, "outcome" => OUTCOME_ERROR);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let base_prices: HashMap<Uuid, (u64, String)> = entries
        .into_iter()
        .filter(|entry| {
            entry.modifier_option_id.is_none() && requested_set.contains(&entry.item_id)
        })
        .map(|entry| (entry.item_id, (entry.price_cents, entry.currency)))
        .collect();

    let items: Vec<PriceItem> = requested_ids
        .into_iter()
        .filter_map(|id| {
            base_prices
                .get(&id)
                .map(|(price_cents, currency)| PriceItem {
                    product_id: id,
                    value: (*price_cents as f64) / 100.0,
                    currency_code: currency.clone(),
                })
        })
        .collect();

    let outcome = if items.is_empty() {
        "empty"
    } else {
        OUTCOME_HIT
    };
    metrics::counter!(CATALOG_PRICES_TOTAL, 1u64, "outcome" => outcome);
    Ok(Json(PriceListResponse { items }))
}

pub async fn search_products(
    State(state): State<AppState>,
    Query(query): Query<ProductSearchQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Some(sku) = query.sku.as_deref().filter(|s| !s.is_empty()) {
        let row = apex_edge_storage::get_catalog_item_by_sku(&state.pool, state.store_id, sku)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let arr: Vec<ProductSearchResult> = row.map(to_product_result).into_iter().collect();
        return Ok(Json(
            serde_json::to_value(arr).unwrap_or(serde_json::json!([])),
        ));
    }
    let category_id = query
        .category_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);
    let offset = (page - 1) * per_page;
    let q = query.q.as_deref();
    let (items, total) = apex_edge_storage::list_catalog_items(
        &state.pool,
        state.store_id,
        category_id,
        q,
        per_page,
        offset,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let items = items.into_iter().map(to_product_result).collect();
    Ok(Json(
        serde_json::to_value(ProductListResponse {
            items,
            total,
            page,
            per_page,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
}

/// Fetch a single product by its UUID. Returns full product detail including
/// availability and all image URLs for the PDP gallery.
pub async fn get_product_by_id(
    State(state): State<AppState>,
    Path(product_id): Path<Uuid>,
) -> Result<Json<ProductSearchResult>, StatusCode> {
    let result = apex_edge_storage::get_catalog_item(&state.pool, state.store_id, product_id).await;
    match result {
        Ok(Some(row)) => {
            metrics::counter!(CATALOG_PRODUCT_BY_ID_TOTAL, 1u64, "outcome" => OUTCOME_HIT);
            Ok(Json(to_product_result(row)))
        }
        Ok(None) => {
            metrics::counter!(CATALOG_PRODUCT_BY_ID_TOTAL, 1u64, "outcome" => OUTCOME_NOT_FOUND);
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => {
            metrics::counter!(CATALOG_PRODUCT_BY_ID_TOTAL, 1u64, "outcome" => OUTCOME_ERROR);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PriceQuery;

    #[test]
    fn price_query_accepts_single_product_id() {
        let parsed: PriceQuery =
            serde_json::from_str(r#"{"productId":"30000000-0000-0000-0000-00000000000c"}"#)
                .expect("parse single productId");
        assert_eq!(parsed.product_id.len(), 1);
        assert_eq!(parsed.product_id[0], "30000000-0000-0000-0000-00000000000c");
    }

    #[test]
    fn price_query_accepts_repeated_product_ids() {
        let parsed: PriceQuery = serde_json::from_str(
            r#"{"productId":["30000000-0000-0000-0000-00000000000c","30000000-0000-0000-0000-00000000000d"]}"#,
        )
        .expect("parse repeated productId");
        assert_eq!(parsed.product_id.len(), 2);
    }
}
