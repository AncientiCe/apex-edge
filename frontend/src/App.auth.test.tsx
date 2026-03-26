import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import App from './App';

const mockApi = vi.hoisted(() => {
  return {
    getHealth: vi.fn(),
    getReady: vi.fn(),
    getProductById: vi.fn(),
    listProducts: vi.fn(),
    listCategories: vi.fn(),
    searchCustomers: vi.fn(),
    postPosCommand: vi.fn(),
    getCartState: vi.fn(),
    listOrderDocuments: vi.fn(),
    getDocument: vi.fn(),
    createGiftReceipt: vi.fn(),
    buildEnvelope: vi.fn((storeId: string, registerId: string, command: unknown) => ({
      version: { major: 1, minor: 0, patch: 0 },
      idempotency_key: crypto.randomUUID(),
      store_id: storeId,
      register_id: registerId,
      payload: command,
    })),
    configureAuthTransport: vi.fn(),
    createPairingCode: vi.fn(),
    pairDevice: vi.fn(),
    exchangeSession: vi.fn(),
    refreshSession: vi.fn(),
    revokeSession: vi.fn(),
    generateMockExternalToken: vi.fn(),
    startJourneyTracking: vi.fn(),
    stopJourneyTracking: vi.fn(() => ({
      totalRequests: 7,
      localRequests: 5,
      nonLocalRequests: 2,
      failedRequests: 1,
      totalLatencyMs: 123,
    })),
    getJourneySummary: vi.fn(() => ({
      totalRequests: 0,
      localRequests: 0,
      nonLocalRequests: 0,
      failedRequests: 0,
      totalLatencyMs: 0,
    })),
    resetJourneyTracking: vi.fn(),
  };
});

vi.mock('./api/client', () => mockApi);

describe('App auth gate', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockApi.getCartState.mockResolvedValue(null);
    mockApi.postPosCommand.mockResolvedValue({
      version: { major: 1, minor: 0, patch: 0 },
      success: true,
      idempotency_key: crypto.randomUUID(),
      payload: {
        cart_id: 'cart-1',
        customer_id: null,
        state: 'open',
        lines: [],
        applied_promos: [],
        applied_coupons: [],
        subtotal_cents: 0,
        discount_cents: 0,
        tax_cents: 0,
        total_cents: 0,
        tendered_cents: 0,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      },
      errors: [],
    });
    mockApi.createPairingCode.mockResolvedValue({
      pairing_code_id: crypto.randomUUID(),
      code: '123456',
      expires_at: new Date(Date.now() + 60_000).toISOString(),
    });
    mockApi.pairDevice.mockResolvedValue({
      device_id: crypto.randomUUID(),
      device_secret: 'device-secret',
    });
    mockApi.generateMockExternalToken.mockResolvedValue('external-token');
    mockApi.exchangeSession.mockResolvedValue({
      access_token: 'hub-access',
      refresh_token: 'hub-refresh',
      expires_at: new Date(Date.now() + 60_000).toISOString(),
      refresh_expires_at: new Date(Date.now() + 3_600_000).toISOString(),
    });
    mockApi.revokeSession.mockResolvedValue({ revoked: true });
    mockApi.listProducts.mockResolvedValue({
      items: [
        {
          id: 'prod-1',
          sku: 'sku-1',
          name: 'Demo Product',
          description: null,
          image_urls: [],
          category_id: null,
          category_name: null,
          unit_price_cents: 500,
          available_qty: 10,
          is_active: true,
          is_preorder: false,
        },
      ],
      total: 1,
      page: 1,
      per_page: 24,
    });
    mockApi.listCategories.mockResolvedValue([]);
    mockApi.searchCustomers.mockResolvedValue([]);
    mockApi.listOrderDocuments.mockResolvedValue([]);
  });

  it('blocks protected UI before sign in in strict mode', () => {
    render(<App />);
    expect(screen.getByText(/authentication required/i)).toBeInTheDocument();
  });

  it('unlocks app after pair and sign in', async () => {
    render(<App />);
    fireEvent.change(screen.getByLabelText(/mock token secret/i), {
      target: { value: 'test-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: /pair & sign in/i }));
    await waitFor(() =>
      expect(mockApi.createPairingCode).toHaveBeenCalledTimes(1)
    );
    await waitFor(() =>
      expect(
        screen.queryByText(/authentication required/i)
      ).not.toBeInTheDocument()
    );
    await waitFor(() =>
      expect(mockApi.startJourneyTracking).toHaveBeenCalledTimes(1)
    );
  });

  it('sign out revokes session and locks app again', async () => {
    render(<App />);
    fireEvent.change(screen.getByLabelText(/mock token secret/i), {
      target: { value: 'test-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: /pair & sign in/i }));
    await waitFor(() =>
      expect(
        screen.queryByText(/authentication required/i)
      ).not.toBeInTheDocument()
    );
    fireEvent.click(screen.getByRole('button', { name: /sign out/i }));
    await waitFor(() => expect(mockApi.revokeSession).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByText(/authentication required/i)).toBeInTheDocument()
    );
  });

  it('stops journey tracking at sale complete and renders http summary totals', async () => {
    mockApi.postPosCommand.mockImplementation(
      async (_baseUrl: string, envelope: { payload: { action: string } }) => {
        const action = envelope.payload.action;
        if (action === 'create_cart') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: 'cart-1',
              customer_id: null,
              state: 'open',
              lines: [],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 0,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 0,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'add_line_item' || action === 'set_tendering') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: 'cart-1',
              customer_id: null,
              state: 'open',
              lines: [{
                line_id: 'line-1',
                item_id: 'prod-1',
                sku: 'sku-1',
                name: 'Demo Product',
                quantity: 1,
                unit_price_cents: 500,
                line_total_cents: 500,
                discount_cents: 0,
                tax_cents: 0,
                modifier_option_ids: [],
                notes: null,
              }],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 500,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 500,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'add_payment') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: 'cart-1',
              customer_id: null,
              state: 'paid',
              lines: [{
                line_id: 'line-1',
                item_id: 'prod-1',
                sku: 'sku-1',
                name: 'Demo Product',
                quantity: 1,
                unit_price_cents: 500,
                line_total_cents: 500,
                discount_cents: 0,
                tax_cents: 0,
                modifier_option_ids: [],
                notes: null,
              }],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 500,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 500,
              tendered_cents: 500,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'finalize_order') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              order_id: '11111111-1111-1111-1111-111111111111',
              total_cents: 500,
            },
            errors: [],
          };
        }
        return {
          version: { major: 1, minor: 0, patch: 0 },
          success: false,
          idempotency_key: crypto.randomUUID(),
          payload: null,
          errors: [{ code: 'UNEXPECTED', message: `unexpected action ${action}` }],
        };
      }
    );

    render(<App />);
    fireEvent.change(screen.getByLabelText(/mock token secret/i), {
      target: { value: 'test-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: /pair & sign in/i }));
    await waitFor(() => expect(mockApi.createPairingCode).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(
        screen.queryByText(/authentication required/i)
      ).not.toBeInTheDocument()
    );

    fireEvent.click(screen.getByRole('button', { name: /go to catalog/i }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /add demo product to cart/i })).toBeInTheDocument()
    );
    fireEvent.click(screen.getByRole('button', { name: /add demo product to cart/i }));

    const nav = screen.getByRole('navigation', { name: /main/i });
    fireEvent.click(within(nav).getByRole('button', { name: /cart/i }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /pay \$5\.00/i })).toBeInTheDocument()
    );
    fireEvent.click(screen.getByRole('button', { name: /pay \$5\.00/i }));

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /add payment/i })).toBeInTheDocument()
    );
    fireEvent.change(screen.getByPlaceholderText(/enter cash amount/i), {
      target: { value: '5' },
    });
    fireEvent.click(screen.getByRole('button', { name: /add payment/i }));

    await waitFor(() => expect(screen.getByText(/sale complete/i)).toBeInTheDocument());
    await waitFor(() => expect(mockApi.stopJourneyTracking).toHaveBeenCalledTimes(1));
    expect(screen.getByText(/^Total requests$/i)).toBeInTheDocument();
    expect(screen.getByText(/^Local requests$/i)).toBeInTheDocument();
    expect(screen.getByText(/^Non-local requests$/i)).toBeInTheDocument();
    expect(screen.getByText(/^Failed requests$/i)).toBeInTheDocument();
    expect(screen.getByText(/^Total latency$/i)).toBeInTheDocument();
  });

  it('keeps attached customer when product is added right after applying customer', async () => {
    const customer = {
      id: 'cust-1',
      code: 'CUST01',
      name: 'Demo Customer',
      email: 'demo@example.com',
    };
    const cartCustomer = new Map<string, string | null>();
    let createCartCalls = 0;

    mockApi.searchCustomers.mockImplementation(async (_baseUrl: string, q: string) => {
      if (q.toLowerCase().includes('demo') || q === customer.id) return [customer];
      return [];
    });

    mockApi.postPosCommand.mockImplementation(
      async (_baseUrl: string, envelope: { payload: { action: string; payload: Record<string, unknown> } }) => {
        const action = envelope.payload.action;
        const payload = envelope.payload.payload;
        if (action === 'create_cart') {
          createCartCalls += 1;
          const cartId = createCartCalls === 1 ? 'cart-1' : `cart-${createCartCalls}`;
          cartCustomer.set(cartId, null);
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: cartId,
              customer_id: null,
              state: 'open',
              lines: [],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 0,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 0,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'set_customer') {
          const cartId = String(payload.cart_id);
          const customerId = String(payload.customer_id);
          cartCustomer.set(cartId, customerId);
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: cartId,
              customer_id: customerId,
              state: 'open',
              lines: [],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 0,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 0,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'add_line_item') {
          const cartId = String(payload.cart_id);
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: cartId,
              customer_id: cartCustomer.get(cartId) ?? null,
              state: 'itemized',
              lines: [{
                line_id: 'line-1',
                item_id: 'prod-1',
                sku: 'sku-1',
                name: 'Demo Product',
                quantity: 1,
                unit_price_cents: 500,
                line_total_cents: 500,
                discount_cents: 0,
                tax_cents: 0,
                modifier_option_ids: [],
                notes: null,
              }],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 500,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 500,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        return {
          version: { major: 1, minor: 0, patch: 0 },
          success: true,
          idempotency_key: crypto.randomUUID(),
          payload: null,
          errors: [],
        };
      }
    );

    render(<App />);
    fireEvent.change(screen.getByLabelText(/mock token secret/i), {
      target: { value: 'test-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: /pair & sign in/i }));
    await waitFor(() =>
      expect(screen.queryByText(/authentication required/i)).not.toBeInTheDocument()
    );

    fireEvent.change(screen.getByLabelText(/search customer/i), {
      target: { value: 'demo' },
    });
    fireEvent.keyDown(screen.getByLabelText(/search customer/i), { key: 'Enter' });
    await waitFor(() => expect(screen.getByText(/Demo Customer/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /demo customer/i }));

    fireEvent.click(screen.getByRole('button', { name: /apply customer to cart/i }));
    fireEvent.click(screen.getByRole('button', { name: /go to catalog/i }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /add demo product to cart/i })).toBeInTheDocument()
    );
    fireEvent.click(screen.getByRole('button', { name: /add demo product to cart/i }));

    const nav = screen.getByRole('navigation', { name: /main/i });
    fireEvent.click(within(nav).getByRole('button', { name: /cart/i }));
    await waitFor(() => expect(screen.getByText(/Demo Customer/i)).toBeInTheDocument());
  });

  it('does not drop attached customer banner when add-line response omits customer_id', async () => {
    const customer = {
      id: 'cust-1',
      code: 'CUST01',
      name: 'Demo Customer',
      email: 'demo@example.com',
    };

    mockApi.searchCustomers.mockImplementation(async (_baseUrl: string, q: string) => {
      if (q.toLowerCase().includes('demo') || q === customer.id) return [customer];
      return [];
    });

    mockApi.postPosCommand.mockImplementation(
      async (_baseUrl: string, envelope: { payload: { action: string; payload: Record<string, unknown> } }) => {
        const action = envelope.payload.action;
        const payload = envelope.payload.payload;
        if (action === 'create_cart') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: 'cart-1',
              customer_id: null,
              state: 'open',
              lines: [],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 0,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 0,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'set_customer') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: 'cart-1',
              customer_id: String(payload.customer_id),
              state: 'open',
              lines: [],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 0,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 0,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        if (action === 'add_line_item') {
          return {
            version: { major: 1, minor: 0, patch: 0 },
            success: true,
            idempotency_key: crypto.randomUUID(),
            payload: {
              cart_id: 'cart-1',
              customer_id: null,
              state: 'itemized',
              lines: [{
                line_id: 'line-1',
                item_id: 'prod-1',
                sku: 'sku-1',
                name: 'Demo Product',
                quantity: 1,
                unit_price_cents: 500,
                line_total_cents: 500,
                discount_cents: 0,
                tax_cents: 0,
                modifier_option_ids: [],
                notes: null,
              }],
              applied_promos: [],
              applied_coupons: [],
              subtotal_cents: 500,
              discount_cents: 0,
              tax_cents: 0,
              total_cents: 500,
              tendered_cents: 0,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
            errors: [],
          };
        }
        return {
          version: { major: 1, minor: 0, patch: 0 },
          success: true,
          idempotency_key: crypto.randomUUID(),
          payload: null,
          errors: [],
        };
      }
    );

    render(<App />);
    fireEvent.change(screen.getByLabelText(/mock token secret/i), {
      target: { value: 'test-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: /pair & sign in/i }));
    await waitFor(() =>
      expect(screen.queryByText(/authentication required/i)).not.toBeInTheDocument()
    );

    fireEvent.change(screen.getByLabelText(/search customer/i), {
      target: { value: 'demo' },
    });
    fireEvent.keyDown(screen.getByLabelText(/search customer/i), { key: 'Enter' });
    await waitFor(() => expect(screen.getByText(/Demo Customer/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /demo customer/i }));
    fireEvent.click(screen.getByRole('button', { name: /apply customer to cart/i }));
    fireEvent.click(screen.getByRole('button', { name: /go to catalog/i }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /add demo product to cart/i })).toBeInTheDocument()
    );
    fireEvent.click(screen.getByRole('button', { name: /add demo product to cart/i }));

    const nav = screen.getByRole('navigation', { name: /main/i });
    fireEvent.click(within(nav).getByRole('button', { name: /cart/i }));
    await waitFor(() => expect(screen.getByText(/Demo Customer/i)).toBeInTheDocument());
  });
});
