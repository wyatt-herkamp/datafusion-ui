//! Persisted user settings, loaded once at boot and saved on change.
//!
//! Stored as TOML at the OS config dir (e.g. `~/.config/datafusion-ui/config.toml`
//! on Linux). Every field carries a `#[serde(default)]` so a partial or
//! older-version file still deserializes — missing keys fall back to the
//! `Default` impls below. Loading is best-effort: a missing or invalid file logs
//! a warning and yields `Config::default()`.

use std::sync::Arc;

use std::path::{Path, PathBuf};

use datafusion::execution::config::SessionConfig;
use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
use datafusion::execution::memory_pool::{
    FairSpillPool, GreedyMemoryPool, MemoryPool, UnboundedMemoryPool,
};
use datafusion::execution::runtime_env::{RuntimeEnv, RuntimeEnvBuilder};
use serde::{Deserialize, Serialize};
/// Bounds for the UI scale factor (see [`Appearance::ui_scale`]).
pub const UI_SCALE_MIN: f32 = 0.7;
pub const UI_SCALE_MAX: f32 = 1.6;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub appearance: Appearance,
    pub session: SessionSettings,
    pub runtime: RuntimeSettings,
    /// UI fetch limit: the most rows a query pulls into the results grid.
    pub result_row_cap: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            appearance: Appearance::default(),
            session: SessionSettings::default(),
            runtime: RuntimeSettings::default(),
            result_row_cap: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Whole-UI zoom applied via Iced's `scale_factor` (text, padding, chrome).
    pub ui_scale: f32,
    /// Which color theme to use. `System` follows the OS light/dark preference
    /// detected at startup (see [`detect_system_dark`]).
    pub theme: ThemeChoice,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            ui_scale: 1.0,
            theme: ThemeChoice::default(),
        }
    }
}

/// User-selectable color theme. Persisted in `config.toml` as kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    /// Follow the OS light/dark preference (resolved once at startup).
    #[default]
    System,
    /// Signature warm dark theme.
    Instrument,
    /// Light theme (keeps the app's accent identity).
    Light,
    /// One Dark (Mark Skelton / Atom One Dark).
    OneDark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 4] = [
        ThemeChoice::System,
        ThemeChoice::Instrument,
        ThemeChoice::Light,
        ThemeChoice::OneDark,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Instrument => "Instrument",
            ThemeChoice::Light => "Light",
            ThemeChoice::OneDark => "One Dark",
        }
    }
}

/// Read the OS light/dark preference. Returns `true` for dark; an unknown or
/// undetectable preference falls back to dark (the app's signature default).
pub fn detect_system_dark() -> bool {
    !matches!(dark_light::detect(), Ok(dark_light::Mode::Light))
}

/// Knobs mapped onto the local DataFusion [`SessionConfig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    // -- Performance --
    /// Pinned to 1 by default so `SELECT *` over a sorted file returns rows in
    /// storage order. Raising this parallelizes scans but interleaves output.
    pub target_partitions: usize,
    pub batch_size: usize,
    pub repartition_file_scans: bool,
    pub repartition_joins: bool,
    pub repartition_aggregations: bool,
    pub repartition_sorts: bool,
    // -- Behavior --
    pub information_schema: bool,
    pub collect_statistics: bool,
    pub default_catalog: String,
    pub default_schema: String,
}

impl Default for SessionSettings {
    fn default() -> Self {
        SessionSettings {
            target_partitions: 1,
            batch_size: 8192,
            repartition_file_scans: false,
            repartition_joins: true,
            repartition_aggregations: true,
            repartition_sorts: true,
            information_schema: true,
            collect_statistics: false,
            default_catalog: "datafusion".to_string(),
            default_schema: "public".to_string(),
        }
    }
}

impl SessionSettings {
    /// Build a [`SessionConfig`] from these settings. Mirrors the defaults the
    /// old per-file session used (single partition, no file-scan repartition).
    pub fn to_session_config(&self) -> SessionConfig {
        SessionConfig::new()
            .with_target_partitions(self.target_partitions.max(1))
            .with_batch_size(self.batch_size.max(1))
            .with_repartition_file_scans(self.repartition_file_scans)
            .with_repartition_joins(self.repartition_joins)
            .with_repartition_aggregations(self.repartition_aggregations)
            .with_repartition_sorts(self.repartition_sorts)
            .with_information_schema(self.information_schema)
            .with_collect_statistics(self.collect_statistics)
            .with_default_catalog_and_schema(
                self.default_catalog.clone(),
                self.default_schema.clone(),
            )
    }
}

/// Which memory pool the DataFusion runtime uses to bound query memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MemoryPoolKind {
    /// No limit — DataFusion's default.
    #[default]
    Unbounded,
    /// Fixed limit, first-come-first-served across consumers.
    Greedy,
    /// Fixed limit, shared fairly across spilling consumers.
    FairSpill,
}

impl MemoryPoolKind {
    pub const ALL: [MemoryPoolKind; 3] = [
        MemoryPoolKind::Unbounded,
        MemoryPoolKind::Greedy,
        MemoryPoolKind::FairSpill,
    ];
    pub fn label(self) -> &'static str {
        match self {
            MemoryPoolKind::Unbounded => "Unbounded",
            MemoryPoolKind::Greedy => "Greedy",
            MemoryPoolKind::FairSpill => "Fair-spill",
        }
    }
}

/// How the DataFusion runtime spills intermediate data to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiskManagerKind {
    /// Spill to the OS temp directory — DataFusion's default.
    #[default]
    Os,
    /// Spill to a user-specified directory.
    Specified,
    /// No spilling to disk (memory-only; large spills will error).
    Disabled,
}

impl DiskManagerKind {
    pub const ALL: [DiskManagerKind; 3] = [
        DiskManagerKind::Os,
        DiskManagerKind::Specified,
        DiskManagerKind::Disabled,
    ];
    pub fn label(self) -> &'static str {
        match self {
            DiskManagerKind::Os => "OS temp dir",
            DiskManagerKind::Specified => "Custom dir",
            DiskManagerKind::Disabled => "Disabled",
        }
    }
}

/// Settings for the DataFusion [`RuntimeEnv`] (memory pool + disk manager).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSettings {
    pub memory_pool: MemoryPoolKind,
    /// Memory limit in MiB, applied when `memory_pool` is Greedy or Fair-spill.
    pub memory_limit_mb: usize,
    pub disk_manager: DiskManagerKind,
    /// Spill directory, used when `disk_manager` is `Specified`.
    pub disk_manager_path: String,
    /// Cap on the spill directory size in MiB; `0` keeps DataFusion's default.
    pub max_temp_dir_size_mb: usize,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        RuntimeSettings {
            memory_pool: MemoryPoolKind::Unbounded,
            memory_limit_mb: 4096,
            disk_manager: DiskManagerKind::Os,
            disk_manager_path: String::new(),
            max_temp_dir_size_mb: 0,
        }
    }
}

impl RuntimeSettings {
    /// Build a [`RuntimeEnv`] from these settings, falling back to the default
    /// runtime if construction fails (e.g. an unusable spill directory).
    pub fn to_runtime_env(&self) -> Arc<RuntimeEnv> {
        let bytes = self.memory_limit_mb.saturating_mul(1024 * 1024).max(1);
        let pool: Arc<dyn MemoryPool> = match self.memory_pool {
            MemoryPoolKind::Unbounded => Arc::new(UnboundedMemoryPool::default()),
            MemoryPoolKind::Greedy => Arc::new(GreedyMemoryPool::new(bytes)),
            MemoryPoolKind::FairSpill => Arc::new(FairSpillPool::new(bytes)),
        };

        let mode = match self.disk_manager {
            DiskManagerKind::Os => DiskManagerMode::OsTmpDirectory,
            DiskManagerKind::Disabled => DiskManagerMode::Disabled,
            DiskManagerKind::Specified => {
                DiskManagerMode::Directories(vec![PathBuf::from(&self.disk_manager_path)])
            }
        };
        let mut disk = DiskManagerBuilder::default().with_mode(mode);
        if self.max_temp_dir_size_mb > 0 {
            disk =
                disk.with_max_temp_directory_size((self.max_temp_dir_size_mb as u64) * 1024 * 1024);
        }

        let builder = RuntimeEnvBuilder::new()
            .with_memory_pool(pool)
            .with_disk_manager_builder(disk);
        builder.build_arc().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "invalid runtime settings; using default runtime");
            RuntimeEnvBuilder::new()
                .build_arc()
                .expect("default runtime env builds")
        })
    }
}

impl Config {
    /// Load from disk, falling back to defaults on any error (missing file,
    /// parse failure, no config dir).
    pub fn load(app_dir: impl AsRef<Path>) -> Self {
        let path = app_dir.as_ref().join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => {
                    tracing::info!(path = %path.display(), "loaded config");
                    cfg.sanitized()
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "invalid config; using defaults");
                    Config::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "could not read config; using defaults");
                Config::default()
            }
        }
    }

    /// Persist to disk (best-effort), creating the config dir if needed.
    pub fn save(&self, app_dir: &Path) {
        let path = app_dir.join("config.toml");
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(error = %e, "could not create config dir");
            return;
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::warn!(error = %e, path = %path.display(), "could not write config");
                } else {
                    tracing::info!(path = %path.display(), "saved config");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize config"),
        }
    }

    /// Clamp/repair values that must stay in range regardless of what was on disk.
    pub fn sanitized(mut self) -> Self {
        self.appearance.ui_scale = self.appearance.ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX);
        if self.result_row_cap == 0 {
            self.result_row_cap = 10_000;
        }
        self
    }
}
