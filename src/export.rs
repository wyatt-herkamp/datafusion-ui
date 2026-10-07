//! Format-aware export of a query result stream. Unified across engines: it
//! consumes a `SendableRecordBatchStream` (local `execute_stream` or a Flight
//! re-fetch) and writes Parquet / CSV / JSON with the chosen settings.
//!
//! Parquet carries full compression control; CSV/JSON expose header / delimiter
//! / ndjson. (File-level gzip for CSV/JSON is a deliberate follow-up — it would
//! pull in a new compression dependency.)

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

use datafusion::arrow::csv::WriterBuilder as CsvWriterBuilder;
use datafusion::arrow::json::{ArrayWriter, LineDelimitedWriter};
use datafusion::physical_plan::SendableRecordBatchStream;
use futures::StreamExt;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::{WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

use crate::error::ExportError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Parquet,
    Csv,
    Json,
}

impl ExportFormat {
    pub const ALL: [Self; 3] = [Self::Parquet, Self::Csv, Self::Json];

    pub fn label(self) -> &'static str {
        match self {
            Self::Parquet => "Parquet",
            Self::Csv => "CSV",
            Self::Json => "JSON",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Parquet => "parquet",
            Self::Csv => "csv",
            Self::Json => "json",
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
impl std::fmt::Display for ParquetCompression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
impl ParquetCompression {
    pub const ALL: [Self; 5] = [Self::None, Self::Snappy, Self::Gzip, Self::Zstd, Self::Lz4];

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Snappy => "Snappy",
            Self::Gzip => "Gzip",
            Self::Zstd => "Zstd",
            Self::Lz4 => "LZ4",
        }
    }

    fn to_parquet(self) -> Compression {
        match self {
            Self::None => Compression::UNCOMPRESSED,
            Self::Snappy => Compression::SNAPPY,
            Self::Gzip => Compression::GZIP(Default::default()),
            Self::Zstd => Compression::ZSTD(Default::default()),
            Self::Lz4 => Compression::LZ4_RAW,
        }
    }
}

#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParquetEncoding {
    PLAIN = 0,
    PLAIN_DICTIONARY = 2,
    RLE = 3,
    #[deprecated]
    BIT_PACKED = 4,
    DELTA_BINARY_PACKED = 5,
    DELTA_LENGTH_BYTE_ARRAY = 6,
    DELTA_BYTE_ARRAY = 7,
    RLE_DICTIONARY = 8,
    BYTE_STREAM_SPLIT = 9,
}
impl std::fmt::Display for ParquetEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
#[allow(deprecated)]
impl ParquetEncoding {
    pub const ALL: [Self; 9] = [
        Self::PLAIN,
        Self::PLAIN_DICTIONARY,
        Self::RLE,
        Self::BIT_PACKED,
        Self::DELTA_BINARY_PACKED,
        Self::DELTA_LENGTH_BYTE_ARRAY,
        Self::DELTA_BYTE_ARRAY,
        Self::RLE_DICTIONARY,
        Self::BYTE_STREAM_SPLIT,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::PLAIN => "Plain",
            Self::PLAIN_DICTIONARY => "PlainDictionary",
            Self::RLE => "Rle",
            Self::BIT_PACKED => "BitPacked",
            Self::DELTA_BINARY_PACKED => "DeltaBinaryPacked",
            Self::DELTA_LENGTH_BYTE_ARRAY => "DeltaLengthByteArray",
            Self::DELTA_BYTE_ARRAY => "DeltaByteArray",
            Self::RLE_DICTIONARY => "RleDictionary",
            Self::BYTE_STREAM_SPLIT => "ByteStreamSplit",
        }
    }

    fn to_parquet(self) -> Encoding {
        match self {
            Self::PLAIN => Encoding::PLAIN,
            Self::PLAIN_DICTIONARY => Encoding::PLAIN_DICTIONARY,
            Self::RLE => Encoding::RLE,
            Self::BIT_PACKED => Encoding::BIT_PACKED,
            Self::DELTA_BINARY_PACKED => Encoding::DELTA_BINARY_PACKED,
            Self::DELTA_LENGTH_BYTE_ARRAY => Encoding::DELTA_LENGTH_BYTE_ARRAY,
            Self::DELTA_BYTE_ARRAY => Encoding::DELTA_BYTE_ARRAY,
            Self::RLE_DICTIONARY => Encoding::RLE_DICTIONARY,
            Self::BYTE_STREAM_SPLIT => Encoding::BYTE_STREAM_SPLIT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParquetVersion {
    V1_0,
    V2_0,
}
impl std::fmt::Display for ParquetVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl ParquetVersion {
    pub const ALL: [Self; 2] = [Self::V1_0, Self::V2_0];

    fn to_parquet(self) -> WriterVersion {
        match self {
            Self::V1_0 => WriterVersion::PARQUET_1_0,
            Self::V2_0 => WriterVersion::PARQUET_2_0,
        }
    }
}

pub trait ExportOptions {
    fn format() -> ExportFormat;

    /// Drain `stream` into `path` in the chosen format. Writers are synchronous and
    /// driven incrementally as batches arrive, so memory stays bounded for the
    /// local engine (Flight pre-buffers, see `run_sql_stream`).
    async fn write_stream(
        &self,
        stream: SendableRecordBatchStream,
        path: PathBuf,
    ) -> Result<PathBuf, ExportError>;
}

#[derive(Debug, Clone)]
pub struct ParquetOptions {
    pub compression: ParquetCompression,
    pub encoding: Option<parquet::basic::Encoding>,
    pub dictionary: Option<bool>,
    pub version: ParquetVersion,
    pub per_column_options: HashMap<String, ParquetColumnOptions>,
}
impl Default for ParquetOptions {
    fn default() -> Self {
        Self {
            compression: ParquetCompression::Zstd,
            encoding: None,
            dictionary: None,
            version: ParquetVersion::V2_0,
            per_column_options: HashMap::new(),
        }
    }
}
impl ExportOptions for ParquetOptions {
    fn format() -> ExportFormat {
        ExportFormat::Parquet
    }

    async fn write_stream(
        &self,
        mut stream: SendableRecordBatchStream,
        path: PathBuf,
    ) -> Result<PathBuf, ExportError> {
        let schema = stream.schema();
        let file = File::create(&path).map_err(|e| ExportError::CreateFile(e.to_string()))?;
        tracing::info!(dest = %path.display(), format = ?Self::format(), "exporting query result");

        let write = |op: &'static str, e: &dyn std::fmt::Display| ExportError::Write {
            op,
            msg: e.to_string(),
        };

        let mut props = WriterProperties::builder()
            .set_compression(self.compression.to_parquet())
            .set_writer_version(self.version.to_parquet());
        if let Some(dictionary) = self.dictionary {
            props = props.set_dictionary_enabled(dictionary);
        }
        if let Some(encoding) = self.encoding {
            props = props.set_encoding(encoding);
        }

        for (col, co) in self.per_column_options.iter() {
            let col = ColumnPath::from(col.clone());
            if let Some(value) = co.compression {
                props = props.set_column_compression(col.clone(), value.to_parquet());
            }
            if let Some(value) = co.dictionary {
                props = props.set_column_dictionary_enabled(col.clone(), value);
            }
            if let Some(value) = co.encoding {
                props = props.set_column_encoding(col.clone(), value.to_parquet());
            }
        }

        let mut writer = ArrowWriter::try_new(file, schema, Some(props.build()))
            .map_err(|e| write("open parquet writer", &e))?;

        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|e| write("read batch", &e))?;
            writer
                .write(&batch)
                .map_err(|e| write("write parquet", &e))?;
        }
        writer.close().map_err(|e| write("finish parquet", &e))?;

        tracing::info!(dest = %path.display(), "export complete");
        Ok(path)
    }
}

#[derive(Debug, Default, Clone)]
pub struct ParquetColumnOptions {
    pub compression: Option<ParquetCompression>,
    pub encoding: Option<ParquetEncoding>,
    pub dictionary: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct CsvOptions {
    pub header: bool,
    pub delimiter: u8,
}
impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            header: true,
            delimiter: b',',
        }
    }
}
impl ExportOptions for CsvOptions {
    fn format() -> ExportFormat {
        ExportFormat::Csv
    }

    async fn write_stream(
        &self,
        mut stream: SendableRecordBatchStream,
        path: PathBuf,
    ) -> Result<PathBuf, ExportError> {
        let file = File::create(&path).map_err(|e| ExportError::CreateFile(e.to_string()))?;
        tracing::info!(dest = %path.display(), format = ?Self::format(), "exporting query result");

        let write = |op: &'static str, e: &dyn std::fmt::Display| ExportError::Write {
            op,
            msg: e.to_string(),
        };

        let mut writer = CsvWriterBuilder::new()
            .with_header(self.header)
            .with_delimiter(self.delimiter)
            .build(file);

        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|e| write("read batch", &e))?;
            writer.write(&batch).map_err(|e| write("write csv", &e))?;
        }

        tracing::info!(dest = %path.display(), "export complete");
        Ok(path)
    }
}

#[derive(Debug, Clone)]
pub struct JsonOptions {
    /// JSON: newline-delimited (one object per line) vs a single JSON array.
    pub ndjson: bool,
}
impl Default for JsonOptions {
    fn default() -> Self {
        Self { ndjson: true }
    }
}
impl ExportOptions for JsonOptions {
    fn format() -> ExportFormat {
        ExportFormat::Json
    }

    async fn write_stream(
        &self,
        mut stream: SendableRecordBatchStream,
        path: PathBuf,
    ) -> Result<PathBuf, ExportError> {
        let file = File::create(&path).map_err(|e| ExportError::CreateFile(e.to_string()))?;
        tracing::info!(dest = %path.display(), format = ?Self::format(), "exporting query result");

        let write = |op: &'static str, e: &dyn std::fmt::Display| ExportError::Write {
            op,
            msg: e.to_string(),
        };

        if self.ndjson {
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

        tracing::info!(dest = %path.display(), "export complete");
        Ok(path)
    }
}
