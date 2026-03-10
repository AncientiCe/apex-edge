import { useState } from 'react';
import type { CartState, ProductSearchResult, CustomerSearchResult } from '../api/types';
import type { PosCommand } from '../api/types';

interface Props {
  cartId: string | null;
  cartState: CartState | null;
  products: ProductSearchResult[];
  customers: CustomerSearchResult[];
  onPosCommand: (cmd: PosCommand) => void;
}

export function CartPanel({ cartId, cartState, products, customers, onPosCommand }: Props) {
  const [selectedProductId, setSelectedProductId] = useState<string>('');
  const [selectedCustomerId, setSelectedCustomerId] = useState<string>('');
  const [quantity, setQuantity] = useState(1);

  const handleCreateCart = () => {
    onPosCommand({
      action: 'create_cart',
      payload: { cart_id: null },
    });
  };

  const handleAddLineItem = () => {
    if (!cartId || !selectedProductId) return;
    onPosCommand({
      action: 'add_line_item',
      payload: {
        cart_id: cartId,
        item_id: selectedProductId,
        modifier_option_ids: [],
        quantity,
        notes: null,
      },
    });
  };

  const handleSetCustomer = () => {
    if (!cartId || !selectedCustomerId) return;
    onPosCommand({
      action: 'set_customer',
      payload: { cart_id: cartId, customer_id: selectedCustomerId },
    });
  };

  return (
    <section className="panel cart">
      <h2>Cart</h2>
      <div className="row">
        <button type="button" onClick={handleCreateCart} disabled={!!cartId && cartState?.state !== 'finalized' && cartState?.state !== 'voided'}>
          Create cart
        </button>
        {cartId && <span className="status">cart: {cartId.slice(0, 8)}…</span>}
      </div>
      {cartState && (
        <>
          <div className="row">
            <span className="status">state: {cartState.state}</span>
            <span className="status">total: {(cartState.total_cents / 100).toFixed(2)}</span>
          </div>
          {cartState.lines.length > 0 && (
            <div className="pre" style={{ marginBottom: '0.5rem', fontSize: '0.875rem' }}>
              {cartState.lines.map((l) => (
                <div key={l.line_id}>
                  {l.name} × {l.quantity} — {(l.line_total_cents / 100).toFixed(2)}
                </div>
              ))}
            </div>
          )}
          <div className="row">
            <select
              value={selectedProductId}
              onChange={(e) => setSelectedProductId(e.target.value)}
              aria-label="Product"
            >
              <option value="">Select product</option>
              {products.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.sku} — {p.name}
                </option>
              ))}
            </select>
            <input
              type="number"
              min={1}
              value={quantity}
              onChange={(e) => setQuantity(parseInt(e.target.value, 10) || 1)}
              style={{ width: 56 }}
            />
            <button type="button" onClick={handleAddLineItem} disabled={!selectedProductId}>
              Add line
            </button>
          </div>
          <div className="row">
            <select
              value={selectedCustomerId}
              onChange={(e) => setSelectedCustomerId(e.target.value)}
              aria-label="Customer"
            >
              <option value="">Select customer</option>
              {customers.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.code} — {c.name}
                </option>
              ))}
            </select>
            <button type="button" onClick={handleSetCustomer} disabled={!selectedCustomerId}>
              Set customer
            </button>
          </div>
        </>
      )}
    </section>
  );
}
