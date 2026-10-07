//! The SQL workspace view: a strip of editor tabs (each bound to its own
//! source), the active editor + results grid, a shared in-memory query-history
//! panel, and the FlightSQL connect modal. Styling reuses `crate::theme`.

use iced::alignment::Vertical;
use iced::keyboard::Key;
use iced::keyboard::key::Named;
use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::text_editor::{Action, Binding, Edit, KeyPress, Status as EditorStatus};
use iced::widget::{
    Space, button, checkbox, column, container, mouse_area, opaque, pick_list, pin, row,
    scrollable, stack, text, text_editor, text_input, toggler,
};
use iced::{Background, Border, Color, Element, Length, Theme};
use sql_ide::CompletionKind;

use crate::app::{
    App, AuthKind, CompletionState, ConnectForm, ExportDialogState, FlightMessage, HistoryStatus,
    MAX_EDITOR_HEIGHT, MIN_EDITOR_HEIGHT, Message, RESULT_PAGE_SIZE, SourceRef, SqlEditorTab,
    SqlMessage,
};
use crate::export::{ExportFormat, ParquetColumnOptions, ParquetCompression};
use crate::export::{ParquetEncoding, ParquetVersion};
use crate::sqlide_highlight::HighlightSettings;
use crate::sqlide_highlight::SqlHighlighter;
use crate::theme::palette;
use crate::theme::{self, FONT_UI_MEDIUM};
use crate::widgets::resize_handle_vertical;

/// Inner padding of the editor text area, also applied to the gutter so line
/// numbers align with their lines.
pub(crate) const EDITOR_PAD: f32 = 10.0;
const EDITOR_FONT_SIZE: f32 = 13.0;
/// Exact row height of one editor line (shared by the gutter and the popup
/// placement maths).
pub(crate) const LINE_H: f32 = EDITOR_FONT_SIZE * 1.3;
/// Advance width of JetBrains Mono at `EDITOR_FONT_SIZE`.
const CHAR_W: f32 = 7.8;
/// Tallest the completion popup gets before it scrolls internally.
const COMPLETION_MAX_H: f32 = 200.0;

/// Widget id of an editor tab's scroll area (target of scroll-to-caret).
pub(crate) fn editor_scroll_id(id: u64) -> iced::widget::Id {
    iced::widget::Id::from(format!("sql-editor-scroll-{id}"))
}

pub fn view(app: &App) -> Element<'_, Message> {
    let strip = editor_tab_strip(app);

    let active = app.sql.editors.get(app.sql.active);
    let body: Element<'_, Message> = match active {
        Some(tab) => editor_pane(tab, app.sql.editor_height),
        None => empty_state(),
    };

    let split = row![
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, 8]),
        history_panel(app),
    ]
    .spacing(0);

    let main: Element<'_, Message> = column![strip, split].spacing(8).height(Length::Fill).into();

    // Nested-cell detail overlay for the active editor's results grid.
    match active.and_then(|t| t.cell_detail.as_ref().map(|d| (t.id, d))) {
        Some((id, detail)) => stack![
            main,
            crate::views::data::cell_detail_overlay(
                detail,
                active.and_then(|t| t.cell_detail_editor.as_ref()),
                id,
                SqlMessage::CloseCellDetail { id }.into(),
            )
        ]
        .into(),
        None => main,
    }
}

// -- Editor tab strip ---------------------------------------------------------

fn editor_tab_strip(app: &App) -> Element<'_, Message> {
    let mut r = row![].spacing(0).align_y(iced::Alignment::Center);
    for (i, tab) in app.sql.editors.iter().enumerate() {
        let active = i == app.sql.active;
        let label = theme::ui_medium(elide(&tab.title, 22))
            .size(12)
            .style(move |_: &Theme| iced::widget::text::Style {
                color: Some(if active {
                    theme::palette::fg_primary()
                } else {
                    theme::palette::fg_muted()
                }),
            });
        let select = button(label)
            .padding([8, 8])
            .style(theme::tab_button(active))
            .on_press(SqlMessage::EditorSelect(tab.id).into());
        let close = button(text("✕").size(10))
            .style(theme::ghost_button)
            .padding([2, 6])
            .on_press(SqlMessage::EditorClose(tab.id).into());
        let underline = container(Space::new())
            .height(Length::Fixed(if active { 2.0 } else { 0.0 }))
            .width(Length::Fill)
            .style(theme::tab_underline);
        let cell = column![
            row![select, close].align_y(iced::Alignment::Center),
            underline,
        ]
        .width(Length::Shrink);
        r = r.push(cell);
    }

    let plus = button(theme::ui_medium("+  New query").size(12))
        .style(theme::ghost_button)
        .padding([8, 10])
        .on_press(SqlMessage::NewQueryToggle.into());
    r = r.push(plus);

    let strip = container(r).padding([0, 4]).width(Length::Fill);

    if app.sql.source_picker_open {
        column![strip, source_picker(app)].spacing(4).into()
    } else {
        strip.into()
    }
}

fn source_picker(app: &App) -> Element<'_, Message> {
    let mut col = column![theme::label_text("Open a query against")].spacing(4);
    let mut any = false;
    for ft in &app.files {
        if !ft.registered {
            continue;
        }
        let name = ft
            .summary
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        col = col.push(picker_button(
            format!("file: {name}"),
            SourceRef::File(ft.id),
        ));
        any = true;
    }
    for (i, c) in app.connections.iter().enumerate() {
        col = col.push(picker_button(
            format!("flight: {}", c.label),
            SourceRef::Flight(i),
        ));
        any = true;
    }
    if !any {
        col = col.push(theme::mono_sm("No sources available."));
    }
    container(col)
        .padding(10)
        .width(Length::Fixed(280.0))
        .style(theme::surface_2)
        .into()
}

fn picker_button<'a>(label: String, src: SourceRef) -> Element<'a, Message> {
    button(theme::mono_sm(label))
        .style(theme::ghost_button)
        .width(Length::Fill)
        .padding([4, 8])
        .on_press(SqlMessage::NewQueryForSource(src).into())
        .into()
}

// -- Active editor pane -------------------------------------------------------

fn editor_pane(tab: &SqlEditorTab, editor_height: f32) -> Element<'_, Message> {
    let id = tab.id;
    let popup_open = tab.completion.as_ref().is_some_and(|c| !c.items.is_empty());
    let caret = tab.content.cursor().position;
    let line_count = tab.content.line_count().max(1);
    let diag_line = tab
        .diagnostics
        .first()
        .and_then(|d| d.line)
        .map(|l| l.saturating_sub(1) as usize);
    let error_at = tab.diagnostics.first().and_then(|d| {
        let line = d.line?.saturating_sub(1) as usize;
        let col_chars = d.column?.saturating_sub(1) as usize;
        let byte = tab.content.line(line).map(|l| {
            l.text
                .char_indices()
                .nth(col_chars)
                .map_or(l.text.len(), |(b, _)| b)
        })?;
        Some((line, byte))
    });
    let settings = HighlightSettings {
        bracket_pair: tab.bracket_pair,
        error_at,
    };

    // The editor grows with its content (`Shrink`) and the surrounding
    // scrollable provides the viewport, so a line-number gutter laid out
    // beside it scrolls in lockstep.
    let editor = text_editor(&tab.content)
        .on_action(move |a| SqlMessage::EditorAction(id, a).into())
        .key_binding(move |kp| editor_key_binding(id, popup_open, kp))
        .highlight_with::<SqlHighlighter>(settings, crate::sqlide_highlight::to_format)
        .font(theme::FONT_MONO)
        .size(EDITOR_FONT_SIZE)
        .line_height(LineHeight::Absolute(LINE_H.into()))
        // No wrapping keeps (line, column) a faithful grid for popup placement.
        .wrapping(Wrapping::None)
        .height(Length::Shrink)
        // Fill the viewport even for short documents so a click anywhere in
        // the box (not just on a line) focuses the editor. Padding is added
        // on top of this minimum, so subtract it to avoid a phantom scrollbar.
        .min_height(editor_height - 2.0 * EDITOR_PAD)
        .padding(EDITOR_PAD)
        .style(editor_style);

    let gutter_w = gutter_width(line_count);
    let scroll = scrollable(row![
        gutter(line_count, caret.line, diag_line, gutter_w),
        editor
    ])
    .id(editor_scroll_id(id))
    .width(Length::Fill)
    .height(Length::Fixed(editor_height))
    .on_scroll(move |vp| {
        SqlMessage::EditorScrolled {
            id,
            offset_y: vp.absolute_offset().y,
            height: vp.bounds().height,
        }
        .into()
    });

    // Float the completion popup over the editor near the cursor. iced exposes
    // no pixel caret, so approximate from the (line, column) text position and
    // monospace metrics (CHAR_W is calibrated for JetBrains Mono at size 13).
    //
    // The scroll area is ALWAYS the first child of a `stack`, whether or not
    // the popup is shown. Changing the surrounding widget type would shift the
    // editor's position in the tree and make iced rebuild its state — dropping
    // keyboard focus, which silently blocks typing.
    let editor_framed = container(scroll)
        .width(Length::Fill)
        .style(editor_frame_style);
    let mut layers = stack![editor_framed];
    if let Some(c) = &tab.completion
        && !c.items.is_empty()
    {
        let scroll_y = tab.editor_viewport.map_or(0.0, |v| v.offset_y);
        let x = gutter_w + EDITOR_PAD + tab.caret_char_column() as f32 * CHAR_W;
        let line_top = EDITOR_PAD + caret.line as f32 * LINE_H - scroll_y;
        let below = line_top + LINE_H;
        // Rough popup height (rows are ~21px, capped like the list itself).
        // Prefer below the caret line, then above; when neither fits, take
        // the roomier side and cap the list so it never clips at the frame.
        let est_h = (c.items.len().min(9) as f32 * 21.0 + 8.0).min(COMPLETION_MAX_H);
        let room_below = editor_height - below;
        let (y, max_h) = if est_h <= room_below {
            (below, COMPLETION_MAX_H)
        } else if est_h <= line_top {
            (line_top - est_h, COMPLETION_MAX_H)
        } else if room_below >= line_top {
            (below, room_below)
        } else {
            let h = est_h.min(line_top);
            (line_top - h, h)
        };
        // Only show the popup while its anchor line is inside the viewport.
        if line_top >= 0.0 && below <= editor_height && max_h >= LINE_H {
            // `opaque` must wrap the popup box itself, NOT the `pin`: `pin`
            // defaults to Length::Fill, so `opaque(pin(..))` would mark the
            // whole editor area as click-capturing and swallow every click
            // before it reaches the editor.
            layers = layers.push(pin(opaque(completion_list(id, c, max_h))).x(x).y(y));
        }
    }
    let editor_layer: Element<'_, Message> = layers.into();

    let divider = resize_handle_vertical(
        editor_height,
        |h| SqlMessage::EditorResize(h).into(),
        SqlMessage::EditorResizeEnd.into(),
        SqlMessage::EditorResizeReset.into(),
    )
    .range(MIN_EDITOR_HEIGHT, MAX_EDITOR_HEIGHT)
    .idle_visible();

    let mut explain_btn = button(theme::ui_medium("Explain").size(12))
        .style(theme::ghost_button)
        .padding([6, 12]);
    let mut explain_an_btn = button(theme::ui_medium("Explain Analyze").size(12))
        .style(theme::ghost_button)
        .padding([6, 12]);
    let mut export_btn = button(theme::ui_medium("Export…").size(12))
        .style(theme::ghost_button)
        .padding([6, 12]);
    if !tab.running {
        explain_btn = explain_btn.on_press(SqlMessage::Explain(id).into());
        explain_an_btn = explain_an_btn.on_press(SqlMessage::ExplainAnalyze(id).into());
        export_btn = export_btn.on_press(SqlMessage::ExportOpen(id).into());
    }
    // While a query runs, the Run button becomes a Cancel button that aborts it.
    let run: Element<'_, Message> = if tab.running {
        button(theme::ui_medium("Cancel  ◼").size(12))
            .style(theme::danger_button)
            .padding([6, 16])
            .on_press(SqlMessage::Cancel(id).into())
            .into()
    } else {
        button(theme::ui_medium("Run  ▷").size(12))
            .style(theme::accent_button)
            .padding([6, 16])
            .on_press(SqlMessage::Run(id).into())
            .into()
    };

    let source_pill = container(theme::mono_sm(elide(&tab.title, 32)).wrapping(Wrapping::None))
        .padding([2, 8])
        .style(source_pill_style);

    let meta: Element<'_, Message> = if tab.running {
        theme::mono_sm("running…").wrapping(Wrapping::None).into()
    } else if let (Some(elapsed), Some(rows)) = (tab.last_elapsed, tab.last_row_count) {
        theme::mono_sm(format!("{} rows · {}", count(rows), elapsed))
            .wrapping(Wrapping::None)
            .style(muted)
            .into()
    } else {
        Space::new().width(Length::Fixed(0.0)).into()
    };

    let trunc: Element<'_, Message> = if tab.truncated {
        container(
            theme::mono_sm(format!(
                "first {} rows (capped)",
                count(tab.last_row_count.unwrap_or(0))
            ))
            .wrapping(Wrapping::None),
        )
        .padding([2, 8])
        .style(theme::notice_pill)
        .into()
    } else {
        Space::new().width(Length::Fixed(0.0)).into()
    };

    // Secondary info shrinks and clips so the action buttons (the only
    // fixed-width items) stay visible and on-screen as the window narrows.
    let info = container(
        row![
            source_pill,
            Space::new().width(Length::Fixed(10.0)),
            meta,
            Space::new().width(Length::Fixed(10.0)),
            trunc,
        ]
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .clip(true);

    let toolbar = row![
        info,
        Space::new().width(Length::Fixed(10.0)),
        export_btn,
        Space::new().width(Length::Fixed(6.0)),
        explain_btn,
        Space::new().width(Length::Fixed(6.0)),
        explain_an_btn,
        Space::new().width(Length::Fixed(6.0)),
        run,
    ]
    .align_y(iced::Alignment::Center);

    let error: Element<'_, Message> = match &tab.error {
        Some(e) => container(
            text(format!("Error: {e}"))
                .size(12)
                .color(theme::palette::accent_rose())
                .wrapping(Wrapping::Word),
        )
        .padding(8)
        .width(Length::Fill)
        .style(error_style)
        .into(),
        None => Space::new().width(Length::Fixed(0.0)).into(),
    };

    let results: Element<'_, Message> = match tab.batch.as_ref() {
        Some(b) if b.num_rows() > 0 => match tab.explain {
            // Formatted plan view (with a toggle back to the raw grid).
            Some(kind) if !tab.explain_raw => column![
                explain_toggle(id, kind, tab.explain_raw),
                crate::views::explain::view_plan(b, kind),
            ]
            .spacing(6)
            .height(Length::Fill)
            .into(),
            // EXPLAIN result, but the user asked for the raw grid.
            Some(kind) => column![
                explain_toggle(id, kind, tab.explain_raw),
                results_grid(
                    b,
                    id,
                    &tab.insights,
                    &tab.col_widths,
                    tab.row_height,
                    tab.page
                ),
            ]
            .spacing(6)
            .height(Length::Fill)
            .into(),
            // Ordinary result set.
            None => results_grid(
                b,
                id,
                &tab.insights,
                &tab.col_widths,
                tab.row_height,
                tab.page,
            ),
        },
        Some(_) => container(theme::mono_sm("(query returned no rows)"))
            .padding(12)
            .into(),
        None => {
            let msg = if tab.running {
                "Running…"
            } else {
                "Press Run (or Ctrl+Enter in the editor) to execute."
            };
            container(theme::mono_sm(msg)).padding(12).into()
        }
    };

    let base: Element<'_, Message> = column![
        editor_layer,
        divider,
        status_bar(tab),
        toolbar,
        error,
        results
    ]
    .spacing(6)
    .height(Length::Fill)
    .into();

    match tab.export_dialog.as_ref() {
        Some(dialog) => stack![base, export_overlay(id, dialog)].into(),
        None => base,
    }
}

/// Width of the line-number gutter for `line_count` lines.
fn gutter_width(line_count: usize) -> f32 {
    let digits = line_count.max(1).to_string().len().max(2) as f32;
    digits * CHAR_W + 18.0
}

/// Line numbers beside the editor: one row per line at exactly `LINE_H`, the
/// caret line emphasised, the first syntax error marked.
fn gutter<'a>(
    line_count: usize,
    caret_line: usize,
    diag_line: Option<usize>,
    width: f32,
) -> Element<'a, Message> {
    let mut col = column![].spacing(0);
    for i in 0..line_count {
        let is_caret = i == caret_line;
        let is_err = diag_line == Some(i);
        let color = if is_err {
            palette::accent_rose()
        } else if is_caret {
            palette::fg_primary()
        } else {
            palette::fg_dim()
        };
        let label = text(format!("{}", i + 1))
            .font(if is_caret {
                theme::FONT_MONO_MEDIUM
            } else {
                theme::FONT_MONO
            })
            .size(EDITOR_FONT_SIZE)
            .line_height(LineHeight::Absolute(LINE_H.into()))
            .color(color)
            .wrapping(Wrapping::None)
            .align_x(iced::alignment::Horizontal::Right)
            .width(Length::Fill);
        col = col.push(
            container(label)
                .height(Length::Fixed(LINE_H))
                .width(Length::Fill)
                .padding([0, 8]),
        );
    }
    container(col)
        .width(Length::Fixed(width))
        .padding([EDITOR_PAD, 0.0])
        .style(gutter_style)
        .into()
}

/// Caret position, selection size, the Ctrl+Enter target, and the first
/// syntax diagnostic (clickable: jumps the caret to it).
fn status_bar(tab: &SqlEditorTab) -> Element<'_, Message> {
    let id = tab.id;
    let caret = tab.content.cursor().position;
    let mut items = row![].spacing(14).align_y(iced::Alignment::Center);

    items = items.push(
        theme::mono_sm(format!(
            "Ln {}, Col {}",
            caret.line + 1,
            tab.caret_char_column() + 1
        ))
        .style(muted)
        .wrapping(Wrapping::None),
    );

    let selection = tab.content.selection().filter(|s| !s.is_empty());
    if let Some(sel) = &selection {
        items = items.push(
            theme::mono_sm(format!("{} selected", count(sel.chars().count() as i64)))
                .style(muted)
                .wrapping(Wrapping::None),
        );
    }

    let hint = if selection.is_some() {
        "Ctrl+Enter runs selection".to_string()
    } else {
        let text = tab.content.text();
        match sql_ide::statement_index(&text, tab.caret_byte_offset()) {
            Some((i, n)) => format!("Ctrl+Enter runs statement {} of {n}", i + 1),
            None => "Ctrl+Enter runs query".to_string(),
        }
    };
    items = items.push(theme::mono_sm(hint).style(muted).wrapping(Wrapping::None));

    items = items.push(Space::new().width(Length::Fill));

    if let Some(d) = tab.diagnostics.first() {
        let location = match (d.line, d.column) {
            (Some(l), Some(c)) => format!(" (Ln {l}, Col {c})"),
            _ => String::new(),
        };
        let message = elide_oneline(strip_location(&d.message), 90);
        items = items.push(
            button(
                text(format!("⚠ {message}{location}"))
                    .size(11)
                    .color(palette::accent_warm())
                    .wrapping(Wrapping::None),
            )
            .style(theme::ghost_button)
            .padding([1, 6])
            .on_press(SqlMessage::GotoDiagnostic(id).into()),
        );
    }

    // Format lives here rather than in the action toolbar, which is already
    // full at narrow widths.
    let mut format_btn = button(theme::ui_medium("Format").size(11))
        .style(theme::ghost_button)
        .padding([1, 8]);
    if !tab.content.is_empty() {
        format_btn = format_btn.on_press(SqlMessage::Format(id).into());
    }
    items = items.push(format_btn);

    container(items)
        .width(Length::Fill)
        .padding([0, 4])
        .clip(true)
        .into()
}

/// Drop sqlparser's trailing ` at Line: N, Column: M` (we show it ourselves).
fn strip_location(message: &str) -> &str {
    message
        .rfind(" at Line:")
        .map_or(message, |i| &message[..i])
        .trim_end()
}

#[derive(Debug, Clone, PartialEq)]
enum TriState {
    Default,
    Yes,
    No,
}
impl std::fmt::Display for TriState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl From<TriState> for Option<bool> {
    fn from(value: TriState) -> Self {
        match value {
            TriState::Default => None,
            TriState::Yes => Some(true),
            TriState::No => Some(false),
        }
    }
}
impl From<Option<bool>> for TriState {
    fn from(value: Option<bool>) -> Self {
        match value {
            Some(true) => TriState::Yes,
            Some(false) => TriState::No,
            None => TriState::Default,
        }
    }
}
impl TriState {
    const ALL: &[TriState] = &[Self::Default, Self::Yes, Self::No];
}

/// Modal: pick an export format and its settings, then write the full
/// (uncapped) query result to a file.
fn export_overlay<'a>(id: u64, dialog: &'a ExportDialogState) -> Element<'a, Message> {
    let fmt = dialog.format;

    // Format selector.
    let mut formats = row![].spacing(6);
    for f in ExportFormat::ALL {
        formats = formats.push(
            button(theme::ui_medium(f.label()).size(12))
                .style(theme::tab_button(f == fmt))
                .padding([4, 12])
                .on_press(SqlMessage::ExportSetFormat(id, f).into()),
        );
    }

    // Format-specific settings.
    let settings: Element<'a, Message> = match fmt {
        ExportFormat::Parquet => {
            let opts = &dialog.opts_parquet;

            let comp = pick_list(
                ParquetCompression::ALL.as_slice(),
                Some(opts.compression),
                move |c| SqlMessage::ExportSetCompression(id, c).into(),
            );

            let dict = pick_list(
                TriState::ALL,
                Some(TriState::from(opts.dictionary)),
                move |dict| SqlMessage::ExportSetDictionary(id, dict.into()).into(),
            );

            let version = pick_list(ParquetVersion::ALL, Some(opts.version), move |v| {
                SqlMessage::ExportSetParquetVersion(id, v).into()
            });

            let col_name = text_input("Configure column...", &dialog.parquet_column_name)
                .on_input(move |s| SqlMessage::ExportParquetColumnName(id, s).into())
                .on_submit(
                    SqlMessage::ExportParquetColumnOptions {
                        id,
                        column: dialog.parquet_column_name.clone(),
                        options: Some(ParquetColumnOptions::default()),
                    }
                    .into(),
                );

            let col_opts_header = row![theme::label_text("Column options"), col_name]
                .spacing(6)
                .align_y(Vertical::Center);

            let mut col_opts = column![].spacing(12);

            for (col, opts) in opts.per_column_options.iter() {
                let label = text(col).font(FONT_UI_MEDIUM).size(12);
                let remove = button(theme::ui_medium("x"))
                    .style(theme::danger_button)
                    .padding(4)
                    .on_press(
                        SqlMessage::ExportParquetColumnOptions {
                            id,
                            column: col.clone(),
                            options: None,
                        }
                        .into(),
                    );

                let encoding =
                    pick_list(ParquetEncoding::ALL.as_slice(), opts.encoding, move |c| {
                        SqlMessage::ExportParquetColumnOptions {
                            id,
                            column: col.clone(),
                            options: Some(ParquetColumnOptions {
                                encoding: Some(c),
                                ..opts.clone()
                            }),
                        }
                        .into()
                    });

                let comp = pick_list(
                    ParquetCompression::ALL.as_slice(),
                    opts.compression,
                    move |c| {
                        SqlMessage::ExportParquetColumnOptions {
                            id,
                            column: col.clone(),
                            options: Some(ParquetColumnOptions {
                                compression: Some(c),
                                ..opts.clone()
                            }),
                        }
                        .into()
                    },
                );
                let dict = pick_list(
                    TriState::ALL,
                    Some(TriState::from(opts.dictionary)),
                    move |c| {
                        SqlMessage::ExportParquetColumnOptions {
                            id,
                            column: col.clone(),
                            options: Some(ParquetColumnOptions {
                                dictionary: c.into(),
                                ..opts.clone()
                            }),
                        }
                        .into()
                    },
                );

                col_opts = col_opts.push(column![
                    row![remove, labelled("Column", label.into())]
                        .align_y(Vertical::Center)
                        .spacing(8),
                    column![
                        labelled("Compression", comp.into()),
                        labelled("Dictionary", dict.into()),
                        labelled("Encoding", encoding.into())
                    ]
                    .padding([0, 12]),
                ]);
            }

            let col_opts = scrollable(col_opts).width(Length::Fill).height(
                if opts.per_column_options.is_empty() {
                    Length::Shrink
                } else {
                    Length::Fixed(150.0)
                },
            );

            column![
                labelled("Compression", comp.into()),
                labelled("Dictionary", dict.into()),
                labelled("Version", version.into()),
                col_opts_header,
                col_opts
            ]
            .spacing(12)
            .into()
        }
        ExportFormat::Csv => {
            let opts = &dialog.opts_csv;

            let header = checkbox(opts.header)
                .on_toggle(move |b| SqlMessage::ExportToggleHeader(id, b).into());

            let delim = text_input("", &(opts.delimiter as char).to_string())
                .on_input(move |s| SqlMessage::ExportDelimiter(id, s).into())
                .padding([6, 8])
                .width(Length::Fixed(40.0));

            column![
                labelled("Header row", header.into()),
                labelled("Delimiter", delim.into()),
            ]
            .spacing(12)
            .into()
        }
        ExportFormat::Json => {
            let opts = &dialog.opts_json;

            let ndjson = toggler(opts.ndjson)
                .label(if opts.ndjson {
                    "NDJSON (one object per line)"
                } else {
                    "JSON array"
                })
                .font(FONT_UI_MEDIUM)
                .text_size(12)
                .on_toggle(move |b| SqlMessage::ExportToggleNdjson(id, b).into());

            labelled("Layout", ndjson.into())
        }
    };

    let err: Element<'a, Message> = match &dialog.error {
        Some(e) => container(
            text(format!("Export failed: {e}"))
                .size(12)
                .color(theme::palette::accent_rose())
                .wrapping(Wrapping::Word),
        )
        .into(),
        None => Space::new().height(Length::Fixed(0.0)).into(),
    };

    let mut confirm = button(
        theme::ui_medium(if dialog.in_progress {
            "Exporting…"
        } else {
            "Export"
        })
        .size(13),
    )
    .style(theme::accent_button)
    .padding([6, 16]);
    if !dialog.in_progress {
        confirm = confirm.on_press(SqlMessage::ExportConfirm(id).into());
    }
    let cancel = button(theme::ui("Cancel").size(13))
        .style(theme::ghost_button)
        .padding([6, 12])
        .on_press(SqlMessage::ExportCancel(id).into());

    let actions = row![
        Space::new().width(Length::Fill),
        cancel,
        Space::new().width(Length::Fixed(6.0)),
        confirm,
    ]
    .align_y(iced::Alignment::Center);

    let panel = container(
        column![
            theme::display_strong("Export query results"),
            theme::mono_sm("Re-runs the query and writes the full (uncapped) result.").style(muted),
            labelled("Format", formats.into()),
            settings,
            err,
            actions,
        ]
        .spacing(12),
    )
    .padding(18)
    .width(Length::Fixed(460.0))
    .style(theme::surface_2);

    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::backdrop),
    )
    .on_press(SqlMessage::ExportCancel(id).into());

    let centered = container(opaque(panel))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    stack![backdrop, centered].into()
}

/// The standard scrollable results grid for a SQL editor's batch, paginated
/// client-side over the already-fetched (capped) result.
fn results_grid<'a>(
    batch: &'a arrow::record_batch::RecordBatch,
    id: u64,
    insights: &'a [crate::wrangle::insights::ColumnInsight],
    col_widths: &'a [f32],
    row_height: f32,
    page: usize,
) -> Element<'a, Message> {
    let total = batch.num_rows();
    let pages = total.div_ceil(RESULT_PAGE_SIZE).max(1);
    let page = page.min(pages - 1);

    let grid = scrollable(crate::views::data::view_grid(
        batch,
        insights,
        id,
        page,
        RESULT_PAGE_SIZE,
        col_widths,
        row_height,
    ))
    .direction(iced::widget::scrollable::Direction::Both {
        vertical: iced::widget::scrollable::Scrollbar::default(),
        horizontal: iced::widget::scrollable::Scrollbar::default(),
    })
    .width(Length::Fill)
    .height(Length::Fill);

    if pages <= 1 {
        return grid.into();
    }
    column![pagination_bar(id, page, pages, total), grid]
        .spacing(6)
        .height(Length::Fill)
        .into()
}

/// Prev/next/first/last controls + a `rows a–b of N · page x/y` label.
fn pagination_bar<'a>(id: u64, page: usize, pages: usize, total: usize) -> Element<'a, Message> {
    let start = page * RESULT_PAGE_SIZE + 1;
    let end = ((page + 1) * RESULT_PAGE_SIZE).min(total);
    let nav = |glyph: &'static str, target: usize, enabled: bool| {
        let b = button(theme::ui_medium(glyph).size(12))
            .style(theme::ghost_button)
            .padding([2, 8]);
        if enabled {
            b.on_press(SqlMessage::SetResultPage { id, page: target }.into())
        } else {
            b
        }
    };
    let has_prev = page > 0;
    let has_next = page + 1 < pages;
    row![
        nav("⏮", 0, has_prev),
        nav("◀", page.saturating_sub(1), has_prev),
        theme::mono_sm(format!(
            "rows {start}–{end} of {total}  ·  page {}/{}",
            page + 1,
            pages
        ))
        .color(palette::fg_muted()),
        nav("▶", page + 1, has_next),
        nav("⏭", pages - 1, has_next),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Formatted | Raw segmented toggle for an EXPLAIN result.
fn explain_toggle<'a>(
    id: u64,
    kind: crate::explain::ExplainKind,
    raw: bool,
) -> Element<'a, Message> {
    let formatted = button(theme::ui_medium("Formatted").size(11))
        .style(theme::tab_button(!raw))
        .padding([3, 10])
        .on_press_maybe(raw.then_some(SqlMessage::ExplainToggleRaw(id).into()));
    let raw_btn = button(theme::ui_medium("Raw").size(11))
        .style(theme::tab_button(raw))
        .padding([3, 10])
        .on_press_maybe((!raw).then_some(SqlMessage::ExplainToggleRaw(id).into()));
    row![theme::label_text(kind.label()), formatted, raw_btn]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
}

/// Editor key bindings. Command chords come first (undo/redo, run, format,
/// comment, duplicate, completion request); while the completion popup is
/// open, arrows move the selection, Tab/Enter accept and Esc dismisses;
/// otherwise Tab/Shift+Tab indent and Enter auto-indents. Everything else
/// falls through to iced's defaults.
fn editor_key_binding(id: u64, open: bool, kp: KeyPress) -> Option<Binding<Message>> {
    if !matches!(kp.status, EditorStatus::Focused { .. }) {
        return None;
    }
    let shift = kp.modifiers.shift();
    if kp.modifiers.command() {
        let msg: Option<SqlMessage> = match &kp.key {
            Key::Character(c) => match c.as_str() {
                "z" if shift => Some(SqlMessage::Redo(id)),
                "z" => Some(SqlMessage::Undo(id)),
                "y" => Some(SqlMessage::Redo(id)),
                "f" if shift => Some(SqlMessage::Format(id)),
                "/" => Some(SqlMessage::ToggleComment(id)),
                "d" => Some(SqlMessage::DuplicateLine(id)),
                _ => None,
            },
            Key::Named(Named::Enter) if shift => Some(SqlMessage::Run(id)),
            Key::Named(Named::Enter) => Some(SqlMessage::RunSmart(id)),
            Key::Named(Named::Space) => Some(SqlMessage::CompletionRequest(id)),
            _ => None,
        };
        if let Some(m) = msg {
            return Some(Binding::Custom(m.into()));
        }
    }
    if open {
        match &kp.key {
            Key::Named(Named::ArrowDown) => {
                return Some(Binding::Custom(SqlMessage::CompletionMove(id, 1).into()));
            }
            Key::Named(Named::ArrowUp) => {
                return Some(Binding::Custom(SqlMessage::CompletionMove(id, -1).into()));
            }
            Key::Named(Named::Tab) | Key::Named(Named::Enter) => {
                return Some(Binding::Custom(
                    SqlMessage::CompletionAcceptSelected(id).into(),
                ));
            }
            Key::Named(Named::Escape) => {
                return Some(Binding::Custom(SqlMessage::CompletionDismiss(id).into()));
            }
            _ => {}
        }
    }
    match &kp.key {
        Key::Named(Named::Tab) => {
            let edit = if shift { Edit::Unindent } else { Edit::Indent };
            return Some(Binding::Custom(
                SqlMessage::EditorAction(id, Action::Edit(edit)).into(),
            ));
        }
        Key::Named(Named::Enter) if !shift && !kp.modifiers.alt() => {
            return Some(Binding::Custom(SqlMessage::NewlineAutoIndent(id).into()));
        }
        _ => {}
    }
    Binding::from_key_press(kp)
}

/// The floating completion list. Capped to a sane number of visible rows.
fn completion_list<'a>(id: u64, c: &'a CompletionState, max_h: f32) -> Element<'a, Message> {
    const MAX_VISIBLE: usize = 50;
    let mut col = column![].spacing(0);
    for (i, item) in c.items.iter().take(MAX_VISIBLE).enumerate() {
        let selected = i == c.selected;
        let label = theme::mono_sm(elide(&item.label, 28))
            .color(if selected {
                theme::palette::fg_primary()
            } else {
                theme::palette::fg_muted()
            })
            .wrapping(Wrapping::None);
        let detail = theme::mono_sm(elide(item.detail.as_deref().unwrap_or(""), 22))
            .size(10)
            .style(muted)
            .wrapping(Wrapping::None);
        let tag = theme::mono_sm(kind_tag(item.kind))
            .size(9)
            .style(muted)
            .wrapping(Wrapping::None);
        let content = row![label, Space::new().width(Length::Fill), detail, tag]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        let btn = button(content)
            .style(if selected {
                theme::accent_button
            } else {
                theme::ghost_button
            })
            .width(Length::Fill)
            .padding([2, 8])
            .on_press(SqlMessage::CompletionAccept(id, i).into());
        col = col.push(btn);
    }
    container(scrollable(col))
        .width(Length::Fixed(340.0))
        .max_height(max_h)
        .style(theme::surface_2)
        .into()
}

fn kind_tag(kind: CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Keyword => "kw",
        CompletionKind::Function => "fn",
        CompletionKind::Table => "table",
        CompletionKind::Column => "col",
    }
}

fn empty_state<'a>() -> Element<'a, Message> {
    container(
        column![
            theme::ui("No query open.").size(15),
            theme::mono_sm("Click \"+ New query\" to start, or connect to a FlightSQL server."),
        ]
        .spacing(8),
    )
    .padding(24)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

// -- History panel ------------------------------------------------------------

fn history_panel(app: &App) -> Element<'_, Message> {
    if app.sql.history_collapsed {
        return container(
            button(theme::label_text("◂ History"))
                .style(theme::ghost_button)
                .padding([6, 8])
                .on_press(SqlMessage::HistoryToggle.into()),
        )
        .padding(6)
        .height(Length::Fill)
        .style(sidebar_style)
        .into();
    }

    let header = row![
        theme::ui_medium("Query History").size(13),
        Space::new().width(Length::Fill),
        button(theme::label_text("Hide ▸"))
            .style(theme::ghost_button)
            .padding([4, 6])
            .on_press(SqlMessage::HistoryToggle.into()),
    ]
    .align_y(iced::Alignment::Center);

    let mut col = column![header].spacing(6);
    if app.sql.history.is_empty() {
        col = col.push(theme::mono_sm("Queries you run will appear here."));
    } else {
        for (i, entry) in app.sql.history.iter().enumerate().rev() {
            col = col.push(history_row(i, entry));
        }
    }

    container(scrollable(container(col).padding([0, 10])).height(Length::Fill))
        .width(Length::Fixed(320.0))
        .height(Length::Fill)
        .style(sidebar_style)
        .into()
}

fn history_row<'a>(i: usize, entry: &'a crate::app::QueryHistoryEntry) -> Element<'a, Message> {
    let dot_color = match entry.status {
        HistoryStatus::Ok => theme::palette::accent_cool(),
        HistoryStatus::Err(_) => theme::palette::accent_rose(),
    };
    let dot = text("●").size(9).color(dot_color);
    let preview = theme::mono_sm(elide_oneline(&entry.sql, 52)).wrapping(Wrapping::None);

    let rows_label = match (&entry.status, entry.row_count) {
        (HistoryStatus::Ok, Some(r)) => format!("{} rows", count(r)),
        (HistoryStatus::Err(e), _) => format!("error: {}", elide_oneline(e, 28)),
        _ => "—".to_string(),
    };
    let meta = theme::mono_sm(format!(
        "{} · {} · {} · {}",
        entry.source_label,
        rows_label,
        entry.elapsed,
        relative_time(entry.ran_at),
    ))
    .size(10)
    .style(muted)
    .wrapping(Wrapping::None);

    let actions = row![
        button(text("Load").size(10))
            .style(theme::ghost_button)
            .padding([2, 8])
            .on_press(SqlMessage::HistoryLoad(i).into()),
        button(text("Re-run").size(10))
            .style(theme::ghost_button)
            .padding([2, 8])
            .on_press(SqlMessage::HistoryRerun(i).into()),
    ]
    .spacing(4);

    container(
        column![
            row![dot, preview]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            meta,
            actions,
        ]
        .spacing(4),
    )
    .padding([6, 10])
    .width(Length::Fill)
    .style(history_row_style)
    .into()
}

// -- Connect modal ------------------------------------------------------------

pub fn connect_modal(form: &ConnectForm) -> Element<'_, Message> {
    let url = text_input("http://127.0.0.1:50051", &form.url)
        .on_input(|s| FlightMessage::ConnectFormUrl(s).into())
        .on_submit(FlightMessage::ConnectSubmit.into())
        .padding(8)
        .size(13);

    let auth_row = row![
        auth_button("None", AuthKind::None, form.auth_kind),
        auth_button("Bearer", AuthKind::Bearer, form.auth_kind),
        auth_button("Basic", AuthKind::Basic, form.auth_kind),
    ]
    .spacing(4);

    let auth_fields: Element<'_, Message> = match form.auth_kind {
        AuthKind::None => Space::new().width(Length::Fixed(0.0)).into(),
        AuthKind::Bearer => labelled(
            "Token",
            text_input("bearer token", &form.token)
                .on_input(|s| FlightMessage::ConnectFormToken(s).into())
                .secure(true)
                .padding(8)
                .size(13)
                .into(),
        ),
        AuthKind::Basic => column![
            labelled(
                "Username",
                text_input("username", &form.user)
                    .on_input(|s| FlightMessage::ConnectFormUser(s).into())
                    .padding(8)
                    .size(13)
                    .into(),
            ),
            labelled(
                "Password",
                text_input("password", &form.pass)
                    .on_input(|s| FlightMessage::ConnectFormPass(s).into())
                    .secure(true)
                    .padding(8)
                    .size(13)
                    .into(),
            ),
        ]
        .spacing(8)
        .into(),
    };

    let err: Element<'_, Message> = match &form.error {
        Some(e) => text(e.clone())
            .size(12)
            .color(theme::palette::accent_rose())
            .wrapping(Wrapping::Word)
            .into(),
        None => Space::new().width(Length::Fixed(0.0)).into(),
    };

    let mut connect = button(theme::ui_medium(if form.connecting {
        "Connecting…"
    } else {
        "Connect"
    }))
    .style(theme::accent_button)
    .padding([6, 16]);
    if !form.connecting {
        connect = connect.on_press(FlightMessage::ConnectSubmit.into());
    }
    let cancel = button(theme::ui("Cancel").size(13))
        .style(theme::ghost_button)
        .padding([6, 12])
        .on_press(FlightMessage::CloseConnectForm.into());

    let actions = row![
        Space::new().width(Length::Fill),
        cancel,
        Space::new().width(Length::Fixed(6.0)),
        connect,
    ]
    .align_y(iced::Alignment::Center);

    let panel = container(
        column![
            theme::display_strong("Connect to FlightSQL"),
            labelled("Server URL", url.into()),
            labelled("Authentication", auth_row.into()),
            auth_fields,
            err,
            actions,
        ]
        .spacing(12),
    )
    .padding(18)
    .width(Length::Fixed(460.0))
    .style(theme::surface_2);

    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::backdrop),
    )
    .on_press(FlightMessage::CloseConnectForm.into());

    let centered = container(opaque(panel))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    stack![backdrop, centered].into()
}

fn auth_button<'a>(label: &'a str, kind: AuthKind, current: AuthKind) -> Element<'a, Message> {
    let active = kind == current;
    let mut b = button(text(label).size(12)).padding([4, 12]);
    b = if active {
        b.style(theme::accent_button)
    } else {
        b.style(theme::ghost_button)
    };
    b.on_press(FlightMessage::ConnectFormAuthKind(kind).into())
        .into()
}

fn labelled<'a>(label: &'a str, child: Element<'a, Message>) -> Element<'a, Message> {
    row![theme::label_text(label), child]
        .spacing(6)
        .align_y(Vertical::Center)
        .into()
}

// -- helpers ------------------------------------------------------------------

fn muted(_: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme::palette::fg_muted()),
    }
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Collapse whitespace/newlines into single spaces, then elide.
fn elide_oneline(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    elide(&flat, max)
}

/// Coarse "time since" label for history entries (recomputed each render).
fn relative_time(t: std::time::SystemTime) -> String {
    match t.elapsed() {
        Ok(d) => {
            let secs = d.as_secs();
            if secs < 5 {
                "just now".to_string()
            } else if secs < 60 {
                format!("{secs}s ago")
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else {
                format!("{}h ago", secs / 3600)
            }
        }
        Err(_) => "—".to_string(),
    }
}

fn count(n: i64) -> String {
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

// -- styles -------------------------------------------------------------------

/// The text area itself is borderless; the frame around the scroll area
/// (gutter + text) draws the border so it does not scroll with the content.
fn editor_style(_theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        placeholder: palette::fg_dim(),
        value: palette::fg_primary(),
        selection: palette::accent_warm_soft(),
    }
}

fn editor_frame_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_deep())),
        border: Border {
            color: palette::border_subtle(),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn gutter_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface())),
        ..ContainerStyle::default()
    }
}

fn source_pill_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::accent_warm_soft())),
        text_color: Some(palette::accent_warm()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 3.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn error_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(Color {
            a: 0.10,
            ..palette::accent_rose()
        })),
        text_color: Some(palette::accent_rose()),
        border: Border {
            color: palette::accent_rose(),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn sidebar_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..ContainerStyle::default()
    }
}

fn history_row_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle {
        background: Some(Background::Color(palette::bg_surface_2())),
        text_color: Some(palette::fg_primary()),
        border: Border {
            color: palette::border_subtle(),
            width: 1.0,
            radius: 3.0.into(),
        },
        ..ContainerStyle::default()
    }
}
