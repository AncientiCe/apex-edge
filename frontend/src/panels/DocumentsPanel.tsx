import { useState } from 'react';
import type { DocumentSummary } from '../api/types';

interface Props {
  orderId: string | null;
  orderDocuments: { orderId: string; docs: DocumentSummary[] } | null;
  selectedDocContent: string | null;
  onListOrderDocuments: (orderId: string) => void;
  onGetDocument: (id: string) => void;
}

export function DocumentsPanel({
  orderId,
  orderDocuments,
  selectedDocContent,
  onListOrderDocuments,
  onGetDocument,
}: Props) {
  const [inputOrderId, setInputOrderId] = useState(orderId ?? '');

  const effectiveOrderId = orderId ?? inputOrderId;
  const handleList = () => {
    if (effectiveOrderId) onListOrderDocuments(effectiveOrderId);
  };

  return (
    <section className="panel documents">
      <h2>Documents</h2>
      <div className="row">
        <input
          type="text"
          placeholder="Order ID"
          value={orderId ?? inputOrderId}
          onChange={(e) => setInputOrderId(e.target.value)}
          readOnly={!!orderId}
          style={{ minWidth: 280 }}
        />
        <button type="button" onClick={handleList} disabled={!effectiveOrderId}>
          List documents
        </button>
      </div>
      {orderDocuments && (
        <div style={{ marginTop: '0.5rem' }}>
          <div className="status" style={{ marginBottom: '0.25rem' }}>
            {orderDocuments.docs.length} document(s)
          </div>
          <ul style={{ margin: 0, paddingLeft: '1.25rem' }}>
            {orderDocuments.docs.map((d) => (
              <li key={d.id}>
                <button
                  type="button"
                  onClick={() => onGetDocument(d.id)}
                  style={{ background: 'none', border: 'none', padding: 0, cursor: 'pointer', color: 'var(--accent)' }}
                >
                  {d.document_type} — {d.id.slice(0, 8)}…
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {selectedDocContent !== null && (
        <pre className="pre" style={{ marginTop: '0.5rem', maxHeight: 120, overflow: 'auto', fontSize: '0.75rem' }}>
          {selectedDocContent}
        </pre>
      )}
    </section>
  );
}
