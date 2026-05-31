use iced::Task;

use super::super::*;

impl App {
    pub(crate) fn update_flight(&mut self, m: FlightMessage) -> Task<Message> {
        match m {
            FlightMessage::OpenConnectForm => {
                self.connect_form = Some(ConnectForm {
                    url: "http://127.0.0.1:50051".to_string(),
                    ..ConnectForm::default()
                });
                Task::none()
            }
            FlightMessage::CloseConnectForm => {
                self.connect_form = None;
                Task::none()
            }
            FlightMessage::ConnectFormUrl(v) => {
                if let Some(f) = &mut self.connect_form {
                    f.url = v;
                }
                Task::none()
            }
            FlightMessage::ConnectFormAuthKind(k) => {
                if let Some(f) = &mut self.connect_form {
                    f.auth_kind = k;
                }
                Task::none()
            }
            FlightMessage::ConnectFormToken(v) => {
                if let Some(f) = &mut self.connect_form {
                    f.token = v;
                }
                Task::none()
            }
            FlightMessage::ConnectFormUser(v) => {
                if let Some(f) = &mut self.connect_form {
                    f.user = v;
                }
                Task::none()
            }
            FlightMessage::ConnectFormPass(v) => {
                if let Some(f) = &mut self.connect_form {
                    f.pass = v;
                }
                Task::none()
            }
            FlightMessage::ConnectSubmit => {
                let Some(form) = &mut self.connect_form else {
                    return Task::none();
                };
                let url = form.url.trim().to_string();
                if url.is_empty() {
                    form.error = Some("Enter a server URL".into());
                    return Task::none();
                }
                let auth = match form.auth_kind {
                    AuthKind::None => None,
                    AuthKind::Bearer => Some(FlightAuth::Bearer(form.token.trim().to_string())),
                    AuthKind::Basic => Some(FlightAuth::Basic {
                        user: form.user.clone(),
                        pass: form.pass.clone(),
                    }),
                };
                form.connecting = true;
                form.error = None;
                let cfg = FlightSqlConfig { url, auth };
                Task::perform(FlightSqlClient::connect(cfg), |r| {
                    FlightMessage::Connected(r).into()
                })
            }
            FlightMessage::Connected(Ok(client)) => {
                self.connect_form = None;
                let idx = self.connections.len();
                self.connections.push(client);
                self.conn_info.push(ConnInfoState::default());
                self.explorer.on_connect();
                self.selection = Selection::Sql;
                let conn = self.connections[idx].clone();
                self.push_editor(QueryEngine::Flight(conn), "SELECT 1".to_string());
                Task::none()
            }
            FlightMessage::Connected(Err(e)) => {
                tracing::error!(error = %e, "flightsql connect failed");
                if let Some(f) = &mut self.connect_form {
                    f.connecting = false;
                    f.error = Some(e.to_string());
                }
                Task::none()
            }
            FlightMessage::Disconnect(idx) => {
                if idx < self.connections.len() {
                    let removed = self.connections.remove(idx);
                    if idx < self.conn_info.len() {
                        self.conn_info.remove(idx);
                    }
                    self.explorer.on_disconnect(idx);
                    // Close editor tabs bound to this connection.
                    self.sql.editors.retain(|t| match &t.engine {
                        QueryEngine::Flight(c) => !Arc::ptr_eq(c, &removed),
                        QueryEngine::Local(_) => true,
                    });
                    if self.sql.active >= self.sql.editors.len() {
                        self.sql.active = self.sql.editors.len().saturating_sub(1);
                    }
                    self.ensure_valid_selection();
                }
                Task::none()
            }
        }
    }
}
