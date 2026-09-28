import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { StoreOpsPanel, type LastSale } from './StoreOpsPanel';
import type { CartState, PosCommand, PosResponseEnvelope } from '../api/types';

const V = { major: 1, minor: 0, patch: 0 };

function ok(payload: unknown): PosResponseEnvelope<unknown> {
  return { version: V, success: true, idempotency_key: 'k', payload, errors: [] };
}

function fail(code: string): PosResponseEnvelope<unknown> {
  return {
    version: V,
    success: false,
    idempotency_key: 'k',
    payload: null,
    errors: [{ code, message: code, field: null }],
  };
}

function cart(id: string): CartState {
  return {
    cart_id: id,
    customer_id: null,
    state: 'itemized',
    lines: [],
    applied_promos: [],
    applied_coupons: [],
    subtotal_cents: 0,
    discount_cents: 0,
    tax_cents: 0,
    total_cents: 0,
    tendered_cents: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

const lastSale: LastSale = {
  orderId: 'order-1',
  totalCents: 2420,
  lines: [
    { sku: 'SKU-1', name: 'Coffee', quantity: 2, unit_price_cents: 1000, tax_cents: 200 },
    { sku: 'SKU-2', name: 'Cake', quantity: 1, unit_price_cents: 200, tax_cents: 20 },
  ],
};

type PanelProps = Parameters<typeof StoreOpsPanel>[0];

/** Holds the shift the way App does, so open/close round-trips through the parent. */
function Harness(props: PanelProps) {
  const [shiftId, setShiftId] = useState(props.shiftId);
  return (
    <StoreOpsPanel
      {...props}
      shiftId={shiftId}
      onShiftChange={(id) => {
        props.onShiftChange(id);
        setShiftId(id);
      }}
    />
  );
}

function renderPanel(
  send: (c: PosCommand) => Promise<PosResponseEnvelope<unknown> | null>,
  overrides: Partial<PanelProps> = {}
) {
  const props = {
    send,
    cartId: null,
    lastSale: null,
    shiftId: null,
    onShiftChange: vi.fn(),
    onCartParked: vi.fn(),
    onCartRecalled: vi.fn(),
    ...overrides,
  };
  render(<Harness {...props} />);
  return props;
}

describe('StoreOpsPanel till', () => {
  it('opens a till with the float in cents, then runs an X report for that shift', async () => {
    const send = vi.fn(async (c: PosCommand) => {
      if (c.action === 'open_till') return ok({ shift_id: 'shift-1', state: 'open' });
      if (c.action === 'get_x_report') return ok({ expected_cents: 5250, cash_sales_cents: 250 });
      return null;
    });
    renderPanel(send);

    expect(screen.getByRole('button', { name: 'X Report' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Opening float'), { target: { value: '50.00' } });
    fireEvent.click(screen.getByRole('button', { name: 'Open Till' }));

    await screen.findByText('shift-1');
    expect(send).toHaveBeenCalledWith({
      action: 'open_till',
      payload: { register_id: null, associate_id: null, opening_float_cents: 5000 },
    });

    fireEvent.click(screen.getByRole('button', { name: 'X Report' }));
    await screen.findByText('$52.50');
    expect(send).toHaveBeenCalledWith({ action: 'get_x_report', payload: { shift_id: 'shift-1' } });
  });

  it('closes the till with the counted cash and shows the variance', async () => {
    const send = vi.fn(async (c: PosCommand) => {
      if (c.action === 'open_till') return ok({ shift_id: 'shift-1' });
      if (c.action === 'close_till')
        return ok({ state: 'closed', expected_cents: 5000, variance_cents: -500 });
      return null;
    });
    renderPanel(send);
    fireEvent.click(screen.getByRole('button', { name: 'Open Till' }));
    await screen.findByText('shift-1');

    fireEvent.change(screen.getByLabelText('Counted cash'), { target: { value: '45' } });
    fireEvent.click(screen.getByRole('button', { name: 'Close Till' }));

    await screen.findByText('−$5.00');
    expect(send).toHaveBeenCalledWith({
      action: 'close_till',
      payload: { shift_id: 'shift-1', counted_cents: 4500, approval_id: null },
    });
  });
});

describe('StoreOpsPanel shift recovery', () => {
  it('picks up the shift the hub says is already open for this register', async () => {
    const send = vi.fn(async () => ({
      ...fail('SHIFT_ALREADY_OPEN'),
      payload: { shift_id: 'shift-old' },
    }));
    const props = renderPanel(send);

    fireEvent.click(screen.getByRole('button', { name: 'Open Till' }));
    await screen.findByText('shift-old');
    expect(props.onShiftChange).toHaveBeenCalledWith('shift-old');
  });
});

describe('StoreOpsPanel shift across tab switches', () => {
  it('keeps using a shift the app remembers after the panel is remounted', async () => {
    const send = vi.fn(async () => ok({ expected_cents: 10000 }));
    const props = renderPanel(send, { shiftId: 'shift-9' });

    expect(screen.getByText('shift-9')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'X Report' }));
    await screen.findByText('$100.00');
    expect(send).toHaveBeenCalledWith({ action: 'get_x_report', payload: { shift_id: 'shift-9' } });
    expect(props.onShiftChange).not.toHaveBeenCalled();
  });

  it('reports the shift to the app when opened and clears it when closed', async () => {
    const send = vi.fn(async (c: PosCommand) =>
      c.action === 'open_till' ? ok({ shift_id: 'shift-1' }) : ok({ state: 'closed', variance_cents: 0 })
    );
    const onShiftChange = vi.fn();
    const { rerender } = render(
      <StoreOpsPanel
        send={send}
        cartId={null}
        lastSale={null}
        shiftId={null}
        onShiftChange={onShiftChange}
        onCartParked={vi.fn()}
        onCartRecalled={vi.fn()}
      />
    );
    fireEvent.click(screen.getByRole('button', { name: 'Open Till' }));
    await waitFor(() => expect(onShiftChange).toHaveBeenCalledWith('shift-1'));

    rerender(
      <StoreOpsPanel
        send={send}
        cartId={null}
        lastSale={null}
        shiftId="shift-1"
        onShiftChange={onShiftChange}
        onCartParked={vi.fn()}
        onCartRecalled={vi.fn()}
      />
    );
    fireEvent.click(screen.getByRole('button', { name: 'Close Till' }));
    await waitFor(() => expect(onShiftChange).toHaveBeenLastCalledWith(null));
  });
});

describe('StoreOpsPanel parked carts', () => {
  it('cannot park without a cart', () => {
    renderPanel(vi.fn(async () => null));
    expect(screen.getByRole('button', { name: 'Park Current Cart' })).toBeDisabled();
  });

  it('parks the current cart, lists parked carts, and recalls one', async () => {
    const send = vi.fn(async (c: PosCommand) => {
      if (c.action === 'park_cart') return ok({ parked_cart_id: 'p-1' });
      if (c.action === 'list_parked_carts')
        return ok([
          { parked_cart_id: 'p-1', cart_id: 'cart-1', note: 'ATM', total_cents: 1000, line_count: 1 },
        ]);
      if (c.action === 'recall_cart') return ok(cart('cart-1'));
      return null;
    });
    const props = renderPanel(send, { cartId: 'cart-1' });

    fireEvent.change(screen.getByLabelText('Park note'), { target: { value: 'ATM' } });
    fireEvent.click(screen.getByRole('button', { name: 'Park Current Cart' }));
    await waitFor(() => expect(props.onCartParked).toHaveBeenCalled());
    expect(send).toHaveBeenCalledWith({
      action: 'park_cart',
      payload: { cart_id: 'cart-1', note: 'ATM' },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Refresh Parked' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Recall ATM' }));
    await waitFor(() => expect(props.onCartRecalled).toHaveBeenCalledWith(cart('cart-1')));
    expect(send).toHaveBeenCalledWith({ action: 'recall_cart', payload: { parked_cart_id: 'p-1' } });
  });
});

describe('StoreOpsPanel returns', () => {
  it('is disabled until there is a completed sale to return', () => {
    renderPanel(vi.fn(async () => null));
    expect(screen.getByRole('button', { name: 'Return Last Sale' })).toBeDisabled();
  });

  it('returns every line of the last sale against the original order and refunds in cash', async () => {
    const send = vi.fn(async (c: PosCommand) => {
      switch (c.action) {
        case 'start_return':
          return ok({ id: 'ret-1', state: 'open' });
        case 'return_line_item':
          return ok({ id: 'ret-1', total_cents: 2420 });
        case 'refund_tender':
          return ok({ id: 'ret-1', state: 'paid' });
        case 'finalize_return':
          return ok({ id: 'ret-1', state: 'finalized' });
        default:
          return null;
      }
    });
    renderPanel(send, { lastSale });

    fireEvent.click(screen.getByRole('button', { name: 'Return Last Sale' }));
    await screen.findByText('Return ret-1 finalized, refunded $24.20 cash');

    const actions = send.mock.calls.map(([c]) => c.action);
    expect(actions).toEqual([
      'start_return',
      'return_line_item',
      'return_line_item',
      'refund_tender',
      'finalize_return',
    ]);
    expect(send.mock.calls[0][0]).toEqual({
      action: 'start_return',
      payload: {
        return_id: null,
        original_order_id: 'order-1',
        reason_code: 'customer_return',
        approval_id: null,
        shift_id: null,
      },
    });
    expect(send.mock.calls[1][0]).toEqual({
      action: 'return_line_item',
      payload: {
        return_id: 'ret-1',
        sku: 'SKU-1',
        name: 'Coffee',
        quantity: 2,
        unit_price_cents: 1000,
        tax_cents: 200,
        original_line_id: null,
      },
    });
    expect(send.mock.calls[3][0]).toEqual({
      action: 'refund_tender',
      payload: { return_id: 'ret-1', tender_type: 'cash', amount_cents: 2420, external_reference: null },
    });
  });

  it('stops at the first failed step and says which one', async () => {
    const send = vi.fn(async (c: PosCommand) =>
      c.action === 'start_return' ? ok({ id: 'ret-1' }) : fail('RETURN_EXCEEDS_ORIGINAL')
    );
    renderPanel(send, { lastSale });

    fireEvent.click(screen.getByRole('button', { name: 'Return Last Sale' }));
    await screen.findByText('Return failed at return_line_item');
    expect(send).toHaveBeenCalledTimes(2);
  });
});
