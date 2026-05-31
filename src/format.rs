use arrow::array::{
    Array, AsArray, FixedSizeListArray, LargeListArray, ListArray, MapArray, StructArray,
};
use arrow::datatypes::{DataType, TimeUnit};
use arrow::record_batch::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};

pub fn default_options() -> FormatOptions<'static> {
    FormatOptions::default()
        .with_display_error(true)
        .with_null("∅")
}

pub fn is_nested(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(_, _)
            | DataType::Struct(_)
            | DataType::Map(_, _)
    )
}

pub fn cell(array: &dyn Array, row: usize, opts: &FormatOptions) -> String {
    if array.is_null(row) {
        return opts.null().to_string();
    }
    if is_nested(array.data_type()) {
        return compact_preview(array, row, opts);
    }
    match ArrayFormatter::try_new(array, opts) {
        Ok(fmt) => fmt
            .value(row)
            .try_to_string()
            .unwrap_or_else(|e| format!("<err: {e}>")),
        Err(e) => format!("<unsupported: {e}>"),
    }
}

pub fn row_strings(batch: &RecordBatch, row: usize, opts: &FormatOptions) -> Vec<String> {
    (0..batch.num_columns())
        .map(|c| cell(batch.column(c).as_ref(), row, opts))
        .collect()
}

const PREVIEW_INLINE_LEN: usize = 3;
const PREVIEW_INLINE_STRUCT_FIELDS: usize = 3;

/// Compact one-line preview for a nested cell, suitable for the grid.
fn compact_preview(array: &dyn Array, row: usize, opts: &FormatOptions) -> String {
    match array.data_type() {
        DataType::List(_) => list_preview(array.as_list::<i32>(), row, opts),
        DataType::LargeList(_) => large_list_preview(array.as_list::<i64>(), row, opts),
        DataType::FixedSizeList(_, _) => fsl_preview(array.as_fixed_size_list(), row, opts),
        DataType::Struct(_) => struct_preview(array.as_struct(), row, opts),
        DataType::Map(_, _) => map_preview(array.as_map(), row, opts),
        _ => leaf_inline(array, row, opts),
    }
}

fn list_preview(arr: &ListArray, row: usize, opts: &FormatOptions) -> String {
    let values = arr.value(row);
    list_render(values.as_ref(), opts)
}

fn large_list_preview(arr: &LargeListArray, row: usize, opts: &FormatOptions) -> String {
    let values = arr.value(row);
    list_render(values.as_ref(), opts)
}

fn fsl_preview(arr: &FixedSizeListArray, row: usize, opts: &FormatOptions) -> String {
    let values = arr.value(row);
    list_render(values.as_ref(), opts)
}

fn list_render(values: &dyn Array, opts: &FormatOptions) -> String {
    let n = values.len();
    if n == 0 {
        return "[]".to_string();
    }
    if n > PREVIEW_INLINE_LEN {
        return format!("[{n} items]");
    }
    let mut out = String::from("[");
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&leaf_inline(values, i, opts));
    }
    out.push(']');
    out
}

fn struct_preview(arr: &StructArray, row: usize, opts: &FormatOptions) -> String {
    let fields = arr.fields();
    let cols = arr.columns();
    if fields.is_empty() {
        return "{}".to_string();
    }
    let show = fields.len().min(PREVIEW_INLINE_STRUCT_FIELDS);
    let mut out = String::from("{");
    for i in 0..show {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(fields[i].name());
        out.push_str(": ");
        out.push_str(&leaf_inline(cols[i].as_ref(), row, opts));
    }
    if fields.len() > show {
        out.push_str(", …");
    }
    out.push('}');
    out
}

fn map_preview(arr: &MapArray, row: usize, opts: &FormatOptions) -> String {
    let entries_struct = arr.value(row);
    let n = entries_struct.len();
    if n == 0 {
        return "{}".to_string();
    }
    if n > PREVIEW_INLINE_LEN {
        return format!("{{{n} entries}}");
    }
    let keys = entries_struct.column(0);
    let values = entries_struct.column(1);
    let mut out = String::from("{");
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&leaf_inline(keys.as_ref(), i, opts));
        out.push_str(": ");
        out.push_str(&leaf_inline(values.as_ref(), i, opts));
    }
    out.push('}');
    out
}

/// Inline rendering of a single value: nested → recursive compact preview, else ArrayFormatter.
fn leaf_inline(array: &dyn Array, row: usize, opts: &FormatOptions) -> String {
    if array.is_null(row) {
        return opts.null().to_string();
    }
    if is_nested(array.data_type()) {
        return compact_preview(array, row, opts);
    }
    match ArrayFormatter::try_new(array, opts) {
        Ok(fmt) => fmt
            .value(row)
            .try_to_string()
            .unwrap_or_else(|e| format!("<err: {e}>")),
        Err(_) => "<?>".to_string(),
    }
}

/// Tree node built from a single nested cell, suitable for rendering as an indented tree.
#[derive(Debug, Clone)]
pub enum NestedNode {
    Null,
    Leaf(String),
    List(Vec<NestedNode>),
    Struct(Vec<(String, NestedNode)>),
    Map(Vec<(NestedNode, NestedNode)>),
}

impl NestedNode {
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 0);
        out
    }

    fn write_json(&self, out: &mut String, indent: usize) {
        match self {
            NestedNode::Null => out.push_str("null"),
            NestedNode::Leaf(s) => out.push_str(&json_scalar(s)),
            NestedNode::List(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('\n');
                    push_indent(out, indent + 1);
                    item.write_json(out, indent + 1);
                }
                out.push('\n');
                push_indent(out, indent);
                out.push(']');
            }
            NestedNode::Struct(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('\n');
                    push_indent(out, indent + 1);
                    out.push('"');
                    out.push_str(&escape_json(k));
                    out.push_str("\": ");
                    v.write_json(out, indent + 1);
                }
                out.push('\n');
                push_indent(out, indent);
                out.push('}');
            }
            NestedNode::Map(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('\n');
                    push_indent(out, indent + 1);
                    let key_str = match k {
                        NestedNode::Leaf(s) => s.clone(),
                        NestedNode::Null => "null".to_string(),
                        _ => k.to_json_string(),
                    };
                    out.push('"');
                    out.push_str(&escape_json(&key_str));
                    out.push_str("\": ");
                    v.write_json(out, indent + 1);
                }
                out.push('\n');
                push_indent(out, indent);
                out.push('}');
            }
        }
    }
}

fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Best-effort scalar JSON encoding. Numbers and booleans pass through; everything else is quoted.
fn json_scalar(s: &str) -> String {
    if s == "true" || s == "false" || s == "null" {
        return s.to_string();
    }
    if s.parse::<f64>().is_ok() && !s.is_empty() {
        return s.to_string();
    }
    format!("\"{}\"", escape_json(s))
}

/// Build a tree node from a nested cell.
pub fn cell_node(array: &dyn Array, row: usize, opts: &FormatOptions) -> NestedNode {
    if array.is_null(row) {
        return NestedNode::Null;
    }
    match array.data_type() {
        DataType::List(_) => list_node(array.as_list::<i32>(), row, opts),
        DataType::LargeList(_) => large_list_node(array.as_list::<i64>(), row, opts),
        DataType::FixedSizeList(_, _) => fsl_node(array.as_fixed_size_list(), row, opts),
        DataType::Struct(_) => struct_node(array.as_struct(), row, opts),
        DataType::Map(_, _) => map_node(array.as_map(), row, opts),
        _ => leaf_node(array, row, opts),
    }
}

fn list_node(arr: &ListArray, row: usize, opts: &FormatOptions) -> NestedNode {
    let values = arr.value(row);
    let n = values.len();
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        items.push(cell_node(values.as_ref(), i, opts));
    }
    NestedNode::List(items)
}

fn large_list_node(arr: &LargeListArray, row: usize, opts: &FormatOptions) -> NestedNode {
    let values = arr.value(row);
    let n = values.len();
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        items.push(cell_node(values.as_ref(), i, opts));
    }
    NestedNode::List(items)
}

fn fsl_node(arr: &FixedSizeListArray, row: usize, opts: &FormatOptions) -> NestedNode {
    let values = arr.value(row);
    let n = values.len();
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        items.push(cell_node(values.as_ref(), i, opts));
    }
    NestedNode::List(items)
}

fn struct_node(arr: &StructArray, row: usize, opts: &FormatOptions) -> NestedNode {
    let fields = arr.fields();
    let cols = arr.columns();
    let mut out = Vec::with_capacity(fields.len());
    for (f, c) in fields.iter().zip(cols.iter()) {
        out.push((f.name().clone(), cell_node(c.as_ref(), row, opts)));
    }
    NestedNode::Struct(out)
}

fn map_node(arr: &MapArray, row: usize, opts: &FormatOptions) -> NestedNode {
    let entries_struct = arr.value(row);
    let keys = entries_struct.column(0);
    let values = entries_struct.column(1);
    let n = entries_struct.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push((
            cell_node(keys.as_ref(), i, opts),
            cell_node(values.as_ref(), i, opts),
        ));
    }
    NestedNode::Map(out)
}

fn leaf_node(array: &dyn Array, row: usize, opts: &FormatOptions) -> NestedNode {
    let s = match ArrayFormatter::try_new(array, opts) {
        Ok(fmt) => fmt
            .value(row)
            .try_to_string()
            .unwrap_or_else(|e| format!("<err: {e}>")),
        Err(_) => "<?>".to_string(),
    };
    NestedNode::Leaf(s)
}

/// Compact, human-friendly type label (one line) suitable for table columns.
pub fn type_label(dt: &DataType) -> String {
    type_label_with(dt, 3)
}

fn type_label_with(dt: &DataType, struct_field_budget: usize) -> String {
    match dt {
        DataType::Null => "Null".into(),
        DataType::Boolean => "Bool".into(),
        DataType::Int8 => "Int8".into(),
        DataType::Int16 => "Int16".into(),
        DataType::Int32 => "Int32".into(),
        DataType::Int64 => "Int64".into(),
        DataType::UInt8 => "UInt8".into(),
        DataType::UInt16 => "UInt16".into(),
        DataType::UInt32 => "UInt32".into(),
        DataType::UInt64 => "UInt64".into(),
        DataType::Float16 => "Float16".into(),
        DataType::Float32 => "Float32".into(),
        DataType::Float64 => "Float64".into(),
        DataType::Utf8 => "Utf8".into(),
        DataType::LargeUtf8 => "LargeUtf8".into(),
        DataType::Utf8View => "Utf8View".into(),
        DataType::Binary => "Binary".into(),
        DataType::LargeBinary => "LargeBinary".into(),
        DataType::BinaryView => "BinaryView".into(),
        DataType::FixedSizeBinary(n) => format!("FixedSizeBinary({n})"),
        DataType::Date32 => "Date32".into(),
        DataType::Date64 => "Date64".into(),
        DataType::Time32(u) => format!("Time32({})", time_unit_label(u)),
        DataType::Time64(u) => format!("Time64({})", time_unit_label(u)),
        DataType::Timestamp(u, tz) => match tz {
            Some(tz) => format!("Timestamp({}, {})", time_unit_label(u), tz),
            None => format!("Timestamp({})", time_unit_label(u)),
        },
        DataType::Duration(u) => format!("Duration({})", time_unit_label(u)),
        DataType::Interval(unit) => format!("Interval({unit:?})"),
        DataType::Decimal32(p, s) => format!("Decimal32({p}, {s})"),
        DataType::Decimal64(p, s) => format!("Decimal64({p}, {s})"),
        DataType::Decimal128(p, s) => format!("Decimal128({p}, {s})"),
        DataType::Decimal256(p, s) => format!("Decimal256({p}, {s})"),
        DataType::List(f) => format!(
            "List<{}>",
            type_label_with(f.data_type(), struct_field_budget)
        ),
        DataType::LargeList(f) => {
            format!(
                "LargeList<{}>",
                type_label_with(f.data_type(), struct_field_budget)
            )
        }
        DataType::ListView(f) => {
            format!(
                "ListView<{}>",
                type_label_with(f.data_type(), struct_field_budget)
            )
        }
        DataType::LargeListView(f) => format!(
            "LargeListView<{}>",
            type_label_with(f.data_type(), struct_field_budget)
        ),
        DataType::FixedSizeList(f, n) => format!(
            "FixedSizeList<{}, {n}>",
            type_label_with(f.data_type(), struct_field_budget)
        ),
        DataType::Struct(fields) => {
            let show = fields.len().min(struct_field_budget);
            let mut s = String::from("Struct{");
            for i in 0..show {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(fields[i].name());
                s.push_str(": ");
                s.push_str(&type_label_with(fields[i].data_type(), 1));
            }
            if fields.len() > show {
                s.push_str(", …");
            }
            s.push('}');
            s
        }
        DataType::Map(entry_field, _) => {
            // Map's child is a Struct{key,value}.
            if let DataType::Struct(fs) = entry_field.data_type()
                && fs.len() == 2
            {
                return format!(
                    "Map<{}, {}>",
                    type_label_with(fs[0].data_type(), 1),
                    type_label_with(fs[1].data_type(), 1)
                );
            }
            "Map<?>".to_string()
        }
        DataType::Dictionary(k, v) => format!(
            "Dict<{}, {}>",
            type_label_with(k, 1),
            type_label_with(v, struct_field_budget)
        ),
        DataType::RunEndEncoded(_, v) => {
            format!(
                "REE<{}>",
                type_label_with(v.data_type(), struct_field_budget)
            )
        }
        DataType::Union(_, _) => "Union".into(),
    }
}

fn time_unit_label(u: &TimeUnit) -> &'static str {
    match u {
        TimeUnit::Second => "s",
        TimeUnit::Millisecond => "ms",
        TimeUnit::Microsecond => "µs",
        TimeUnit::Nanosecond => "ns",
    }
}

/// Multi-line, fully-expanded type description for tooltips.
pub fn type_label_full(dt: &DataType) -> String {
    let mut out = String::new();
    write_type_full(&mut out, dt, 0);
    out
}

fn write_type_full(out: &mut String, dt: &DataType, indent: usize) {
    match dt {
        DataType::Struct(fields) => {
            out.push_str("Struct {\n");
            for f in fields.iter() {
                push_indent(out, indent + 1);
                out.push_str(f.name());
                out.push_str(": ");
                write_type_full(out, f.data_type(), indent + 1);
                if f.is_nullable() {
                    out.push_str(" (nullable)");
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push('}');
        }
        DataType::List(f) | DataType::LargeList(f) => {
            let label = if matches!(dt, DataType::List(_)) {
                "List<"
            } else {
                "LargeList<"
            };
            out.push_str(label);
            write_type_full(out, f.data_type(), indent);
            out.push('>');
        }
        DataType::FixedSizeList(f, n) => {
            out.push_str("FixedSizeList<");
            write_type_full(out, f.data_type(), indent);
            out.push_str(&format!(", {n}>"));
        }
        DataType::Map(entry, _) => {
            if let DataType::Struct(fs) = entry.data_type()
                && fs.len() == 2
            {
                out.push_str("Map<");
                write_type_full(out, fs[0].data_type(), indent);
                out.push_str(", ");
                write_type_full(out, fs[1].data_type(), indent);
                out.push('>');
            } else {
                out.push_str("Map<?>");
            }
        }
        _ => out.push_str(&type_label(dt)),
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", value, UNITS[unit])
    }
}
