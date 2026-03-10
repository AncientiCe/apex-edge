import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  getHealth,
  getReady,
  listProducts,
  listCategories,
  searchCustomers,
  postPosCommand,
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

const STORE_ID = '00000000-0000-0000-0000-000000000000';
const REGISTER_ID = '00000000-0000-0000-0000-000000000000';

export type LogEntry = { ts: string; kind: 'req' | 'res' | 'err'; text: string };
type Stage = 'customers' | 'catalog' | 'cart' | 'pay' | 'summary';
type Toast = { id: number; message: string };
type SaleSummary = {
  finalize: FinalizeResult;
  cartSnapshot: CartState | null;
  documents: DocumentSummary[];
};

function useEventLog() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const log = useCallback((kind: LogEntry['kind'], text: string) => {
    const ts = new Date().toISOString().slice(11, 23);
    setEntries((prev) => [...prev.slice(-199), { ts, kind, text }]);
  }, []);
  return [entries, log] as const;
}

export default function App() {
  const [baseUrl, setBaseUrl] = useState(
    () => import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:3000'
  );
  const [healthStatus, setHealthStatus] = useState<string | null>(null);
  const [readyStatus, setReadyStatus] = useState<string | null>(null);
  const [stage, setStage] = useState<Stage>('customers');
  const [cartId, setCartId] = useState<string | null>(null);
  const [cartState, setCartState] = useState<CartState | null>(null);
  const [saleSummary, setSaleSummary] = useState<SaleSummary | null>(null);
  const [categories, setCategories] = useState<CategoryResult[]>([]);
  const [productList, setProductList] = useState<ProductListResponse | null>(null);
  const [customers, setCustomers] = useState<CustomerSearchResult[]>([]);
  const [selectedCustomerId, setSelectedCustomerId] = useState<string | null>(null);
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
      const envelope = buildEnvelope(STORE_ID, REGISTER_ID, command);
      logEvent('req', `POST /pos/command ${(command as { action: string }).action}`);
      try {
        const res = await postPosCommand(baseUrl, envelope);
        if (res.success && res.payload) {
          if ('order_id' in res.payload) {
            const finalize = res.payload as FinalizeResult;
            setSaleSummary({
              finalize,
              cartSnapshot: cartState,
              documents: [],
            });
            setCartId(null);
            setCartState(null);
            setStage('summary');
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
    [baseUrl, cartState, fetchSummaryDocuments, logEvent, pushToast]
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
      pushToast('Customer applied to cart');
    }
  }, [ensureCart, selectedCustomerId, sendPosCommand, pushToast]);

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
      pushToast('Payment complete. Placing order...');
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
      const win = window.open('', '_blank');
      if (win) {
        win.document.write(`<pre style="font-family: ui-monospace,monospace">${content}</pre>`);
        win.document.close();
      }
    },
    [baseUrl, logEvent]
  );

  const onPrint = useCallback(async () => {
    if (!saleSummary) return;
    let docs = saleSummary.documents;
    if (docs.length === 0) {
      docs = await listOrderDocuments(baseUrl, saleSummary.finalize.order_id);
      setSaleSummary((prev) => (prev ? { ...prev, documents: docs } : prev));
    }
    const doc = docs.find((d) => d.document_type === 'receipt') ?? docs[0];
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
    setStage('customers');
    pushToast('Sale completed');
  }, [pushToast]);

  // Keep an active cart at all times for the POS flow.
  useEffect(() => {
    if (!baseUrl || cartId || stage === 'summary') {
      return;
    }
    void ensureCart();
  }, [baseUrl, cartId, ensureCart, stage]);

  return (
    <div className="app pos-app">
      <header className="pos-header">
        <ConnectionPanel
          baseUrl={baseUrl}
          setBaseUrl={setBaseUrl}
          healthStatus={healthStatus}
          readyStatus={readyStatus}
          onCheckHealth={checkHealth}
          onCheckReady={checkReady}
        />
        <nav className="pos-nav" aria-label="Main">
          <button
            type="button"
            className={stage === 'customers' ? 'active' : ''}
            onClick={() => setStage('customers')}
            aria-current={stage === 'customers' ? 'page' : undefined}
          >
            Customers
          </button>
          <button
            type="button"
            className={stage === 'catalog' ? 'active' : ''}
            onClick={() => setStage('catalog')}
            aria-current={stage === 'catalog' ? 'page' : undefined}
          >
            Catalog
          </button>
          <button
            type="button"
            className={stage === 'cart' || stage === 'pay' || stage === 'summary' ? 'active' : ''}
            onClick={() => {
              if (stage === 'summary') return;
              setStage('cart');
            }}
            aria-current={stage === 'cart' || stage === 'pay' || stage === 'summary' ? 'page' : undefined}
          >
            Cart
            {cartItemCount > 0 && <span className="cart-badge">{cartItemCount}</span>}
          </button>
        </nav>
      </header>
      <div className="pos-main">
        {stage === 'customers' && (
          <section className="panel">
            <CustomerPanel
              onSearch={onSearchCustomers}
              customers={customers}
              onSelectCustomer={setSelectedCustomerId}
              selectedCustomerId={selectedCustomerId}
              disabled={!baseUrl}
            />
            <div className="row">
              <button type="button" onClick={onSetCustomer} disabled={!selectedCustomerId}>
                Apply selected customer
              </button>
              <button type="button" onClick={() => setStage('catalog')}>
                Go to catalog
              </button>
            </div>
          </section>
        )}
        {stage === 'catalog' && (
          <CatalogPanel
            baseUrl={baseUrl}
            categories={categories}
            productList={productList}
            onLoadCategories={onLoadCategories}
            onLoadProducts={onLoadProducts}
            onAddProduct={onAddProduct}
          />
        )}
        {stage === 'cart' && (
          <section className="panel">
            <CartPanel cartState={cartState} onGoPay={onGoToPay} canPay={cartItemCount > 0} />
          </section>
        )}
        {stage === 'pay' && (
          <section className="panel">
            <h2>Pay (Cash)</h2>
            <div className="row">
              <span className="status">
                Due: {((cartState?.total_cents ?? 0) / 100).toFixed(2)}
              </span>
              <span className="status">
                Tendered: {((cartState?.tendered_cents ?? 0) / 100).toFixed(2)}
              </span>
            </div>
            <div className="row">
              <input
                type="number"
                min="0"
                step="0.01"
                value={cashAmount}
                onChange={(e) => setCashAmount(e.target.value)}
                placeholder="Cash amount"
              />
              <button type="button" className="primary" onClick={onAddCashPayment}>
                Add cash payment
              </button>
            </div>
            <div className="status">
              When tendered reaches total, order is placed automatically.
            </div>
          </section>
        )}
        {stage === 'summary' && saleSummary && (
          <section className="panel">
            <h2>Sale Summary</h2>
            <div className="summary-grid">
              <div><strong>Order ID</strong><div className="status">{saleSummary.finalize.order_id}</div></div>
              <div><strong>Total</strong><div className="status">{(saleSummary.finalize.total_cents / 100).toFixed(2)}</div></div>
              <div><strong>Subtotal</strong><div className="status">{((saleSummary.cartSnapshot?.subtotal_cents ?? 0) / 100).toFixed(2)}</div></div>
              <div><strong>Discounts</strong><div className="status">{((saleSummary.cartSnapshot?.discount_cents ?? 0) / 100).toFixed(2)}</div></div>
              <div><strong>Taxes</strong><div className="status">{((saleSummary.cartSnapshot?.tax_cents ?? 0) / 100).toFixed(2)}</div></div>
            </div>
            <div className="row">
              <button type="button" onClick={onPrint}>Print</button>
              <button type="button" onClick={onGiftReceipt}>Gift receipt</button>
              <button type="button" className="primary" onClick={onAcceptSummary}>Accept</button>
            </div>
          </section>
        )}
      </div>
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
      <div className="toast-stack">
        {toasts.map((toast) => (
          <div key={toast.id} className="toast">
            {toast.message}
          </div>
        ))}
      </div>
    </div>
  );
}
