//! An iced [`Highlighter`] that colors SQL using `sql_ide`'s lexer.
//!
//! SQL tokens (other than rare multi-line strings/comments) do not span lines,
//! so each editor line is lexed independently — no cross-line state is needed.

use std::ops::Range;

use iced::Font;
use iced::Theme;
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use sql_ide::TokenKind;

use crate::theme::palette;

/// Stateless-per-line SQL highlighter. `current` only tracks which line the
/// editor will ask about next, as the trait requires.
#[derive(Debug)]
pub struct SqlHighlighter {
    current: usize,
}

impl Highlighter for SqlHighlighter {
    type Settings = ();
    type Highlight = TokenKind;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, TokenKind)>;

    fn new(_settings: &Self::Settings) -> Self {
        Self { current: 0 }
    }

    fn update(&mut self, _new_settings: &Self::Settings) {
        self.current = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.current = line;
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.current += 1;
        sql_ide::highlight_spans(line).into_iter()
    }

    fn current_line(&self) -> usize {
        self.current
    }
}

/// Map a token kind to an editor color. A plain `fn` (not a closure) because
/// `text_editor::highlight_with` takes a function pointer.
pub fn to_format(kind: &TokenKind, _theme: &Theme) -> Format<Font> {
    let color = match kind {
        TokenKind::Keyword => palette::accent_warm(),
        TokenKind::StringLit => palette::accent_cool(),
        TokenKind::Number => palette::accent_violet(),
        TokenKind::Comment => palette::fg_dim(),
        TokenKind::Punct => palette::fg_muted(),
        TokenKind::Identifier | TokenKind::Other | TokenKind::Whitespace => palette::fg_primary(),
    };
    Format {
        color: Some(color),
        font: None,
    }
}
