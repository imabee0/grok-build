//! Masked single-line text input: reveals only the last 4 graphemes of what
//! has been typed, with a caller-supplied placeholder shown while empty.
//!
//! Lifted out of the welcome screen's OAuth-token paste box (loopback auth)
//! so the provider manager's API-key entry field can reuse the exact same
//! reveal policy and cursor-remapping logic instead of drifting from it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

fn grapheme_visible(index: usize, total: usize) -> bool {
    total <= 8 || index + 4 >= total
}

pub struct MaskedInput {
    pub display: String,
    pub cursor_byte: usize,
}

/// Mask `input`, keeping only its last 4 graphemes legible (or all of it when
/// 8 graphemes or fewer). Remaps `cursor_byte` into the masked string's byte
/// space so the caller's cursor still lands in the right visual column.
pub fn build_masked_input(input: &str, cursor_byte: usize) -> MaskedInput {
    let graphemes: Vec<(usize, &str)> = input.grapheme_indices(true).collect();
    let total = graphemes.len();
    let mut display = String::new();
    let mut mapped_cursor = None;
    for (index, (byte, grapheme)) in graphemes.into_iter().enumerate() {
        if byte == cursor_byte {
            mapped_cursor = Some(display.len());
        }
        if grapheme_visible(index, total) {
            display.push_str(grapheme);
        } else {
            display.push('\u{2022}');
        }
    }
    MaskedInput {
        cursor_byte: mapped_cursor.unwrap_or(display.len()),
        display,
    }
}

/// The masked, viewport-scrolled `(text, cursor_column)` pair ready to render
/// in a `width`-column-wide field. Shows `placeholder` while `input` is empty.
pub fn masked_input_view(
    input: &str,
    cursor_byte: usize,
    width: usize,
    placeholder: &str,
) -> (String, usize) {
    if input.is_empty() {
        return (placeholder.to_string(), 0);
    }
    let masked = build_masked_input(input, cursor_byte);
    let buffer =
        bcode_ratatui_textarea::EditBuffer::from_parts(masked.display.as_str(), masked.cursor_byte);
    let viewport = buffer.single_line_viewport(width);
    (
        masked.display[viewport.visible_byte_range].to_owned(),
        viewport.cursor_display_column,
    )
}

/// Render a bordered single-line masked input box with a live block cursor.
pub fn render_masked_input_box(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    input: &str,
    cursor_byte: usize,
    placeholder: &str,
) {
    let prompt_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent_user))
        .padding(Padding {
            left: 2,
            right: 1,
            top: 0,
            bottom: 0,
        });
    let inner = prompt_block.inner(area);
    prompt_block.render(area, buf);

    if inner.height > 0 && inner.width > 2 {
        let prompt = crate::glyphs::prompt_arrow();
        let prompt_width = prompt.width() as u16;
        let input_width = inner.width.saturating_sub(prompt_width);
        let (display, cursor_column) =
            masked_input_view(input, cursor_byte, input_width as usize, placeholder);

        let style = if input.is_empty() {
            Style::default().fg(theme.gray_dim)
        } else {
            Style::default().fg(theme.accent_user)
        };

        let line = Line::from(vec![
            Span::styled(prompt, Style::default().fg(theme.accent_user)),
            Span::styled(display, style),
        ]);
        buf.set_line(inner.x, inner.y, &line, inner.width);
        if input_width > 0 {
            let cursor_x = inner.x + prompt_width + cursor_column as u16;
            if let Some(cell) = buf.cell_mut((cursor_x, inner.y)) {
                cell.set_style(Style::default().fg(theme.bg_base).bg(theme.text_primary));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn masked_input_preserves_reveal_policy() {
        assert_eq!(
            masked_input_view("", 0, 24, "Paste your token here..."),
            ("Paste your token here...".to_string(), 0)
        );
        assert_eq!(build_masked_input("12345678", 8).display, "12345678");
        assert_eq!(build_masked_input("123456789", 9).display, "•••••6789");

        let input = "abcdefghMIDDLEwxyz";
        let masked = build_masked_input(input, input.len()).display;
        assert!(masked.starts_with("••••"));
        assert!(masked.ends_with("wxyz"));
        assert!(!masked.contains("MIDDLE"));
        assert!(masked.contains("\u{2022}"));

        let input = "测试令牌一二三四五六七八九十";
        let masked = build_masked_input(input, input.len()).display;
        assert!(masked.starts_with("••••"));
        assert!(masked.contains("\u{2022}"));
    }

    #[test]
    fn masked_input_mapping_handles_zero_width_combining_and_zwj_middle() {
        let prefix = "abcdefgh";
        let hidden = "\u{200b}e\u{301}👩🏽\u{200d}💻MID";
        let suffix = "wxyz";
        let token = format!("{prefix}{hidden}{suffix}");
        let before = prefix.len();
        let inside = prefix.len() + "\u{200b}e\u{301}".len();
        let after = prefix.len() + hidden.len();
        let expected = format!("{}{}", "\u{2022}".repeat(14), suffix);

        let before_masked = build_masked_input(&token, before);
        let inside_masked = build_masked_input(&token, inside);
        let after_masked = build_masked_input(&token, after);
        assert_eq!(before_masked.display, expected);
        assert_eq!(inside_masked.display, expected);
        assert_eq!(after_masked.display, expected);
        assert_eq!(before_masked.cursor_byte, "\u{2022}".len() * 8);
        assert_eq!(inside_masked.cursor_byte, "\u{2022}".len() * 10);
        assert_eq!(after_masked.cursor_byte, "\u{2022}".len() * 14);

        for width in [1, 2, 5] {
            for cursor in [before, inside, after] {
                let (view, cursor_column) = masked_input_view(&token, cursor, width, "placeholder");
                assert!(view.width() <= width);
                assert!(cursor_column < width);
                assert!(!view.contains('\u{200b}'));
                assert!(!view.contains("e\u{301}"));
                assert!(!view.contains("👩🏽\u{200d}💻"));
                assert!(!view.contains("MID"));
            }
        }

        let wide_prefix = "中bcdefgh";
        let wide_token = format!("{wide_prefix}HIDDEN{suffix}");
        let (_, cursor_column) =
            masked_input_view(&wide_token, wide_prefix.len(), 40, "placeholder");
        assert_eq!(cursor_column, wide_prefix.graphemes(true).count());
    }

    #[test]
    fn masked_input_render_keeps_narrow_caret_visible() {
        let token = "abcdefghSECRET-MIDDLEwxyz";
        let cursor = "abcdefghSECRET".len();
        let area = Rect::new(0, 0, 9, 3);
        let theme = Theme::current();
        let mut buffer = Buffer::empty(area);
        render_masked_input_box(area, &mut buffer, &theme, token, cursor, "placeholder");
        assert!((0..area.width).any(|x| buffer[(x, 1)].bg == theme.text_primary));
    }
}
