import { useCallback, useEffect, useState } from 'react';
import type { CategoryResult, ProductSearchResult, ProductListResponse } from '../api/types';

interface Props {
  baseUrl: string;
  categories: CategoryResult[];
  productList: ProductListResponse | null;
  onLoadCategories: () => void;
  onLoadProducts: (params: { q?: string; category_id?: string; page: number }) => void;
  onAddProduct: (product: ProductSearchResult, quantity: number) => void;
  onViewProduct?: (product: ProductSearchResult) => void;
}

export function CatalogPanel({
  baseUrl,
  categories,
  productList,
  onLoadCategories,
  onLoadProducts,
  onAddProduct,
  onViewProduct,
}: Props) {
  const [q, setQ] = useState('');
  const [submittedQ, setSubmittedQ] = useState('');
  const [categoryId, setCategoryId] = useState<string>('');
  const [page, setPage] = useState(1);

  useEffect(() => {
    if (!baseUrl) return;
    onLoadCategories();
  }, [baseUrl, onLoadCategories]);

  useEffect(() => {
    if (!baseUrl) return;
    onLoadProducts({
      q: submittedQ.trim() || undefined,
      category_id: categoryId || undefined,
      page,
    });
  }, [baseUrl, categoryId, page, submittedQ, onLoadProducts]);

  const totalPages = productList
    ? Math.max(1, Math.ceil(productList.total / productList.per_page))
    : 0;

  const handleSearch = useCallback(() => {
    setSubmittedQ(q);
    setPage(1);
  }, [q]);

  return (
    <div>
      {/* Search bar */}
      <div className="ios-card" style={{ padding: '0.6rem 1rem', marginBottom: '0.75rem' }}>
        <input
          type="text"
          placeholder="Search products…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          className="ios-search"
          style={{ width: '100%' }}
          aria-label="Search products"
        />
      </div>

      {/* Category chips */}
      <div className="chip-row">
        <button
          type="button"
          className={`chip${categoryId === '' ? ' active' : ''}`}
          onClick={() => { setCategoryId(''); setPage(1); }}
        >
          All
        </button>
        {categories.map((c) => (
          <button
            key={c.id}
            type="button"
            className={`chip${categoryId === c.id ? ' active' : ''}`}
            onClick={() => { setCategoryId(c.id); setPage(1); }}
          >
            {c.name}
          </button>
        ))}
      </div>

      {/* Product grid */}
      <div className="catalog-grid">
        {(productList?.items ?? []).map((p) => {
          const isOutOfStock = !p.is_active || (p.available_qty !== null && p.available_qty <= 0);
          return (
            <div key={p.id} className={`catalog-card${isOutOfStock ? ' out-of-stock' : ''}`}>
              {p.image_urls.length > 0 && (
                <div className="catalog-card-image-wrap">
                  <img
                    src={p.image_urls[0]}
                    alt={p.name}
                    className="catalog-card-image"
                  />
                </div>
              )}
              <div className="catalog-card-sku">{p.sku}</div>
              <div className="catalog-card-name">{p.name}</div>
              {p.description && (
                <div className="catalog-card-desc">{p.description}</div>
              )}
              <div className="catalog-card-availability">
                {isOutOfStock ? (
                  <span className="avail-badge out-of-stock">Out of Stock</span>
                ) : p.available_qty !== null ? (
                  <span className={`avail-badge${p.available_qty <= 5 ? ' low-stock' : ' in-stock'}`}>
                    {p.available_qty <= 5 ? `${p.available_qty} left` : 'In Stock'}
                  </span>
                ) : (
                  <span className="avail-badge in-stock">Available</span>
                )}
              </div>
              <div className="catalog-card-footer">
                {onViewProduct && (
                  <button
                    type="button"
                    className="btn-sm"
                    onClick={() => onViewProduct(p)}
                    aria-label={`View ${p.name}`}
                  >
                    View
                  </button>
                )}
                <button
                  type="button"
                  className="btn-add"
                  onClick={() => onAddProduct(p, 1)}
                  aria-label={`Add ${p.name} to cart`}
                  disabled={isOutOfStock}
                >
                  + Add
                </button>
              </div>
            </div>
          );
        })}
      </div>

      {/* Pagination */}
      {productList && productList.total > 0 && (
        <div className="catalog-pagination">
          <button
            type="button"
            className="btn-sm"
            disabled={page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            ← Prev
          </button>
          <span className="catalog-page-info">
            {page} / {totalPages}
          </span>
          <button
            type="button"
            className="btn-sm"
            disabled={page >= totalPages}
            onClick={() => setPage((p) => p + 1)}
          >
            Next →
          </button>
        </div>
      )}
    </div>
  );
}
