use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::metadata::ParquetMetaData;

use crate::error::ParquetError;

#[derive(Debug, Clone)]
pub struct FileSummary {
    pub path: PathBuf,
    pub metadata: Arc<ParquetMetaData>,
    pub schema: SchemaRef,
    pub total_rows: i64,
    pub file_size_bytes: u64,
}

pub async fn load_metadata(path: PathBuf) -> Result<FileSummary, ParquetError> {
    tokio::task::spawn_blocking(move || load_metadata_blocking(path))
        .await
        .map_err(|e| ParquetError::Join(e.to_string()))?
}

fn load_metadata_blocking(path: PathBuf) -> Result<FileSummary, ParquetError> {
    let span = tracing::info_span!("load_metadata", path = %path.display());
    let _enter = span.enter();

    let file_size_bytes = std::fs::metadata(&path)
        .map_err(|e| ParquetError::Stat(e.to_string()))?
        .len();

    let file = File::open(&path).map_err(|e| ParquetError::Open(e.to_string()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| ParquetError::InvalidParquet(e.to_string()))?;

    let schema = builder.schema().clone();
    let metadata = builder.metadata().clone();
    let total_rows = metadata.file_metadata().num_rows();

    tracing::info!(
        rows = total_rows,
        row_groups = metadata.num_row_groups(),
        columns = schema.fields().len(),
        size_bytes = file_size_bytes,
        "loaded parquet metadata",
    );

    Ok(FileSummary {
        path,
        metadata,
        schema,
        total_rows,
        file_size_bytes,
    })
}
