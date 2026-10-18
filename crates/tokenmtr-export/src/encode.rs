//! Row encoding for file sinks (D18): one CSV record or JSON object per line.
//!
//! Both formats come from the same [`DatasetSchema`], so header and keys cannot drift from the
//! `SQLite` columns. The decoder reads back what the encoder wrote; file sinks use it to find the
//! last delivered revision, to repair a torn last line and to merge snapshots.

use camino::Utf8Path;
use tokenmtr_config::FileFormat;
use tokenmtr_store::Dataset;

use crate::error::SinkError;
use crate::schema::{DatasetSchema, ExportRow, Value, schema};

/// A row as found in an export file: its identity plus the exact bytes of its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRow {
    /// The primary key.
    pub id: String,
    /// The row's revision.
    pub revision: u64,
    /// The encoded record including its trailing newline.
    pub bytes: Vec<u8>,
}

/// The rows of a file and how much of it is whole lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Rows in file order.
    pub rows: Vec<StoredRow>,
    /// Length of the prefix that ends in a newline; bytes after it are a torn write.
    pub valid_len: usize,
}

/// Encodes and decodes rows of one dataset in one format.
#[derive(Debug, Clone, Copy)]
pub struct RowEncoder {
    format: FileFormat,
    schema: &'static DatasetSchema,
}

impl RowEncoder {
    /// An encoder for `dataset` rows.
    #[must_use]
    pub fn new(format: FileFormat, dataset: Dataset) -> Self {
        Self {
            format,
            schema: schema(dataset),
        }
    }

    /// The bytes a new file starts with: the CSV header, nothing for JSONL.
    ///
    /// # Errors
    /// [`SinkError::Csv`] when the writer fails.
    pub fn header(&self) -> Result<Vec<u8>, SinkError> {
        match self.format {
            FileFormat::Csv => csv_record(self.schema.names()),
            FileFormat::Jsonl => Ok(Vec::new()),
        }
    }

    /// Encodes one row as a line ending in a newline.
    ///
    /// # Errors
    /// [`SinkError::Csv`] or [`SinkError::Json`] when the writer fails.
    pub fn encode(&self, row: &ExportRow) -> Result<Vec<u8>, SinkError> {
        match self.format {
            FileFormat::Csv => csv_record(
                row.values
                    .iter()
                    .map(|v| v.to_plain().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(String::as_str),
            ),
            FileFormat::Jsonl => self.jsonl_line(row),
        }
    }

    fn jsonl_line(&self, row: &ExportRow) -> Result<Vec<u8>, SinkError> {
        let mut line = String::from("{");
        for (index, (column, value)) in self.schema.columns.iter().zip(&row.values).enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push_str(&serde_json::to_string(column.name)?);
            line.push(':');
            line.push_str(&json_value(value)?);
        }
        line.push_str("}\n");
        Ok(line.into_bytes())
    }

    /// Decodes a whole file. A last line without a newline is a torn write and is excluded.
    ///
    /// # Errors
    /// [`SinkError::Corrupt`] when the CSV header differs from the schema or a complete line is not
    /// a record of this dataset.
    pub fn decode(&self, path: &Utf8Path, bytes: &[u8]) -> Result<Decoded, SinkError> {
        let valid_len = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |index| index + 1);
        let whole = &bytes[..valid_len];
        let corrupt = |reason: String| SinkError::Corrupt {
            path: path.to_owned(),
            reason,
        };
        let rows = match self.format {
            FileFormat::Csv => self.decode_csv(whole).map_err(corrupt)?,
            FileFormat::Jsonl => self.decode_jsonl(whole).map_err(corrupt)?,
        };
        Ok(Decoded { rows, valid_len })
    }

    fn decode_csv(&self, whole: &[u8]) -> Result<Vec<StoredRow>, String> {
        if whole.is_empty() {
            return Ok(Vec::new());
        }
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(whole);
        let header = reader.headers().map_err(|e| e.to_string())?;
        if !header.iter().eq(self.schema.names()) {
            return Err(format!(
                "header {:?} differs from the {} schema",
                header.iter().collect::<Vec<_>>(),
                self.schema.dataset
            ));
        }
        let mut rows = Vec::new();
        for record in reader.records() {
            let record = record.map_err(|e| e.to_string())?;
            let id = record.get(0).unwrap_or_default().to_owned();
            let revision = record
                .get(DatasetSchema::REVISION_INDEX)
                .and_then(|r| r.parse().ok())
                .ok_or_else(|| format!("record {id} has no numeric revision"))?;
            let bytes = csv_record(record.iter()).map_err(|e| e.to_string())?;
            rows.push(StoredRow {
                id,
                revision,
                bytes,
            });
        }
        Ok(rows)
    }

    fn decode_jsonl(&self, whole: &[u8]) -> Result<Vec<StoredRow>, String> {
        let key = self.schema.key().name;
        let mut rows = Vec::new();
        for line in whole
            .split(|b| *b == b'\n')
            .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        {
            let object: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(line).map_err(|e| e.to_string())?;
            let id = object
                .get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("line without a string `{key}`"))?
                .to_owned();
            let revision = object
                .get("revision")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("line {id} has no numeric revision"))?;
            let mut bytes = line.to_vec();
            bytes.push(b'\n');
            rows.push(StoredRow {
                id,
                revision,
                bytes,
            });
        }
        Ok(rows)
    }
}

fn csv_record<'a>(fields: impl IntoIterator<Item = &'a str>) -> Result<Vec<u8>, SinkError> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    writer.write_record(fields)?;
    writer
        .into_inner()
        .map_err(|e| SinkError::Csv(csv::Error::from(e.into_error())))
}

fn json_value(value: &Value) -> Result<String, serde_json::Error> {
    Ok(match value {
        Value::Null => "null".to_owned(),
        Value::Text(text) => serde_json::to_string(text)?,
        Value::Integer(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Real(x) => {
            serde_json::Number::from_f64(*x).map_or_else(|| "null".to_owned(), |n| n.to_string())
        }
        // A decimal's text is already a valid JSON number and keeps every digit.
        Value::Decimal(d) => d.to_string(),
    })
}
