# ApexEdge Architecture

- **POS/MPOS** <-> **ApexEdge** (northbound): cart, checkout, payment, finalize.
- **ApexEdge** <-> **HQ** (southbound): data sync in, order submission out (durable outbox).
- **Local-first**: catalog, prices, promos, coupons, config available on hub; sync is async with checkpoints.
- **Print**: persistent queue, template rendering, device adapters (ESC/POS, PDF, network).

Related: [README](../../README.md) · [Contracts](../contracts/README.md) · [Runbook](../runbook/README.md) · [Contributing](../../CONTRIBUTING.md) · [Security](../../SECURITY.md)

---

## Mermaid Diagrams

High-level, transparent diagrams for each major system piece. Each section has purpose, diagram, and interpretation notes.

### 1. System Context

**Purpose:** Show actors and trust boundaries: northbound (POS), southbound (HQ), and local persistence.

```mermaid
flowchart LR
    subgraph northbound [Northbound]
        POS[POS / MPOS]
    end
    subgraph apex [ApexEdge Hub]
        API[apex_edge_api]
    end
    subgraph southbound [Southbound]
        HQ[HQ]
    end
    subgraph local [Local]
        SQLite[(SQLite)]
    end
    POS -->|"POST /pos/command\ncart/checkout"| API
    API -->|"cart state / finalize result"| POS
    API --> SQLite
    SQLite --> API
    API -->|"outbox dispatch\nPOST submit"| HQ
    HQ -->|"catalog, prices, promos,\nconfig sync"| API
```

**Notes:**
- **Inputs:** POS sends `PosRequestEnvelope`; HQ pushes catalog/prices/promos/config; ApexEdge reads/writes SQLite.
- **Outputs:** POS gets `PosResponseEnvelope` (cart state or finalize); HQ receives `HqOrderSubmissionEnvelope`; documents are generated and stored for POS retrieval.
- **Trust boundaries:** External = POS, HQ; local = SQLite; ApexEdge is the single hub between them.

### 2. Runtime Bootstrap

**Purpose:** Startup sequence from binary entrypoint to listening server (DB, migrations, sync scheduling, outbox dispatcher, metrics, router).

```mermaid
sequenceDiagram
    participant Main as apex_edge main
    participant Storage as apex_edge_storage
    participant Sync as apex_edge_sync
    participant Outbox as apex_edge_outbox
    participant Metrics as apex_edge_metrics
    participant App as build_router
    participant Axum as axum::serve
    Main->>Storage: create_sqlite_pool(APEX_EDGE_DB)
    Main->>Storage: run_migrations(pool)
    opt APEX_EDGE_SEED_DEMO set
        Main->>Storage: seed_demo_data(pool)
    end
    opt APEX_EDGE_SYNC_SOURCE_URL set
        Main->>Sync: run_sync_ndjson once on startup
        Main->>Main: tokio::spawn daily sync loop (24h interval)
    end
    opt APEX_EDGE_HQ_SUBMIT_URL set
        Main->>Outbox: tokio::spawn run_dispatcher_loop (30s interval)
    end
    Main->>Main: parse APEX_EDGE_ALLOWED_ORIGINS → Vec<HeaderValue>
    Main->>Metrics: install_recorder()
    Main->>App: build_router(pool, store_id, metrics_handle, allowed_origins)
    App->>App: AppState { pool, store_id, metrics_handle }
    App->>App: CorsLayer — wildcard if empty, list if set
    App->>App: Router with /health, /ready, /pos/command, /documents, /orders, /metrics, /sync/status
    Main->>Axum: serve(TcpListener::bind(0.0.0.0:3000), app)
    Axum-->>Main: listening
```

**Notes:**
- **Inputs:** Env `APEX_EDGE_DB` (default `apex_edge.db`); `APEX_EDGE_SYNC_SOURCE_URL` (optional, enables sync); `APEX_EDGE_HQ_SUBMIT_URL` (optional, enables outbox dispatch); `APEX_EDGE_SEED_DEMO` (optional, seeds demo catalog/customers/promotions); `APEX_EDGE_ALLOWED_ORIGINS` (optional, comma-separated; empty = wildcard CORS for local dev, non-empty = restricted).
- **Outputs:** HTTP server on port 3000; DB migrated; optional background sync and dispatcher tasks spawned.
- **Failure path:** Pool or migration failure exits main; server bind failure propagates. Sync and dispatcher errors are logged and retried on next cycle without stopping the process.

### 3. HTTP Surface (Routes and Owners)

**Purpose:** Map every HTTP route to handler and owner crate/module for tracing and metrics ownership.

```mermaid
flowchart TB
    subgraph routes [HTTP Routes]
        R1["GET /health"]
        R2["GET /ready"]
        R3["POST /pos/command"]
        R4["GET /catalog/products"]
        R4b["GET /catalog/products/:id"]
        R5["GET /catalog/categories"]
        R6["GET /customers"]
        R7["GET /documents/:id"]
        R8["GET /orders/:order_id/documents"]
        R9["GET /metrics"]
        R10["GET /sync/status"]
    end
    subgraph api [apex_edge_api]
        H[health]
        P[pos]
        C[catalog_search]
        CC[catalog_categories]
        CS[customer_search]
        D[documents]
        M[metrics_handler]
        SS[sync_status]
    end
    R1 --> H
    R2 --> H
    R3 --> P
    R4 --> C
    R4b --> C
    R5 --> CC
    R6 --> CS
    R7 --> D
    R8 --> D
    R9 --> M
    R10 --> SS
```

**Notes:**
- **Inputs:** Incoming requests to the listed paths; `/ready` and document/pos handlers use `AppState` (pool).
- **Outputs:** JSON or Prometheus scrape; `/ready` returns 503 if DB probe fails.
- **Ownership:** All route behaviors owned by `apex-edge-api`; health = `health` module; pos = `pos`; documents = `documents`; metrics = `metrics_handler`. See [METRICS_BEHAVIORS.md](../METRICS_BEHAVIORS.md).

### 4. POS Command Flow

**Purpose:** Envelope validation and version gate; success vs unsupported-version response.

```mermaid
flowchart TB
    Start([POST /pos/command]) --> Parse[Parse PosRequestEnvelope]
    Parse --> CheckVersion{version == V1_0_0?}
    CheckVersion -->|No| ErrVersion[PosResponseEnvelope success=false errors=UNSUPPORTED_VERSION]
    CheckVersion -->|Yes| Success[PosResponseEnvelope success=true payload=...]
    ErrVersion --> Response([JSON response])
    Success --> Response
```

**Notes:**
- **Inputs:** `PosRequestEnvelope<PosCommand>` with `version`, `idempotency_key`, `store_id`, `register_id`, `payload`.
- **Outputs:** `PosResponseEnvelope` — either success with payload or failure with `PosError` code `UNSUPPORTED_VERSION`.
- **Failure path:** Unsupported contract version returns 200 with `success: false` and errors; no 4xx/5xx for version mismatch (contract-defined).

### 5. Document Retrieval Flow

**Purpose:** `get_document` and `list_order_documents`: request → storage → response with status codes.

```mermaid
sequenceDiagram
    participant Client as POS
    participant Router as Axum Router
    participant Docs as apex_edge_api::documents
    participant Storage as apex_edge_storage
    participant DB as SQLite
    Client->>Router: GET /documents/:id
    Router->>Docs: get_document(id)
    Docs->>Storage: get_document(pool, id)
    Storage->>DB: query
    DB-->>Storage: row or none
    Storage-->>Docs: Ok(doc) or Ok(None)
    alt doc found
        Docs-->>Client: 200 JSON DocumentResponse
    else not found
        Docs-->>Client: 404
    else storage error
        Docs-->>Client: 500
    end
    Client->>Router: GET /orders/:order_id/documents
    Router->>Docs: list_order_documents(order_id)
    Docs->>Storage: list_documents_for_order(pool, order_id)
    Storage->>DB: query
    DB-->>Storage: rows
    Storage-->>Docs: Ok(docs)
    Docs-->>Client: 200 JSON Vec DocumentSummary
```

**Notes:**
- **Inputs:** `GET /documents/:id` (UUID); `GET /orders/:order_id/documents` (order UUID). Both use shared `AppState.pool`.
- **Outputs:** Single document (content, status, mime_type) or list of document summaries; 404 when document missing; 500 on storage error.
- **Failure path:** Storage errors map to 500; missing document to 404. List endpoint returns 500 on storage error only.

### 6. Outbox Dispatch Flow

**Purpose:** Background loop fires every 30 seconds; each cycle polls pending outbox rows, POSTs to HQ, and marks accepted/retry/dead-letter. Wired in `main.rs` when `APEX_EDGE_HQ_SUBMIT_URL` is set.

```mermaid
flowchart TB
    EnvCheck{APEX_EDGE_HQ_SUBMIT_URL set?} -->|Yes| SpawnLoop[tokio::spawn run_dispatcher_loop 30s interval]
    SpawnLoop --> Tick[tick every 30s]
    Tick --> RunOnce[run_once pool, client, hq_url]
    RunOnce --> Fetch[fetch_pending_outbox pool, limit 10]
    Fetch --> Loop{for each row}
    Loop --> POST[POST row.payload to HQ submit URL]
    POST --> Result{response?}
    Result -->|success + accepted| MarkDelivered[mark_delivered]
    Result -->|success + not_accepted or non-2xx or network error| CheckAttempts{attempts >= 10?}
    CheckAttempts -->|Yes| DLQ[mark_dead_letter]
    CheckAttempts -->|No| Retry[schedule_retry with backoff]
    MarkDelivered --> Loop
    Retry --> Loop
    DLQ --> Loop
    Loop --> CountMetrics[emit OUTBOX_DISPATCH_ATTEMPTS_TOTAL and OUTBOX_DISPATCHER_CYCLES_TOTAL]
    CountMetrics --> Tick
    EnvCheck -->|No| Skip[dispatcher not started]
```

**Notes:**
- **Inputs:** `pool`, HTTP `client`, `APEX_EDGE_HQ_SUBMIT_URL` (env); pending rows from `apex_edge_storage::outbox`. Background loop started once at startup.
- **Outputs:** Rows marked delivered when HQ returns `accepted`; retry scheduled with exponential backoff (base 5s, capped at 320s); DLQ when `MAX_ATTEMPTS` (10) reached.
- **Metrics:** `apex_edge_outbox_dispatch_attempts_total{outcome}`, `apex_edge_outbox_dispatch_duration_seconds`, `apex_edge_outbox_dlq_total`, `apex_edge_outbox_dispatcher_cycles_total{outcome}`.
- **Failure path:** Cycle-level errors (storage, network) are logged and counted; loop continues on next tick without stopping the process.

### 7. Sync Ingest and Entity Application Flow

**Purpose:** Full sync pipeline: fetch NDJSON from HQ, apply each entity to its storage table, then advance the per-entity checkpoint. All entities supported: catalog, categories, price_book, tax_rules, customers, promotions. Unknown entities advance checkpoint without storage (forward-compatibility).

```mermaid
flowchart TB
    RunSync[run_sync_ndjson] --> ForEntity{for each entity in config}
    ForEntity --> Fetch[fetch_entity_ndjson_stream from HQ URL]
    Fetch --> Apply[apply_entity_batch pool, entity, payloads, store_id]
    Apply --> EntitySwitch{entity?}
    EntitySwitch -->|catalog| InsertCatalogItems["replace_catalog_items\n(persists is_active)"]
    EntitySwitch -->|categories| InsertCategory[insert_category per item]
    EntitySwitch -->|price_book| ReplacePriceBook[replace_price_book_entries atomically]
    EntitySwitch -->|tax_rules| InsertTaxRule[insert_tax_rule per item]
    EntitySwitch -->|customers| InsertCustomer[insert_customer per item]
    EntitySwitch -->|promotions| InsertPromotion[insert_promotion per item]
    EntitySwitch -->|inventory| ReplaceInventory["replace_inventory_levels\n(available_qty, is_available, image_urls)"]
    EntitySwitch -->|unknown| SkipLog[log debug skip]
    InsertCatalogItems --> Ingest[ingest_batch advance checkpoint]
    InsertCategory --> Ingest
    ReplacePriceBook --> Ingest
    InsertTaxRule --> Ingest
    InsertCustomer --> Ingest
    InsertPromotion --> Ingest
    ReplaceInventory --> Ingest
    SkipLog --> Ingest
    Ingest --> ForEntity
    ForEntity --> UpdateStatus[upsert_latest_sync_run success]
```

**Notes:**
- **Inputs:** `pool`, `SyncSourceConfig` (base URL + entity paths), `ContractVersion`, `store_id`. Contract types: `CatalogItem`, `Category`, `PriceBook`, `TaxRule`, `Customer`, `Promotion`, `InventoryLevel`.
- **Outputs:** Each entity's data persisted to its storage table; checkpoint advanced per entity; sync run status updated.
- **Metrics:** `apex_edge_sync_ingest_batches_total{entity, outcome}`, `apex_edge_sync_ingest_duration_seconds{entity}`.
- **Failure path:** Invalid JSON payload fails the entity's batch with `IngestError::InvalidPayload`; the whole sync run is marked `failed`; checkpoint does not advance for failed entities; next run retries.
- **price_book:** Synced with delete-and-replace semantics (atomically replaces all price book entries for the store in a transaction).
- **inventory:** Updates `available_qty`, `is_available`, and `image_urls` on existing `catalog_items` rows. Missing item IDs are silently skipped (forward-compatible). Default `available_qty = NULL` means untracked — no stock constraint applied.

### 8. Observability and Behavior Ownership

**Purpose:** Map behavior names and crate/module ownership for metrics and health; transparency for documentation.

```mermaid
flowchart TB
    subgraph behaviors [Behaviors]
        B1[health_check]
        B2[ready_check]
        B3[pos_command]
        B4[get_document]
        B5[list_order_documents]
        B6[outbox_dispatch]
        B7[sync_ingest]
        B8[document_render]
    end
    subgraph owners [Owner Crate / Module]
        O1[apex_edge_api / health]
        O2[apex_edge_api / health]
        O3[apex_edge_api / pos]
        O4[apex_edge_api / documents]
        O5[apex_edge_api / documents]
        O6[apex_edge_outbox / dispatcher]
        O7[apex_edge_sync / ingest]
        O8[apex_edge_printing / generator]
    end
    B1 --> O1
    B2 --> O2
    B3 --> O3
    B4 --> O4
    B5 --> O5
    B6 --> O6
    B7 --> O7
    B8 --> O8
```

**Notes:**
- **Inputs:** Route or flow (see table in [METRICS_BEHAVIORS.md](../METRICS_BEHAVIORS.md)); health/ready = liveness/readiness; `/metrics` = Prometheus scrape when metrics handle present.
- **Outputs:** Each behavior is the unit of ownership for metrics and tracing; DB probe only in ready_check; document fetch/list via storage; outbox and sync via their crates.
- **Transparency:** Single source of truth for route → behavior → owner is METRICS_BEHAVIORS.md; tiers (Tier 1–5) define implementation priority.

### 9. Local POS Simulator Frontend

**Purpose:** Document the local-only POS simulator UI: a POS-style interface with catalog (categories, product search, pagination), customer search (name/email/code/id), cart, checkout, and documents.

```mermaid
sequenceDiagram
    participant User as Cashier
    participant UI as POSSimulatorUI
    participant API as ApexEdge API
    participant Storage as SQLite
    User->>UI: Connect, New sale
    UI->>API: GET /health, GET /ready
    API-->>UI: status
    UI->>API: GET /catalog/categories
    API->>Storage: list_categories
    Storage-->>API: categories
    API-->>UI: category list
    UI->>API: GET /catalog/products?q=&category_id=&page=&per_page=
    API->>Storage: list_catalog_items
    Storage-->>API: items, total
    API-->>UI: paginated products
    UI->>API: GET /customers?q=
    API->>Storage: search_customers
    Storage-->>API: customers
    API-->>UI: customer list
    User->>UI: Add product, Remove line, Set customer, Checkout
    UI->>API: POST /pos/command create_cart, add_line_item, remove_line_item, set_customer, set_tendering, add_payment, finalize_order
    API->>Storage: cart/order
    Storage-->>API: cart state or finalize result
    API-->>UI: state
    UI->>API: GET /orders/:order_id/documents, GET /documents/:id
    API->>Storage: list/get documents
    Storage-->>UI: document list/content
```

**Notes:**
- **Inputs:** Backend base URL; catalog filters (search q, category, page); customer search q (name, email, code, or id); cart actions and checkout.
- **Outputs:** Categories and paginated product list; customer search results; cart state and finalize result; document list and content.
- **API:** `GET /catalog/categories`, `GET /catalog/products?q=&category_id=&page=&per_page=`, `GET /customers?q=` (and legacy `?code=` for exact code). Products support search by SKU, name, or description; customers by code, name, email, or id.
- **POS commands:** `create_cart`, `add_line_item` (optional `unit_price_override_cents` for positive price override), `remove_line_item` (removes a line by `line_id`; re-runs pricing pipeline on remaining lines; transitions cart back to Open when last line is removed), `set_customer`, `apply_manual_discount` (reason mandatory; kinds: percent_cart, percent_item, fixed_cart, fixed_item), `set_tendering`, `add_payment`, `finalize_order`. Promotions (coupons and automatic) are seeded and applied in pipeline; manual discounts applied after promos and included in order metadata to HQ.
- **Customer on cart:** When `set_customer` succeeds, the API handler looks up the customer record and populates `customer_name` and `customer_code` in `CartState`. Every subsequent command that returns `CartState` also enriches these fields. The cart panel shows a banner with the customer name and code whenever a customer is attached.
- **Layout:** Mobile-first, app-like UI: fixed bottom tab bar (Customers / Catalog / Sync / Cart) with safe-area insets; 44px minimum touch targets; full viewport height (`100dvh`). At 768px+ nav moves to header; at 1024px (e.g. iPad landscape) content is constrained with larger catalog grid. Event log shown from 768px only.
- **Scope:** Simulator runs as a separate dev server (e.g. Vite on port 5173); CORS enabled. Local use only.
- **Product Detail Page:** Clicking "View" on any catalog card navigates to `/product/:id` (URL route). PDP fetches full product via `GET /catalog/products/:id`, displays image gallery (thumbnail strip + main image), availability badge, quantity stepper, and "Add to Cart" button. After add-to-cart, navigates back to `/catalog`. Add-to-cart is disabled when item is inactive or out of stock.
- **Availability in catalog:** Product cards show availability badge (Out of Stock / low stock / In Stock / Available). The "+ Add" button is disabled for out-of-stock or inactive items. Images (first thumbnail) shown when synced.

### 10. Example Sync Source and Streamed Sync

**Purpose:** Document the separate example-sync-source tool and how ApexEdge pulls sync data on startup and daily via NDJSON streaming; sync status is persisted and exposed to the frontend.

```mermaid
flowchart LR
    subgraph sourceTool [Example Sync Source Tool]
        NDJSON[NDJSON Entity Endpoints]
    end
    subgraph edgeApp [ApexEdge]
        Scheduler[Startup and Daily Scheduler]
        Fetcher[NDJSON Stream Fetcher]
        Ingest[Ingest and Checkpoint]
        StatusStore[Latest Sync Status Store]
        StatusAPI[GET /sync/status]
        UI[Frontend Sync Status Panel]
    end
    Scheduler --> Fetcher
    Fetcher --> Ingest
    Ingest --> StatusStore
    StatusStore --> StatusAPI
    StatusAPI --> UI
    NDJSON --> Fetcher
```

**Notes:**
- **Example sync source:** Separate binary `tools/example-sync-source`; serves NDJSON per entity (first line `{"total": N}`, then N lines of base64 payload). Contract-only coupling; no app runtime dependencies. Run with `cargo run -p example-sync-source` (default port 3030; `SYNC_SOURCE_PORT` env). Entities: catalog, categories, price_book, tax_rules, promotions, customers, coupons, **inventory** (per-item availability + image URLs).
- **ApexEdge sync:** When `APEX_EDGE_SYNC_SOURCE_URL` is set, main runs sync once on startup then spawns a 24h periodic task. `run_sync_ndjson` streams each entity (line-by-line), collects payloads per entity, ingests in batch, advances checkpoints, and updates latest sync run + per-entity status in storage.
- **Sync status:** Stored in `sync_run` (single row) and `entity_sync_status`; exposed at `GET /sync/status`. Frontend Sync tab shows last sync time, run state (idle/syncing), and per-entity progress (current, total, percent, status).
- **Failure path:** Sync errors are logged; latest run is marked `failed` with error message; next scheduled run proceeds after 24h.

### 11. Stock and Availability Sync

**Purpose:** Document how inventory levels and product availability are synced from HQ and enforced on the POS add-to-cart path and exposed in the product catalog API.

```mermaid
flowchart TB
    subgraph hq [HQ Sync Source]
        CatalogEnt["catalog entity\n(CatalogItem.is_active)"]
        InventoryEnt["inventory entity\n(InventoryLevel)"]
    end
    subgraph sync [apex_edge_sync]
        ApplyBatch[apply_entity_batch]
    end
    subgraph storage [SQLite catalog_items]
        IsActive[is_active col]
        AvailQty[available_qty col]
        ImgUrls[image_urls col]
    end
    subgraph api [apex_edge_api]
        ProductSearch["GET /catalog/products\nGET /catalog/products/:id"]
        AddLine["POST /pos/command\nadd_line_item"]
    end
    CatalogEnt --> ApplyBatch
    InventoryEnt --> ApplyBatch
    ApplyBatch --> IsActive
    ApplyBatch --> AvailQty
    ApplyBatch --> ImgUrls
    IsActive --> ProductSearch
    AvailQty --> ProductSearch
    ImgUrls --> ProductSearch
    IsActive --> AddLine
    AvailQty --> AddLine
    AddLine -->|"OUT_OF_STOCK\nINSUFFICIENT_STOCK"| StockError[POS error response]
    AddLine --> CartState[cart state updated]
```

**Notes:**
- **Inputs:** `catalog` sync entity persists `is_active` from `CatalogItem`. `inventory` sync entity persists `available_qty`, `is_available`, and `image_urls` from `InventoryLevel` (per-item, per-store).
- **Outputs:** `ProductSearchResult` now includes `is_active`, `available_qty` (nullable — `null` = untracked), and `image_urls`. `GET /catalog/products/:id` returns full product detail for PDP.
- **Stock enforcement:** `add_line_item` checks `CatalogItemRow::check_quantity` before inserting a line. Returns `OUT_OF_STOCK` if `is_active=false` or `available_qty <= 0`; returns `INSUFFICIENT_STOCK` if `quantity > available_qty`. Items with `available_qty = NULL` (inventory not yet synced) are not constrained.
- **Metrics:** `apex_edge_catalog_stock_checks_total{outcome}` counts add-to-cart stock checks (ok, OUT_OF_STOCK, INSUFFICIENT_STOCK). `apex_edge_catalog_product_by_id_total{outcome}` counts product-by-id requests.
- **Failure path:** HQ may not have inventory synced for all items — defaults to NULL (untracked), which never blocks cart. is_active defaults to 1 (active).

### 12. Product Detail Page (PDP) with Image Gallery

**Purpose:** Document the URL-routed Product Detail Page in the POS simulator frontend; image gallery, quantity stepper, availability badge, and add-to-cart flow.

```mermaid
sequenceDiagram
    participant Cashier as Cashier
    participant CatalogUI as CatalogPanel
    participant Router as react-router
    participant PDP as ProductDetailPage
    participant API as ApexEdge API
    participant POS as POS command
    Cashier->>CatalogUI: Click "View" on product card
    CatalogUI->>Router: navigate("/product/:id")
    Router->>PDP: render ProductDetailPage
    PDP->>API: GET /catalog/products/:id
    API-->>PDP: ProductSearchResult with availability+images
    PDP-->>Cashier: Render gallery, availability badge, quantity stepper
    Cashier->>PDP: Adjust quantity, click "Add to Cart"
    PDP->>POS: POST /pos/command add_line_item(item_id, quantity)
    POS-->>PDP: cart state updated
    PDP->>Router: navigate("/catalog")
```

**Notes:**
- **Inputs:** URL parameter `:id` (product UUID). Backend `GET /catalog/products/:id` returns full `ProductSearchResult` including `available_qty`, `is_active`, and `image_urls`.
- **Outputs:** PDP displays product name, SKU, description, availability badge (Out of Stock / low stock / In Stock / Available-untracked), image gallery with thumbnail strip, quantity stepper, and Add to Cart button.
- **Routing:** PDP is at `/product/:id`. CatalogPanel "View" button navigates there. PDP Back button and post-add-to-cart both navigate to `/catalog`. Main POS app continues at `/*` routes.
- **Availability enforcement:** "Add to Cart" button is disabled when `is_active=false` or `available_qty <= 0`. Quantity stepper is capped at `available_qty` when tracked.
- **Image gallery:** Thumbnail strip shows all `image_urls`; clicking a thumbnail swaps the main image. Keyboard-accessible. Falls back to placeholder icon when no images are synced.

### 13. Internal Security Baseline (CORS)

**Purpose:** Document the configurable CORS posture introduced for the v0.1.0 internal-alpha security baseline. By default the hub allows all origins (suitable for local dev); a comma-separated env var locks CORS to an explicit allowlist in controlled deployments.

```mermaid
flowchart TD
    Start([build_router called]) --> CheckOrigins{APEX_EDGE_ALLOWED_ORIGINS set?}
    CheckOrigins -->|Empty / unset| Wildcard["CorsLayer: allow_origin(Any)\n⚠ local dev only"]
    CheckOrigins -->|Non-empty list| Restricted["CorsLayer: AllowOrigin::list(origins)\nonly listed origins receive CORS headers"]
    Wildcard --> Router[Axum Router]
    Restricted --> Router
    Router --> Browser[Browser preflight / request]
    Browser -->|Origin in list or wildcard| ACAO["access-control-allow-origin: <origin>"]
    Browser -->|Origin not in list| NoHeader["No access-control-allow-origin\nbrowser blocks request"]
```

**Notes:**
- **Inputs:** Env `APEX_EDGE_ALLOWED_ORIGINS` — comma-separated list of allowed origins (e.g. `http://localhost:5173,https://pos.example.internal`). Unset or empty = wildcard (logs a warning).
- **Outputs:** `access-control-allow-origin` header on preflight and actual responses; restricted list means unknown origins receive no matching header and browsers enforce the block.
- **Failure path:** Malformed origin strings (not valid `HeaderValue`) are silently skipped; if all entries are invalid the fallback is wildcard with a warning.
- **Tests:** `cors_restricted_trusted_origin_is_allowed` and `cors_restricted_unknown_origin_is_rejected` in `apex-edge/tests/cors_http.rs` verify both branches.

### 14. Synced PDF Receipt Templates

**Purpose:** Document how receipt and gift-receipt documents are produced from synced HTML templates, rendered with cart/order data, and output as PDFs for the POS to open or print.

```mermaid
flowchart LR
    HqTemplates[HQTemplateSync] --> SyncApply[SyncApplyPrintTemplates]
    SyncApply --> TemplateStore[(PrintTemplatesSQLite)]
    FinalizeOrder[FinalizeOrder] --> ReceiptVm[BuildReceiptViewModel]
    ReceiptVm --> TemplateResolve[ResolveTemplateByStoreDocType]
    TemplateResolve --> HtmlRender[RenderHtmlTemplate]
    HtmlRender --> PdfEngine[HeadlessChromePdf]
    PdfEngine --> Documents[(DocumentsSQLite)]
    Documents --> FrontendOpen[FrontendOpenPdf]
    FrontendOpen --> BrowserPrint[BrowserPrintAttempt]
```

**Notes:**
- **Inputs:** Sync entity `print_templates` with payloads `PrintTemplateConfig` (id, document_type, template_body, version); store_id from sync context. Finalize/gift-receipt use receipt view-model (order_id, store/customer/totals/lines/payments, tenant, logo placeholder).
- **Outputs:** Documents table row with `mime_type application/pdf` and base64-encoded PDF in `content`; frontend opens via Blob URL and attempts print.
- **Template engine:** `{{key}}` substitution and `{{#each key}}...{{/each}}` for arrays; HTML template rendered to PDF via headless Chrome.
- **Failure path:** Missing template falls back to plain-text receipt. Template render error or PDF engine failure marks document as failed and is recorded in `apex_edge_document_render_total{outcome=template_error|pdf_error}`.
- **Metrics:** `apex_edge_document_render_total{document_type, outcome}`, `apex_edge_document_render_duration_seconds{document_type}`. Sync of `print_templates` is covered by `apex_edge_sync_ingest_batches_total{entity=print_templates}`.

### 15. Edge Auth and Device Trust

**Purpose:** Document local hub authentication so mPOS clients can pair once, then exchange external associate identity tokens for hub sessions used to call protected northbound routes.

```mermaid
sequenceDiagram
    participant Admin as Hub Admin
    participant POS as mPOS Device
    participant API as apex_edge_api(auth)
    participant Store as apex_edge_storage(auth tables)
    participant Ext as External IdP Token

    Admin->>API: POST /auth/pairing-codes
    API->>Store: create device_pairing_codes (hashed code, TTL, attempts)
    API-->>Admin: one-time pairing code

    POS->>API: POST /auth/devices/pair (pairing_code + device metadata)
    API->>Store: validate/consume pairing code
    API->>Store: create trusted_devices (hashed device secret)
    API-->>POS: device_id + device_secret

    POS->>API: POST /auth/sessions/exchange (external token + device proof)
    API->>API: verify external token issuer/audience/signature
    API->>Store: validate trusted device + create auth_sessions
    API-->>POS: hub access_token + refresh_token

    POS->>API: GET /catalog/products (Authorization: Bearer access_token)
    API->>Store: validate session not revoked/expired + device active
    API-->>POS: protected route response
```

**Notes:**
- **Inputs:** Pairing requests (`store_id`, `created_by`), device metadata (`device_name`, optional `platform`), external associate token (`iss`, `aud`, `sub`, `store_id` claims), and bearer session tokens on protected routes.
- **Outputs:** `trusted_devices`, `device_pairing_codes`, `auth_sessions`, and `associate_identities` persisted locally. Protected routes return `401` when session/device validation fails.
- **Protection scope:** `/pos/*`, `/catalog/*`, `/customers`, `/documents/*`, `/orders/*`, `/sync/status` are protected when auth is enabled. `/health`, `/ready`, and auth bootstrap/session endpoints remain callable as designed.
- **Failure path:** Invalid/expired/consumed pairing code, device mismatch, token validation failure, and revoked/expired sessions all fail closed with `401`/`400`; attempts are tracked on pairing codes.
- **Metrics:** `apex_edge_auth_requests_total{operation,outcome}`, `apex_edge_auth_request_duration_seconds{operation}`, `apex_edge_auth_sessions_total{outcome}`, `apex_edge_device_pairings_total{outcome}`.
