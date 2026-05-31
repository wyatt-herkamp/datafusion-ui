//! Friendly rendering of `EXPLAIN` / `EXPLAIN ANALYZE` output.
//!
//! DataFusion returns EXPLAIN as a `(plan_type, plan)` table where each `plan`
//! cell is a newline-separated, space-indented operator tree. Shown in the raw
//! grid that's an unreadable wall of text in one cell; here we parse the
//! indentation into a real tree, emphasise operator names, and lift ANALYZE
//! `metrics=[…]` into pills.

use arrow::array::StringArray;
use arrow::record_batch::RecordBatch;
use iced::widget::text::Wrapping;
use iced::widget::{Space, column, container, responsive, row, scrollable};
use iced::{Element, Length};

use crate::app::Message;
use crate::explain::ExplainKind;
use crate::theme;

const INDENT_PX: f32 = 16.0;

pub fn view_plan<'a>(batch: &'a RecordBatch, kind: ExplainKind) -> Element<'a, Message> {
    let plan_type = str_col(batch, "plan_type");
    let plan = str_col(batch, "plan");

    let mut sections = column![].spacing(16);
    for r in 0..batch.num_rows() {
        let title = plan_type
            .as_ref()
            .map(|a| pretty_title(a.value(r)))
            .unwrap_or_else(|| format!("Plan {}", r + 1));
        let body = plan.as_ref().map(|a| a.value(r)).unwrap_or("");

        let mut lines = column![].spacing(1);
        for line in body.lines() {
            lines = lines.push(plan_line(line, kind));
        }
        sections = sections.push(column![theme::label_text(&title), lines].spacing(6));
    }

    // Vertical-only scroll: width is bounded to the viewport so long operator
    // detail wraps downward instead of stretching the view ever wider.
    scrollable(container(sections).padding(12).width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One operator: name on its own row, then wrapped detail and metric pills on
/// following indented rows — favouring vertical space over horizontal width.
fn plan_line<'a>(line: &str, kind: ExplainKind) -> Element<'a, Message> {
    let depth = (line.len() - line.trim_start().len()) / 2;
    let indent = depth as f32 * INDENT_PX;
    let trimmed = line.trim_start();

    // Split off ANALYZE metrics, if any.
    let (body, metrics) = match trimmed.find("metrics=[") {
        Some(i) => {
            let after = &trimmed[i + "metrics=[".len()..];
            let inner = after.split(']').next().unwrap_or("");
            // Drop a trailing ", " that precedes "metrics=" so the detail is clean.
            let body = trimmed[..i].trim_end().trim_end_matches(',');
            (body, Some(inner.to_string()))
        }
        None => (trimmed, None),
    };

    // Operator name = up to first ':'; the rest is detail.
    let (op, detail) = match body.find(':') {
        Some(i) => (&body[..i], body[i + 1..].trim()),
        None => (body, ""),
    };

    let detail_indent = indent + 14.0;
    let mut col = column![
        row![
            Space::new().width(Length::Fixed(indent)),
            theme::ui_medium(op.to_string()).size(12),
        ]
        .spacing(8)
    ]
    .spacing(2);

    if !detail.is_empty() {
        col = col.push(
            row![
                Space::new().width(Length::Fixed(detail_indent)),
                theme::mono_sm(detail.to_string())
                    .color(theme::palette::fg_muted())
                    .wrapping(Wrapping::Word)
                    .width(Length::Fill),
            ]
            .spacing(0),
        );
    }

    if kind.is_analyze()
        && let Some(metrics) = metrics
    {
        let items: Vec<String> = metrics
            .split(", ")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if !items.is_empty() {
            col = col.push(metric_pills(items, detail_indent));
        }
    }

    col.into()
}

/// Lay metric pills out left-to-right, wrapping onto new rows so they never
/// overflow the available width (which would clip the last pill's background
/// while its text spilled past it). Uses `responsive` to learn the real width.
fn metric_pills<'a>(items: Vec<String>, indent: f32) -> Element<'a, Message> {
    // JetBrains Mono advance at size 10 ≈ 6px/char; pill padding [2,7] = 14px
    // horizontal, plus 6px inter-pill spacing. Overestimate slightly so we wrap
    // before touching the edge rather than after.
    const CHAR_W: f32 = 6.5;
    const PILL_EXTRA: f32 = 14.0 + 6.0;

    responsive(move |size| {
        let avail = (size.width - indent - 16.0).max(120.0);
        let new_row = || {
            row![Space::new().width(Length::Fixed(indent))]
                .spacing(6)
                .align_y(iced::Alignment::Center)
        };
        let mut rows = column![].spacing(4);
        let mut current = new_row();
        let mut in_row = 0u32;
        let mut used = 0.0f32;
        for m in &items {
            let cost = m.chars().count() as f32 * CHAR_W + PILL_EXTRA;
            if in_row > 0 && used + cost > avail {
                rows = rows.push(current);
                current = new_row();
                in_row = 0;
                used = 0.0;
            }
            current = current.push(metric_pill(m.clone()));
            in_row += 1;
            used += cost;
        }
        rows.push(current).into()
    })
    .height(Length::Shrink)
    .into()
}

fn metric_pill<'a>(label: String) -> Element<'a, Message> {
    // No wrapping: a metric is a short token, and under horizontal pressure a
    // wrapped second line would spill below the (one-line-tall) pill background.
    container(theme::mono_sm(label).size(10).wrapping(Wrapping::None))
        .padding([2, 7])
        .style(|_: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(theme::palette::accent_warm_soft())),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// `logical_plan` → `Logical Plan`, etc.
fn pretty_title(raw: &str) -> String {
    raw.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
}
