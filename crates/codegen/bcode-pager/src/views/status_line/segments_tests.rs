use super::*;

const DIR: &str = "/home/user/project";

fn context() -> StatusLineContext {
    let mut ctx = crate::app::status_line::test_context(DIR);
    ctx.session_name = Some("status_line work".into());
    ctx.model.display_name = Some("bcode".into());
    ctx.cost.total_cost_usd = Some(0.3745);
    ctx.context_window.used_percentage = Some(42);
    ctx.context_window.auto_compact_threshold_percent = Some(80);
    ctx.context_window.session_usage = Some(bcode_status_line::StatusLineSessionUsage {
        input_tokens: 12_000,
        output_tokens: 3_100,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 88_000,
    });
    ctx
}

fn plain(ctx: &StatusLineContext, turn_elapsed: Option<Duration>) -> String {
    compose_builtin(ctx, turn_elapsed, StatusLineItem::ALL)
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(SEGMENT_SEPARATOR)
}

#[test]
fn composes_every_segment_in_order() {
    assert_eq!(
        plain(&context(), Some(Duration::from_secs(83))),
        "project │ bcode │ 42% ctx │ 37.5¢ │ 100k↑ 3.1k↓ │ 88% cached │ 1m23s │ status_line work"
    );
}

#[test]
fn omits_segments_whose_data_is_missing_or_rounds_to_zero() {
    let mut ctx = context();
    ctx.cost.total_cost_usd = Some(0.0004);
    assert_eq!(
        plain(&ctx, Some(Duration::from_millis(400))),
        "project │ bcode │ 42% ctx │ 100k↑ 3.1k↓ │ 88% cached │ status_line work"
    );

    // Cents, not dollars, below $1: the smallest spend the row can paint is a
    // tenth of a cent, where "$0.01" rounds up and "$0.00" reads as free.
    ctx.cost.total_cost_usd = Some(0.0006);
    assert!(plain(&ctx, None).contains("0.1¢"), "{}", plain(&ctx, None));

    // A dollar and over stays in dollars.
    ctx.cost.total_cost_usd = Some(2.5);
    assert!(plain(&ctx, None).contains("$2.50"));

    ctx.context_window.used_percentage = None;
    assert!(compose_builtin(&ctx, None, &[StatusLineItem::Context]).is_empty());
}

#[test]
fn name_past_its_budget_is_cut_by_painted_columns() {
    let mut ctx = context();
    ctx.session_name = Some("辺".repeat(SESSION_NAME_COLS));
    let cut = &compose_builtin(&ctx, None, &[StatusLineItem::SessionName])[0].text;

    let width = super::super::painted_width(cut);
    assert!(
        cut.ends_with('…'),
        "a cut name has to say it was cut: {cut}"
    );
    assert!(
        width <= SESSION_NAME_COLS,
        "{width} columns overruns the {SESSION_NAME_COLS} the segment was given"
    );
    // The cut fills the budget to within one cluster; a byte or character cut would leave more unused
    assert!(
        width + 2 > SESSION_NAME_COLS,
        "{width} columns leaves more than a cluster of the budget unused"
    );
}

#[test]
fn cost_the_session_does_not_have_omits_its_segment() {
    let mut ctx = context();
    ctx.cost.total_cost_usd = None;
    assert!(compose_builtin(&ctx, None, &[StatusLineItem::Cost]).is_empty());
}

#[test]
fn context_segment_warns_near_compaction() {
    let mut ctx = context();
    let tone =
        |ctx: &StatusLineContext| compose_builtin(ctx, None, &[StatusLineItem::Context])[0].tone;

    ctx.context_window.used_percentage = Some(90);
    assert_eq!(tone(&ctx), SegmentTone::Warn);
    ctx.context_window.used_percentage = Some(50);
    assert_eq!(tone(&ctx), SegmentTone::Dim);

    ctx.context_window.auto_compact_threshold_percent = Some(65);
    ctx.context_window.used_percentage = Some(70);
    assert_eq!(tone(&ctx), SegmentTone::Warn);
}

#[test]
fn token_counts_truncate_so_they_never_overstate() {
    // 1999 is 1.9k, never 2.0k: the figure must not claim tokens that were not
    // used, and a status line has no room for exact counts.
    assert_eq!(super::compact_count(999), "999");
    assert_eq!(super::compact_count(1_999), "1.9k");
    assert_eq!(super::compact_count(12_345), "12k");
    assert_eq!(super::compact_count(1_999_999), "1.9M");
    assert_eq!(super::compact_count(0), "0");
}

#[test]
fn cache_share_counts_every_input_bucket() {
    let mut ctx = context();
    // The payload's input_tokens is disjoint from the cache buckets, so the
    // denominator is the sum of all three, not input_tokens alone.
    ctx.context_window.session_usage = Some(bcode_status_line::StatusLineSessionUsage {
        input_tokens: 25,
        output_tokens: 0,
        cache_creation_input_tokens: 25,
        cache_read_input_tokens: 50,
    });
    assert_eq!(
        compose_builtin(&ctx, None, &[StatusLineItem::Cache])[0].text,
        "50% cached"
    );
}

#[test]
fn token_and_cache_segments_vanish_before_the_first_call() {
    let mut ctx = context();
    ctx.context_window.session_usage = None;
    assert!(compose_builtin(&ctx, None, &[StatusLineItem::Tokens]).is_empty());
    assert!(compose_builtin(&ctx, None, &[StatusLineItem::Cache]).is_empty());
}
