import { useCallback, useState } from 'react';
import {
  getHealth,
  getReady,
  searchProducts,
  searchCustomers,
  postPosCommand,
  listOrderDocuments,
  getDocument,
  buildEnvelope,
  type ApiError,
} from './api/client';
import type { CartState, FinalizeResult, ProductSearchResult, CustomerSearchResult } from './api/types';
import { ConnectionPanel } from './panels/ConnectionPanel';
import { LookupPanel } from './panels/LookupPanel';
import { CartPanel } from './panels/CartPanel';
import { CheckoutPanel } from './panels/CheckoutPanel';
import { DocumentsPanel } from './panels/DocumentsPanel';
import { EventLogPanel } from './panels/EventLogPanel';

const STORE_ID = '00000000-0000-0000-0000-000000000000';
const REGISTER_ID = '00000000-0000-0000-0000-000000000000';

export type LogEntry = { ts: string; kind: 'req' | 'res' | 'err'; text: string };

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
  const [cartId, setCartId] = useState<string | null>(null);
  const [cartState, setCartState] = useState<CartState | null>(null);
  const [orderId, setOrderId] = useState<string | null>(null);
  const [finalizeResult, setFinalizeResult] = useState<FinalizeResult | null>(null);
  const [products, setProducts] = useState<ProductSearchResult[]>([]);
  const [customers, setCustomers] = useState<CustomerSearchResult[]>([]);
  const [orderDocuments, setOrderDocuments] = useState<{ orderId: string; docs: Awaited<ReturnType<typeof listOrderDocuments>> } | null>(null);
  const [selectedDocContent, setSelectedDocContent] = useState<string | null>(null);
  const [eventLog, logEvent] = useEventLog();

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

  const onSearchProducts = useCallback(
    async (sku: string) => {
      logEvent('req', `GET /catalog/products?sku=${sku}`);
      try {
        const list = await searchProducts(baseUrl, sku);
        setProducts(list);
        logEvent('res', `products: ${list.length} found`);
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `products: ${err.message}`);
        setProducts([]);
      }
    },
    [baseUrl, logEvent]
  );

  const onSearchCustomers = useCallback(
    async (code: string) => {
      logEvent('req', `GET /customers?code=${code}`);
      try {
        const list = await searchCustomers(baseUrl, code);
        setCustomers(list);
        logEvent('res', `customers: ${list.length} found`);
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `customers: ${err.message}`);
        setCustomers([]);
      }
    },
    [baseUrl, logEvent]
  );

  const onPosCommand = useCallback(
    async (command: Parameters<typeof buildEnvelope>[2]) => {
      const envelope = buildEnvelope(STORE_ID, REGISTER_ID, command);
      logEvent('req', `POST /pos/command ${(command as { action: string }).action}`);
      try {
        const res = await postPosCommand(baseUrl, envelope);
        if (res.success && res.payload) {
          if ('order_id' in res.payload) {
            setFinalizeResult(res.payload as FinalizeResult);
            setOrderId((res.payload as FinalizeResult).order_id);
            setCartId(null);
            setCartState(null);
            logEvent('res', `finalize order_id=${(res.payload as FinalizeResult).order_id}`);
          } else {
            const state = res.payload as CartState;
            setCartState(state);
            setCartId(state.cart_id);
            logEvent('res', `cart_id=${state.cart_id} state=${state.state}`);
          }
        } else {
          const errMsg = res.errors?.length ? res.errors.map((e) => e.message).join('; ') : 'Unknown error';
          logEvent('err', errMsg);
        }
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `pos: ${err.message}`);
      }
    },
    [baseUrl, logEvent]
  );

  const onListOrderDocuments = useCallback(
    async (oid: string) => {
      logEvent('req', `GET /orders/${oid}/documents`);
      try {
        const docs = await listOrderDocuments(baseUrl, oid);
        setOrderDocuments({ orderId: oid, docs });
        logEvent('res', `documents: ${docs.length}`);
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `list docs: ${err.message}`);
        setOrderDocuments(null);
      }
    },
    [baseUrl, logEvent]
  );

  const onGetDocument = useCallback(
    async (id: string) => {
      logEvent('req', `GET /documents/${id}`);
      try {
        const doc = await getDocument(baseUrl, id);
        setSelectedDocContent(doc.content ?? doc.error_message ?? '(empty)');
        logEvent('res', `document ${id}`);
      } catch (e) {
        const err = e as ApiError;
        logEvent('err', `get doc: ${err.message}`);
        setSelectedDocContent(null);
      }
    },
    [baseUrl, logEvent]
  );

  return (
    <div className="app">
      <ConnectionPanel
        baseUrl={baseUrl}
        setBaseUrl={setBaseUrl}
        healthStatus={healthStatus}
        readyStatus={readyStatus}
        onCheckHealth={checkHealth}
        onCheckReady={checkReady}
      />
      <LookupPanel
        onSearchProducts={onSearchProducts}
        onSearchCustomers={onSearchCustomers}
        products={products}
        customers={customers}
      />
      <CartPanel
        cartId={cartId}
        cartState={cartState}
        products={products}
        customers={customers}
        onPosCommand={onPosCommand}
      />
      <CheckoutPanel
        cartId={cartId}
        cartState={cartState}
        finalizeResult={finalizeResult}
        onPosCommand={onPosCommand}
      />
      <DocumentsPanel
        orderId={orderId}
        orderDocuments={orderDocuments}
        selectedDocContent={selectedDocContent}
        onListOrderDocuments={onListOrderDocuments}
        onGetDocument={onGetDocument}
      />
      <EventLogPanel entries={eventLog} />
    </div>
  );
}
