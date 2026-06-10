use {super::Price, assert_float_eq::assert_float_absolute_eq, std::num::NonZeroI64};

#[test]
fn price_constructs() {
    let price = Price::parse_str("42.68", -8).unwrap();
    assert_eq!(price.0.get(), 4_268_000_000);
    assert_float_absolute_eq!(price.to_f64(-8).unwrap(), 42.68);

    let price2 = Price::from_integer(2, -8).unwrap();
    assert_eq!(price2.0.get(), 200_000_000);
    assert_float_absolute_eq!(price2.to_f64(-8).unwrap(), 2.);

    let price3 = Price::from_mantissa(123_456).unwrap();
    assert_eq!(price3.0.get(), 123_456);
    assert_float_absolute_eq!(price3.to_f64(-8).unwrap(), 0.001_234_56);

    let price4 = Price::from_f64(42.68, -8).unwrap();
    assert_eq!(price4.0.get(), 4_268_000_000);
    assert_float_absolute_eq!(price4.to_f64(-8).unwrap(), 42.68);
}

#[test]
fn price_constructs_with_negative_mantissa() {
    let price = Price::parse_str("-42.68", -8).unwrap();
    assert_eq!(price.0.get(), -4_268_000_000);
    assert_float_absolute_eq!(price.to_f64(-8).unwrap(), -42.68);

    let price2 = Price::from_integer(-2, -8).unwrap();
    assert_eq!(price2.0.get(), -200_000_000);
    assert_float_absolute_eq!(price2.to_f64(-8).unwrap(), -2.);

    let price3 = Price::from_mantissa(-123_456).unwrap();
    assert_eq!(price3.0.get(), -123_456);
    assert_float_absolute_eq!(price3.to_f64(-8).unwrap(), -0.001_234_56);

    let price4 = Price::from_f64(-42.68, -8).unwrap();
    assert_eq!(price4.0.get(), -4_268_000_000);
    assert_float_absolute_eq!(price4.to_f64(-8).unwrap(), -42.68);
}

#[test]
fn price_constructs_with_zero_exponent() {
    let price = Price::parse_str("42", 0).unwrap();
    assert_eq!(price.0.get(), 42);
    assert_float_absolute_eq!(price.to_f64(0).unwrap(), 42.);

    let price2 = Price::from_integer(2, 0).unwrap();
    assert_eq!(price2.0.get(), 2);
    assert_float_absolute_eq!(price2.to_f64(0).unwrap(), 2.);

    let price3 = Price::from_mantissa(123_456).unwrap();
    assert_eq!(price3.0.get(), 123_456);
    assert_float_absolute_eq!(price3.to_f64(0).unwrap(), 123_456.);

    let price4 = Price::from_f64(42., 0).unwrap();
    assert_eq!(price4.0.get(), 42);
    assert_float_absolute_eq!(price4.to_f64(0).unwrap(), 42.);
}

#[test]
fn price_constructs_with_positive_exponent() {
    let price = Price::parse_str("42_680_000", 3).unwrap();
    assert_eq!(price.0.get(), 42_680);
    assert_float_absolute_eq!(price.to_f64(3).unwrap(), 42_680_000.);

    let price2 = Price::from_integer(200_000, 3).unwrap();
    assert_eq!(price2.0.get(), 200);
    assert_float_absolute_eq!(price2.to_f64(3).unwrap(), 200_000.);

    let price3 = Price::from_mantissa(123_456).unwrap();
    assert_eq!(price3.0.get(), 123_456);
    assert_float_absolute_eq!(price3.to_f64(3).unwrap(), 123_456_000.);

    let price4 = Price::from_f64(42_680_000., 3).unwrap();
    assert_eq!(price4.0.get(), 42_680);
    assert_float_absolute_eq!(price4.to_f64(3).unwrap(), 42_680_000.);
}

#[test]
fn price_rejects_zero_mantissa() {
    Price::parse_str("0.0", -8).unwrap_err();
    Price::from_integer(0, -8).unwrap_err();
    Price::from_mantissa(0).unwrap_err();
    Price::from_f64(-0.0, -8).unwrap_err();

    Price::parse_str("0.0", 8).unwrap_err();
    Price::from_integer(0, 8).unwrap_err();
    Price::from_f64(-0.0, 8).unwrap_err();
}

#[test]
fn price_rejects_too_precise() {
    Price::parse_str("42.68", 0).unwrap_err();
    Price::parse_str("42.68", -1).unwrap_err();
    Price::parse_str("42.68", -2).unwrap();

    Price::parse_str("42_680", 3).unwrap_err();
    Price::parse_str("42_600", 3).unwrap_err();
    Price::parse_str("42_000", 3).unwrap();
}

#[test]
fn price_ops() {
    let price1 = Price::parse_str("12.34", -8).unwrap();
    let price2 = Price::parse_str("23.45", -8).unwrap();
    assert_float_absolute_eq!(
        price1
            .add_with_same_exponent(price2)
            .unwrap()
            .to_f64(-8)
            .unwrap(),
        12.34 + 23.45
    );
    assert_float_absolute_eq!(
        price1
            .sub_with_same_exponent(price2)
            .unwrap()
            .to_f64(-8)
            .unwrap(),
        12.34 - 23.45
    );
    assert_float_absolute_eq!(
        price1.mul_integer(2).unwrap().to_f64(-8).unwrap(),
        12.34 * 2.
    );
    assert_float_absolute_eq!(
        price1.div_integer(2).unwrap().to_f64(-8).unwrap(),
        12.34 / 2.
    );

    assert_float_absolute_eq!(
        price1.mul_decimal(3456, -2).unwrap().to_f64(-8).unwrap(),
        12.34 * 34.56
    );

    assert_eq!(
        price1.mul_decimal(34, 2).unwrap().mantissa_i64(),
        1234000000 * 3400
    );
    let price2 = Price::parse_str("42_000", 3).unwrap();
    assert_float_absolute_eq!(
        price2.mul_integer(2).unwrap().to_f64(3).unwrap(),
        42_000. * 2.
    );
    assert_float_absolute_eq!(
        price2.div_integer(2).unwrap().to_f64(3).unwrap(),
        42_000. / 2.
    );
    assert_float_absolute_eq!(
        price2.mul_decimal(3456, -2).unwrap().to_f64(3).unwrap(),
        (42_000_f64 * 34.56 / 1000.).floor() * 1000.
    );
}

#[test]
fn has_moved_with_same_exponent_basic() {
    // 50 ppm = 0.5 bps
    let threshold_ppm = 50;
    let base = Price::from_mantissa(1_000_000).unwrap();

    // Exactly at threshold: 1_000_000 * 50 / 1_000_000 = 50 → not moved
    let at_threshold = Price::from_mantissa(1_000_050).unwrap();
    assert!(!base.has_moved_with_same_exponent(at_threshold, threshold_ppm));

    // Just above threshold
    let above = Price::from_mantissa(1_000_051).unwrap();
    assert!(base.has_moved_with_same_exponent(above, threshold_ppm));

    // No movement
    let same = Price::from_mantissa(1_000_000).unwrap();
    assert!(!base.has_moved_with_same_exponent(same, threshold_ppm));
}

#[test]
fn has_moved_with_same_exponent_negative_prices() {
    let threshold_ppm = 50;
    let base = Price::from_mantissa(-1_000_000).unwrap();

    let moved = Price::from_mantissa(-1_000_051).unwrap();
    assert!(base.has_moved_with_same_exponent(moved, threshold_ppm));

    let not_moved = Price::from_mantissa(-1_000_050).unwrap();
    assert!(!base.has_moved_with_same_exponent(not_moved, threshold_ppm));
}

#[test]
fn has_moved_with_same_exponent_zero_threshold() {
    let base = Price::from_mantissa(1_000_000).unwrap();

    // Any difference should count as moved with 0 ppm threshold
    let different = Price::from_mantissa(1_000_001).unwrap();
    assert!(base.has_moved_with_same_exponent(different, 0));

    // Same price should not count as moved even with 0 threshold
    let same = Price::from_mantissa(1_000_000).unwrap();
    assert!(!base.has_moved_with_same_exponent(same, 0));
}

#[test]
fn has_moved_with_same_exponent_lhs_overflows_rhs_does_not() {
    // diff * 1_000_000 overflows u64, old_abs * threshold_ppm does not → true
    let base = Price::from_nonzero_mantissa(NonZeroI64::new(1).unwrap());
    // diff = i64::MAX - 1, diff * 1_000_000 overflows
    let far = Price::from_nonzero_mantissa(NonZeroI64::new(i64::MAX).unwrap());
    // old_abs * threshold = 1 * 50 = 50, fits in u64
    assert!(base.has_moved_with_same_exponent(far, 50));
}

#[test]
fn has_moved_with_same_exponent_rhs_overflows_lhs_does_not() {
    // diff * 1_000_000 does not overflow, old_abs * threshold_ppm overflows → false
    let base = Price::from_nonzero_mantissa(NonZeroI64::new(i64::MAX).unwrap());
    // diff = 1, diff * 1_000_000 = 1_000_000 fits
    let near = Price::from_nonzero_mantissa(NonZeroI64::new(i64::MAX - 1).unwrap());
    // old_abs * threshold = i64::MAX * 50, overflows u64
    assert!(!base.has_moved_with_same_exponent(near, 50));
}

#[test]
fn has_moved_with_same_exponent_both_overflow() {
    // Both sides overflow → falls through to u128
    let base = Price::from_nonzero_mantissa(NonZeroI64::new(i64::MAX / 2).unwrap());
    // diff = i64::MAX / 2 - 1, diff * 1_000_000 overflows
    let far = Price::from_nonzero_mantissa(NonZeroI64::new(1).unwrap());
    // old_abs * 50 also overflows for i64::MAX / 2
    // actual: diff ≈ 4.6e18, diff * 1e6 ≈ 4.6e24 vs old_abs * 50 ≈ 2.3e20 → moved
    assert!(base.has_moved_with_same_exponent(far, 50));
}
