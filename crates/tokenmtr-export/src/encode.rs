//! Row encoding for file sinks: one CSV record or JSON object per line.
//!
//! Both formats come from the same [`DatasetSchema`], so header and keys cannot drift from the
//! `SQLite` columns. The decoder reads back what the encoder wrote; file sinks use it to find the
//! last delivered revision, to repair a torn last line and to merge snapshots.

use std::io::{self, BufRead as _, BufReader, Read};

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
    /// Length of the prefix made of whole records; bytes after it are a torn write.
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

    /// Decodes a whole file. A torn last record (see [`RowEncoder::scan`]) is excluded.
    ///
    /// # Errors
    /// [`SinkError::Corrupt`] when the CSV header differs from the schema or a whole record is not
    /// a record of this dataset.
    pub fn decode(&self, path: &Utf8Path, bytes: &[u8]) -> Result<Decoded, SinkError> {
        let mut rows = Vec::new();
        let valid_len = self.walk(path, bytes, |row| rows.push(row))?;
        Ok(Decoded {
            rows,
            valid_len: usize::try_from(valid_len).unwrap_or(bytes.len()),
        })
    }

    /// Streams a file without holding it in memory: its highest revision and the length of its
    /// whole records. The last record is whole when the file ends with a newline and, for CSV,
    /// the record has every column: the last column of each dataset is never quoted, so a write
    /// torn inside a quoted multi-line field is caught.
    pub(crate) fn scan(&self, path: &Utf8Path, input: impl Read) -> Result<Scanned, SinkError> {
        let mut max_revision = 0;
        let valid_len = self.walk(path, input, |row| {
            max_revision = max_revision.max(row.revision);
        })?;
        Ok(Scanned {
            max_revision,
            valid_len,
        })
    }

    fn walk(
        &self,
        path: &Utf8Path,
        input: impl Read,
        each: impl FnMut(StoredRow),
    ) -> Result<u64, SinkError> {
        let walked = match self.format {
            FileFormat::Csv => self.walk_csv(input, each),
            FileFormat::Jsonl => self.walk_jsonl(input, each),
        };
        walked.map_err(|fault| match fault {
            Fault::Io(e) => SinkError::io("read", path, e),
            Fault::Corrupt(reason) => SinkError::Corrupt {
                path: path.to_owned(),
                reason,
            },
        })
    }

    fn walk_csv(&self, input: impl Read, mut each: impl FnMut(StoredRow)) -> Result<u64, Fault> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(LastByte {
                inner: input,
                last: None,
            });
        let mut header_seen = false;
        let mut valid_len = 0;
        // A record is whole once another one follows it; the last one is checked at the end.
        let mut previous: Option<csv::StringRecord> = None;
        loop {
            let start = reader.position().byte();
            let mut record = csv::StringRecord::new();
            if !reader.read_record(&mut record)? {
                break;
            }
            if let Some(whole) = previous.replace(record) {
                self.csv_whole(&whole, &mut header_seen, &mut each)?;
                valid_len = start;
            }
        }
        if let Some(last) = previous
            && reader.get_ref().last == Some(b'\n')
            && last.len() == self.schema.columns.len()
        {
            self.csv_whole(&last, &mut header_seen, &mut each)?;
            valid_len = reader.position().byte();
        }
        Ok(valid_len)
    }

    fn csv_whole(
        &self,
        record: &csv::StringRecord,
        header_seen: &mut bool,
        each: &mut impl FnMut(StoredRow),
    ) -> Result<(), Fault> {
        if !*header_seen {
            if !record.iter().eq(self.schema.names()) {
                return Err(Fault::Corrupt(format!(
                    "header {:?} differs from the {} schema",
                    record.iter().collect::<Vec<_>>(),
                    self.schema.dataset
                )));
            }
            *header_seen = true;
            return Ok(());
        }
        let id = record.get(0).unwrap_or_default().to_owned();
        if record.len() != self.schema.columns.len() {
            return Err(Fault::Corrupt(format!(
                "record {id} has {} fields, the {} schema has {}",
                record.len(),
                self.schema.dataset,
                self.schema.columns.len()
            )));
        }
        let revision = record
            .get(DatasetSchema::REVISION_INDEX)
            .and_then(|r| r.parse().ok())
            .ok_or_else(|| Fault::Corrupt(format!("record {id} has no numeric revision")))?;
        let bytes = csv_record(record.iter()).map_err(|e| Fault::Corrupt(e.to_string()))?;
        each(StoredRow {
            id,
            revision,
            bytes,
        });
        Ok(())
    }

    fn walk_jsonl(&self, input: impl Read, mut each: impl FnMut(StoredRow)) -> Result<u64, Fault> {
        let mut input = BufReader::new(input);
        let mut line = Vec::new();
        let mut valid_len = 0_u64;
        loop {
            line.clear();
            let read = input.read_until(b'\n', &mut line).map_err(Fault::Io)?;
            // End of file, or a last line without its newline: a torn write.
            let Some(body) = line.strip_suffix(b"\n") else {
                break;
            };
            valid_len = valid_len.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if !body.iter().all(u8::is_ascii_whitespace) {
                each(self.jsonl_row(body).map_err(Fault::Corrupt)?);
            }
        }
        Ok(valid_len)
    }

    fn jsonl_row(&self, line: &[u8]) -> Result<StoredRow, String> {
        let key = self.schema.key().name;
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
        Ok(StoredRow {
            id,
            revision,
            bytes,
        })
    }
}

/// What [`RowEncoder::scan`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scanned {
    /// Highest revision of a whole record; 0 when there is none.
    pub(crate) max_revision: u64,
    /// Length of the prefix made of whole records; bytes after it are a torn write.
    pub(crate) valid_len: u64,
}

/// Why a walk over a file stopped.
enum Fault {
    Io(io::Error),
    Corrupt(String),
}

impl From<csv::Error> for Fault {
    fn from(error: csv::Error) -> Self {
        let reason = error.to_string();
        match error.into_kind() {
            csv::ErrorKind::Io(e) => Self::Io(e),
            _ => Self::Corrupt(reason),
        }
    }
}

/// Remembers the last byte read, so the CSV walk knows whether the file ends with a newline.
struct LastByte<R> {
    inner: R,
    last: Option<u8>,
}

impl<R: Read> Read for LastByte<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        if let Some(byte) = buf.get(..read).and_then(<[u8]>::last) {
            self.last = Some(*byte);
        }
        Ok(read)
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
        Value::Decimal { .. } => value.to_plain().unwrap_or_default(),
    })
}
