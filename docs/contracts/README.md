# ApexEdge Contracts

Canonical message and data shapes for POS <-> ApexEdge and ApexEdge <-> HQ.

Related: [README](../../README.md) · [Architecture](../architecture/README.md) · [Runbook](../runbook/README.md)

## Versioning

- **Semver**: `major.minor.patch`. Breaking changes bump major.
- **Compatibility**: Additive-only for minor/patch; new optional fields allowed.
- **Headers**: All request/response envelopes carry `version`.

## POS <-> ApexEdge

- **Envelope**: `PosRequestEnvelope<T>` with `version`, `idempotency_key`, `store_id`, `register_id`, `payload`.
- **Commands**: `PosCommand` enum (CreateCart, AddLineItem, ApplyPromo, ApplyCoupon, AddPayment, FinalizeOrder, etc.).
- **Responses**: `PosResponseEnvelope<T>` with `success`, `payload`, `errors`.
- **Cart state**: `CartState` with lines, totals, applied promos/coupons, state kind.

## Documents Northbound Contract

- **List**: `GET /orders/:order_id/documents` returns `[{ id, type, document_type, status, mime_type, created_at, completed_at }]`.
- **Get**: `GET /documents/:id` returns `{ id, type, document_type, status, mime_type, content, error_message }`.
- **Canonical types**: both `type` and `document_type` return canonical POS-facing values:
  - internal `customer_receipt` or `receipt` => `sales_receipt`
  - internal `gift_receipt` => `gift_receipt`
  - unknown values pass through unchanged
- **Content**: PDF payloads are base64-encoded in `content` when `mime_type=application/pdf`.

## Edge Auth (mPOS -> ApexEdge)

- **Pairing**: `AuthCreatePairingCodeRequest/Response`, `AuthDevicePairRequest/Response`.
- **Session exchange**: `AuthSessionExchangeRequest` exchanges an external associate token + trusted device proof for hub `access_token` and `refresh_token`.
- **Session lifecycle**: `AuthSessionRefreshRequest`, `AuthSessionRevokeResponse`.
- **Security model**: one-time pairing code enrollment for devices; additive contracts only.

## ApexEdge -> HQ

- **Order submission**: `HqOrderSubmissionEnvelope` with `submission_id`, `store_id`, `sequence_number`, `order`, `checksum`, `submitted_at`.
- **Payload**: `HqOrderPayload` (order_id, lines, totals, payments, coupons).
- **Response**: `HqOrderSubmissionResponse` (accepted, hq_order_ref, errors).
- **Idempotency**: HQ must accept same `submission_id` idempotently.

## Sync (HQ -> ApexEdge)

- Catalog, PriceBook, TaxRule, Promotion, CouponDefinition, StoreConfig, RegisterConfig, TenderType, PrintTemplateConfig.
- Each entity has `version` for conflict policy (HQWins / EdgeWins / MergeRules).

## Financial safety and approval authorization

- Concurrent copies of one envelope key are serialized in the active hub. Retrying an uncertain refund must reuse the original key: its durable reservation binds the return, tender and amount, and the provider receives the same refund identity. Do not issue a replacement command to work around an uncertain outcome.
- Receipted return lines identify the original line (or an unambiguous original SKU) and quantity. The hub derives paid value, discounts, tax and tax inclusion from the original ledger; client monetary fields do not override it. Quantities are reserved across return documents. Refunded or refund-pending returns cannot be voided.
- Approval requester and decision actor come from authentication. Legacy requested_by and approver_id fields do not override the authenticated identity. Decisions require a manager, supervisor or admin in the verified external token's roles array, or a stored API token scope permitting approvals. Missing authentication returns 401; insufficient privilege, wrong-store access and self-approval return 403. The path approval id must match the body id.
