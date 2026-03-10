use apex_edge_contracts::{
    CartStateKind, CouponDefinition, PriceBookEntry, PromoAction, PromoCondition, Promotion,
    PromotionType, TaxRule,
};
use apex_edge_domain::{apply_promos_to_lines, AddLineItemInput, Cart, CartLineItem};
use apex_edge_domain::{base_price_cents, check_eligibility, coupon_discount_cents, tax_for_line};
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

#[test]
fn cart_progression_buy_2_promo_repeats_on_every_pair() {
    let mut cart = Cart::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let item_id = Uuid::new_v4();
    let promo = Promotion {
        id: Uuid::new_v4(),
        code: Some("BUY2_20".into()),
        name: "Buy 2 get 20% off".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
        priority: 10,
        valid_from: Utc::now() - Duration::minutes(1),
        valid_until: Some(Utc::now() + Duration::minutes(1)),
        conditions: vec![PromoCondition::ItemInBasket {
            item_id,
            min_quantity: 2,
        }],
        actions: vec![PromoAction::ApplyToItem {
            item_id,
            max_quantity: None,
        }],
        version: 1,
    };
    let promos = vec![promo];

    let add_and_reprice = |cart: &mut Cart, name_suffix: &str, line_id: Uuid| {
        cart.add_line_item(AddLineItemInput {
            line_id,
            item_id,
            sku: "BAKERY-106".into(),
            name: format!("Bakery 106 {name_suffix}"),
            quantity: 1,
            unit_price_cents: 100,
            modifier_option_ids: vec![],
            notes: None,
        });
        let results =
            apply_promos_to_lines(&cart.lines, |_| Uuid::nil(), &promos, cart.subtotal_cents());
        cart.apply_pricing(results);
    };

    // 1 item => no promo
    add_and_reprice(
        &mut cart,
        "A",
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("valid uuid"),
    );
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].discount_cents, 0);
    assert_eq!(cart.discount_cents(), 0);

    // 2 items => promo applies
    add_and_reprice(
        &mut cart,
        "B",
        Uuid::parse_str("00000000-0000-0000-0000-000000000002").expect("valid uuid"),
    );
    assert_eq!(cart.lines.len(), 2);
    let discount_after_two: u64 = cart.lines.iter().map(|l| l.discount_cents).sum();
    assert_eq!(discount_after_two, 40, "20% off first two items => 20 + 20");

    // 3rd item => third should not be discounted yet; total still only first pair discounted
    add_and_reprice(
        &mut cart,
        "C",
        Uuid::parse_str("00000000-0000-0000-0000-000000000003").expect("valid uuid"),
    );
    assert_eq!(cart.lines.len(), 3);
    assert_eq!(
        cart.lines[2].discount_cents, 0,
        "third item must not get promo discount"
    );
    let discount_after_three: u64 = cart.lines.iter().map(|l| l.discount_cents).sum();
    assert_eq!(
        discount_after_three, 40,
        "total discount must remain capped at two items"
    );

    // 4th item => promo repeats, so all 4 items are discounted
    add_and_reprice(
        &mut cart,
        "D",
        Uuid::parse_str("00000000-0000-0000-0000-000000000004").expect("valid uuid"),
    );
    assert_eq!(cart.lines.len(), 4);
    let discounted_line_count = cart.lines.iter().filter(|l| l.discount_cents > 0).count();
    assert_eq!(
        discounted_line_count, 4,
        "all four items should be discounted after second qualifying pair"
    );
    let discount_after_four: u64 = cart.lines.iter().map(|l| l.discount_cents).sum();
    assert_eq!(
        discount_after_four, 80,
        "20% off all four items => 80 total"
    );
}
