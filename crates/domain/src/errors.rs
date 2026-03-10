//! Domain errors.

use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum DomainError {
    #[error("cart not found: {0}")]
    CartNotFound(Uuid),

    #[error("invalid transition: {0}")]
    InvalidTransition(String),

    #[error("item not found: {0}")]
    ItemNotFound(Uuid),

    #[error("line not found: {0}")]
    LineNotFound(Uuid),

    #[error("promo not applicable: {0}")]
    PromoNotApplicable(String),

    #[error("coupon invalid: {0}")]
    CouponInvalid(String),

    #[error("payment exceeds total")]
    PaymentExceedsTotal,

    #[error("tender not allowed")]
    TenderNotAllowed,

    #[error("validation: {0}")]
    Validation(String),
}
