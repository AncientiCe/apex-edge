import { useState } from 'react';
import type { ProductSearchResult, CustomerSearchResult } from '../api/types';

interface Props {
  onSearchProducts: (sku: string) => void;
  onSearchCustomers: (code: string) => void;
  products: ProductSearchResult[];
  customers: CustomerSearchResult[];
}

export function LookupPanel({
  onSearchProducts,
  onSearchCustomers,
  products,
  customers,
}: Props) {
  const [sku, setSku] = useState('');
  const [code, setCode] = useState('');

  return (
    <section className="panel lookup">
      <h2>Lookup</h2>
      <div className="row">
        <input
          type="text"
          placeholder="SKU"
          value={sku}
          onChange={(e) => setSku(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && onSearchProducts(sku)}
        />
        <button type="button" onClick={() => onSearchProducts(sku)}>
          Search products
        </button>
        {products.length > 0 && <span className="status">({products.length} found)</span>}
      </div>
      <div className="row">
        <input
          type="text"
          placeholder="Customer code"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && onSearchCustomers(code)}
        />
        <button type="button" onClick={() => onSearchCustomers(code)}>
          Search customers
        </button>
        {customers.length > 0 && <span className="status">({customers.length} found)</span>}
      </div>
    </section>
  );
}
