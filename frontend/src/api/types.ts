/**
 * Types mirroring apex-edge contracts for POS envelope and commands.
 * Version is sent as kebab-case object: { major, minor, patch }.
 */

export interface ContractVersion {
  major: number;
  minor: number;
  patch: number;
}

export interface PosRequestEnvelope<T> {
  version: ContractVersion;
  idempotency_key: string;
  store_id: string;
  register_id: string;
  payload: T;
}

export interface PosResponseEnvelope<T> {
  version: ContractVersion;
  success: boolean;
  idempotency_key: string;
  payload: T | null;
  errors: PosError[];
}

export interface PosError {
  code: string;
  message: string;
  field: string | null;
}

// Command payloads (only those used by simulator)
export interface CreateCartPayload {
  cart_id?: string | null;
}

export interface SetCustomerPayload {
  cart_id: string;
  customer_id: string;
}

export interface AddLineItemPayload {
  cart_id: string;
  item_id: string;
  modifier_option_ids: string[];
  quantity: number;
  notes?: string | null;
  /** Positive price override (cents per unit); if set, overrides catalog price. */
  unit_price_override_cents?: number | null;
}

export interface RemoveLineItemPayload {
  cart_id: string;
  line_id: string;
}

export interface ApplyCouponPayload {
  cart_id: string;
  coupon_code: string;
}

export interface SetTenderingPayload {
  cart_id: string;
}

export interface AddPaymentPayload {
  cart_id: string;
  tender_id: string;
  amount_cents: number;
  external_reference?: string | null;
}

export interface FinalizeOrderPayload {
  cart_id: string;
}

export type PosCommand =
  | { action: 'create_cart'; payload: CreateCartPayload }
  | { action: 'set_customer'; payload: SetCustomerPayload }
  | { action: 'add_line_item'; payload: AddLineItemPayload }
  | { action: 'remove_line_item'; payload: RemoveLineItemPayload }
  | { action: 'apply_coupon'; payload: ApplyCouponPayload }
  | { action: 'set_tendering'; payload: SetTenderingPayload }
  | { action: 'add_payment'; payload: AddPaymentPayload }
  | { action: 'finalize_order'; payload: FinalizeOrderPayload };

export interface CartState {
  cart_id: string;
  customer_id: string | null;
  state: string;
  lines: CartLine[];
  applied_promos: AppliedPromoInfo[];
  applied_coupons: AppliedCouponInfo[];
  manual_discounts?: ManualDiscountInfo[];
  subtotal_cents: number;
  discount_cents: number;
  tax_cents: number;
  total_cents: number;
  tendered_cents: number;
  created_at: string;
  updated_at: string;
}

export interface AppliedPromoInfo {
  promo_id: string;
  name: string;
  code?: string | null;
}

export interface ManualDiscountInfo {
  reason: string;
  amount_cents: number;
  line_id: string | null;
}

export interface CartLine {
  line_id: string;
  item_id: string;
  sku: string;
  name: string;
  quantity: number;
  unit_price_cents: number;
  line_total_cents: number;
  discount_cents: number;
  tax_cents: number;
  modifier_option_ids: string[];
  notes: string | null;
}

export interface AppliedCouponInfo {
  coupon_id: string;
  code: string;
  discount_cents: number;
}

export interface FinalizeResult {
  order_id: string;
  cart_id: string;
  total_cents: number;
  print_job_ids: string[];
}

export interface ProductSearchResult {
  id: string;
  sku: string;
  name: string;
  category_id: string;
  tax_category_id: string;
  description?: string | null;
}

export interface ProductListResponse {
  items: ProductSearchResult[];
  total: number;
  page: number;
  per_page: number;
}

export interface CategoryResult {
  id: string;
  name: string;
}

export interface CustomerSearchResult {
  id: string;
  code: string;
  name: string;
  email?: string | null;
}

export interface DocumentSummary {
  id: string;
  document_type: string;
  status: string;
  mime_type: string;
  created_at: string;
  completed_at: string | null;
}

export interface DocumentResponse {
  id: string;
  document_type: string;
  status: string;
  mime_type: string;
  content: string | null;
  error_message: string | null;
}

export interface EntitySyncStatusDto {
  entity: string;
  current: number;
  total: number | null;
  percent: number | null;
  last_synced_at: string | null;
  status: string;
}

export interface SyncStatusResponse {
  last_sync_at: string | null;
  is_syncing: boolean;
  entities: EntitySyncStatusDto[];
}
