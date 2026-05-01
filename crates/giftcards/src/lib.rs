//! Local-first gift card state machine.

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GiftCardState {
    Issued,
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GiftCard {
    pub id: Uuid,
    pub code: String,
    pub balance_cents: u64,
    pub currency: String,
    pub state: GiftCardState,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum GiftCardError {
    #[error("gift card is not active")]
    NotActive,
    #[error("amount must be greater than zero")]
    InvalidAmount,
    #[error("insufficient gift card balance")]
    InsufficientBalance,
}

impl GiftCard {
    pub fn issue(code: impl Into<String>, currency: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            code: code.into(),
            balance_cents: 0,
            currency: currency.into(),
            state: GiftCardState::Issued,
        }
    }

    pub fn activate(&mut self, opening_balance_cents: u64) -> Result<(), GiftCardError> {
        validate_amount(opening_balance_cents)?;
        self.balance_cents = opening_balance_cents;
        self.state = GiftCardState::Active;
        Ok(())
    }

    pub fn reload(&mut self, amount_cents: u64) -> Result<(), GiftCardError> {
        ensure_active(&self.state)?;
        validate_amount(amount_cents)?;
        self.balance_cents = self.balance_cents.saturating_add(amount_cents);
        Ok(())
    }

    pub fn redeem(&mut self, amount_cents: u64) -> Result<u64, GiftCardError> {
        ensure_active(&self.state)?;
        validate_amount(amount_cents)?;
        if amount_cents > self.balance_cents {
            return Err(GiftCardError::InsufficientBalance);
        }
        self.balance_cents -= amount_cents;
        Ok(amount_cents)
    }
}

fn validate_amount(amount_cents: u64) -> Result<(), GiftCardError> {
    if amount_cents == 0 {
        return Err(GiftCardError::InvalidAmount);
    }
    Ok(())
}

fn ensure_active(state: &GiftCardState) -> Result<(), GiftCardError> {
    if *state != GiftCardState::Active {
        return Err(GiftCardError::NotActive);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gift_card_issue_activate_reload_and_redeem() {
        let mut card = GiftCard::issue("GC-123", "USD");

        assert_eq!(card.state, GiftCardState::Issued);
        card.activate(2_500).expect("activate");
        card.reload(500).expect("reload");
        let redeemed = card.redeem(1_200).expect("redeem");

        assert_eq!(redeemed, 1_200);
        assert_eq!(card.balance_cents, 1_800);
    }

    #[test]
    fn gift_card_rejects_over_redeem() {
        let mut card = GiftCard::issue("GC-123", "USD");
        card.activate(500).expect("activate");

        assert_eq!(card.redeem(600), Err(GiftCardError::InsufficientBalance));
    }
}
