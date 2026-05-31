//! Per-domain `update` handlers. [`App::update`] (in the parent module) is a thin
//! router that dispatches each [`Message`](super::Message) variant to one of
//! these `impl App` blocks, keeping each domain's message handling self-contained.

mod conn_info;
mod explorer;
mod file;
mod flight;
mod grid;
mod settings;
mod sql;
