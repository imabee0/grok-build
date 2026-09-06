//! Local cost calculation for providers that do not report a price.
//!
//! Some endpoints return the cost of a request directly; those always win and
//! never reach this crate. Everyone else bills from a rate card, so bcode has to
//! do the arithmetic itself, and there are three ways to get it wrong:
//!
//! 1. **Double-billing cached input.** `prompt_tokens` is *inclusive* of cache
//!    reads and writes on every provider bcode supports, so billable input is
//!    `prompt - cache_read - cache_write`. Charging the full prompt at the input
//!    rate *and* the cached portion at the cache rate overstates every cached
//!    call. This is the single load-bearing decision here.
//! 2. **Losing money to floats.** Rates are fractions of a cent per token and a
//!    session makes thousands of calls, so everything below is exact integer
//!    arithmetic in USD ticks (1e10 per USD) with no intermediate float.
//! 3. **Pricing the whole session at today's rate.** Providers with time-of-day
//!    pricing must be billed per call at the rate in force *when the call was
//!    made*, never recomputed at display time.
//!
//! Rates are USD per 1,000,000 tokens, matching how providers publish them.

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};

/// USD ticks per dollar. Matches the wire encoding used for provider-reported
/// costs, so locally computed and provider-reported costs are the same unit.
pub const TICKS_PER_USD: i128 = 10_000_000_000;

/// Token counts for one call, as reported by the provider.
///
/// `input_tokens` is inclusive of `cache_read_tokens` and `cache_write_tokens`;
/// `output_tokens` is inclusive of `reasoning_tokens`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallTokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
}

/// One rate card, in USD per 1M tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    /// Uncached prompt tokens.
    pub input: f64,
    /// Completion tokens, reasoning included unless `reasoning` is set.
    pub output: f64,
    /// Prompt tokens served from cache. Defaults to the input rate when unset,
    /// which is the conservative reading: never quietly bill a cache hit as free.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Prompt tokens written to cache. Defaults to the input rate when unset.
    #[serde(default)]
    pub cache_write: Option<f64>,
    /// Reasoning tokens, when a provider bills them separately. Defaults to the
    /// output rate.
    #[serde(default)]
    pub reasoning: Option<f64>,
}

/// A recurring window in which a different rate applies.
///
/// Expressed in a named IANA-style offset rather than local time: providers
/// publish these in their own timezone and a developer's laptop is not it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// Fixed offset from UTC, in hours, that `start`/`end` are expressed in.
    #[serde(default)]
    pub utc_offset_hours: i8,
    /// Inclusive start, `"HH:MM"`.
    pub start: String,
    /// Exclusive end, `"HH:MM"`. May be less than `start` to wrap midnight.
    pub end: String,
    /// Weekdays the window applies to, lowercase three-letter names. Empty = all.
    #[serde(default)]
    pub days: Vec<String>,
    /// Rates while inside the window. Mutually exclusive with `multiplier`.
    #[serde(default)]
    pub rates: Option<Rates>,
    /// Scale the base rates instead of replacing them. Providers usually
    /// document off-peak as "half price", and one number cannot drift from the
    /// base card the way a duplicated table can.
    #[serde(default)]
    pub multiplier: Option<f64>,
}

/// Base rates plus any time-of-use windows for one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    #[serde(flatten)]
    pub base: Rates,
    #[serde(default)]
    pub window: Vec<Window>,
}

/// The rate card in force, and the label to show for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveRates {
    pub rates: Rates,
    /// `None` when the base card applies; `Some(index)` names the window.
    pub window: Option<usize>,
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    // 24:00 is a legitimate exclusive end-of-day.
    if h > 24 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

fn weekday_name(d: chrono::Weekday) -> &'static str {
    match d {
        chrono::Weekday::Mon => "mon",
        chrono::Weekday::Tue => "tue",
        chrono::Weekday::Wed => "wed",
        chrono::Weekday::Thu => "thu",
        chrono::Weekday::Fri => "fri",
        chrono::Weekday::Sat => "sat",
        chrono::Weekday::Sun => "sun",
    }
}

impl Window {
    /// Whether `at` falls inside this window.
    ///
    /// A window that wraps midnight (`end <= start`) is two half-open intervals,
    /// and its weekday filter applies to the day the window *starts*, so an
    /// overnight Friday window does not silently become a Saturday one.
    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        let Some(start) = parse_hhmm(&self.start) else {
            return false;
        };
        let Some(end) = parse_hhmm(&self.end) else {
            return false;
        };
        let offset = chrono::FixedOffset::east_opt(i32::from(self.utc_offset_hours) * 3600);
        let Some(offset) = offset else {
            return false;
        };
        let local = at.with_timezone(&offset);
        let minute = local.hour() * 60 + local.minute();

        let (inside, start_day) = if start <= end {
            (minute >= start && minute < end, local.weekday())
        } else if minute >= start {
            (true, local.weekday()) // evening half: window began today
        } else if minute < end {
            (true, local.weekday().pred()) // morning half: window began yesterday
        } else {
            (false, local.weekday())
        };
        if !inside {
            return false;
        }
        self.days.is_empty()
            || self
                .days
                .iter()
                .any(|d| d.eq_ignore_ascii_case(weekday_name(start_day)))
    }
}

impl Rates {
    fn scaled(self, factor: f64) -> Self {
        Self {
            input: self.input * factor,
            output: self.output * factor,
            cache_read: self.cache_read.map(|v| v * factor),
            cache_write: self.cache_write.map(|v| v * factor),
            reasoning: self.reasoning.map(|v| v * factor),
        }
    }
}

impl ModelPricing {
    /// The rates in force at `at`. First matching window wins, so order is
    /// significant and the most specific window belongs first.
    pub fn rates_at(&self, at: DateTime<Utc>) -> EffectiveRates {
        for (i, w) in self.window.iter().enumerate() {
            if !w.contains(at) {
                continue;
            }
            let rates = match (w.rates, w.multiplier) {
                (Some(r), _) => r,
                (None, Some(m)) => self.base.scaled(m),
                (None, None) => self.base,
            };
            return EffectiveRates {
                rates,
                window: Some(i),
            };
        }
        EffectiveRates {
            rates: self.base,
            window: None,
        }
    }

    /// Cost of one call in USD ticks, priced at the rate in force at `at`.
    pub fn cost_ticks_at(&self, tokens: CallTokens, at: DateTime<Utc>) -> i64 {
        cost_ticks(tokens, self.rates_at(at).rates)
    }
}

/// USD-per-1M-tokens rate applied to a token count, as exact integer ticks.
///
/// `rate * 1e10 / 1e6` is `rate * 1e4`, so a rate is converted to
/// ticks-per-million once, then multiplied by the count and divided down. The
/// only rounding is the final division, and it rounds to nearest.
fn line_ticks(tokens: u64, usd_per_million: f64) -> i128 {
    if tokens == 0 || usd_per_million <= 0.0 {
        return 0;
    }
    let ticks_per_million = (usd_per_million * TICKS_PER_USD as f64).round() as i128;
    let product = i128::from(tokens) * ticks_per_million;
    (product + 500_000) / 1_000_000
}

/// Cost of one call in USD ticks under a fixed rate card.
///
/// Cached and cache-written tokens are subtracted from the prompt before the
/// input rate is applied, because every provider bcode supports reports
/// `prompt_tokens` inclusive of them. Subtractions saturate: a provider that
/// reports more cached tokens than prompt tokens must not produce a negative
/// charge.
pub fn cost_ticks(tokens: CallTokens, rates: Rates) -> i64 {
    let cached = tokens
        .cache_read_tokens
        .saturating_add(tokens.cache_write_tokens);
    let billable_input = tokens.input_tokens.saturating_sub(cached);
    let billable_output = tokens.output_tokens.saturating_sub(tokens.reasoning_tokens);

    let cache_read_rate = rates.cache_read.unwrap_or(rates.input);
    let cache_write_rate = rates.cache_write.unwrap_or(rates.input);
    let reasoning_rate = rates.reasoning.unwrap_or(rates.output);

    let total = line_ticks(billable_input, rates.input)
        + line_ticks(billable_output, rates.output)
        + line_ticks(tokens.cache_read_tokens, cache_read_rate)
        + line_ticks(tokens.cache_write_tokens, cache_write_rate)
        + line_ticks(tokens.reasoning_tokens, reasoning_rate);

    total.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// The whole price list, keyed by catalog model id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PricingTable {
    #[serde(default)]
    pub pricing: std::collections::HashMap<String, ModelPricing>,
}

impl PricingTable {
    /// Built-in rate card. Overridden per model by `[pricing.<id>]` in config.
    pub fn builtin() -> Self {
        toml::from_str(include_str!("../pricing.toml")).expect("built-in pricing.toml is malformed")
    }

    pub fn get(&self, model_id: &str) -> Option<&ModelPricing> {
        self.pricing.get(model_id)
    }

    /// Cost of one call, or `None` when the model has no rate card -- which the
    /// UI must render as "unknown", never as zero.
    pub fn cost_ticks(&self, model_id: &str, tokens: CallTokens, at: DateTime<Utc>) -> Option<i64> {
        Some(self.get(model_id)?.cost_ticks_at(tokens, at))
    }

    /// Merge user overrides over the built-in card, per model.
    pub fn overlay(&mut self, other: PricingTable) {
        self.pricing.extend(other.pricing);
    }
}

/// Format ticks the way the UI wants: cents below a dollar, dollars above.
pub fn format_ticks(ticks: i64) -> String {
    let usd = ticks as f64 / TICKS_PER_USD as f64;
    if usd < 1.0 {
        format!("{:.1}\u{00A2}", usd * 100.0)
    } else {
        format!("${usd:.2}")
    }
}

/// Convenience for callers holding a unix-millis timestamp.
pub fn utc_from_millis(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
