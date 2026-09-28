import type { CartState } from './api/types';
import type { LastSale } from './panels/StoreOpsPanel';

/** The lines of a finished sale, priced as sold, so the Store Ops panel can return it. */
export function lastSaleFrom(orderId: string, cart: CartState | null): LastSale | null {
  if (!cart || cart.lines.length === 0) return null;
  const lines = cart.lines.map((l) => ({
    sku: l.sku,
    name: l.name,
    quantity: l.quantity,
    unit_price_cents: Math.floor((l.line_total_cents - l.discount_cents) / Math.max(1, l.quantity)),
    tax_cents: l.tax_cents,
    tax_inclusive: l.tax_inclusive ?? false,
  }));
  // Inclusive tax is already inside the price; exclusive tax is owed on top.
  const totalCents = lines.reduce(
    (sum, l) => sum + l.quantity * l.unit_price_cents + (l.tax_inclusive ? 0 : l.tax_cents),
    0
  );
  return { orderId, totalCents, lines };
}
