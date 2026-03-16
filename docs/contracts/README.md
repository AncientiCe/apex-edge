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
