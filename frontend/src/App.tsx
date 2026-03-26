import { useCallback, useEffect, useMemo, useState } from 'react';
import { BrowserRouter, Route, Routes, useNavigate, useParams } from 'react-router-dom';
import {
  configureAuthTransport,
  createPairingCode,
  pairDevice,
  exchangeSession,
  type JourneyHttpSummary,
  startJourneyTracking,
  stopJourneyTracking,
  revokeSession,
  generateMockExternalToken,
  getHealth,
  getProductById,
  getReady,
  listProducts,
  listCategories,
  searchCustomers,
  postPosCommand,
  getCartState,
  listOrderDocuments,
  getDocument,
  createGiftReceipt,
  buildEnvelope,
  type ApiError,
} from './api/client';
import type {
  CartState,
  FinalizeResult,
  PosResponseEnvelope,
  PosCommand,
  ProductSearchResult,
  CustomerSearchResult,
  CategoryResult,
  ProductListResponse,
  DocumentSummary,
} from './api/types';
import { ConnectionPanel } from './panels/ConnectionPanel';
import { CatalogPanel } from './panels/CatalogPanel';
import { CustomerPanel } from './panels/CustomerPanel';
import { CartPanel } from './panels/CartPanel';
import { EventLogPanel } from './panels/EventLogPanel';
import { SyncStatusPanel } from './panels/SyncStatusPanel';
import { ProductDetailPage } from './panels/ProductDetailPage';

const STORE_ID = '00000000-0000-0000-0000-000000000000';
const REGISTER_ID = '00000000-0000-0000-0000-000000000000';
const LS_CART_ID = 'apex_edge_cart_id';
const DEFAULT_ASSOCIATE_ID = 'associate-1';

export type LogEntry = { ts: string; kind: 'req' | 'res' | 'err'; text: string };
type Stage = 'customers' | 'catalog' | 'cart' | 'pay' | 'summary' | 'sync';
type Toast = { id: number; message: string };

/** Inner PDP route that loads product by ID from URL params. */
function ProductDetailRoute({
  baseUrl,
  onAddProduct,
}: {
  baseUrl: string;
  onAddProduct: (product: ProductSearchResult, quantity: number) => void;
}) {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [product, setProduct] = useState<ProductSearchResult | null | undefined>(undefined);

  useEffect(() => {
    if (!id || !baseUrl) return;
    let cancelled = false;
    getProductById(baseUrl, id)
      .then((p) => { if (!cancelled) setProduct(p); })
      .catch(() => { if (!cancelled) setProduct(null); });
    return () => { cancelled = true; };
  }, [id, baseUrl]);

  return (
    <ProductDetailPage
      product={product ?? null}
      onAddProduct={(p, qty) => {
        onAddProduct(p, qty);
        navigate('/catalog');
      }}
      onBack={() => navigate('/catalog')}
    />
  );
}
type SaleSummary = {
  finalize: FinalizeResult;
  cartSnapshot: CartState | null;
  documents: DocumentSummary[];
  journeyHttp: JourneyHttpSummary;
};

function useEventLog() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const log = useCallback((kind: LogEntry['kind'], text: string) => {
    const ts = new Date().toISOString().slice(11, 23);
    setEntries((prev) => [...prev.slice(-199), { ts, kind, text }]);
  }, []);
  return [entries, log] as const;
}

function AppInner() {
  const navigate = useNavigate();
  const [baseUrl, setBaseUrl] = useState(
    () => import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:3000'
  );
  const [healthStatus, setHealthStatus] = useState<string | null>(null);
  const [readyStatus, setReadyStatus] = useState<string | null>(null);
  const [storeId, setStoreId] = useState<string>(STORE_ID);
  const [associateId, setAssociateId] = useState<string>(DEFAULT_ASSOCIATE_ID);
  const [deviceName, setDeviceName] = useState<string>(
    () => import.meta.env.VITE_AUTH_DEFAULT_DEVICE_NAME ?? 'Simulator iPad'
  );
  const [devicePlatform, setDevicePlatform] = useState<string>('ios');
  const [authIssuer, setAuthIssuer] = useState<string>(
    () => import.meta.env.VITE_AUTH_EXTERNAL_ISSUER ?? 'https://issuer.example'
  );
  const [authAudience, setAuthAudience] = useState<string>(
    () => import.meta.env.VITE_AUTH_EXTERNAL_AUDIENCE ?? 'mpos'
  );
  const [mockTokenSecret, setMockTokenSecret] = useState<string>(
    () => import.meta.env.VITE_AUTH_EXTERNAL_HS256_SECRET ?? ''
  );
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const [deviceSecret, setDeviceSecret] = useState<string | null>(null);
  const [accessToken, setAccessToken] = useState<string | null>(null);
  const [refreshToken, setRefreshToken] = useState<string | null>(null);
  const [expiresAt, setExpiresAt] = useState<string | null>(null);
  const [authReady, setAuthReady] = useState<boolean>(false);
  const [authBusy, setAuthBusy] = useState<boolean>(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [stage, setStage] = useState<Stage>('customers');
  const [cartId, setCartId] = useState<string | null>(
    () => localStorage.getItem(LS_CART_ID)
  );
  const [cartState, setCartState] = useState<CartState | null>(null);
  const [saleSummary, setSaleSummary] = useState<SaleSummary | null>(null);
  const [categories, setCategories] = useState<CategoryResult[]>([]);
  const [productList, setProductList] = useState<ProductListResponse | null>(null);
  const [customers, setCustomers] = useState<CustomerSearchResult[]>([]);
  const [selectedCustomerId, setSelectedCustomerId] = useState<string | null>(null);
  const [attachedCustomer, setAttachedCustomer] = useState<CustomerSearchResult | null>(null);
  const [eventLog, logEvent] = useEventLog();
  const [eventLogOpen, setEventLogOpen] = useState(false);
  const [cashAmount, setCashAmount] = useState('');
  const [toasts, setToasts] = useState<Toast[]>([]);
  const cartItemCount = useMemo(
    () => cartState?.lines.reduce((sum, line) => sum + line.quantity, 0) ?? 0,
    [cartState]
  );

  const pushToast = useCallback((message: string) => {
    const id = Date.now() + Math.floor(Math.random() * 1000);
    setToasts((prev) => [...prev, { id, message }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 2200);
  }, []);

  const clearAuthState = useCallback((reason?: string) => {
    setDeviceId(null);
    setDeviceSecret(null);
    setAccessToken(null);
    setRefreshToken(null);
    setExpiresAt(null);
    setAuthReady(false);
    if (reason) {
      setAuthError(reason);
    }
  }, []);

  useEffect(() => {
    configureAuthTransport({
      getAccessToken: () => accessToken,
      getRefreshToken: () => refreshToken,
      onTokens: (tokens) => {
        setAccessToken(tokens.accessToken);
        setRefreshToken(tokens.refreshToken);
        setExpiresAt(tokens.expiresAt);
        setAuthReady(true);
        setAuthError(null);
      },
      onAuthFailure: (reason) => {
        clearAuthState(`Authentication lost: ${reason}`);
        pushToast('Session expired. Sign in again.');
      },
    });
    return () => configureAuthTransport(null);
  }, [accessToken, clearAuthState, pushToast, refreshToken]);

  const onPairAndSignIn = useCallback(async () => {
    if (!baseUrl) return;
    if (!mockTokenSecret.trim()) {
      setAuthError('Mock token secret is required.');
      return;
    }
    setAuthBusy(true);
    setAuthError(null);
    try {
      logEvent('req', 'POST /auth/pairing-codes');
      const pairing = await createPairingCode(baseUrl, {
        store_id: storeId,
        created_by: 'simulator',
      });
      logEvent('res', `pairing code id=${pairing.pairing_code_id}`);

      logEvent('req', 'POST /auth/devices/pair');
      const paired = await pairDevice(baseUrl, {
        pairing_code: pairing.code,
        store_id: storeId,
        device_name: deviceName,
        platform: devicePlatform || null,
      });
      setDeviceId(paired.device_id);
      setDeviceSecret(paired.device_secret);
      logEvent('res', `device paired id=${paired.device_id}`);

      const external = await generateMockExternalToken({
        associateId,
        issuer: authIssuer,
        audience: authAudience,
        storeId,
        secret: mockTokenSecret,
      });
      logEvent('req', 'POST /auth/sessions/exchange');
      const session = await exchangeSession(baseUrl, {
        external_token: external,
        device_id: paired.device_id,
        device_secret: paired.device_secret,
      });
      setAccessToken(session.access_token);
      setRefreshToken(session.refresh_token);
      setExpiresAt(session.expires_at);
      setAuthReady(true);
      startJourneyTracking('login_succeeded');
      logEvent('res', 'session exchanged');
      pushToast('Authenticated');
    } catch (e) {
      const err = e as ApiError;
      setAuthError(err.message);
      setAuthReady(false);
      logEvent('err', `auth: ${err.message}`);
      pushToast(err.message);
    } finally {
      setAuthBusy(false);
    }
  }, [
    associateId,
    authAudience,
    authIssuer,
    baseUrl,
    deviceName,
    devicePlatform,
    logEvent,
    mockTokenSecret,
    pushToast,
    storeId,
  ]);

  const onSignOut = useCallback(async () => {
    try {
      if (baseUrl && accessToken) {
        logEvent('req', 'POST /auth/sessions/revoke');
        await revokeSession(baseUrl, accessToken);
        logEvent('res', 'session revoked');
      }
    } catch (e) {
      const err = e as ApiError;
      logEvent('err', `revoke: ${err.message}`);
    } finally {
      clearAuthState();
      pushToast('Signed out');
    }
  }, [accessToken, baseUrl, clearAuthState, logEvent, pushToast]);

  const checkHealth = useCallback(async () => {
    logEvent('req', `GET ${baseUrl}/health`);
    try {
      const r = await getHealth(baseUrl);
      setHealthStatus(r.status);
      logEvent('res', `health: ${r.status}`);
    } catch (e) {
      const err = e as ApiError;
      setHealthStatus(`error: ${err.message}`);
      logEvent('err', `health: ${err.message}`);
    }
  }, [baseUrl, logEvent]);

  const checkReady = useCallback(async () => {
    logEvent('req', `GET ${baseUrl}/ready`);
    try {
      const r = await getReady(baseUrl);
      setReadyStatus(r.status);
      logEvent('res', `ready: ${r.status}`);
    } catch (e) {
      const err = e as ApiError;
      setReadyStatus(`error: ${err.message}`);
      logEvent('err', `ready: ${err.message}`);
    }
  }, [baseUrl, logEvent]);

  const onLoadCategories = useCallback(async () => {
    logEvent('req', 'GET /catalog/categories');
    try {
      const list = await listCategories(baseUrl);
      setCategories(list);
      logEvent('res', `categories: ${list.length}`);
    } catch (e) {
      const err = e as ApiError;
      logEvent('err', `categories: ${err.message}`);
      setCategories([]);
    }
  }, [baseUrl, logEvent]);

  const onLoadProducts = useCallback(
    async (params: { q?: string; category_id?: string; page: number }) => {
      logEvent('req', `GET /catalog/products?page=${params.page}`);
      try {
        const res = await listProducts(baseUrl, {
          q: params.q,
          category_id: params.category_id,
          page: params.page,
          per_page: 24,
        });
        setProductList(res);
        logEvent('res', `products: ${res.items.length} (total ${res.total})`);
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `products: ${err.message}`);
        setProductList(null);
      }
    },
    [baseUrl, logEvent]
  );

  const onSearchCustomers = useCallback(
    async (q: string) => {
      logEvent('req', `GET /customers?q=${q}`);
      try {
        const list = await searchCustomers(baseUrl, q);
        setCustomers(list);
        logEvent('res', `customers: ${list.length}`);
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `customers: ${err.message}`);
        setCustomers([]);
      }
    },
    [baseUrl, logEvent]
  );

  const fetchSummaryDocuments = useCallback(
    async (orderId: string) => {
      try {
        const docs = await listOrderDocuments(baseUrl, orderId);
        setSaleSummary((prev) =>
          prev && prev.finalize.order_id === orderId ? { ...prev, documents: docs } : prev
        );
      } catch {
        // Best-effort only; summary still works without auto-loaded docs.
      }
    },
    [baseUrl]
  );

  const sendPosCommand = useCallback(
    async (command: PosCommand): Promise<PosResponseEnvelope<CartState | FinalizeResult | null> | null> => {
      const envelope = buildEnvelope(storeId, REGISTER_ID, command);
      logEvent('req', `POST /pos/command ${(command as { action: string }).action}`);
      try {
        const res = await postPosCommand(baseUrl, envelope);
        if (res.success && res.payload) {
          if ('order_id' in res.payload) {
            const finalize = res.payload as FinalizeResult;
            const journeyHttp = stopJourneyTracking('sale_complete');
            setSaleSummary({
              finalize,
              cartSnapshot: cartState,
              documents: [],
              journeyHttp,
            });
            setCartId(null);
            setCartState(null);
            setStage('summary');
            console.info(
              `[journey-http] total=${journeyHttp.totalRequests} local=${journeyHttp.localRequests} non_local=${journeyHttp.nonLocalRequests} failed=${journeyHttp.failedRequests} latency_ms=${journeyHttp.totalLatencyMs}`
            );
            logEvent('res', `finalize order_id=${finalize.order_id}`);
            void fetchSummaryDocuments(finalize.order_id);
          } else {
            const state = res.payload as CartState;
            setCartState(state);
            setCartId(state.cart_id);
            logEvent('res', `cart_id=${state.cart_id} state=${state.state}`);
          }
        } else {
          const errMsg =
            res.errors?.length ? res.errors.map((e) => e.message).join('; ') : 'Unknown error';
          logEvent('err', errMsg);
          pushToast(errMsg);
        }
        return res;
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `pos: ${err.message}`);
        pushToast(err.message);
        return null;
      }
    },
    [baseUrl, cartState, fetchSummaryDocuments, logEvent, pushToast, storeId]
  );

  const ensureCart = useCallback(async (): Promise<string | null> => {
    if (cartId) return cartId;
    const res = await sendPosCommand({
      action: 'create_cart',
      payload: { cart_id: null },
    });
    if (res?.success && res.payload && !('order_id' in res.payload)) {
      return (res.payload as CartState).cart_id;
    }
    return null;
  }, [cartId, sendPosCommand]);

  const onAddProduct = useCallback(
    async (product: ProductSearchResult, quantity: number) => {
      const cid = await ensureCart();
      if (!cid) return;
      const addRes = await sendPosCommand({
        action: 'add_line_item',
        payload: {
          cart_id: cid,
          item_id: product.id,
          modifier_option_ids: [],
          quantity,
          notes: null,
        },
      });
      if (addRes?.success) {
        pushToast(`Added ${quantity} × ${product.name}`);
      }
    },
    [ensureCart, pushToast, sendPosCommand]
  );

  const onSetCustomer = useCallback(async () => {
    if (!selectedCustomerId) return;
    const cid = await ensureCart();
    if (!cid) return;
    const res = await sendPosCommand({
      action: 'set_customer',
      payload: { cart_id: cid, customer_id: selectedCustomerId },
    });
    if (res?.success) {
      const selected = customers.find((c) => c.id === selectedCustomerId) ?? null;
      if (selected) {
        setAttachedCustomer(selected);
      }
      pushToast('Customer applied to cart');
    }
  }, [customers, ensureCart, selectedCustomerId, sendPosCommand, pushToast]);

  const onRemoveLine = useCallback(
    async (lineId: string) => {
      if (!cartId) return;
      await sendPosCommand({
        action: 'remove_line_item',
        payload: { cart_id: cartId, line_id: lineId },
      });
    },
    [cartId, sendPosCommand]
  );

  const onGoToPay = useCallback(async () => {
    if (!cartId) return;
    const res = await sendPosCommand({
      action: 'set_tendering',
      payload: { cart_id: cartId },
    });
    if (res?.success) {
      setStage('pay');
    }
  }, [cartId, sendPosCommand]);

  const onApplyCoupon = useCallback(async (couponCode: string) => {
    const code = couponCode.trim();
    if (!code) return;
    const cid = await ensureCart();
    if (!cid) return;
    const res = await sendPosCommand({
      action: 'apply_coupon',
      payload: { cart_id: cid, coupon_code: code },
    });
    if (res?.success) {
      pushToast(`Coupon applied: ${code}`);
    }
  }, [ensureCart, pushToast, sendPosCommand]);

  const onAddCashPayment = useCallback(async () => {
    if (!cartId) return;
    const amountCents = Math.round((parseFloat(cashAmount) || 0) * 100);
    if (amountCents <= 0) return;
    const payRes = await sendPosCommand({
      action: 'add_payment',
      payload: {
        cart_id: cartId,
        tender_id: '00000000-0000-0000-0000-000000000001',
        amount_cents: amountCents,
        external_reference: null,
      },
    });
    setCashAmount('');
    if (
      payRes?.success &&
      payRes.payload &&
      !('order_id' in payRes.payload) &&
      (payRes.payload as CartState).state === 'paid'
    ) {
      pushToast('Payment complete. Placing order…');
      await sendPosCommand({
        action: 'finalize_order',
        payload: { cart_id: cartId },
      });
    }
  }, [cartId, cashAmount, sendPosCommand, pushToast]);

  const openDocument = useCallback(
    async (documentId: string) => {
      logEvent('req', `GET /documents/${documentId}`);
      const doc = await getDocument(baseUrl, documentId);
      logEvent('res', `document ${documentId}`);
      const content = doc.content ?? doc.error_message ?? '(empty)';
      const mimeType = doc.mime_type ?? 'text/plain';

      if (mimeType === 'application/pdf' && content) {
        try {
          const binary = atob(content.trim());
          const bytes = new Uint8Array(binary.length);
          for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
          const blob = new Blob([bytes], { type: 'application/pdf' });
          const url = URL.createObjectURL(blob);
          const win = window.open(url, '_blank');
          if (win) {
            win.addEventListener('load', () => {
              try {
                win.print();
              } catch {
                // Print not supported or blocked; PDF is still open in tab
              }
            });
            // Revoke after a delay so the new window can load the blob
            setTimeout(() => URL.revokeObjectURL(url), 60000);
          } else {
            // Popup blocked: fall back to same-tab navigation
            window.location.href = url;
          }
        } catch {
          pushToast('Could not open PDF');
          const win = window.open('', '_blank');
          if (win) {
            win.document.write(`<pre style="font-family: ui-monospace,monospace">${content}</pre>`);
            win.document.close();
          }
        }
        return;
      }

      const win = window.open('', '_blank');
      if (win) {
        win.document.write(`<pre style="font-family: ui-monospace,monospace">${content}</pre>`);
        win.document.close();
      }
    },
    [baseUrl, logEvent, pushToast]
  );

  const onPrint = useCallback(async () => {
    if (!saleSummary) return;
    let docs = saleSummary.documents;
    if (docs.length === 0) {
      docs = await listOrderDocuments(baseUrl, saleSummary.finalize.order_id);
      setSaleSummary((prev) => (prev ? { ...prev, documents: docs } : prev));
    }
    const doc =
      docs.find((d) => d.document_type === 'customer_receipt') ??
      docs.find((d) => d.document_type === 'receipt') ??
      docs[0];
    if (!doc) {
      pushToast('No printable document found');
      return;
    }
    await openDocument(doc.id);
  }, [baseUrl, openDocument, pushToast, saleSummary]);

  const onGiftReceipt = useCallback(async () => {
    if (!saleSummary) return;
    logEvent('req', `POST /orders/${saleSummary.finalize.order_id}/documents/gift-receipt`);
    try {
      const gift = await createGiftReceipt(baseUrl, saleSummary.finalize.order_id);
      setSaleSummary((prev) =>
        prev ? { ...prev, documents: [...prev.documents, gift] } : prev
      );
      logEvent('res', `gift_receipt ${gift.id}`);
      pushToast('Gift receipt generated');
      await openDocument(gift.id);
    } catch (e) {
      const err = e as ApiError;
      logEvent('err', `gift receipt: ${err.message}`);
      pushToast(err.message);
    }
  }, [baseUrl, logEvent, openDocument, pushToast, saleSummary]);

  const onAcceptSummary = useCallback(() => {
    setSaleSummary(null);
    setCartId(null);
    setCartState(null);
    setSelectedCustomerId(null);
    setAttachedCustomer(null);
    setStage('customers');
    startJourneyTracking('new_sale_started');
    pushToast('Sale completed');
  }, [pushToast]);

  // Persist cart ID to localStorage whenever it changes.
  useEffect(() => {
    if (cartId) {
      localStorage.setItem(LS_CART_ID, cartId);
    } else {
      localStorage.removeItem(LS_CART_ID);
    }
  }, [cartId]);

  // On mount: if a cart ID was saved, restore its state from the orchestrator.
  useEffect(() => {
    if (!authReady) return;
    const savedId = localStorage.getItem(LS_CART_ID);
    if (!savedId || !baseUrl) return;
    getCartState(baseUrl, savedId)
      .then((state) => {
        if (state) {
          setCartId(state.cart_id);
          setCartState(state);
        } else {
          // Cart no longer exists on the backend — clear stale session.
          localStorage.removeItem(LS_CART_ID);
          setCartId(null);
        }
      })
      .catch(() => {
        // Network error on restore is non-fatal; let user start fresh.
      });
  }, [authReady, baseUrl]);

  useEffect(() => {
    if (!authReady || !baseUrl || cartId || stage === 'summary') {
      return;
    }
    void ensureCart();
  }, [authReady, baseUrl, cartId, ensureCart, stage]);

  // Resolve the attached customer from the edge hub whenever the cart's customer_id changes.
  // Checks the already-fetched customers list first; falls back to a targeted hub lookup.
  useEffect(() => {
    if (!authReady) return;
    const customerId = cartState?.customer_id ?? null;
    if (!customerId) {
      if (!cartState || cartState.lines.length === 0) {
        setAttachedCustomer(null);
      }
      return;
    }
    if (attachedCustomer?.id === customerId) return;
    const fromList = customers.find((c) => c.id === customerId);
    if (fromList) {
      setAttachedCustomer(fromList);
      return;
    }
    void searchCustomers(baseUrl, customerId).then((results) => {
      const matched = results.find((r) => r.id === customerId);
      if (matched) {
        setAttachedCustomer(matched);
      } else {
        setAttachedCustomer((prev) => (prev?.id === customerId ? prev : null));
      }
    });
  }, [authReady, cartState, baseUrl, customers, attachedCustomer]);

  const isCartTab = stage === 'cart' || stage === 'pay' || stage === 'summary';

  const onViewProduct = useCallback(
    (product: ProductSearchResult) => {
      navigate(`/product/${product.id}`);
    },
    [navigate]
  );

  const mainContent = (
    <div className="pos-app">
      <header className="pos-header">
        <ConnectionPanel
          baseUrl={baseUrl}
          setBaseUrl={setBaseUrl}
          healthStatus={healthStatus}
          readyStatus={readyStatus}
          storeId={storeId}
          setStoreId={setStoreId}
          associateId={associateId}
          setAssociateId={setAssociateId}
          deviceName={deviceName}
          setDeviceName={setDeviceName}
          devicePlatform={devicePlatform}
          setDevicePlatform={setDevicePlatform}
          authIssuer={authIssuer}
          setAuthIssuer={setAuthIssuer}
          authAudience={authAudience}
          setAuthAudience={setAuthAudience}
          mockTokenSecret={mockTokenSecret}
          setMockTokenSecret={setMockTokenSecret}
          authReady={authReady}
          authBusy={authBusy}
          authError={authError}
          deviceId={deviceId}
          hasDeviceSecret={Boolean(deviceSecret)}
          tokenExpiresAt={expiresAt}
          onPairAndSignIn={onPairAndSignIn}
          onSignOut={onSignOut}
          onCheckHealth={checkHealth}
          onCheckReady={checkReady}
        />
      </header>

      <div className="pos-main">
        {!authReady && (
          <section className="auth-gate">
            <h2>Authentication required</h2>
            <p>
              Pair this simulator and sign in from the header before accessing
              protected POS routes.
            </p>
          </section>
        )}

        {/* ── Customers ── */}
        {authReady && stage === 'customers' && (
          <>
            <CustomerPanel
              onSearch={onSearchCustomers}
              customers={customers}
              onSelectCustomer={setSelectedCustomerId}
              selectedCustomerId={selectedCustomerId}
              disabled={!baseUrl}
            />
            <div className="btn-stack">
              <button
                type="button"
                className="btn-secondary"
                onClick={onSetCustomer}
                disabled={!selectedCustomerId}
              >
                Apply Customer to Cart
              </button>
              <button
                type="button"
                className="btn-primary"
                onClick={() => setStage('catalog')}
              >
                Go to Catalog →
              </button>
            </div>
          </>
        )}

        {/* ── Sync Status ── */}
        {authReady && stage === 'sync' && (
          <SyncStatusPanel baseUrl={baseUrl} disabled={!baseUrl} />
        )}

        {/* ── Catalog ── */}
        {authReady && stage === 'catalog' && (
          <CatalogPanel
            baseUrl={baseUrl}
            categories={categories}
            productList={productList}
            onLoadCategories={onLoadCategories}
            onLoadProducts={onLoadProducts}
            onAddProduct={onAddProduct}
            onViewProduct={onViewProduct}
          />
        )}

        {/* ── Cart ── */}
        {authReady && stage === 'cart' && (
          <CartPanel
            cartState={cartState}
            attachedCustomer={attachedCustomer}
            onGoPay={onGoToPay}
            canPay={cartItemCount > 0}
            onRemoveLine={onRemoveLine}
            onApplyCoupon={onApplyCoupon}
          />
        )}

        {/* ── Pay ── */}
        {authReady && stage === 'pay' && (
          <div>
            <p className="ios-section-header">Cash Payment</p>
            <div className="ios-card">
              <div className="pay-amount-block">
                <div className="pay-amount-label">Amount Due</div>
                <div className="pay-amount-value">
                  ${((cartState?.total_cents ?? 0) / 100).toFixed(2)}
                </div>
                <div className="pay-tendered-row">
                  <span>Tendered: <strong>${((cartState?.tendered_cents ?? 0) / 100).toFixed(2)}</strong></span>
                  <span>
                    Remaining:{' '}
                    <strong>
                      ${(Math.max(0, (cartState?.total_cents ?? 0) - (cartState?.tendered_cents ?? 0)) / 100).toFixed(2)}
                    </strong>
                  </span>
                </div>
              </div>
              <div className="pay-input-row">
                <input
                  type="number"
                  min="0"
                  step="0.01"
                  value={cashAmount}
                  onChange={(e) => setCashAmount(e.target.value)}
                  placeholder="Enter cash amount"
                  className="ios-input"
                  onKeyDown={(e) => e.key === 'Enter' && onAddCashPayment()}
                />
              </div>
              <div style={{ padding: '0 1rem 1rem' }}>
                <button
                  type="button"
                  className="btn-primary"
                  onClick={onAddCashPayment}
                  disabled={!cashAmount || parseFloat(cashAmount) <= 0}
                >
                  Add Payment
                </button>
              </div>
              <div className="pay-hint">
                Order is placed automatically when tendered ≥ total.
              </div>
            </div>
          </div>
        )}

        {/* ── Summary ── */}
        {authReady && stage === 'summary' && saleSummary && (
          <div>
            <p className="ios-section-header">Sale Complete</p>
            <div className="ios-card">
              <div className="summary-total-block">
                <div className="summary-total-label">Total Charged</div>
                <div className="summary-total-value">
                  ${(saleSummary.finalize.total_cents / 100).toFixed(2)}
                </div>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Order ID</span>
                <span className="ios-row-value" style={{ fontSize: '0.75rem', fontFamily: 'ui-monospace,monospace' }}>
                  {saleSummary.finalize.order_id.slice(0, 8)}…
                </span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Subtotal</span>
                <span className="ios-row-value">
                  ${((saleSummary.cartSnapshot?.subtotal_cents ?? 0) / 100).toFixed(2)}
                </span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Discounts</span>
                <span className="ios-row-value" style={{ color: 'var(--green)' }}>
                  −${((saleSummary.cartSnapshot?.discount_cents ?? 0) / 100).toFixed(2)}
                </span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Taxes</span>
                <span className="ios-row-value">
                  ${((saleSummary.cartSnapshot?.tax_cents ?? 0) / 100).toFixed(2)}
                </span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Total requests</span>
                <span className="ios-row-value">{saleSummary.journeyHttp.totalRequests}</span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Local requests</span>
                <span className="ios-row-value">{saleSummary.journeyHttp.localRequests}</span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Non-local requests</span>
                <span className="ios-row-value">{saleSummary.journeyHttp.nonLocalRequests}</span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Failed requests</span>
                <span className="ios-row-value">{saleSummary.journeyHttp.failedRequests}</span>
              </div>
              <div className="ios-row">
                <span className="ios-row-title">Total latency</span>
                <span className="ios-row-value">{saleSummary.journeyHttp.totalLatencyMs}ms</span>
              </div>
            </div>
            <div className="btn-stack">
              <button type="button" className="btn-secondary" onClick={onPrint}>
                Print Receipt
              </button>
              <button type="button" className="btn-secondary" onClick={onGiftReceipt}>
                Gift Receipt
              </button>
              <button type="button" className="btn-primary btn-green" onClick={onAcceptSummary}>
                Done
              </button>
            </div>
          </div>
        )}
      </div>

      {/* ── Footer (event log, tablet+) ── */}
      <footer className="pos-footer">
        <button
          type="button"
          className="event-log-toggle"
          onClick={() => setEventLogOpen((o) => !o)}
        >
          {eventLogOpen ? 'Hide' : 'Show'} event log
        </button>
        {eventLogOpen && <EventLogPanel entries={eventLog} />}
      </footer>

      {/* ── Bottom Tab Bar ── */}
      <nav className="pos-nav" aria-label="Main">
        <button
          type="button"
          className={stage === 'customers' ? 'active' : ''}
          onClick={() => setStage('customers')}
          aria-current={stage === 'customers' ? 'page' : undefined}
        >
          <span className="nav-icon">👤</span>
          <span className="nav-label">Customers</span>
        </button>
        <button
          type="button"
          className={stage === 'catalog' ? 'active' : ''}
          onClick={() => setStage('catalog')}
          aria-current={stage === 'catalog' ? 'page' : undefined}
        >
          <span className="nav-icon">⊞</span>
          <span className="nav-label">Catalog</span>
        </button>
        <button
          type="button"
          className={stage === 'sync' ? 'active' : ''}
          onClick={() => setStage('sync')}
          aria-current={stage === 'sync' ? 'page' : undefined}
        >
          <span className="nav-icon">↻</span>
          <span className="nav-label">Sync</span>
        </button>
        <button
          type="button"
          className={isCartTab ? 'active' : ''}
          onClick={() => {
            if (stage === 'summary') return;
            setStage('cart');
          }}
          aria-current={isCartTab ? 'page' : undefined}
        >
          <span className="nav-icon">
            🛒
            {cartItemCount > 0 && (
              <span className="cart-badge">{cartItemCount}</span>
            )}
          </span>
          <span className="nav-label">Cart</span>
        </button>
      </nav>

      {/* ── Toasts ── */}
      <div className="toast-stack">
        {toasts.map((toast) => (
          <div key={toast.id} className="toast">
            {toast.message}
          </div>
        ))}
      </div>
    </div>
  );

  return (
    <Routes>
      <Route
        path="/product/:id"
        element={
          <ProductDetailRoute baseUrl={baseUrl} onAddProduct={onAddProduct} />
        }
      />
      <Route path="/*" element={mainContent} />
    </Routes>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <AppInner />
    </BrowserRouter>
  );
}
