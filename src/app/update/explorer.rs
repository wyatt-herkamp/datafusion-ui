use iced::Task;

use super::super::*;

impl App {
    pub(crate) fn update_explorer(&mut self, m: ExplorerMessage) -> Task<Message> {
        match m {
            ExplorerMessage::PanelToggle => {
                self.explorer.collapsed = !self.explorer.collapsed;
                Task::none()
            }
            ExplorerMessage::Toggle(target) => match self.explorer.toggle(&target) {
                Some(req) => self.spawn_explorer_load(req),
                None => Task::none(),
            },
            ExplorerMessage::Loaded { target, load } => {
                self.explorer.apply(&target, load);
                Task::none()
            }
            ExplorerMessage::InsertTable { conn, qualified } => {
                // The table lives on a FlightSQL connection, so the query must run
                // against a tab bound to that connection — not whatever editor
                // (e.g. the local workspace) happens to be active. Switch to an
                // existing tab for this connection, or open one.
                let Some(client) = self.connections.get(conn).cloned() else {
                    return Task::none();
                };
                let existing = self.sql.editors.iter().position(
                    |t| matches!(&t.engine, QueryEngine::Flight(c) if Arc::ptr_eq(c, &client)),
                );
                match existing {
                    Some(i) => self.sql.active = i,
                    None => {
                        self.push_editor(QueryEngine::Flight(client), "SELECT 1".to_string());
                    }
                }
                if let Some(t) = self.sql.editors.get_mut(self.sql.active) {
                    let sql = format!("SELECT * FROM {qualified} LIMIT 100");
                    t.content = text_editor::Content::with_text(&sql);
                }
                self.selection = Selection::Sql;
                Task::none()
            }
        }
    }
}
