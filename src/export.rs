//! Format-aware export of a query result stream. Unified across engines: it
//! consumes a `SendableRecordBatchStream` (local `execute_stream` or a Flight
//! re-fetch) and writes Parquet / CSV / JSON with the chosen settings.
//!
//! Parquet carries full compression control; CSV/JSON expose header / delimiter
//! / ndjson. (File-level gzip for CSV/JSON is a deliberate follow-up — it would
//! pull in a new compression dependency.)

use std::fs::File;
use std::path::PathBuf;

use datafusion::arrow::csv::WriterBuilder as CsvWriterBuilder;
use datafusion::arrow::json::{ArrayWriter, LineDelimitedWriter};
use datafusion::physical_plan::SendableRecordBatchStream;
use futures::StreamExt;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::error::ExportError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Parquet,
    Csv,
    Json,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] =
        [ExportFormat::Parquet, ExportFormat::Csv, ExportFormat::Json];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Parquet => "Parquet",
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Parquet => "parquet",
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParquetCompression {
    None,
    Snappy,
    Gzip,
    Zstd,
    Lz4,
}

impl ParquetCompression {
    pub const ALL: [ParquetCompression; 5] = [
        ParquetCompression::None,
        ParquetCompression::Snappy,
        ParquetCompression::Gzip,
        ParquetCompression::Zstd,
        ParquetCompression::Lz4,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ParquetCompression::None => "None",
            ParquetCompression::Snappy => "Snappy",
            ParquetCompression::Gzip => "Gzip",
            ParquetCompression::Zstd => "Zstd",
            ParquetCompression::Lz4 => "LZ4",
        }
    }

    fn to_parquet(self) -> Compression {
        match self {
            ParquetCompression::None => Compression::UNCOMPRESSED,
            ParquetCompression::Snappy => Compression::SNAPPY,
            ParquetCompression::Gzip => Compression::GZIP(Default::default()),
            ParquetCompression::Zstd => Compression::ZSTD(Default::default()),
            ParquetCompression::Lz4 => Compression::LZ4_RAW,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub parquet_compression: ParquetCompression,
    pub csv_header: bool,
    pub csv_delimiter: u8,
    /// JSON: newline-delimited (one object per line) vs a single JSON array.
    pub json_ndjson: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            format: ExportFormat::Parquet,
            parquet_compression: ParquetCompression::Zstd,
            csv_header: true,
            csv_delimiter: b',',
            json_ndjson: true,
        }
    }
}

/// Drain `stream` into `path` in the chosen format. Writers are synchronous and
/// driven incrementally as batches arrive, so memory stays bounded for the
/// local engine (Flight pre-buffers, see `run_sql_stream`).
pub async fn write_stream(
    mut stream: SendableRecordBatchStream,
    path: PathBuf,
    opts: ExportOptions,
) -> Result<PathBuf, ExportError> {
    let schema = stream.schema();
    let file = File::create(&path).map_err(|e| ExportError::CreateFile(e.to_string()))?;
    tracing::info!(dest = %path.display(), format = ?opts.format, "exporting query result");

    let write = |op: &'static str, e: &dyn std::fmt::Display| ExportError::Write {
        op,
        msg: e.to_string(),
    };

    match opts.format {
        ExportFormat::Parquet => {
            let props = WriterProperties::builder()
                .set_compression(opts.parquet_compression.to_parquet())
                .build();
            let mut writer = ArrowWriter::try_new(file, schema, Some(props))
                .map_err(|e| write("open parquet writer", &e))?;
            while let Some(batch) = stream.next().await {
                let batch = batch.map_err(|e| write("read batch", &e))?;
                writer
                    .write(&batch)
                    .map_err(|e| write("write parquet", &e))?;
            }
            writer.close().map_err(|e| write("finish parquet", &e))?;
        }
        ExportFormat::Csv => {
            let mut writer = CsvWriterBuilder::new()
                .with_header(opts.csv_header)
                .with_delimiter(opts.csv_delimiter)
                .build(file);
            while let Some(batch) = stream.next().await {
                let batch = batch.map_err(|e| write("read batch", &e))?;
                writer.write(&batch).map_err(|e| write("write csv", &e))?;
            }
        }
        ExportFormat::Json => {
            if opts.json_ndjson {
                let mut writer = LineDelimitedWriter::new(file);
                while let Some(batch) = stream.next().await {
                    let batch = batch.map_err(|e| write("read batch", &e))?;
                    writer.write(&batch).map_err(|e| write("write json", &e))?;
                }
                writer.finish().map_err(|e| write("finish json", &e))?;
            } else {
                let mut writer = ArrayWriter::new(file);
                while let Some(batch) = stream.next().await {
                    let batch = batch.map_err(|e| write("read batch", &e))?;
                    writer.write(&batch).map_err(|e| write("write json", &e))?;
                }
                writer.finish().map_err(|e| write("finish json", &e))?;
            }
        }
    }

    tracing::info!(dest = %path.display(), "export complete");
    Ok(path)
}
