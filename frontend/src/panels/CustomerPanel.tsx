import { useCallback, useState } from 'react';
import type { CustomerSearchResult } from '../api/types';

interface Props {
  onSearch: (q: string) => void;
  customers: CustomerSearchResult[];
  onSelectCustomer: (customerId: string) => void;
  selectedCustomerId: string | null;
  disabled: boolean;
}

export function CustomerPanel({
  onSearch,
  customers,
  onSelectCustomer,
  selectedCustomerId,
  disabled,
}: Props) {
  const [q, setQ] = useState('');

  const handleSearch = useCallback(() => {
    onSearch(q);
  }, [q, onSearch]);

  const selected = customers.find((c) => c.id === selectedCustomerId);

  return (
    <div>
      <p className="ios-section-header">Find Customer</p>
      <div className="ios-card" style={{ padding: '0.6rem 1rem', marginBottom: '0.75rem' }}>
        <input
          type="text"
          placeholder="Name, email, code, or ID"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && !disabled && handleSearch()}
          aria-label="Search customer"
          className="ios-search"
          style={{ width: '100%' }}
          disabled={disabled}
        />
      </div>

      {selected && (
        <div className="customer-selected-banner">
          <span>✓</span>
          <span>{selected.name} selected</span>
        </div>
      )}

      {customers.length > 0 && (
        <>
          <p className="ios-section-header">Results</p>
          <div className="ios-card">
            {customers.map((c) => (
              <button
                key={c.id}
                type="button"
                className="ios-row"
                style={{ width: '100%', background: 'none', border: 'none', textAlign: 'left', cursor: 'pointer' }}
                onClick={() => onSelectCustomer(c.id)}
                aria-pressed={selectedCustomerId === c.id}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div className="customer-name-text">{c.name}</div>
                  <div className="customer-meta-text">
                    {c.code}{c.email ? ` · ${c.email}` : ''}
                  </div>
                </div>
                {selectedCustomerId === c.id && (
                  <span className="customer-checkmark">✓</span>
                )}
              </button>
            ))}
          </div>
        </>
      )}

      {customers.length === 0 && q.trim() && (
        <div className="ios-card">
          <div className="customer-empty">No customers found.</div>
        </div>
      )}
    </div>
  );
}
