//! The `builtin` row: one segment per [`StatusLineItem`] the session asked for, each already cut to the columns it may use.

use std::time::Duration;

use bcode_status_line::{StatusLineContext, StatusLineItem};

use super::fit_columns;

pub const SEGMENT_SEPARATOR: &str = " │ ";

const CONTEXT_WARN_PCT: u8 = 80;

// Columns, not bytes: a byte budget halves a CJK or emoji name.
const CWD_COLS: usize = 40;
const MODEL_COLS: usize = 30;
const SESSION_NAME_COLS: usize = 40;

/// The smallest spend the row can paint truthfully: a tenth of a cent, the
/// resolution of the cents format below. Anything under it would render
/// `0.0\u{00A2}`, which reads as free.
const MIN_DISPLAYED_COST_USD: f64 = 0.0005;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentTone {
    Dim,
    Warn,
}

/// A `builtin` segment, already cut to the columns it may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSegment {
    // Not `pub`: a struct literal elsewhere would skip the control-character filter in [`Self::new`]
    // Read through [`Self::text`]
    pub(super) text: String,
    pub(super) tone: SegmentTone,
}

impl StatusSegment {
    fn toned(text: String, tone: SegmentTone) -> Self {
        Self::new(text, tone)
    }

    /// Read access for tests in other modules; the fields stay closed so a literal cannot skip [`Self::new`].
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    fn dim(text: impl Into<String>) -> Self {
        Self::new(text, SegmentTone::Dim)
    }

    pub(crate) fn warn(text: impl Into<String>) -> Self {
        Self::new(text, SegmentTone::Warn)
    }

    /// Control characters are dropped here rather than at the painter.
    /// A segment carries the user's own text: a cwd, a model name, a config value they typed.
    /// Only [`SanitizedText`](super::SanitizedText) filters the path a script's output takes.
    fn new(text: impl Into<String>, tone: SegmentTone) -> Self {
        let text: String = text.into();
        Self {
            text: text.chars().filter(|c| !c.is_control()).collect(),
            tone,
        }
    }
}


/// Token counts get large fast, so render them the way a human reads them.
/// Truncates rather than rounds: 1999 is "1.9k", never "2.0k", so the figure
/// never claims more tokens than were actually used.
fn compact_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => {
            let whole = n / 1_000;
            let tenth = (n % 1_000) / 100;
            if whole < 10 {
                format!("{whole}.{tenth}k")
            } else {
                format!("{whole}k")
            }
        }
        _ => {
            let whole = n / 1_000_000;
            let tenth = (n % 1_000_000) / 100_000;
            format!("{whole}.{tenth}M")
        }
    }
}

#[must_use]
pub fn compose_builtin(
    ctx: &StatusLineContext,
    turn_elapsed: Option<Duration>,
    items: &[StatusLineItem],
) -> Vec<StatusSegment> {
    items
        .iter()
        .filter_map(|item| match item {
            StatusLineItem::Cwd => {
                let short = ctx.cwd.rsplit(['/', '\\']).find(|s| !s.is_empty())?;
                Some(StatusSegment::dim(fit_columns(short, CWD_COLS)))
            }
            StatusLineItem::Model => {
                let model = ctx
                    .model
                    .display_name
                    .as_deref()
                    .filter(|s| !s.is_empty())?;
                Some(StatusSegment::dim(fit_columns(model, MODEL_COLS)))
            }
            StatusLineItem::Context => {
                let window = &ctx.context_window;
                let pct = window.used_percentage?;
                let warn_at = window
                    .auto_compact_threshold_percent
                    .unwrap_or(CONTEXT_WARN_PCT);
                let tone = if pct >= warn_at {
                    SegmentTone::Warn
                } else {
                    SegmentTone::Dim
                };
                Some(StatusSegment::toned(format!("{pct}% ctx"), tone))
            }
            StatusLineItem::Cost => ctx
                .cost
                .total_cost_usd
                .filter(|usd| *usd >= MIN_DISPLAYED_COST_USD)
                // Cents below a dollar: a coding session often costs a few
                // cents, and "$0.00" reads as free rather than as small.
                .map(|usd| {
                    StatusSegment::dim(if usd < 1.0 {
                        format!("{:.1}\u{00A2}", usd * 100.0)
                    } else {
                        format!("${usd:.2}")
                    })
                }),
            StatusLineItem::Tokens => {
                let u = ctx.context_window.session_usage.as_ref()?;
                let input = u
                    .input_tokens
                    .saturating_add(u.cache_creation_input_tokens)
                    .saturating_add(u.cache_read_input_tokens);
                if input == 0 && u.output_tokens == 0 {
                    return None;
                }
                Some(StatusSegment::dim(format!(
                    "{}\u{2191} {}\u{2193}",
                    compact_count(input),
                    compact_count(u.output_tokens)
                )))
            }
            StatusLineItem::Cache => {
                let u = ctx.context_window.session_usage.as_ref()?;
                let input = u
                    .input_tokens
                    .saturating_add(u.cache_creation_input_tokens)
                    .saturating_add(u.cache_read_input_tokens);
                if input == 0 {
                    return None;
                }
                // Integer maths: the ratio is a display figure, and a f64 round
                // trip here can render 100% for a session that missed a token.
                let pct = u.cache_read_input_tokens.saturating_mul(100) / input;
                Some(StatusSegment::dim(format!("{pct}% cached")))
            }
            StatusLineItem::TurnTimer => {
                let secs = turn_elapsed?.as_secs();
                let text = match secs {
                    0 => return None,
                    s if s < 60 => format!("{s}s"),
                    s => format!("{}m{:02}s", s / 60, s % 60),
                };
                Some(StatusSegment::dim(text))
            }
            StatusLineItem::SessionName => {
                let name = ctx.session_name.as_deref().filter(|s| !s.is_empty())?;
                Some(StatusSegment::dim(fit_columns(name, SESSION_NAME_COLS)))
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "segments_tests.rs"]
mod tests;
