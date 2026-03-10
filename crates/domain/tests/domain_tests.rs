use apex_edge_contracts::{CartStateKind, CouponDefinition, PriceBookEntry, TaxRule};
use apex_edge_domain::{base_price_cents, check_eligibility, coupon_discount_cents, tax_for_line};
use apex_edge_domain::{Cart, CartLineItem};
use chrono::{Duration, Utc};
use uuid::Uuid;

#[test]
fn pricing_pipeline_primitives_compute_expected_values() {
    let item = Uuid::new_v4();
    let mod1 = Uuid::new_v4();
    let tax_cat = Uuid::new_v4();

    let entries = vec![
        PriceBookEntry {
            item_id: item,
            modifier_option_id: None,
            price_cents: 500,
            currency: "USD".into(),
        },
        PriceBookEntry {
            item_id: item,
            modifier_option_id: Some(mod1),
            price_cents: 100,
            currency: "USD".into(),
        },
    ];
    let total = base_price_cents(item, &[mod1], 2, &entries);
    assert_eq!(total, 1200);

    let rules = vec![TaxRule {
        id: Uuid::new_v4(),
        tax_category_id: tax_cat,
        rate_bps: 1000,
        name: "VAT".into(),
        inclusive: false,
        version: 1,
    }];
    assert_eq!(tax_for_line(1200, tax_cat, &rules, false), 120);
    assert_eq!(coupon_discount_cents(300, 250), 250);
}

#[test]
fn cart_payment_and_finalize_journey_works() {
    let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    cart.lines.push(CartLineItem {
        line_id: Uuid::new_v4(),
        item_id: Uuid::new_v4(),
        sku: "SKU".into(),
        name: "Item".into(),
        quantity: 1,
        modifier_option_ids: vec![],
        notes: None,
        unit_price_cents: 1000,
        line_total_cents: 1000,
        discount_cents: 0,
        tax_cents: 100,
    });
    cart.state = CartStateKind::Itemized;
    cart.set_tendering();
    cart.add_payment(Uuid::new_v4(), 1100, None)
        .expect("payment should succeed");
    assert_eq!(cart.state, CartStateKind::Paid);
    let order = cart.to_order(Uuid::new_v4()).expect("to_order should work");
    assert_eq!(order.total_cents, 1100);
}

#[test]
fn coupon_eligibility_rejects_expired_coupon() {
    let def = CouponDefinition {
        id: Uuid::new_v4(),
        code: "OLD".into(),
        promo_id: Uuid::new_v4(),
        max_redemptions_total: Some(10),
        max_redemptions_per_customer: Some(1),
        valid_from: Utc::now() - Duration::days(10),
        valid_until: Some(Utc::now() - Duration::days(1)),
        version: 1,
    };
    let out = check_eligibility(&def, 0, Some(0), 1000, 0);
    assert!(!out.valid);
    assert_eq!(out.reason.as_deref(), Some("coupon expired"));
}
