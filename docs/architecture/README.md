# ApexEdge Architecture

- **POS/MPOS** <-> **ApexEdge** (northbound): cart, checkout, payment, finalize.
- **ApexEdge** <-> **HQ** (southbound): data sync in, order submission out (durable outbox).
- **Local-first**: catalog, prices, promos, coupons, config available on hub; sync is async with checkpoints.
- **Print**: persistent queue, template rendering, device adapters (ESC/POS, PDF, network).

See plan for full component list and phased delivery.

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

**Purpose:** Startup sequence from binary entrypoint to listening server (DB, migrations, metrics, router).

```mermaid
sequenceDiagram
    participant Main as apex_edge main
    participant Storage as apex_edge_storage
    participant Metrics as apex_edge_metrics
    participant App as build_router
    participant Axum as axum::serve
    Main->>Storage: create_sqlite_pool(APEX_EDGE_DB)
    Main->>Storage: run_migrations(pool)
    Main->>Metrics: install_recorder()
    Main->>App: build_router(pool, store_id, metrics_handle)
    App->>App: AppState { pool, store_id, metrics_handle }
    App->>App: Router with /health, /ready, /pos/command, /documents, /orders, /metrics
    Main->>Axum: serve(TcpListener::bind(0.0.0.0:3000), app)
    Axum-->>Main: listening
```

**Notes:**
- **Inputs:** Env `APEX_EDGE_DB` (default `apex_edge.db`); optional metrics handle for `/metrics`.
- **Outputs:** HTTP server on port 3000; DB migrated; routes and shared state wired.
- **Failure path:** Pool or migration failure exits main; server bind failure propagates.

### 3. HTTP Surface (Routes and Owners)

**Purpose:** Map every HTTP route to handler and owner crate/module for tracing and metrics ownership.

```mermaid
flowchart TB
    subgraph routes [HTTP Routes]
        R1["GET /health"]
        R2["GET /ready"]
        R3["POST /pos/command"]
        R4["GET /catalog/products"]
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

**Purpose:** Background cycle: poll pending outbox, POST to HQ, accepted vs retry vs dead-letter; backoff at high level.

```mermaid
flowchart TB
    Start([run_once]) --> Fetch[fetch_pending_outbox pool, limit 10]
    Fetch --> Loop{for each row}
    Loop --> POST[POST row.payload to HQ submit URL]
    POST --> Result{response?}
    Result -->|success + accepted| MarkDelivered[mark_delivered]
    Result -->|success + !accepted or non-2xx or network error| CheckAttempts{attempts >= 10?}
    CheckAttempts -->|Yes| DLQ[mark_dead_letter]
    CheckAttempts -->|No| Retry[schedule_retry with backoff]
    MarkDelivered --> Loop
    Retry --> Loop
    DLQ --> Loop
    Loop --> Done([return processed count])
```

**Notes:**
- **Inputs:** `pool`, HTTP `client`, `hq_submit_url`; pending rows from `apex_edge_storage::outbox`.
- **Outputs:** Rows marked delivered when HQ returns success and `accepted`; retry scheduled with exponential backoff (capped); DLQ when `MAX_ATTEMPTS` (10) reached.
- **Failure path:** Network or non-success response increments attempts; after 10 attempts row is marked dead-letter; otherwise `schedule_retry` with backoff.

### 7. Sync Ingest Flow

**Purpose:** Batch ingest with per-entity checkpoint and conflict-policy placeholder.

```mermaid
flowchart LR
    Ingest[ingest_batch] --> GetCP[get_sync_checkpoint entity]
    GetCP --> NextSeq[next_seq = current + payloads.len]
    NextSeq --> SetCP[set_sync_checkpoint entity next_seq]
    SetCP --> Return([Ok next_seq])
```

**Notes:**
- **Inputs:** `pool`, `entity` (e.g. catalog), `ContractVersion`, `payloads` (batch), `ConflictPolicy` (e.g. HqWins). Checkpoint read/write via `apex_edge_storage`.
- **Outputs:** New checkpoint value (sequence) returned; checkpoint advanced atomically per batch.
- **Failure path:** Storage or invalid payload returns `IngestError`; conflict policy is accepted but not yet applied in current ingest (placeholder for future behavior).

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
    end
    subgraph owners [Owner Crate / Module]
        O1[apex_edge_api / health]
        O2[apex_edge_api / health]
        O3[apex_edge_api / pos]
        O4[apex_edge_api / documents]
        O5[apex_edge_api / documents]
        O6[apex_edge_outbox / dispatcher]
        O7[apex_edge_sync / ingest]
    end
    B1 --> O1
    B2 --> O2
    B3 --> O3
    B4 --> O4
    B5 --> O5
    B6 --> O6
    B7 --> O7
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
    User->>UI: Add product, Set customer, Checkout
    UI->>API: POST /pos/command create_cart, add_line_item, set_customer, set_tendering, add_payment, finalize_order
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
- **POS commands:** `create_cart`, `add_line_item` (optional `unit_price_override_cents` for positive price override), `set_customer`, `apply_manual_discount` (reason mandatory; kinds: percent_cart, percent_item, fixed_cart, fixed_item), `set_tendering`, `add_payment`, `finalize_order`. Promotions (coupons and automatic) are seeded and applied in pipeline; manual discounts applied after promos and included in order metadata to HQ.
- **Layout:** Mobile-first, app-like UI: fixed bottom tab bar (Customers / Catalog / Sync / Cart) with safe-area insets; 44px minimum touch targets; full viewport height (`100dvh`). At 768px+ nav moves to header; at 1024px (e.g. iPad landscape) content is constrained with larger catalog grid. Event log shown from 768px only.
- **Scope:** Simulator runs as a separate dev server (e.g. Vite on port 5173); CORS enabled. Local use only.

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
- **Example sync source:** Separate binary `tools/example-sync-source`; serves NDJSON per entity (first line `{"total": N}`, then N lines of base64 payload). Contract-only coupling; no app runtime dependencies. Run with `cargo run -p example-sync-source` (default port 3030; `SYNC_SOURCE_PORT` env).
- **ApexEdge sync:** When `APEX_EDGE_SYNC_SOURCE_URL` is set, main runs sync once on startup then spawns a 24h periodic task. `run_sync_ndjson` streams each entity (line-by-line), collects payloads per entity, ingests in batch, advances checkpoints, and updates latest sync run + per-entity status in storage.
- **Sync status:** Stored in `sync_run` (single row) and `entity_sync_status`; exposed at `GET /sync/status`. Frontend Sync tab shows last sync time, run state (idle/syncing), and per-entity progress (current, total, percent, status).
- **Failure path:** Sync errors are logged; latest run is marked `failed` with error message; next scheduled run proceeds after 24h.
