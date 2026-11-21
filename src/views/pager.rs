use iced::widget::{button, row};
use iced::{Element, Length};

use crate::app::Message;
use crate::theme::{self, palette};

/// Where a page of `page_size` items falls within `total`, with `page` clamped to the last page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageWindow {
    pub page: usize,
    pub pages: usize,
    /// Index of the first item on the page.
    pub start: usize,
    /// One past the last item on the page.
    pub end: usize,
}

impl PageWindow {
    pub fn new(page: usize, page_size: usize, total: usize) -> Self {
        let pages = total.div_ceil(page_size).max(1);
        let page = page.min(pages - 1);
        let start = (page * page_size).min(total);
        let end = (start + page_size).min(total);
        PageWindow {
            page,
            pages,
            start,
            end,
        }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

/// Prev/next/first/last controls + a `{noun} a–b of N · page x/y` label.
pub fn pagination_bar<'a>(
    noun: &str,
    window: PageWindow,
    total: usize,
    on_page: impl Fn(usize) -> Message,
) -> Element<'a, Message> {
    let PageWindow {
        page,
        pages,
        start,
        end,
    } = window;
    let nav = |glyph: &'static str, target: usize, enabled: bool| {
        button(theme::ui_medium(glyph).size(12))
            .style(theme::ghost_button)
            .padding([2, 8])
            .on_press_maybe(enabled.then(|| on_page(target)))
    };
    let has_prev = page > 0;
    let has_next = page + 1 < pages;
    row![
        nav("⏮", 0, has_prev),
        nav("◀", page.saturating_sub(1), has_prev),
        theme::mono_sm(format!(
            "{noun} {}–{end} of {total}  ·  page {}/{pages}",
            start + 1,
            page + 1,
        ))
        .color(palette::fg_muted()),
        nav("▶", page + 1, has_next),
        nav("⏭", pages - 1, has_next),
    ]
    .spacing(8)
    .width(Length::Shrink)
    .align_y(iced::Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_past_the_end_clamps_to_the_last_page() {
        let w = PageWindow::new(99, 100, 250);
        assert_eq!(
            w,
            PageWindow {
                page: 2,
                pages: 3,
                start: 200,
                end: 250
            }
        );
        assert_eq!(w.len(), 50);
    }

    #[test]
    fn an_empty_list_still_has_one_page() {
        let w = PageWindow::new(3, 100, 0);
        assert_eq!(
            w,
            PageWindow {
                page: 0,
                pages: 1,
                start: 0,
                end: 0
            }
        );
    }

    #[test]
    fn an_exact_multiple_has_no_trailing_empty_page() {
        assert_eq!(PageWindow::new(0, 100, 200).pages, 2);
    }
}
