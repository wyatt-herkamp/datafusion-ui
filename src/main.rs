mod app;
mod config;
mod engine;
mod error;
mod explain;
mod explorer;
mod export;
mod flightsql;
mod format;
mod parquet_io;
mod sqlide_highlight;
mod store;
mod theme;
mod views;
mod widgets;
mod wrangle;

use app::App;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
pub struct Args {
    /// Sets the app directory, where config and persistent state are stored. Defaults to `.datafusion-ui` in the home directory.
    #[clap(long, env = "DATAFUSION_UI_APP_DIR")]
    pub app_dir: Option<std::path::PathBuf>,
    #[clap(subcommand)]
    pub subcommand: Option<SubCommand>,
}
#[derive(Debug, Clone, clap::Subcommand)]
pub enum SubCommand {
    /// Opens the app with a specified FlightSQL endpoint. Only use if no file argument is provided, and the endpoint supports FlightSQL.
    FlightSql { endpoint: String },
    /// Opens the app with a specified file.
    File {
        /// The file argument is optional to allow `datafusion-ui file` to open the file picker dialog on launch. If a path is provided, the app will attempt to open it directly.
        path: Option<std::path::PathBuf>,
    },
}
fn main() -> iced::Result {
    human_panic::setup_panic!();
    let args = Args::parse();
    // Default filter quiets DataFusion/arrow internals; override with RUST_LOG.
    let default_filter = "datafusion_ui=info,warn";
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    tracing::info!("starting datafusion-ui");

    let app_dir = match args.app_dir {
        Some(ok) => ok,
        None => match std::env::home_dir() {
            Some(ok) => ok.join(".datafusion-ui"),
            None => {
                eprintln!("Failed to determine home directory");
                std::process::exit(1);
            }
        },
    };
    if let Err(err) = std::fs::create_dir_all(&app_dir) {
        eprintln!("Failed to create app directory at {app_dir:?}: {err}");
        std::process::exit(1);
    }
    iced::application(
        move || App::boot(app_dir.clone(), args.subcommand.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme_for)
    .scale_factor(App::scale_factor)
    .font(theme::GEIST_REGULAR_BYTES)
    .font(theme::GEIST_MEDIUM_BYTES)
    .font(theme::GEIST_SEMIBOLD_BYTES)
    .font(theme::JETBRAINS_MONO_REGULAR_BYTES)
    .font(theme::JETBRAINS_MONO_MEDIUM_BYTES)
    .default_font(theme::FONT_UI)
    .run()
}
