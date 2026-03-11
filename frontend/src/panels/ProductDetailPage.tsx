import { useState } from 'react';
import type { ProductSearchResult } from '../api/types';

interface Props {
  product: ProductSearchResult | null;
  onAddProduct: (product: ProductSearchResult, quantity: number) => void;
  onBack: () => void;
}

function AvailabilityBadge({ product }: { product: ProductSearchResult }) {
  if (!product.is_active || (product.available_qty !== null && product.available_qty <= 0)) {
    return <span className="availability-badge out-of-stock">Out of Stock</span>;
  }
  if (product.available_qty === null) {
    return <span className="availability-badge available">Available</span>;
  }
  if (product.available_qty <= 5) {
    return (
      <span className="availability-badge low-stock">
        {product.available_qty} in stock — low stock
      </span>
    );
  }
  return (
    <span className="availability-badge available">
      {product.available_qty} in stock
    </span>
  );
}

function isSellable(product: ProductSearchResult): boolean {
  if (!product.is_active) return false;
  if (product.available_qty !== null && product.available_qty <= 0) return false;
  return true;
}

export function ProductDetailPage({ product, onAddProduct, onBack }: Props) {
  const [selectedImage, setSelectedImage] = useState(0);
  const [quantity, setQuantity] = useState(1);

  if (!product) {
    return (
      <div className="pdp-loading">
        <p>Loading…</p>
      </div>
    );
  }

  const sellable = isSellable(product);
  const maxQty =
    product.available_qty !== null ? product.available_qty : Infinity;

  function decrement() {
    setQuantity((q) => Math.max(1, q - 1));
  }

  function increment() {
    setQuantity((q) => Math.min(isFinite(maxQty) ? maxQty : 99, q + 1));
  }

  function handleQtyChange(e: React.ChangeEvent<HTMLInputElement>) {
    const v = parseInt(e.target.value, 10);
    if (!isNaN(v) && v >= 1) {
      setQuantity(isFinite(maxQty) ? Math.min(v, maxQty) : v);
    }
  }

  function handleAdd() {
    if (!sellable || !product) return;
    onAddProduct(product, quantity);
  }

  return (
    <div className="pdp-page">
      <button type="button" className="pdp-back btn-secondary" onClick={onBack}>
        ← Back
      </button>

      <div className="pdp-layout">
        {/* Image gallery */}
        <div className="pdp-gallery">
          <div className="pdp-gallery-main">
            {product.image_urls.length > 0 ? (
              <img
                src={product.image_urls[selectedImage]}
                alt={product.name}
                className="pdp-main-image"
              />
            ) : (
              <div className="pdp-no-image" aria-label={`${product.name} placeholder`}>
                <span className="pdp-no-image-icon">📦</span>
              </div>
            )}
          </div>

          {product.image_urls.length > 1 && (
            <div className="pdp-thumbnail-strip" role="list">
              {product.image_urls.map((url, idx) => (
                <button
                  key={url}
                  type="button"
                  className={`pdp-thumbnail-btn${idx === selectedImage ? ' active' : ''}`}
                  onClick={() => setSelectedImage(idx)}
                  aria-pressed={idx === selectedImage}
                  role="listitem"
                >
                  <img
                    src={url}
                    alt={`thumbnail ${idx + 1}`}
                    className="pdp-thumbnail-img"
                  />
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Product details */}
        <div className="pdp-details">
          <p className="pdp-sku">{product.sku}</p>
          <h2 className="pdp-name">{product.name}</h2>

          <AvailabilityBadge product={product} />

          {product.description && (
            <p className="pdp-description">{product.description}</p>
          )}

          {/* Quantity stepper */}
          <div className="pdp-quantity-row">
            <span className="pdp-quantity-label">Quantity</span>
            <div className="pdp-stepper">
              <button
                type="button"
                className="pdp-stepper-btn"
                onClick={decrement}
                disabled={!sellable || quantity <= 1}
                aria-label="−"
              >
                −
              </button>
              <input
                type="number"
                min={1}
                max={isFinite(maxQty) ? maxQty : undefined}
                value={quantity}
                onChange={handleQtyChange}
                className="pdp-stepper-input"
                disabled={!sellable}
                aria-label="quantity"
              />
              <button
                type="button"
                className="pdp-stepper-btn"
                onClick={increment}
                disabled={!sellable || (isFinite(maxQty) && quantity >= maxQty)}
                aria-label="+"
              >
                +
              </button>
            </div>
          </div>

          <button
            type="button"
            className="btn-primary pdp-add-btn"
            onClick={handleAdd}
            disabled={!sellable}
            data-testid="add-to-cart-btn"
          >
            Add to Cart
          </button>
        </div>
      </div>
    </div>
  );
}
