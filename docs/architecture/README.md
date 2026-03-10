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
        R4["GET /documents/:id"]
        R5["GET /orders/:order_id/documents"]
        R6["GET /metrics"]
    end
    subgraph api [apex_edge_api]
        H[health]
        P[pos]
        D[documents]
        M[metrics_handler]
    end
    R1 --> H
    R2 --> H
    R3 --> P
    R4 --> D
    R5 --> D
    R6 --> M
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
