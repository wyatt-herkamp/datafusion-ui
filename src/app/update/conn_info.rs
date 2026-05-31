use iced::Task;

use super::super::*;

impl App {
    pub(crate) fn update_conn_info(&mut self, m: ConnInfoMessage) -> Task<Message> {
        match m {
            ConnInfoMessage::Open(conn) => {
                if conn >= self.connections.len() {
                    return Task::none();
                }
                self.selection = Selection::ConnInfo { conn };
                // Kick a ping and, if not yet loaded, a metadata fetch.
                let mut tasks = vec![Task::done(ConnInfoMessage::Ping(conn).into())];
                let needs_info = self
                    .conn_info
                    .get(conn)
                    .is_some_and(|s| !s.info_loaded && !s.info_loading);
                if needs_info {
                    if let Some(state) = self.conn_info.get_mut(conn) {
                        state.info_loading = true;
                        state.info_error = None;
                    }
                    let client = self.connections[conn].clone();
                    tasks.push(Task::perform(client.server_info(), move |result| {
                        ConnInfoMessage::InfoResult { conn, result }.into()
                    }));
                }
                Task::batch(tasks)
            }
            ConnInfoMessage::Ping(conn) => {
                let Some(client) = self.connections.get(conn).cloned() else {
                    return Task::none();
                };
                if let Some(state) = self.conn_info.get_mut(conn) {
                    state.pinging = true;
                    state.ping_error = None;
                }
                Task::perform(client.ping(), move |result| {
                    ConnInfoMessage::PingResult { conn, result }.into()
                })
            }
            ConnInfoMessage::PingResult { conn, result } => {
                if let Some(state) = self.conn_info.get_mut(conn) {
                    state.pinging = false;
                    match result {
                        Ok(ms) => {
                            state.ping_ms = Some(ms);
                            state.ping_error = None;
                        }
                        Err(e) => state.ping_error = Some(e.to_string()),
                    }
                }
                Task::none()
            }
            ConnInfoMessage::InfoResult { conn, result } => {
                if let Some(state) = self.conn_info.get_mut(conn) {
                    state.info_loading = false;
                    state.info_loaded = true;
                    match result {
                        Ok(info) => state.info = info,
                        Err(e) => state.info_error = Some(e.to_string()),
                    }
                }
                Task::none()
            }
        }
    }
}
