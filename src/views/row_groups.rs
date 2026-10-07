use std::time::Duration;

use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::Wrapping;
use iced::widget::{Row, Space, button, column, container, mouse_area, row, text, tooltip};
use iced::{Background, Border, Element, Length, Padding, Theme};
use parquet::basic::{Compression, Encoding};
use parquet::file::metadata::{ColumnChunkMetaData, RowGroupMetaData};

use crate::app::{FileMessage, Message};
use crate::format::{count, human_bytes};
use crate::parquet_io::FileSummary;
use crate::stats_format::{self, Bound};
use crate::theme as ui_theme;
use crate::views::cell::CellString;
use crate::views::overview::format_sorting_columns;

/// Width of each column in the expanded column-chunk table.
///
/// "Comp" and "Coding" hold the widest fixed vocabulary here — `Uncompressed` and the encoding
/// names, which run to `DELTA_LENGTH_BYTE_ARRAY` — so they get the room, paid for by trimming the
/// columns whose values are short and predictable (counts and byte sizes).
const CHUNK_COL_WIDTHS: [f32; 9] = [200.0, 85.0, 85.0, 90.0, 90.0, 115.0, 195.0, 195.0, 195.0];

const SUMMARY_HEADERS: [&str; 9] = [
    "Index",
    "Rows",
    "Share",
    "Raw",
    "Packed",
    "Ratio",
    "Cols",
    "Packed/row",
    "Sort order",
];

/// Width of each summary column. Sized so the summary rows and the expanded column-chunk table end
/// at the same x — otherwise expanding a group leaves the narrower block above it looking ragged.
const SUMMARY_COL_WIDTHS: [f32; 9] = [120.0, 120.0, 90.0, 120.0, 120.0, 80.0, 70.0, 110.0, 420.0];

/// Left gutter holding the expand/collapse toggle. The expanded table is indented to match, so its
/// contents line up under the summary row's first data column.
const GUTTER: f32 = 40.0;

const fn total_width(widths: &[f32]) -> f32 {
    let mut total = 0.0;
    let mut i = 0;
    while i < widths.len() {
        total += widths[i];
        i += 1;
    }
    total
}

/// Width shared by the stats strip, the summary rows and the expanded table.
const TABLE_WIDTH: f32 = GUTTER + total_width(&CHUNK_COL_WIDTHS);

// Keep the two tables from drifting apart.
const _: () = assert!(total_width(&SUMMARY_COL_WIDTHS) == total_width(&CHUNK_COL_WIDTHS));

pub fn view(file: &FileSummary, selected: Option<usize>) -> Option<Element<'_, Message>> {
    let Some(meta) = &file.metadata else {
        return None;
    };
    let groups = meta.row_groups();
    let total_rows: i64 = groups.iter().map(RowGroupMetaData::num_rows).sum();

    let mut table = column![summary_header()].spacing(0);

    for (i, rg) in groups.iter().enumerate() {
        let expanded = selected == Some(i);
        let zebra = i % 2 == 1;
        let packed: i64 = rg.columns().iter().map(|c| c.compressed_size()).sum();
        let raw = rg.total_byte_size().max(0);

        let summary = row![
            container(
                button(text(if expanded { "▾" } else { "▸" }.to_string()))
                    .on_press(FileMessage::RowGroupToggled(i).into())
                    .style(button::secondary),
            )
            .width(Length::Fixed(GUTTER))
            .padding([2, 4]),
            body_cell(format!("Group {i}"), width(0)),
            body_cell(count(rg.num_rows()), width(1)),
            body_cell(percent(rg.num_rows(), total_rows), width(2)),
            body_cell(human_bytes(raw as u64), width(3)),
            body_cell(human_bytes(packed.max(0) as u64), width(4)),
            body_cell(ratio(raw, packed), width(5)),
            body_cell(count(rg.num_columns() as i64), width(6)),
            body_cell(per_row(packed, rg.num_rows()), width(7)),
            sort_cell(file, rg, width(8)),
        ]
        .spacing(0)
        .align_y(iced::Alignment::Center);

        table =
            table.push(container(summary).style(move |theme: &Theme| body_row_style(theme, zebra)));

        if expanded {
            table = table.push(column_chunk_table(file, i));
        }
    }

    Some(
        column![stats_strip(file, groups, total_rows), table]
            .spacing(14)
            .into(),
    )
}

fn width(index: usize) -> f32 {
    SUMMARY_COL_WIDTHS[index]
}

/// A compact strip of file-wide row-group figures, above the table.
fn stats_strip<'a>(
    file: &'a FileSummary,
    groups: &'a [RowGroupMetaData],
    total_rows: i64,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = if groups.is_empty() {
        ui_theme::muted(ui_theme::ui("This file has no row groups.")).into()
    } else {
        let rows: Vec<i64> = groups.iter().map(RowGroupMetaData::num_rows).collect();
        let packed: Vec<i64> = groups
            .iter()
            .map(|rg| rg.columns().iter().map(|c| c.compressed_size()).sum())
            .collect();

        row![
            stat_tile("Row groups", count(groups.len() as i64), None),
            stat_tile("Total rows", count(total_rows), None),
            spread_tile("Rows / group", &rows, count),
            spread_tile("Packed / group", &packed, |v| human_bytes(v.max(0) as u64)),
            stat_tile(
                "Columns",
                count(groups[0].num_columns() as i64),
                file.metadata
                    .as_ref()
                    .map(|m| format!("{} leaf", m.file_metadata().schema_descr().num_columns())),
            ),
        ]
        .spacing(32)
        .into()
    };

    container(body)
        .width(Length::Fixed(TABLE_WIDTH))
        .padding([12, 16])
        .style(ui_theme::surface_2)
        .into()
}

fn stat_tile<'a>(label: &'a str, value: String, detail: Option<String>) -> Element<'a, Message> {
    let mut col = column![
        ui_theme::label_text(label),
        ui_theme::mono(value).size(15).wrapping(Wrapping::None),
    ]
    .spacing(3);
    if let Some(detail) = detail {
        col = col.push(ui_theme::muted(ui_theme::mono_sm(detail)));
    }
    col.into()
}

/// A tile showing the average across row groups, plus the min–max range when the groups differ —
/// an uneven spread is usually what you came to this tab to find.
fn spread_tile<'a>(
    label: &'a str,
    values: &[i64],
    render: impl Fn(i64) -> String,
) -> Element<'a, Message> {
    let (Some(&min), Some(&max)) = (values.iter().min(), values.iter().max()) else {
        return stat_tile(label, "—".into(), None);
    };
    let average = values.iter().sum::<i64>() / values.len() as i64;
    let detail = (min != max).then(|| format!("{} – {}", render(min), render(max)));
    stat_tile(label, render(average), detail)
}

fn column_chunk_table(file: &FileSummary, rg_idx: usize) -> Element<'_, Message> {
    let w = CHUNK_COL_WIDTHS;
    let mut columns = [
        CcColumn::new("Column", w[0], |cc| cc.column_path().string()),
        CcColumn::new("Values", w[1], |cc| count(cc.num_values())),
        CcColumn::new("Nulls", w[2], |cc| {
            cc.statistics()
                .and_then(|s| s.null_count_opt())
                .map_or_else(|| "—".into(), |val| count(val as i64))
        }),
        CcColumn::new("Raw", w[3], |cc| {
            human_bytes(cc.uncompressed_size().max(0) as u64)
        }),
        CcColumn::new("Packed", w[4], |cc| {
            human_bytes(cc.compressed_size().max(0) as u64)
        }),
        CcColumn::new("Comp", w[5], |cc| {
            format_compression(cc.compression()).to_string()
        }),
        CcColumn::new("Coding", w[6], |cc| format_encodings(cc.encodings())),
        CcColumn::new("Min", w[7], |cc| stat_bound(cc, Bound::Min)),
        CcColumn::new("Max", w[8], |cc| stat_bound(cc, Bound::Max)),
    ];

    let mut header_row = row![];
    for column in columns.iter_mut() {
        header_row = header_row.push(column.element.take());
    }
    let mut table = column![header_row].spacing(0);

    if let Some(meta) = &file.metadata {
        for (idx, cc) in meta.row_group(rg_idx).columns().iter().enumerate() {
            let mut row = row![].spacing(0);

            for column in columns.iter() {
                row = row.push(body_cell((column.view)(cc), column.width));
            }

            let zebra = idx % 2 == 1;
            let styled = container(row).style(move |theme| body_row_style(theme, zebra));
            table = table.push(styled);
        }
    }

    // An accent rule down the gutter ties the detail block to the row it belongs to.
    let detail = row![
        Space::new().width(Length::Fixed(GUTTER - 3.0)),
        container(Space::new())
            .width(Length::Fixed(3.0))
            .height(Length::Fill)
            .style(|_: &Theme| ContainerStyle {
                background: Some(Background::Color(ui_theme::palette::accent_cool())),
                ..ContainerStyle::default()
            }),
        table,
    ];

    container(detail)
        .padding(Padding {
            top: 6.0,
            right: 0.0,
            bottom: 10.0,
            left: 0.0,
        })
        .into()
}

fn sort_cell<'a>(
    file: &'a FileSummary,
    rg: &'a RowGroupMetaData,
    width: f32,
) -> Element<'a, Message> {
    let content: Element<'a, Message> = match rg.sorting_columns() {
        Some(cols) if !cols.is_empty() => {
            Row::with_children(format_sorting_columns(file, cols).into_iter().map(|text| {
                iced::widget::Text::from(text)
                    .size(13)
                    .wrapping(Wrapping::None)
                    .into()
            }))
            .into()
        }
        Some(_) => ui_theme::muted(text("(empty)").size(13)).into(),
        None => ui_theme::muted(text("—").size(13)).into(),
    };

    container(content)
        .width(Length::Fixed(width))
        .padding([4, 10])
        .clip(true)
        .into()
}

fn percent(part: i64, whole: i64) -> String {
    if whole <= 0 {
        return "—".into();
    }
    format!("{:.1}%", part as f64 * 100.0 / whole as f64)
}

fn ratio(raw: i64, packed: i64) -> String {
    if packed <= 0 {
        return "—".into();
    }
    format!("{:.2}x", raw as f64 / packed as f64)
}

fn per_row(bytes: i64, rows: i64) -> String {
    if rows <= 0 || bytes < 0 {
        return "—".into();
    }
    human_bytes(bytes as u64 / rows as u64)
}

fn summary_header() -> Element<'static, Message> {
    let mut r = row![container(text(" ")).width(Length::Fixed(GUTTER))].spacing(0);
    for (label, width) in SUMMARY_HEADERS.iter().zip(SUMMARY_COL_WIDTHS) {
        r = r.push(header_cell(label, Length::Fixed(width)));
    }
    container(r).style(header_row_style).into()
}

fn format_compression(compression: Compression) -> &'static str {
    match compression {
        Compression::UNCOMPRESSED => "Uncompressed",
        Compression::SNAPPY => "Snappy",
        Compression::GZIP(_) => "Gzip",
        Compression::LZO => "LZO",
        Compression::BROTLI(_) => "Brotli",
        Compression::LZ4 => "LZ4",
        Compression::ZSTD(_) => "Zstd",
        Compression::LZ4_RAW => "LZ4_Raw",
    }
}

/// Sort key that floats the encoding you actually came to look at to the front.
///
/// A chunk's encoding set mixes the data-page encoding with the definition/repetition *level*
/// encodings, which are near-always `RLE` and carry no signal. Leading with the value encoding
/// means that when the cell is too narrow for the full list, what gets clipped is the part nobody
/// reads.
#[allow(deprecated)] // BIT_PACKED is deprecated upstream but still shows up in older files.
fn encoding_rank(encoding: Encoding) -> u8 {
    match encoding {
        Encoding::RLE_DICTIONARY | Encoding::PLAIN_DICTIONARY => 0,
        Encoding::DELTA_BINARY_PACKED
        | Encoding::DELTA_LENGTH_BYTE_ARRAY
        | Encoding::DELTA_BYTE_ARRAY
        | Encoding::BYTE_STREAM_SPLIT => 1,
        // In a dictionary-encoded column PLAIN is just the dictionary page's own encoding.
        Encoding::PLAIN => 2,
        Encoding::RLE | Encoding::BIT_PACKED => 3,
    }
}

/// Render a chunk's encodings, most informative first.
///
/// `ColumnChunkMetaData::encodings` walks a bit mask, so the input is already de-duplicated and in
/// discriminant order — which is not a useful order to read in, hence the sort.
fn format_encodings(encodings: impl IntoIterator<Item = Encoding>) -> String {
    let mut list: Vec<Encoding> = encodings.into_iter().collect();
    // Stable, so discriminant order still breaks ties within a rank.
    list.sort_by_key(|e| encoding_rank(*e));
    list.iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}
pub struct CcColumn<'a, 'b> {
    element: Option<Element<'a, Message>>,
    width: f32,
    view: Box<dyn Fn(&'a ColumnChunkMetaData) -> CellString + 'b>,
}
impl<'a, 'b> CcColumn<'a, 'b> {
    pub fn new<S: Into<CellString>>(
        header: &str,
        width: f32,
        view: impl Fn(&'a ColumnChunkMetaData) -> S + 'b,
    ) -> Self {
        Self {
            element: Some(header_cell(header, Length::Fixed(width))),
            width,
            view: Box::new(move |cc| view(cc).into()),
        }
    }
}

fn header_cell<'a>(label: &str, length: Length) -> Element<'a, Message> {
    let label = text(label.to_string()).size(13).wrapping(Wrapping::None);
    container(label)
        .width(length)
        .padding([6, 10])
        .clip(true)
        .into()
}

fn body_cell<'a>(value: impl Into<CellString>, width: f32) -> Element<'a, Message> {
    let value = value.into();
    // Never wrap: a cell with no fixed height that wraps stretches its whole row, so one long
    // encoding list used to make every other cell in the row float in three lines of whitespace.
    // Word wrapping couldn't save the long values anyway — it can't break `DELTA_BINARY_PACKED`.
    let label = text(value.short_or_real().clone())
        .size(13)
        .wrapping(Wrapping::None);
    let inner = container(label)
        .width(Length::Fixed(width))
        .padding([4, 10])
        .clip(true);

    // Only build the tooltip when it's needed: it's constructed even while hidden, and a wide row
    // group would otherwise allocate one per cell per frame. 20.0 = the horizontal padding above.
    let content: Element<'a, Message> = if value.overflows(width - 20.0) {
        tooltip(
            inner,
            ui_theme::tooltip_panel(value.short_or_real().clone()),
            tooltip::Position::Bottom,
        )
        // Every cell is a tooltip target; with no delay, dragging across the table strobes.
        .delay(Duration::from_millis(350))
        .gap(4)
        .into()
    } else {
        inner.into()
    };

    // Outermost, so the click lands whether or not the tooltip is in the way.
    mouse_area(content)
        .on_press(FileMessage::CopyCell(value.real).into())
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn header_row_style(theme: &Theme) -> ContainerStyle {
    let p = theme.extended_palette();
    ContainerStyle {
        background: Some(Background::Color(p.background.strong.color)),
        text_color: Some(p.background.strong.text),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

fn body_row_style(theme: &Theme, zebra: bool) -> ContainerStyle {
    let p = theme.extended_palette();
    let bg = if zebra {
        p.background.weak.color
    } else {
        p.background.base.color
    };
    ContainerStyle {
        background: Some(Background::Color(bg)),
        text_color: Some(p.background.base.text),
        border: Border::default(),
        ..ContainerStyle::default()
    }
}

/// Render one statistics bound, decoded through the column's logical type.
fn stat_bound(cc: &ColumnChunkMetaData, bound: Bound) -> CellString {
    cc.statistics()
        .and_then(|stats| stats_format::stat_value(stats, cc.column_descr(), bound))
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_tables_share_a_right_edge() {
        // The layout only looks right if these match; the const assert above enforces it at compile
        // time, this pins the intended total so a width tweak has to be deliberate.
        assert_eq!(total_width(&CHUNK_COL_WIDTHS), 1250.0);
        assert_eq!(total_width(&SUMMARY_COL_WIDTHS), 1250.0);
        assert_eq!(TABLE_WIDTH, 1290.0);
        assert_eq!(SUMMARY_HEADERS.len(), SUMMARY_COL_WIDTHS.len());
    }

    #[test]
    fn the_value_encoding_leads_the_coding_cell() {
        // Discriminant order would put PLAIN first; the reader wants to see the dictionary.
        assert_eq!(
            format_encodings([Encoding::PLAIN, Encoding::RLE, Encoding::RLE_DICTIONARY]),
            "RLE_DICTIONARY, PLAIN, RLE"
        );
        assert_eq!(
            format_encodings([Encoding::RLE, Encoding::DELTA_BINARY_PACKED]),
            "DELTA_BINARY_PACKED, RLE"
        );
        assert_eq!(format_encodings([]), "");
    }

    #[test]
    fn every_encoding_is_ranked() {
        // Fails loudly if a parquet bump adds a variant that encoding_rank hasn't placed.
        #[allow(deprecated)]
        let variants = Encoding::VARIANTS;
        assert_eq!(variants.len(), 9);
        for &e in variants {
            assert!(encoding_rank(e) <= 3, "{e:?}");
        }
    }

    #[test]
    fn the_comp_column_fits_every_compression_label() {
        // This column held "Uncompressed" in 30px of room until the widths were rebalanced.
        for c in [
            Compression::UNCOMPRESSED,
            Compression::SNAPPY,
            Compression::GZIP(Default::default()),
            Compression::LZO,
            Compression::BROTLI(Default::default()),
            Compression::LZ4,
            Compression::ZSTD(Default::default()),
            Compression::LZ4_RAW,
        ] {
            let cell: CellString = format_compression(c).into();
            assert!(!cell.overflows(CHUNK_COL_WIDTHS[5] - 20.0), "{c:?}");
        }
    }

    #[test]
    fn the_coding_column_fits_a_dictionary_encoded_chunk() {
        // The common case — anything longer clips its tail and falls back to the tooltip.
        let cell: CellString = format_encodings([Encoding::RLE_DICTIONARY, Encoding::PLAIN]).into();
        assert!(!cell.overflows(CHUNK_COL_WIDTHS[6] - 20.0));
    }

    #[test]
    fn derived_columns_handle_empty_and_degenerate_groups() {
        assert_eq!(percent(3, 10), "30.0%");
        assert_eq!(percent(0, 0), "—");
        assert_eq!(ratio(1000, 250), "4.00x");
        assert_eq!(ratio(1000, 0), "—");
        assert_eq!(per_row(2048, 2), "1.00 KiB");
        assert_eq!(per_row(2048, 0), "—");
    }
}
