//! Loyalty provider trait and local points implementation.

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoyaltyAccount {
    pub customer_id: Uuid,
    pub points: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarnRequest {
    pub customer_id: Uuid,
    pub spend_cents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedeemRequest {
    pub customer_id: Uuid,
    pub points: u64,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LoyaltyError {
    #[error("loyalty amount must be greater than zero")]
    InvalidAmount,
    #[error("not enough loyalty points")]
    InsufficientPoints,
}

pub trait LoyaltyProvider: Send + Sync {
    fn provider_code(&self) -> &'static str;
    fn earn(&self, account: &mut LoyaltyAccount, request: EarnRequest)
        -> Result<u64, LoyaltyError>;
    fn redeem(
        &self,
        account: &mut LoyaltyAccount,
        request: RedeemRequest,
    ) -> Result<u64, LoyaltyError>;
}

#[derive(Debug, Clone)]
pub struct LocalLoyaltyProvider {
    cents_per_point: u64,
    cents_per_redeemed_point: u64,
}

impl LocalLoyaltyProvider {
    pub fn new(cents_per_point: u64, cents_per_redeemed_point: u64) -> Self {
        Self {
            cents_per_point: cents_per_point.max(1),
            cents_per_redeemed_point: cents_per_redeemed_point.max(1),
        }
    }
}

impl LoyaltyProvider for LocalLoyaltyProvider {
    fn provider_code(&self) -> &'static str {
        "local"
    }

    fn earn(
        &self,
        account: &mut LoyaltyAccount,
        request: EarnRequest,
    ) -> Result<u64, LoyaltyError> {
        if request.spend_cents == 0 {
            return Err(LoyaltyError::InvalidAmount);
        }
        let earned = request.spend_cents / self.cents_per_point;
        account.points = account.points.saturating_add(earned);
        Ok(earned)
    }

    fn redeem(
        &self,
        account: &mut LoyaltyAccount,
        request: RedeemRequest,
    ) -> Result<u64, LoyaltyError> {
        if request.points == 0 {
            return Err(LoyaltyError::InvalidAmount);
        }
        if request.points > account.points {
            return Err(LoyaltyError::InsufficientPoints);
        }
        account.points -= request.points;
        Ok(request.points.saturating_mul(self.cents_per_redeemed_point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_loyalty_earns_and_redeems_points() {
        let customer_id = Uuid::new_v4();
        let provider = LocalLoyaltyProvider::new(100, 1);
        let mut account = LoyaltyAccount {
            customer_id,
            points: 0,
        };

        let earned = provider
            .earn(
                &mut account,
                EarnRequest {
                    customer_id,
                    spend_cents: 1_250,
                },
            )
            .expect("earn");
        let redeemed_cents = provider
            .redeem(
                &mut account,
                RedeemRequest {
                    customer_id,
                    points: 10,
                },
            )
            .expect("redeem");

        assert_eq!(earned, 12);
        assert_eq!(redeemed_cents, 10);
        assert_eq!(account.points, 2);
    }
}
