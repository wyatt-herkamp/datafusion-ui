//! The shared results grid used by the SQL workspace: a resizable, scrollable
//! table with a per-column stats row, nested-cell expansion, and a cell-detail
//! overlay. (There is no longer a separate per-file "Data" view — data is
//! browsed by querying the file's table in the SQL editor.)

use crate::theme::palette;
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, canvas, column, container, mouse_area, opaque, row, scrollable, stack, text,
    tooltip,
};
use iced::{Background, Border, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::app::{CellDetail, FileMessage, GridMessage, Message, SqlMessage};
use crate::format::{NestedNode, default_options, is_nested, row_strings};
use crate::widgets::resize_handle;
use crate::wrangle::insights::{ColumnInsight, ColumnKind, Histogram, classify};

pub(crate) const CELL_WIDTH: f32 = 180.0;
const ROW_NUMBER_WIDTH: f32 = 60.0;
const ROW_HEIGHT: f32 = 24.0;
const HEADER_HEIGHT: f32 = 48.0;
const INSIGHTS_HEIGHT: f32 = 100.0;
const HISTO_HEIGHT: f32 = 36.0;
const OVERFLOW_CHAR_THRESHOLD: usize = 20;

/// Width of the resize-handle hit area at the right edge of each header cell.
/// Must match `widgets::resize_handle`'s hit width so the label cell + handle
/// together occupy exactly the column width (keeping header/body aligned).
const RESIZE_HANDLE_W: f32 = 8.0;

pub(crate) fn cell_detail_overlay<'a>(
    detail: &'a CellDetail,
    on_close: Message,
) -> Element<'a, Message> {
    let header = row![
        text(format!("{} · row {}", detail.column_name, detail.row + 1))
            .size(14)
            .wrapping(Wrapping::None),
        Space::new().width(Length::Fill),
        button(text("Copy JSON").size(11))
            .style(button::secondary)
            .on_press(FileMessage::CopyCell(detail.node.to_json_string()).into()),
        button(text("Close").size(11))
            .style(button::secondary)
            .on_press(on_close.clone()),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    let type_line = text(detail.type_label.clone()).size(11);

    let tree_widgets = render_node(&detail.node, 0);
    let tree: Element<'a, Message> = scrollable(
        container(column(tree_widgets).spacing(2))
            .padding(8)
            .width(Length::Fill),
    )
    .height(Length::Fill)
    .into();

    let panel = container(column![header, type_line, tree].spacing(8))
        .padding(14)
        .width(Length::Fixed(640.0))
        .height(Length::Fixed(480.0))
        .style(detail_panel_style);

    // Dim backdrop that intercepts clicks (closes on outside-click).
    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(crate::theme::backdrop),
    )
    .on_press(on_close);

    let centered = container(opaque(panel))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    stack![backdrop, centered].into()
}

fn render_node<'a>(node: &NestedNode, indent: usize) -> Vec<Element<'a, Message>> {
    let mut out = Vec::new();
    let pad = "  ".repeat(indent);
    match node {
        NestedNode::Null => {
            out.push(tree_text(format!("{pad}∅"), false));
        }
        NestedNode::Leaf(s) => {
            out.push(tree_text(format!("{pad}{s}"), false));
        }
        NestedNode::List(items) => {
            if items.is_empty() {
                out.push(tree_text(format!("{pad}[ ] (empty)"), true));
            } else {
                out.push(tree_text(format!("{pad}[ {} items ]", items.len()), true));
                for (i, child) in items.iter().enumerate() {
                    match child {
                        NestedNode::Leaf(s) => {
                            out.push(tree_text(format!("{pad}  [{i}] {s}"), false));
                        }
                        NestedNode::Null => {
                            out.push(tree_text(format!("{pad}  [{i}] ∅"), false));
                        }
                        _ => {
                            out.push(tree_text(format!("{pad}  [{i}]"), true));
                            out.extend(render_node(child, indent + 2));
                        }
                    }
                }
            }
        }
        NestedNode::Struct(fields) => {
            if fields.is_empty() {
                out.push(tree_text(format!("{pad}{{ }} (empty)"), true));
            } else {
                for (k, v) in fields.iter() {
                    match v {
                        NestedNode::Leaf(s) => {
                            out.push(tree_text(format!("{pad}{k}: {s}"), false));
                        }
                        NestedNode::Null => {
                            out.push(tree_text(format!("{pad}{k}: ∅"), false));
                        }
                        _ => {
                            out.push(tree_text(format!("{pad}{k}:"), true));
                            out.extend(render_node(v, indent + 1));
                        }
                    }
                }
            }
        }
        NestedNode::Map(entries) => {
            if entries.is_empty() {
                out.push(tree_text(format!("{pad}{{ }} (empty map)"), true));
            } else {
                for (k, v) in entries.iter() {
                    let k_label = match k {
                        NestedNode::Leaf(s) => s.clone(),
                        NestedNode::Null => "∅".to_string(),
                        _ => "<key>".to_string(),
                    };
                    match v {
                        NestedNode::Leaf(s) => {
                            out.push(tree_text(format!("{pad}{k_label} → {s}"), false));
                        }
                        NestedNode::Null => {
                            out.push(tree_text(format!("{pad}{k_label} → ∅"), false));
                        }
                        _ => {
                            out.push(tree_text(format!("{pad}{k_label} →"), true));
                            out.extend(render_node(v, indent + 1));
                        }
                    }
                }
            }
        }
    }
    out
}

fn tree_text<'a>(s: String, is_key: bool) -> Element<'a, Message> {
    let mut t = text(s).size(12).wrapping(Wrapping::None);
    if is_key {
        t = t.style(|theme: &Theme| text::Style {
            color: Some(theme.extended_palette().primary.strong.color),
        });
    }
    t.into()
}

fn detail_panel_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface_2())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_strong(),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..ContainerStyle::default()
    }
}

/// Render a result `batch` as a grid: clickable/resizable headers, a per-column
/// stats row from `insights`, and the data rows. `id` is the owning SQL editor
/// tab (routes nested-cell expansion and column resizes).
pub(crate) fn view_grid<'a>(
    batch: &'a RecordBatch,
    insights: &'a [ColumnInsight],
    id: u64,
    page: usize,
    page_size: usize,
    widths: &[f32],
) -> Element<'a, Message> {
    let opts = default_options();
    let schema = batch.schema();
    let col_w = |c: usize| widths.get(c).copied().unwrap_or(CELL_WIDTH);

    let mut header = row![header_cell("#", ROW_NUMBER_WIDTH)].spacing(0);
    for (idx, f) in schema.fields().iter().enumerate() {
        header = header.push(column_header(
            f.name(),
            col_w(idx),
            f.data_type(),
            &crate::format::type_label_full(f.data_type()),
            idx,
            id,
        ));
    }
    let total_width = ROW_NUMBER_WIDTH + (0..schema.fields().len()).map(col_w).sum::<f32>();
    let header_inner = container(header)
        .height(Length::Fixed(HEADER_HEIGHT))
        .style(header_row_style);
    let header_rule = container(Space::new())
        .width(Length::Fixed(total_width))
        .height(Length::Fixed(1.0))
        .style(|_: &Theme| ContainerStyle {
            background: Some(Background::Color(palette::border_strong())),
            ..ContainerStyle::default()
        });
    let header: Element<'a, Message> = column![header_inner, header_rule].into();

    let mut insights_row = row![spacer_cell(ROW_NUMBER_WIDTH, INSIGHTS_HEIGHT)].spacing(0);
    for (c, _f) in schema.fields().iter().enumerate() {
        let ci = insights.get(c);
        insights_row = insights_row.push(insights_cell(ci, col_w(c)));
    }
    let insights_block: Element<'a, Message> = container(insights_row)
        .height(Length::Fixed(INSIGHTS_HEIGHT))
        .style(insights_row_style)
        .into();

    let offset = page * page_size;
    let end = (offset + page_size).min(batch.num_rows());
    let mut rows_col = column![header, insights_block].spacing(0);
    for r in offset..end {
        let values = row_strings(batch, r, &opts);
        let zebra = r % 2 == 1;
        let mut row_widgets = row![row_number_cell(r + 1, zebra)].spacing(0);
        for (c, v) in values.into_iter().enumerate() {
            let dt = schema.field(c).data_type();
            let is_nested_cell = is_nested(dt);
            let right_align = matches!(classify(dt), ColumnKind::Numeric);
            row_widgets = row_widgets.push(body_cell(
                v,
                col_w(c),
                is_nested_cell,
                right_align,
                r,
                c,
                id,
            ));
        }
        let styled = container(row_widgets)
            .height(Length::Fixed(ROW_HEIGHT))
            .style(body_row_style);
        rows_col = rows_col.push(styled);
        rows_col = rows_col.push(
            container(Space::new())
                .width(Length::Fixed(total_width))
                .height(Length::Fixed(1.0))
                .style(|_: &Theme| ContainerStyle {
                    background: Some(Background::Color(palette::border_subtle())),
                    ..ContainerStyle::default()
                }),
        );
    }

    rows_col.into()
}

fn spacer_cell<'a>(width: f32, height: f32) -> Element<'a, Message> {
    container(Space::new())
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .into()
}

fn insights_cell<'a>(insight: Option<&'a ColumnInsight>, width: f32) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match insight {
        Some(ci) => {
            let visual: Element<'a, Message> = match (&ci.histogram, &ci.top_values) {
                (Some(h), _) => histogram_widget(h.clone(), width - 16.0, HISTO_HEIGHT),
                (None, Some(top)) if !top.is_empty() => top_values_widget(top, width - 16.0),
                _ => Space::new()
                    .width(Length::Fixed(width - 16.0))
                    .height(Length::Fixed(HISTO_HEIGHT))
                    .into(),
            };

            let null_pct = if ci.total > 0 {
                (ci.null_count as f64) * 100.0 / (ci.total as f64)
            } else {
                0.0
            };
            let distinct_label = match ci.distinct {
                Some(d) => format_count(d as i64).to_string(),
                None => "—".to_string(),
            };
            let stats_line = text(format!(
                "distinct {distinct_label}   missing {null_pct:.1}%"
            ))
            .size(11)
            .wrapping(Wrapping::None);

            let range_line: Element<'a, Message> = match (&ci.min, &ci.max) {
                (Some(lo), Some(hi)) => text(format!("min {lo} · max {hi}"))
                    .size(10)
                    .wrapping(Wrapping::None)
                    .into(),
                _ => Space::new().width(Length::Fixed(0.0)).into(),
            };

            column![visual, stats_line, range_line].spacing(2).into()
        }
        None => text("…").size(11).into(),
    };

    container(inner)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .padding([4, 8])
        .clip(true)
        .into()
}

fn histogram_widget<'a>(h: Histogram, width: f32, height: f32) -> Element<'a, Message> {
    canvas(HistoCanvas { h })
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .into()
}

fn top_values_widget<'a>(top: &'a [(String, u64)], width: f32) -> Element<'a, Message> {
    let max = top.iter().map(|(_, c)| *c).max().unwrap_or(1).max(1) as f32;
    let widest_count = top
        .iter()
        .map(|(_, c)| format_count(*c as i64).chars().count())
        .max()
        .unwrap_or(1);
    let count_w = ((widest_count as f32) * 6.5 + 4.0).clamp(28.0, width * 0.35);
    let bar_w = (width * 0.3).max(24.0);
    let label_w = (width - count_w - bar_w - 8.0).max(40.0);
    let mut col = column![].spacing(2);
    for (v, c) in top.iter().take(3) {
        let frac = (*c as f32) / max;
        col = col.push(
            row![
                container(text(elide(v, 14)).size(10).wrapping(Wrapping::None))
                    .width(Length::Fixed(label_w))
                    .clip(true),
                bar(frac, bar_w, 8.0),
                container(
                    text(format_count(*c as i64))
                        .size(10)
                        .wrapping(Wrapping::None)
                )
                .width(Length::Fixed(count_w))
                .clip(true),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(4),
        );
    }
    col.into()
}

fn bar<'a>(frac: f32, width: f32, height: f32) -> Element<'a, Message> {
    let fill = (width * frac.clamp(0.0, 1.0)).max(1.0);
    container(
        container(Space::new())
            .width(Length::Fixed(fill))
            .height(Length::Fixed(height))
            .style(bar_fill_style),
    )
    .width(Length::Fixed(width))
    .height(Length::Fixed(height))
    .style(bar_bg_style)
    .into()
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn format_count(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if neg {
        out.push('-');
    }
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

struct HistoCanvas {
    h: Histogram,
}

impl canvas::Program<Message> for HistoCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry<Renderer>> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let p = theme.extended_palette();
        let bg = p.background.weak.color;
        let fg = p.primary.base.color;

        frame.fill_rectangle(Point::ORIGIN, Size::new(bounds.width, bounds.height), bg);

        let n = self.h.bin_counts.len().max(1);
        let max = self.h.bin_counts.iter().copied().max().unwrap_or(1).max(1) as f32;
        let bar_w = bounds.width / n as f32;
        let pad = 1.0_f32.min(bar_w * 0.2);

        for (i, c) in self.h.bin_counts.iter().enumerate() {
            let h = (*c as f32 / max) * bounds.height;
            if h <= 0.0 {
                continue;
            }
            let x = i as f32 * bar_w + pad * 0.5;
            let y = bounds.height - h;
            frame.fill_rectangle(Point::new(x, y), Size::new((bar_w - pad).max(1.0), h), fg);
        }

        vec![frame.into_geometry()]
    }
}

fn header_cell<'a>(label: &str, width: f32) -> Element<'a, Message> {
    container(crate::theme::label_text(label).wrapping(Wrapping::None))
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .padding([8, 12])
        .clip(true)
        .style(header_cell_style)
        .into()
}

fn column_header<'a>(
    label: &str,
    width: f32,
    dt: &DataType,
    tt: &str,
    idx: usize,
    id: u64,
) -> Element<'a, Message> {
    let name = crate::theme::ui_medium(label.to_string())
        .size(12)
        .wrapping(Wrapping::None);

    let kind = classify(dt);
    let colors = if is_nested(dt) {
        crate::theme::pill_colors_nested()
    } else {
        crate::theme::pill_colors_for(kind)
    };
    let type_str = crate::format::type_label(dt);
    let pill = container(
        text(type_str)
            .font(crate::theme::FONT_MONO)
            .size(9)
            .wrapping(Wrapping::None),
    )
    .padding([1, 6])
    .style(crate::theme::pill_style(colors));

    let inner = column![name, pill].spacing(3);

    let label_width = (width - RESIZE_HANDLE_W).max(0.0);
    let cell = container(inner)
        .width(Length::Fixed(label_width))
        .height(Length::Fill)
        .padding([6, 12])
        .clip(true)
        .style(header_label_style);

    let label_area = tooltip(cell, tooltip_box(tt.to_string()), tooltip::Position::Top);

    let handle = resize_handle(
        width,
        move |w| {
            GridMessage::ColumnResize {
                id,
                col: idx,
                width: w,
            }
            .into()
        },
        GridMessage::ColumnResizeEnd { id }.into(),
        GridMessage::ColumnAutofit { id, col: idx }.into(),
    );

    row![label_area, handle].spacing(0).into()
}

fn header_label_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: None,
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn body_cell<'a>(
    value: String,
    width: f32,
    is_nested: bool,
    right_align: bool,
    row: usize,
    col: usize,
    id: u64,
) -> Element<'a, Message> {
    let display = if is_nested {
        format!("⊞ {value}")
    } else {
        value.clone()
    };
    let is_null = value == "∅";
    let mut label = text(display)
        .font(crate::theme::FONT_MONO)
        .size(12)
        .wrapping(Wrapping::None);
    if right_align {
        label = label
            .align_x(iced::alignment::Horizontal::Right)
            .width(Length::Fill);
    }
    if is_nested {
        label = label.style(|_: &Theme| text::Style {
            color: Some(palette::accent_warm()),
        });
    } else if is_null {
        label = label.style(|_: &Theme| text::Style {
            color: Some(palette::fg_dim()),
        });
    }
    let inner = container(label)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .padding([4, 12])
        .clip(true)
        .style(body_cell_style);

    let with_tooltip: Element<'a, Message> =
        if !is_nested && value.chars().count() > OVERFLOW_CHAR_THRESHOLD {
            tooltip(inner, tooltip_box(value.clone()), tooltip::Position::Top).into()
        } else if is_nested {
            tooltip(
                inner,
                tooltip_box("Click to expand".to_string()),
                tooltip::Position::Top,
            )
            .into()
        } else {
            inner.into()
        };

    let on_press = if is_nested {
        SqlMessage::ShowCellDetail { id, row, col }.into()
    } else {
        FileMessage::CopyCell(value).into()
    };

    mouse_area(with_tooltip).on_press(on_press).into()
}

fn row_number_cell<'a>(n: usize, zebra: bool) -> Element<'a, Message> {
    let _ = zebra;
    container(
        text(format!("{}", n))
            .font(crate::theme::FONT_MONO)
            .size(11)
            .wrapping(Wrapping::None)
            .align_x(iced::alignment::Horizontal::Right)
            .width(Length::Fill)
            .style(|_: &Theme| text::Style {
                color: Some(palette::fg_dim()),
            }),
    )
    .width(Length::Fixed(ROW_NUMBER_WIDTH))
    .height(Length::Fill)
    .padding([4, 10])
    .clip(true)
    .style(row_number_style)
    .into()
}

fn tooltip_box<'a>(content: String) -> Element<'a, Message> {
    container(
        text(content)
            .font(crate::theme::FONT_MONO)
            .size(11)
            .style(|_: &Theme| text::Style {
                color: Some(palette::fg_primary()),
            }),
    )
    .padding([6, 10])
    .max_width(560.0)
    .style(|_: &Theme| ContainerStyle {
        background: Some(Background::Color(palette::bg_surface_2())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_strong(),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..ContainerStyle::default()
    })
    .into()
}

fn insights_row_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface())),
        text_color: Some(palette::fg_muted()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn bar_bg_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface_2())),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

fn bar_fill_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(Color {
            a: 0.75,
            ..palette::accent_warm()
        })),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

fn header_row_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface())),
        text_color: Some(palette::fg_muted()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn header_cell_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: None,
        text_color: Some(palette::fg_muted()),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

fn body_row_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_deep())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn body_cell_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: None,
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn row_number_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_deep())),
        text_color: Some(palette::fg_dim()),
        border: Border {
            color: palette::border_subtle(),
            width: 0.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}
