import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
});
