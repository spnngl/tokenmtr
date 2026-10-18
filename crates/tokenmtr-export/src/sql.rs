//! SQL generated from the export schema. Pure text; the Postgres sink reuses it.
//!
//! Identifiers are double-quoted because some column names (`end`, `plan`) are SQL keywords.
//! Decimal columns are exact integers named with their unit ([`crate::schema::Column::sql_name`]).

use crate::schema::{DatasetSchema, EXPORT_SCHEMA_VERSION};

/// DDL of the table holding the export schema version.
pub const CREATE_SCHEMA_TABLE: &str = "CREATE TABLE IF NOT EXISTS _schema (\
    id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL) STRICT";

/// Inserts the version row when the table is new; an existing row is left for the version check.
#[must_use]
pub fn insert_schema_version() -> String {
    format!("INSERT OR IGNORE INTO _schema (id, version) VALUES (1, {EXPORT_SCHEMA_VERSION})")
}

/// `CREATE TABLE IF NOT EXISTS` for one dataset: the first column is the primary key and nullable
/// columns omit `NOT NULL`.
#[must_use]
pub fn create_table(schema: &DatasetSchema) -> String {
    let columns: Vec<String> = schema
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let mut definition = format!("\"{}\" {}", column.sql_name(), column.ty.sql());
            if index == 0 {
                definition.push_str(" PRIMARY KEY");
            }
            if !column.nullable {
                definition.push_str(" NOT NULL");
            }
            definition
        })
        .collect();
    format!(
        "CREATE TABLE IF NOT EXISTS \"{}\" ({}) STRICT",
        schema.dataset,
        columns.join(", ")
    )
}

/// The upsert of one row: insert, or replace the existing row only when the incoming revision is
/// higher, so redelivering a revision (or an older one) changes nothing.
#[must_use]
pub fn upsert(schema: &DatasetSchema) -> String {
    let table = schema.dataset;
    let names: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("\"{}\"", c.sql_name()))
        .collect();
    let placeholders: Vec<String> = (1..=names.len()).map(|n| format!("?{n}")).collect();
    let assignments: Vec<String> = names
        .iter()
        .skip(1)
        .map(|name| format!("{name} = excluded.{name}"))
        .collect();
    format!(
        "INSERT INTO \"{table}\" ({}) VALUES ({}) ON CONFLICT(\"{}\") DO UPDATE SET {} \
         WHERE excluded.\"revision\" > \"{table}\".\"revision\"",
        names.join(", "),
        placeholders.join(", "),
        schema.key().name,
        assignments.join(", "),
    )
}
