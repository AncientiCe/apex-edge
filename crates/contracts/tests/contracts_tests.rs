use apex_edge_contracts::{
    build_submission_envelope, ContractVersion, HqOrderLine, HqOrderPayload, HqPayment,
};
use chrono::Utc;
use uuid::Uuid;

#[test]
fn contract_version_displays_semver() {
    let v = ContractVersion::new(1, 2, 3);
    assert_eq!(v.to_string(), "1.2.3");
}

#[test]
fn hq_submission_checksum_is_deterministic_for_same_inputs() {
    let payload = HqOrderPayload {
        order_id: Uuid::new_v4(),
        cart_id: Uuid::new_v4(),
        created_at: Utc::now(),
        lines: vec![HqOrderLine {
            line_id: Uuid::new_v4(),
            item_id: Uuid::new_v4(),
            sku: "SKU-1".into(),
            name: "Coffee".into(),
            quantity: 1,
            unit_price_cents: 300,
            line_total_cents: 300,
            discount_cents: 0,
            tax_cents: 30,
            modifier_option_ids: vec![],
            notes: None,
        }],
        subtotal_cents: 300,
        discount_cents: 0,
        tax_cents: 30,
        total_cents: 330,
        payments: vec![HqPayment {
            tender_id: Uuid::new_v4(),
            amount_cents: 330,
            tip_amount_cents: 0,
            external_reference: None,
            provider: None,
            provider_payment_id: None,
            entry_method: None,
        }],
        applied_promo_ids: vec![],
        applied_coupons: vec![],
        metadata: None,
    };

    let submission_id = Uuid::new_v4();
    let store_id = Uuid::new_v4();
    let register_id = Uuid::new_v4();

    let a = build_submission_envelope(submission_id, store_id, register_id, 42, payload.clone());
    let b = build_submission_envelope(submission_id, store_id, register_id, 42, payload);

    assert_eq!(a.version, ContractVersion::V1_0_0);
    assert_eq!(a.checksum, b.checksum);
}
