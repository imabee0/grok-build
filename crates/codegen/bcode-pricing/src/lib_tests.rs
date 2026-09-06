use super::*;
use chrono::TimeZone;

fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

// 2026-09-07 is a Monday; 2026-09-12 a Saturday.
const MON: (i32, u32, u32) = (2026, 9, 7);
const SAT: (i32, u32, u32) = (2026, 9, 12);

fn flat(input: f64, output: f64, cache_read: f64) -> Rates {
    Rates {
        input,
        output,
        cache_read: Some(cache_read),
        cache_write: None,
        reasoning: None,
    }
}

// --- the load-bearing contract: input is inclusive of cache ---------------

#[test]
fn cached_tokens_are_not_billed_twice() {
    // 1000 prompt tokens of which 800 came from cache: 200 at the input rate,
    // 800 at the cache rate. Billing the full 1000 at input would be the classic
    // double-count.
    let tokens = CallTokens {
        input_tokens: 1000,
        output_tokens: 0,
        cache_read_tokens: 800,
        ..Default::default()
    };
    // $10/1M input, $1/1M cache read.
    let ticks = cost_ticks(tokens, flat(10.0, 0.0, 1.0));
    // 200 * 10/1e6 + 800 * 1/1e6 = 0.002 + 0.0008 = $0.0028
    assert_eq!(ticks, 28_000_000);
}

#[test]
fn cache_larger_than_prompt_clamps_instead_of_going_negative() {
    let tokens = CallTokens {
        input_tokens: 100,
        cache_read_tokens: 900, // provider misreport
        ..Default::default()
    };
    assert!(cost_ticks(tokens, flat(10.0, 0.0, 1.0)) >= 0);
}

#[test]
fn reasoning_is_not_billed_twice_as_output() {
    let tokens = CallTokens {
        output_tokens: 1000,
        reasoning_tokens: 400,
        ..Default::default()
    };
    // Both rates equal, so splitting must not change the total.
    let split = cost_ticks(tokens, flat(0.0, 10.0, 0.0));
    let whole = cost_ticks(
        CallTokens {
            output_tokens: 1000,
            ..Default::default()
        },
        flat(0.0, 10.0, 0.0),
    );
    assert_eq!(split, whole);
}

#[test]
fn absent_cache_rate_falls_back_to_input_never_to_free() {
    let tokens = CallTokens {
        input_tokens: 1000,
        cache_read_tokens: 1000,
        ..Default::default()
    };
    let rates = Rates {
        input: 10.0,
        output: 0.0,
        cache_read: None,
        cache_write: None,
        reasoning: None,
    };
    // 1000 tokens at $10/1M = $0.01. Zero here would mean a silent free ride.
    assert_eq!(cost_ticks(tokens, rates), 100_000_000);
}

// --- exactness -------------------------------------------------------------

#[test]
fn many_small_calls_do_not_drift() {
    // The float-accumulation failure mode: 100k calls of one token each must
    // equal one call of 100k tokens exactly.
    let one = CallTokens {
        input_tokens: 1,
        ..Default::default()
    };
    let rates = flat(3.0, 0.0, 0.0);
    let summed: i64 = (0..100_000).map(|_| cost_ticks(one, rates)).sum();
    let bulk = cost_ticks(
        CallTokens {
            input_tokens: 100_000,
            ..Default::default()
        },
        rates,
    );
    assert_eq!(
        summed, bulk,
        "per-call rounding drifted from the bulk figure"
    );
}

// --- time of use -----------------------------------------------------------

fn deepseek() -> ModelPricing {
    PricingTable::builtin()
        .get("deepseek-v4-pro")
        .expect("deepseek-v4-pro in the built-in card")
        .clone()
}

#[test]
fn weekday_peak_blocks_are_double_off_peak() {
    let p = deepseek();
    let off = p.rates_at(at(MON.0, MON.1, MON.2, 12, 0)); // 12:00 Mon, off-peak
    let peak = p.rates_at(at(MON.0, MON.1, MON.2, 2, 0)); // 02:00 Mon, peak
    assert!(off.window.is_none());
    assert!(peak.window.is_some());
    assert!((peak.rates.input - off.rates.input * 2.0).abs() < 1e-9);
}

#[test]
fn monday_before_the_first_peak_block_is_off_peak() {
    // Regression: expressing this as a midnight-wrapping OFF-peak window with a
    // weekday filter priced Mon 00:30 at peak, because the window began Sunday.
    let p = deepseek();
    assert!(p.rates_at(at(MON.0, MON.1, MON.2, 0, 30)).window.is_none());
}

#[test]
fn the_gap_between_peak_blocks_is_off_peak() {
    let p = deepseek();
    assert!(p.rates_at(at(MON.0, MON.1, MON.2, 5, 0)).window.is_none());
}

#[test]
fn weekends_are_never_peak() {
    let p = deepseek();
    for hour in [0, 2, 7, 12, 23] {
        assert!(
            p.rates_at(at(SAT.0, SAT.1, SAT.2, hour, 0))
                .window
                .is_none(),
            "saturday {hour}:00 priced as peak"
        );
    }
}

#[test]
fn window_boundaries_are_half_open() {
    let p = deepseek();
    assert!(
        p.rates_at(at(MON.0, MON.1, MON.2, 1, 0)).window.is_some(),
        "start is inclusive"
    );
    assert!(
        p.rates_at(at(MON.0, MON.1, MON.2, 4, 0)).window.is_none(),
        "end is exclusive"
    );
}

#[test]
fn a_call_is_priced_when_it_happened_not_when_it_is_displayed() {
    let p = deepseek();
    let tokens = CallTokens {
        input_tokens: 1_000_000,
        ..Default::default()
    };
    let during_peak = p.cost_ticks_at(tokens, at(MON.0, MON.1, MON.2, 2, 0));
    let during_off = p.cost_ticks_at(tokens, at(MON.0, MON.1, MON.2, 12, 0));
    assert_eq!(during_peak, during_off * 2);
}

#[test]
fn wrapping_windows_are_supported_even_though_the_card_avoids_them() {
    let w = Window {
        utc_offset_hours: 0,
        start: "22:00".into(),
        end: "02:00".into(),
        days: vec![],
        rates: None,
        multiplier: Some(0.5),
    };
    assert!(w.contains(at(2026, 9, 7, 23, 0)));
    assert!(w.contains(at(2026, 9, 8, 1, 0)));
    assert!(!w.contains(at(2026, 9, 8, 3, 0)));
}

#[test]
fn a_wrapping_window_is_filtered_by_the_day_it_started() {
    // Friday 23:00 -> Saturday 01:00 belongs to Friday.
    let w = Window {
        utc_offset_hours: 0,
        start: "22:00".into(),
        end: "02:00".into(),
        days: vec!["fri".into()],
        rates: None,
        multiplier: Some(0.5),
    };
    assert!(w.contains(at(2026, 9, 11, 23, 0)), "friday evening");
    assert!(
        w.contains(at(2026, 9, 12, 1, 0)),
        "saturday small hours, friday's window"
    );
    assert!(
        !w.contains(at(2026, 9, 12, 23, 0)),
        "saturday evening is not friday's"
    );
}

#[test]
fn offsets_are_honoured() {
    let w = Window {
        utc_offset_hours: 8,
        start: "09:00".into(),
        end: "12:00".into(),
        days: vec![],
        rates: None,
        multiplier: Some(2.0),
    };
    // 09:00 UTC+8 is 01:00 UTC.
    assert!(w.contains(at(2026, 9, 7, 1, 0)));
    assert!(!w.contains(at(2026, 9, 7, 9, 0)));
}

// --- table and formatting ---------------------------------------------------

#[test]
fn builtin_card_parses_and_ids_with_dots_survive() {
    let t = PricingTable::builtin();
    for id in [
        "deepseek-v4-pro",
        "deepseek-v4-flash",
        "deepseek-v4-flash-vision-exp",
        "gpt-5.2-codex",
        "grok-4.6",
    ] {
        assert!(
            t.get(id).is_some(),
            "{id} missing: a dotted id was split by TOML"
        );
    }
}

#[test]
fn an_unpriced_model_is_unknown_not_free() {
    let t = PricingTable::builtin();
    assert_eq!(
        t.cost_ticks("some-local-llama", CallTokens::default(), Utc::now()),
        None
    );
}

#[test]
fn user_overrides_replace_per_model_only() {
    let mut t = PricingTable::builtin();
    let before = t.get("deepseek-v4-flash").unwrap().base.input;
    let user: PricingTable = toml::from_str(
        r#"
        [pricing."deepseek-v4-pro"]
        input = 99.0
        output = 99.0
        "#,
    )
    .unwrap();
    t.overlay(user);
    assert_eq!(t.get("deepseek-v4-pro").unwrap().base.input, 99.0);
    assert_eq!(t.get("deepseek-v4-flash").unwrap().base.input, before);
}

#[test]
fn cents_below_a_dollar_dollars_above() {
    assert_eq!(format_ticks(4_320_000_000), "43.2\u{00A2}");
    assert_eq!(format_ticks(15_000_000_000), "$1.50");
    assert_eq!(format_ticks(0), "0.0\u{00A2}");
}
