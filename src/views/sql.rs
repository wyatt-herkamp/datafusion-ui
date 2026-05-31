//! The SQL workspace view: a strip of editor tabs (each bound to its own
//! source), the active editor + results grid, a shared in-memory query-history
//! panel, and the FlightSQL connect modal. Styling reuses `crate::theme`.

use iced::keyboard::Key;
use iced::keyboard::key::Named;
use iced::widget::container::Style as ContainerStyle;
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{
    Space, button, column, container, mouse_area, opaque, pin, row, scrollable, stack, text,
    text_editor, text_input,
};
use iced::{Background, Border, Color, Element, Length, Theme};
use sql_ide::CompletionKind;

use crate::app::{
    App, AuthKind, CompletionState, ConnectForm, ExportDialogState, FlightMessage, HistoryStatus,
    Message, RESULT_PAGE_SIZE, SourceRef, SqlEditorTab, SqlMessage,
};
use crate::export::{ExportFormat, ParquetCompression};
use crate::sqlide_highlight::SqlHighlighter;
use crate::theme;
use crate::theme::palette;

pub fn view(app: &App) -> Element<'_, Message> {
    let strip = editor_tab_strip(app);

    let active = app.sql.editors.get(app.sql.active);
    let body: Element<'_, Message> = match active {
        Some(tab) => editor_pane(tab),
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
                SqlMessage::CloseCellDetail { id }.into()
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

fn editor_pane(tab: &SqlEditorTab) -> Element<'_, Message> {
    let id = tab.id;
    let popup_open = tab.completion.as_ref().is_some_and(|c| !c.items.is_empty());

    let editor = text_editor(&tab.content)
        .on_action(move |a| SqlMessage::EditorAction(id, a).into())
        .key_binding(move |kp| completion_key_binding(id, popup_open, kp))
        .highlight_with::<SqlHighlighter>((), crate::sqlide_highlight::to_format)
        .font(theme::FONT_MONO)
        .size(13)
        // No wrapping keeps (line, column) a faithful grid for popup placement.
        .wrapping(Wrapping::None)
        .height(Length::Fixed(180.0))
        .padding(10)
        .style(editor_style);

    // Float the completion popup over the editor near the cursor. iced exposes
    // no pixel caret, so approximate from the (line, column) text position and
    // monospace metrics (CHAR_W is calibrated for JetBrains Mono at size 13).
    //
    // The editor is ALWAYS the first child of a `stack`, whether or not the
    // popup is shown. Changing the surrounding widget type (e.g. container vs
    // stack) would shift the editor's position in the tree and make iced rebuild
    // its state — dropping keyboard focus, which silently blocks typing.
    let editor_framed = container(editor)
        .width(Length::Fill)
        .style(editor_frame_style);
    let mut layers = stack![editor_framed];
    if let Some(c) = &tab.completion
        && !c.items.is_empty()
    {
        const PAD: f32 = 10.0;
        const LINE_H: f32 = 13.0 * 1.3;
        const CHAR_W: f32 = 7.8;
        let pos = tab.content.cursor().position;
        let x = PAD + pos.column as f32 * CHAR_W;
        let y = PAD + (pos.line as f32 + 1.0) * LINE_H;
        // `opaque` must wrap the popup box itself, NOT the `pin`: `pin` defaults
        // to Length::Fill, so `opaque(pin(..))` would mark the whole editor area
        // as click-capturing and swallow every click before it reaches the
        // editor. Wrapping the list keeps capture scoped to the popup's bounds.
        layers = layers.push(pin(opaque(completion_list(id, c))).x(x).y(y));
    }
    let editor_layer: Element<'_, Message> = layers.into();

    let mut run = button(theme::ui_medium("Run  ▷").size(12))
        .style(theme::accent_button)
        .padding([6, 16]);
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
        run = run.on_press(SqlMessage::Run(id).into());
        explain_btn = explain_btn.on_press(SqlMessage::Explain(id).into());
        explain_an_btn = explain_an_btn.on_press(SqlMessage::ExplainAnalyze(id).into());
        export_btn = export_btn.on_press(SqlMessage::ExportOpen(id).into());
    }

    let source_pill = container(theme::mono_sm(elide(&tab.title, 32)).wrapping(Wrapping::None))
        .padding([2, 8])
        .style(source_pill_style);

    let meta: Element<'_, Message> = if tab.running {
        theme::mono_sm("running…").wrapping(Wrapping::None).into()
    } else if let (Some(ms), Some(rows)) = (tab.last_elapsed_ms, tab.last_row_count) {
        theme::mono_sm(format!("{} rows · {} ms", count(rows as i64), ms))
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
                count(tab.last_row_count.unwrap_or(0) as i64)
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
                results_grid(b, id, &tab.insights, &tab.col_widths, tab.page),
            ]
            .spacing(6)
            .height(Length::Fill)
            .into(),
            // Ordinary result set.
            None => results_grid(b, id, &tab.insights, &tab.col_widths, tab.page),
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

    // First syntax diagnostic (if any), shown only when no run-error is up.
    let diagnostic: Element<'_, Message> = match (tab.error.is_some(), tab.diagnostics.first()) {
        (false, Some(d)) => container(
            text(format!("⚠ {}", elide_oneline(&d.message, 80)))
                .size(11)
                .color(theme::palette::accent_warm())
                .wrapping(Wrapping::Word),
        )
        .padding([2, 4])
        .width(Length::Fill)
        .into(),
        _ => Space::new().width(Length::Fixed(0.0)).into(),
    };

    let base: Element<'_, Message> = column![editor_layer, toolbar, diagnostic, error, results]
        .spacing(8)
        .height(Length::Fill)
        .into();

    match tab.export_dialog.as_ref() {
        Some(dialog) => stack![base, export_overlay(id, dialog)].into(),
        None => base,
    }
}

/// Modal: pick an export format and its settings, then write the full
/// (uncapped) query result to a file.
fn export_overlay<'a>(id: u64, dialog: &'a ExportDialogState) -> Element<'a, Message> {
    let opts = &dialog.options;

    // Format selector.
    let mut formats = row![].spacing(6);
    for f in ExportFormat::ALL {
        formats = formats.push(
            button(theme::ui_medium(f.label()).size(12))
                .style(theme::tab_button(f == opts.format))
                .padding([4, 12])
                .on_press(SqlMessage::ExportSetFormat(id, f).into()),
        );
    }

    // Format-specific settings.
    let settings: Element<'a, Message> = match opts.format {
        ExportFormat::Parquet => {
            let mut comps = row![].spacing(6);
            for c in ParquetCompression::ALL {
                comps = comps.push(
                    button(theme::ui_medium(c.label()).size(12))
                        .style(theme::tab_button(c == opts.parquet_compression))
                        .padding([4, 10])
                        .on_press(SqlMessage::ExportSetCompression(id, c).into()),
                );
            }
            labelled("Compression", comps.into())
        }
        ExportFormat::Csv => {
            let header =
                button(theme::ui_medium(if opts.csv_header { "On" } else { "Off" }).size(12))
                    .style(theme::tab_button(opts.csv_header))
                    .padding([4, 12])
                    .on_press(SqlMessage::ExportToggleHeader(id).into());
            let delim = text_input("", &(opts.csv_delimiter as char).to_string())
                .on_input(move |s| SqlMessage::ExportDelimiter(id, s).into())
                .padding([6, 8])
                .width(Length::Fixed(60.0));
            column![
                labelled("Header row", header.into()),
                labelled("Delimiter", delim.into()),
            ]
            .spacing(12)
            .into()
        }
        ExportFormat::Json => {
            let ndjson = button(
                theme::ui_medium(if opts.json_ndjson {
                    "NDJSON (one object per line)"
                } else {
                    "JSON array"
                })
                .size(12),
            )
            .style(theme::tab_button(true))
            .padding([4, 12])
            .on_press(SqlMessage::ExportToggleNdjson(id).into());
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

/// Key bindings active while the completion popup is open: arrows move the
/// selection, Tab/Enter accept, Esc dismisses. Ctrl/Cmd+Z and Ctrl/Cmd+Shift+Z
/// (or Ctrl/Cmd+Y) drive our own undo/redo, since iced's `text_editor` has none.
/// All other keys (and every key when the popup is closed) fall through to the
/// editor's default behavior.
fn completion_key_binding(id: u64, open: bool, kp: KeyPress) -> Option<Binding<Message>> {
    // Undo/redo work regardless of whether the completion popup is open.
    if kp.modifiers.command()
        && let Key::Character(c) = &kp.key
    {
        match c.as_str() {
            "z" if kp.modifiers.shift() => {
                return Some(Binding::Custom(SqlMessage::Redo(id).into()));
            }
            "z" => return Some(Binding::Custom(SqlMessage::Undo(id).into())),
            "y" => return Some(Binding::Custom(SqlMessage::Redo(id).into())),
            _ => {}
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
    Binding::from_key_press(kp)
}

/// The floating completion list. Capped to a sane number of visible rows.
fn completion_list<'a>(id: u64, c: &'a CompletionState) -> Element<'a, Message> {
    const MAX_VISIBLE: usize = 50;
    let mut col = column![].spacing(0);
    for (i, item) in c.items.iter().take(MAX_VISIBLE).enumerate() {
        let selected = i == c.selected;
        let label = theme::mono_sm(elide(&item.label, 26))
            .color(if selected {
                theme::palette::fg_primary()
            } else {
                theme::palette::fg_muted()
            })
            .wrapping(Wrapping::None);
        let tag = theme::mono_sm(kind_tag(item.kind))
            .size(9)
            .style(muted)
            .wrapping(Wrapping::None);
        let content = row![label, Space::new().width(Length::Fill), tag]
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
        .width(Length::Fixed(260.0))
        .max_height(200.0)
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
        (HistoryStatus::Ok, Some(r)) => format!("{} rows", count(r as i64)),
        (HistoryStatus::Err(e), _) => format!("error: {}", elide_oneline(e, 28)),
        _ => "—".to_string(),
    };
    let meta = theme::mono_sm(format!(
        "{} · {} · {} ms · {}",
        entry.source_label,
        rows_label,
        entry.elapsed_ms,
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
    column![theme::label_text(label), child].spacing(4).into()
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

fn editor_style(_theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(palette::bg_deep()),
        border: Border {
            color: palette::border_subtle(),
            width: 1.0,
            radius: 3.0.into(),
        },
        placeholder: palette::fg_dim(),
        value: palette::fg_primary(),
        selection: palette::accent_warm_soft(),
    }
}

fn editor_frame_style(_theme: &Theme) -> ContainerStyle {
    ContainerStyle::default()
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
