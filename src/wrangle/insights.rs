//! Per-column statistics computed directly from an in-memory [`RecordBatch`].
//!
//! These power the stats row above the SQL results grid: distinct counts,
//! null share, min/max, a numeric histogram, and top values for low-cardinality
//! columns. Computing straight from the materialized result batch (rather than
//! issuing SQL) means it works identically for local-file and FlightSQL results.

use std::collections::HashMap;

use arrow::array::{Array, Float64Array};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Numeric,
    Boolean,
    Temporal,
    String,
    Other,
}

pub fn classify(dt: &DataType) -> ColumnKind {
    match dt {
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Float16
        | DataType::Float32
        | DataType::Float64
        | DataType::Decimal128(_, _)
        | DataType::Decimal256(_, _) => ColumnKind::Numeric,
        DataType::Boolean => ColumnKind::Boolean,
        DataType::Date32
        | DataType::Date64
        | DataType::Time32(_)
        | DataType::Time64(_)
        | DataType::Timestamp(_, _) => ColumnKind::Temporal,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => ColumnKind::String,
        _ => ColumnKind::Other,
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // name/kind read by the grid stats row.
pub struct ColumnInsight {
    pub name: String,
    pub kind: ColumnKind,
    pub total: u64,
    pub null_count: u64,
    pub distinct: Option<u64>,
    pub min: Option<String>,
    pub max: Option<String>,
    pub histogram: Option<Histogram>,
    pub top_values: Option<Vec<(String, u64)>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // min/max read for tooltip in later UI iteration
pub struct Histogram {
    pub min: f64,
    pub max: f64,
    pub bin_counts: Vec<u64>,
}

const BUCKETS: usize = 10;
const TOP_K: usize = 3;

/// Compute per-column stats for every column of a result batch.
pub fn compute_from_batch(batch: &RecordBatch) -> Vec<ColumnInsight> {
    let schema = batch.schema();
    schema
        .fields()
        .iter()
        .enumerate()
        .map(|(i, field)| compute_column(field.name(), field.data_type(), batch.column(i)))
        .collect()
}

fn compute_column(name: &str, dt: &DataType, array: &dyn Array) -> ColumnInsight {
    let kind = classify(dt);
    let total = array.len() as u64;
    let null_count = array.null_count() as u64;

    let base = ColumnInsight {
        name: name.to_string(),
        kind,
        total,
        null_count,
        distinct: None,
        min: None,
        max: None,
        histogram: None,
        top_values: None,
    };

    // Nested / binary types can't be meaningfully stringified into stats here.
    if is_unaggregatable(dt) {
        return base;
    }

    // Stringify each value once; reused for distinct, min/max, and top values.
    let formatter = match ArrayFormatter::try_new(array, &FormatOptions::default()) {
        Ok(f) => f,
        Err(_) => return base,
    };
    let mut values: Vec<Option<String>> = Vec::with_capacity(array.len());
    for r in 0..array.len() {
        if array.is_null(r) {
            values.push(None);
        } else {
            values.push(formatter.value(r).try_to_string().ok());
        }
    }

    let mut distinct_set: HashMap<&str, u64> = HashMap::new();
    for v in values.iter().flatten() {
        *distinct_set.entry(v.as_str()).or_insert(0) += 1;
    }
    let distinct = Some(distinct_set.len() as u64);

    let (min, max, histogram) = if kind == ColumnKind::Numeric {
        numeric_min_max_histogram(array, &values)
    } else {
        (lexical_min(&values), lexical_max(&values), None)
    };

    // Top values for low-cardinality categorical-ish columns.
    let top_values = if matches!(
        kind,
        ColumnKind::String | ColumnKind::Boolean | ColumnKind::Temporal
    ) {
        Some(top_values(&distinct_set))
    } else {
        None
    };

    ColumnInsight {
        distinct,
        min,
        max,
        histogram,
        top_values,
        ..base
    }
}

/// Nested / unsupported types we skip beyond total + null counts.
fn is_unaggregatable(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(_, _)
            | DataType::ListView(_)
            | DataType::LargeListView(_)
            | DataType::Struct(_)
            | DataType::Map(_, _)
            | DataType::Union(_, _)
            | DataType::Binary
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::FixedSizeBinary(_)
    )
}

/// Numeric min/max (by value, keeping the original formatted text) plus a
/// histogram. Falls back to `(None, None, None)` if the column can't be cast.
fn numeric_min_max_histogram(
    array: &dyn Array,
    values: &[Option<String>],
) -> (Option<String>, Option<String>, Option<Histogram>) {
    let Ok(casted) = arrow::compute::cast(array, &DataType::Float64) else {
        return (None, None, None);
    };
    let Some(f) = casted.as_any().downcast_ref::<Float64Array>() else {
        return (None, None, None);
    };

    let mut min_idx: Option<usize> = None;
    let mut max_idx: Option<usize> = None;
    let mut nums: Vec<f64> = Vec::new();
    for r in 0..f.len() {
        if f.is_null(r) {
            continue;
        }
        let v = f.value(r);
        if v.is_nan() {
            continue;
        }
        nums.push(v);
        match min_idx {
            Some(i) if f.value(i) <= v => {}
            _ => min_idx = Some(r),
        }
        match max_idx {
            Some(i) if f.value(i) >= v => {}
            _ => max_idx = Some(r),
        }
    }

    let pick = |idx: Option<usize>| idx.and_then(|i| values.get(i).cloned().flatten());
    let min = pick(min_idx);
    let max = pick(max_idx);

    let histogram = match (min_idx, max_idx) {
        (Some(lo_i), Some(hi_i)) => {
            let lo = f.value(lo_i);
            let hi = f.value(hi_i);
            Some(histogram_of(&nums, lo, hi))
        }
        _ => None,
    };
    (min, max, histogram)
}

fn histogram_of(nums: &[f64], lo: f64, hi: f64) -> Histogram {
    if hi <= lo {
        return Histogram {
            min: lo,
            max: lo,
            bin_counts: vec![nums.len() as u64],
        };
    }
    let span = hi - lo;
    let mut bin_counts = vec![0u64; BUCKETS];
    for &v in nums {
        let frac = (v - lo) / span;
        let mut bucket = (frac * BUCKETS as f64).floor() as isize;
        bucket = bucket.clamp(0, BUCKETS as isize - 1);
        bin_counts[bucket as usize] += 1;
    }
    Histogram {
        min: lo,
        max: hi,
        bin_counts,
    }
}

fn lexical_min(values: &[Option<String>]) -> Option<String> {
    values.iter().flatten().min().cloned()
}

fn lexical_max(values: &[Option<String>]) -> Option<String> {
    values.iter().flatten().max().cloned()
}

fn top_values(counts: &HashMap<&str, u64>) -> Vec<(String, u64)> {
    let mut pairs: Vec<(String, u64)> = counts.iter().map(|(k, c)| (k.to_string(), *c)).collect();
    // Highest count first; ties broken by value for determinism.
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    pairs.truncate(TOP_K);
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use std::sync::Arc;

    fn batch() -> RecordBatch {
        let ints = Int64Array::from(vec![Some(3), Some(1), Some(2), Some(2), None]);
        let strs = StringArray::from(vec![Some("b"), Some("a"), Some("a"), None, None]);
        let schema = Schema::new(vec![
            Field::new("n", DataType::Int64, true),
            Field::new("s", DataType::Utf8, true),
        ]);
        RecordBatch::try_new(Arc::new(schema), vec![Arc::new(ints), Arc::new(strs)]).unwrap()
    }

    #[test]
    fn numeric_column_stats() {
        let insights = compute_from_batch(&batch());
        let n = &insights[0];
        assert_eq!(n.kind, ColumnKind::Numeric);
        assert_eq!(n.total, 5);
        assert_eq!(n.null_count, 1);
        assert_eq!(n.distinct, Some(3)); // {1,2,3}
        assert_eq!(n.min.as_deref(), Some("1"));
        assert_eq!(n.max.as_deref(), Some("3"));
        assert!(n.histogram.is_some());
        assert!(n.top_values.is_none()); // numeric columns don't get top values
    }

    #[test]
    fn string_column_stats() {
        let insights = compute_from_batch(&batch());
        let s = &insights[1];
        assert_eq!(s.kind, ColumnKind::String);
        assert_eq!(s.total, 5);
        assert_eq!(s.null_count, 2);
        assert_eq!(s.distinct, Some(2)); // {"a","b"}
        assert_eq!(s.min.as_deref(), Some("a"));
        assert_eq!(s.max.as_deref(), Some("b"));
        // "a" appears twice, "b" once → top value is "a".
        let top = s.top_values.as_ref().unwrap();
        assert_eq!(top.first().map(|(v, c)| (v.as_str(), *c)), Some(("a", 2)));
    }
}
