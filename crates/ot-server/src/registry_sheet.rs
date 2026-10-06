//! The registry as a spreadsheet: one row per entity, for export and bulk
//! import (CSV, or XLSX from any spreadsheet program).
//!
//! Columns: `entity_id`, `name`, `status`; the rest of the OTH-GOLD minimum
//! (`class_name`, `domain`, `affiliation`, `track_type`, `cot_type`,
//! `sidc`); `id:<scheme>` for each identifier scheme (several values of one
//! scheme separated by `;`); and `attr:<key>:<type>` for each attribute
//! (`attr:length:number`; without a type, an existing attribute keeps its
//! own and a new one is text). Sheets from earlier versions, with
//! `registry:<key>` and `card:<field>` columns, import as attributes.
//!
//! An import is planned row by row before anything is written. A row updates
//! the entity its `entity_id` names, else the one its identifiers belong to,
//! else creates one. Blank cells leave values as they are; identifiers are
//! only ever added, and never taken from another entity.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ot_store::{AttrType, Entity, RegistryIdentifier, registry::normalize_scheme};
use serde::Serialize;
use serde_json::Value;

/// A spreadsheet format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Xlsx,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "csv" => Some(Self::Csv),
            "xlsx" => Some(Self::Xlsx),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

/// Rows of text under a header.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    /// The file's marking, on a row of its own above the header (see
    /// `crate::marking`); read back when a sheet has one.
    pub marking: Option<String>,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

const SHEET_NAME: &str = "Registry";

/// The minimum's columns after entity_id, name and status.
const MINIMUM_COLUMNS: [&str; 6] = [
    "class_name",
    "domain",
    "affiliation",
    "track_type",
    "cot_type",
    "sidc",
];

/// A cell's text for a JSON value: strings as they are, anything else as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn type_name(t: AttrType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The registry as a sheet, in `entities` order.
pub fn export(entities: &[Entity]) -> Sheet {
    let schemes: BTreeSet<&str> = entities
        .iter()
        .flat_map(|e| e.identifiers.iter().map(|i| i.scheme.as_str()))
        .collect();
    let attributes: BTreeSet<(&str, AttrType)> = entities
        .iter()
        .flat_map(|e| e.attributes.iter().map(|a| (a.key.as_str(), a.kind)))
        .collect();
    let mut header: Vec<String> = ["entity_id", "name", "status", "publish"]
        .map(String::from)
        .to_vec();
    header.extend(MINIMUM_COLUMNS.map(String::from));
    header.extend(schemes.iter().map(|s| format!("id:{s}")));
    header.extend(
        attributes
            .iter()
            .map(|(k, t)| format!("attr:{k}:{}", type_name(*t))),
    );
    let rows = entities
        .iter()
        .map(|e| {
            let mut row = vec![
                e.id.clone(),
                e.name.clone().unwrap_or_default(),
                e.status.clone(),
                match e.publish {
                    Some(ot_store::registry::Publish::Always) => "always".into(),
                    Some(ot_store::registry::Publish::Never) => "never".into(),
                    None => String::new(),
                },
            ];
            for k in MINIMUM_COLUMNS {
                row.push(e.field(k).map(|v| text(&v)).unwrap_or_default());
            }
            for s in &schemes {
                let values: Vec<&str> = e
                    .identifiers
                    .iter()
                    .filter(|i| i.scheme == *s)
                    .map(|i| i.value.as_str())
                    .collect();
                row.push(values.join("; "));
            }
            for (k, t) in &attributes {
                row.push(
                    e.attribute(k)
                        .filter(|a| a.kind == *t)
                        .map(|a| text(&a.value))
                        .unwrap_or_default(),
                );
            }
            row
        })
        .collect();
    Sheet {
        marking: None,
        header,
        rows,
    }
}

/// Write a sheet as a file of `format`.
pub fn write(sheet: &Sheet, format: Format) -> Result<Vec<u8>, String> {
    match format {
        Format::Csv => {
            let mut w = csv::WriterBuilder::new()
                .flexible(true)
                .from_writer(Vec::new());
            if let Some(m) = &sheet.marking {
                w.write_record([m]).map_err(|e| e.to_string())?;
            }
            w.write_record(&sheet.header).map_err(|e| e.to_string())?;
            for row in &sheet.rows {
                w.write_record(row).map_err(|e| e.to_string())?;
            }
            w.into_inner().map_err(|e| e.to_string())
        }
        Format::Xlsx => {
            use rust_xlsxwriter::{Format as Style, Workbook};
            let mut book = Workbook::new();
            let ws = book.add_worksheet();
            ws.set_name(SHEET_NAME).map_err(|e| e.to_string())?;
            let bold = Style::new().set_bold();
            let mut top = 0;
            if let Some(m) = &sheet.marking {
                // In the sheet, and at the top and bottom of every printed page.
                ws.write_string_with_format(0, 0, m, &bold)
                    .map_err(|e| e.to_string())?;
                let print = format!("&C{}", m.replace('&', "&&"));
                ws.set_header(&print);
                ws.set_footer(&print);
                top = 1;
            }
            for (c, h) in sheet.header.iter().enumerate() {
                ws.write_string_with_format(top, c as u16, h, &bold)
                    .map_err(|e| e.to_string())?;
            }
            for (r, row) in sheet.rows.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    // Everything as text: identifiers such as MMSIs must keep
                    // their digits exactly.
                    ws.write_string(top + 1 + r as u32, c as u16, v)
                        .map_err(|e| e.to_string())?;
                }
            }
            ws.set_freeze_panes(top + 1, 0).map_err(|e| e.to_string())?;
            ws.autofit();
            book.save_to_buffer().map_err(|e| e.to_string())
        }
    }
}

/// Read a sheet from a file of `format` (XLSX: the first worksheet).
pub fn read(bytes: &[u8], format: Format) -> Result<Sheet, String> {
    let mut rows: Vec<Vec<String>> = match format {
        Format::Csv => {
            let text = std::str::from_utf8(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
                .map_err(|_| "the CSV is not UTF-8 text".to_owned())?;
            let mut r = csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_reader(text.as_bytes());
            r.records()
                .map(|rec| {
                    rec.map(|rec| rec.iter().map(str::to_owned).collect())
                        .map_err(|e| format!("CSV: {e}"))
                })
                .collect::<Result<_, _>>()?
        }
        Format::Xlsx => {
            use calamine::{Data, Reader, Xlsx};
            let mut book: Xlsx<_> = Xlsx::new(std::io::Cursor::new(bytes.to_vec()))
                .map_err(|e| format!("not a readable XLSX file: {e}"))?;
            let range = book
                .worksheet_range_at(0)
                .ok_or_else(|| "the workbook has no worksheet".to_owned())?
                .map_err(|e| format!("XLSX: {e}"))?;
            range
                .rows()
                .map(|row| {
                    row.iter()
                        .map(|c| match c {
                            Data::Empty => String::new(),
                            Data::String(s) => s.clone(),
                            // Whole numbers typed into a spreadsheet (an MMSI
                            // entered as a number) without a trailing ".0".
                            Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => {
                                format!("{}", *f as i64)
                            }
                            other => other.to_string(),
                        })
                        .collect()
                })
                .collect()
        }
    };
    if rows.is_empty() {
        return Err("the sheet is empty".into());
    }
    // A marking row (one cell, above a header) as OpenTrack writes it.
    let marking = (rows.len() > 1 && is_marking_row(&rows[0])).then(|| rows.remove(0)[0].clone());
    let header = rows
        .remove(0)
        .into_iter()
        .map(|h| h.trim().to_owned())
        .collect();
    Ok(Sheet {
        marking,
        header,
        rows,
    })
}

/// A row holding only a marking: one cell, the first, and not a column
/// name a header could start with.
fn is_marking_row(row: &[String]) -> bool {
    let first = row.first().map(|c| c.trim()).unwrap_or("");
    !first.is_empty()
        && row[1..].iter().all(|c| c.trim().is_empty())
        && !matches!(first.to_ascii_lowercase().as_str(), "entity_id" | "id")
}

/// What an import does with one row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Planned {
    /// The row's number in the sheet (the header is row 1).
    pub row: usize,
    /// `create`, `update`, `unchanged` or `error`.
    pub action: &'static str,
    pub entity_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub identifiers_added: Vec<String>,
    /// Fields the row changes (the minimum's and attributes).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The entity to write (with every identifier it should hold), when it
    /// changes.
    #[serde(skip)]
    pub entity: Option<Entity>,
}

/// What a column holds.
enum Column {
    EntityId,
    Name,
    Status,
    /// The publish override: `always`, `never`, or empty for automatic.
    Publish,
    /// One of the minimum after name, by field name.
    Minimum(&'static str),
    Identifier(String),
    /// An attribute, with the type the header gives.
    Attribute(String, Option<AttrType>),
    Ignored,
}

fn columns(header: &[String]) -> Result<Vec<Column>, String> {
    let mut out = Vec::new();
    let mut unknown = Vec::new();
    for h in header {
        let lower = h.to_ascii_lowercase();
        let prefixed = |p: &str| lower.starts_with(p).then(|| h[p.len()..].trim().to_owned());
        let col = if lower.is_empty() {
            Column::Ignored
        } else if matches!(lower.as_str(), "entity_id" | "id") {
            Column::EntityId
        } else if lower == "name" {
            Column::Name
        } else if lower == "status" {
            Column::Status
        } else if lower == "publish" {
            Column::Publish
        } else if let Some(m) = MINIMUM_COLUMNS.iter().find(|m| **m == lower) {
            Column::Minimum(m)
        } else if let Some(s) = prefixed("id:") {
            Column::Identifier(normalize_scheme(&s).map_err(|e| format!("column {h:?}: {e}"))?)
        } else if let Some(rest) = prefixed("attr:") {
            match rest.rsplit_once(':') {
                Some((k, t)) => {
                    let kind = serde_json::from_value(Value::String(t.trim().to_ascii_lowercase()))
                        .map_err(|_| {
                            format!(
                                "column {h:?}: {t:?} is not text, number, boolean, datetime or json"
                            )
                        })?;
                    Column::Attribute(k.trim().to_owned(), Some(kind))
                }
                None => Column::Attribute(rest, None),
            }
        } else if let Some(k) = prefixed("registry:") {
            // An earlier sheet's registry fields: `cot` and `ship_class` are
            // now the CoT type and class name.
            match k.as_str() {
                "cot" => Column::Minimum("cot_type"),
                "ship_class" => Column::Minimum("class_name"),
                _ => Column::Attribute(k, None),
            }
        } else if let Some(k) = prefixed("card:") {
            Column::Attribute(k, None)
        } else {
            unknown.push(h.clone());
            Column::Ignored
        };
        out.push(col);
    }
    if !unknown.is_empty() {
        return Err(format!(
            "unknown columns: {} (expected entity_id, name, status, publish, {}, id:<scheme>, attr:<key>:<type>)",
            unknown.join(", "),
            MINIMUM_COLUMNS.join(", ")
        ));
    }
    if !out
        .iter()
        .any(|c| matches!(c, Column::EntityId | Column::Identifier(_) | Column::Name))
    {
        return Err("the sheet needs an entity_id, name or id:<scheme> column".into());
    }
    Ok(out)
}

fn valid_entity_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Plan an import. `entity` looks an entity up by id; `holder` finds the
/// entity holding an identifier; `new_id` names new entities.
pub fn plan(
    sheet: &Sheet,
    entity: &mut dyn FnMut(&str) -> Option<Entity>,
    holder: &mut dyn FnMut(&str, &str) -> Option<String>,
    new_id: &mut dyn FnMut() -> String,
) -> Result<Vec<Planned>, String> {
    let cols = columns(&sheet.header)?;
    let mut out = Vec::new();
    // Earlier rows' entities and identifiers: a sheet may name each once.
    let mut rows_of_entity: HashMap<String, usize> = HashMap::new();
    let mut rows_of_identifier: HashMap<String, usize> = HashMap::new();
    for (i, row) in sheet.rows.iter().enumerate() {
        // The header is row 1, or 2 under a marking row.
        let number = i + 2 + usize::from(sheet.marking.is_some());
        let cell = |c: usize| row.get(c).map(|s| s.trim()).unwrap_or("");
        let (mut id, mut name, mut status) = (String::new(), None, None);
        // Only a sheet with the column changes the override (empty: automatic).
        let mut publish: Option<Result<Option<ot_store::registry::Publish>, String>> = None;
        let mut idents: Vec<(String, String)> = Vec::new();
        let mut values: Vec<(&Column, &str)> = Vec::new();
        for (c, col) in cols.iter().enumerate() {
            let v = cell(c);
            if matches!(col, Column::Publish) {
                publish = Some(match v.to_ascii_lowercase().as_str() {
                    "" | "auto" | "automatic" => Ok(None),
                    "always" => Ok(Some(ot_store::registry::Publish::Always)),
                    "never" => Ok(Some(ot_store::registry::Publish::Never)),
                    other => Err(format!("publish {other:?} must be always, never or empty")),
                });
                continue;
            }
            if v.is_empty() {
                continue;
            }
            match col {
                Column::EntityId => id = v.to_owned(),
                Column::Name => name = Some(v.to_owned()),
                Column::Status => status = Some(v.to_ascii_lowercase()),
                Column::Identifier(s) => idents.extend(
                    v.split(';')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(|x| (s.clone(), x.to_owned())),
                ),
                Column::Minimum(_) | Column::Attribute(..) => values.push((col, v)),
                Column::Publish | Column::Ignored => {}
            }
        }
        if id.is_empty() && name.is_none() && idents.is_empty() && values.is_empty() {
            continue;
        }
        let mut errors = Vec::new();
        if let Some(s) = &status
            && !matches!(s.as_str(), "active" | "retired")
        {
            errors.push(format!("status {s:?} must be active or retired"));
        }
        idents.sort();
        idents.dedup();
        // Who holds each identifier now.
        let holders: BTreeMap<String, String> = idents
            .iter()
            .filter_map(|(s, v)| holder(s, v).map(|h| (format!("{s}:{v}"), h)))
            .collect();
        let existing = if id.is_empty() {
            let owners: BTreeSet<&String> = holders.values().collect();
            match owners.len() {
                0 => None,
                1 => entity(owners.iter().next().expect("one")),
                _ => {
                    errors.push(format!(
                        "its identifiers belong to different entities: {}",
                        holders
                            .iter()
                            .map(|(i, h)| format!("{i} ({h})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    None
                }
            }
        } else if !valid_entity_id(&id) {
            errors.push(format!(
                "entity_id {id:?} must be letters, digits, '-', '_', '.' or ':' (at most 64)"
            ));
            None
        } else {
            entity(&id)
        };
        let target = existing
            .as_ref()
            .map(|e| e.id.clone())
            .or_else(|| (!id.is_empty()).then(|| id.clone()))
            .unwrap_or_else(&mut *new_id);
        for (ident, h) in &holders {
            if *h != target {
                errors.push(format!("{ident} already belongs to entity {h}"));
            }
        }
        if let Some(first) = rows_of_entity.insert(target.clone(), number) {
            errors.push(format!("entity {target} is also in row {first}"));
        }
        for (s, v) in &idents {
            if let Some(first) = rows_of_identifier.insert(format!("{s}:{v}"), number) {
                errors.push(format!("{s}:{v} is also in row {first}"));
            }
        }

        // The entity after the row.
        let mut after = existing.clone().unwrap_or_else(|| Entity {
            id: target.clone(),
            status: "active".into(),
            source: Some("sheet".into()),
            ..Default::default()
        });
        if let Some(n) = &name {
            after.name = Some(n.clone());
        }
        match &publish {
            Some(Ok(p)) => after.publish = *p,
            Some(Err(e)) => errors.push(e.clone()),
            None => {}
        }
        if let Some(s) = &status {
            after.status = s.clone();
        }
        let mut fields = Vec::new();
        for (col, v) in values {
            let (key, kind) = match col {
                Column::Minimum(k) => ((*k).to_owned(), None),
                Column::Attribute(k, t) => (k.clone(), *t),
                _ => continue,
            };
            let changed = match kind {
                // A typed column: the attribute takes that type.
                Some(t) if !ot_store::registry::RESERVED.contains(&key.as_str()) => {
                    match t.coerce(&Value::String(v.to_owned())) {
                        Ok(value) => {
                            let before = after.attribute(&key).cloned();
                            match after.attributes.iter_mut().find(|a| a.key == key) {
                                Some(a) => {
                                    a.kind = t;
                                    a.value = value;
                                }
                                None => after.attributes.push(ot_store::Attribute {
                                    key: key.clone(),
                                    kind: t,
                                    value,
                                }),
                            }
                            Ok(after.attribute(&key).cloned() != before)
                        }
                        Err(e) => Err(e),
                    }
                }
                _ => after.set_field(&key, &Value::String(v.to_owned())),
            };
            match changed {
                Ok(true) => fields.push(key),
                Ok(false) => {}
                Err(e) => errors.push(format!("{key}: {e}")),
            }
        }
        let mut identifiers_added = Vec::new();
        for (s, v) in &idents {
            if !after
                .identifiers
                .iter()
                .any(|i| i.scheme == *s && i.value == *v)
            {
                identifiers_added.push(format!("{s}:{v}"));
                after.identifiers.push(RegistryIdentifier {
                    scheme: s.clone(),
                    value: v.clone(),
                    expected_name: after.name.clone(),
                    source: Some("sheet".into()),
                });
            }
        }
        if errors.is_empty()
            && let Err(e) = after.clone().validate()
        {
            errors.push(e.to_string());
        }
        let changes = existing.as_ref() != Some(&after);
        let action = if !errors.is_empty() {
            "error"
        } else if existing.is_none() {
            "create"
        } else if changes {
            "update"
        } else {
            "unchanged"
        };
        out.push(Planned {
            row: number,
            action,
            entity_id: target,
            name: after.name.clone(),
            identifiers_added,
            fields,
            errors,
            entity: (action == "create" || action == "update").then_some(after),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ted() -> Entity {
        serde_json::from_value(json!({
            "id": "ent-ted", "name": "TED STEVENS", "status": "active", "domain": "surface",
            "identifiers": [{"scheme": "mmsi", "value": "338000001"}, {"scheme": "imo", "value": "0012345"}],
            "attributes": [{"key": "flag", "type": "text", "value": "US"},
                           {"key": "crew", "type": "number", "value": 24}]
        }))
        .unwrap()
    }

    #[test]
    fn export_and_read_back_in_both_formats() {
        let sheet = export(&[ted()]);
        assert_eq!(
            sheet.header,
            [
                "entity_id",
                "name",
                "status",
                "publish",
                "class_name",
                "domain",
                "affiliation",
                "track_type",
                "cot_type",
                "sidc",
                "id:imo",
                "id:mmsi",
                "attr:crew:number",
                "attr:flag:text"
            ]
        );
        assert_eq!(
            sheet.rows[0],
            [
                "ent-ted",
                "TED STEVENS",
                "active",
                "",
                "",
                "surface",
                "",
                "",
                "",
                "",
                "0012345",
                "338000001",
                "24",
                "US"
            ]
        );
        for format in [Format::Csv, Format::Xlsx] {
            let bytes = write(&sheet, format).unwrap();
            let back = read(&bytes, format).unwrap();
            assert_eq!(back.header, sheet.header, "{format:?}");
            // Leading zeros survive.
            assert_eq!(back.rows[0], sheet.rows[0], "{format:?}");
        }
    }

    #[test]
    fn a_marked_sheet_reads_back_and_numbers_rows_from_its_header() {
        let mut sheet = export(&[ted()]);
        sheet.marking = Some("SECRET//REL TO USA & GBR".into());
        let csv = String::from_utf8(write(&sheet, Format::Csv).unwrap()).unwrap();
        assert!(
            csv.starts_with("SECRET//REL TO USA & GBR\nentity_id,"),
            "{csv}"
        );
        for format in [Format::Csv, Format::Xlsx] {
            let back = read(&write(&sheet, format).unwrap(), format).unwrap();
            assert_eq!(back, sheet, "{format:?}");
            let mut bad = back.clone();
            bad.rows[0][2] = "nonsense".into();
            // The header is row 2 under the marking, so the entity is row 3.
            assert_eq!(run(&bad)[0].row, 3, "{format:?}");
        }
        // A header with entity_id first alone is a header, not a marking.
        let one = read(b"entity_id\nent-ted\n", Format::Csv).unwrap();
        assert_eq!(
            (one.marking, one.header),
            (None, vec!["entity_id".to_owned()])
        );
    }

    #[test]
    fn the_publish_override_goes_out_and_comes_back() {
        let mut e = ted();
        e.publish = Some(ot_store::registry::Publish::Never);
        let out = export(&[e]);
        assert_eq!(out.rows[0][3], "never");
        // Set, cleared (automatic), and left alone by a sheet without the column.
        let plans = run(&sheet(&["entity_id", "publish"], &[&["ent-ted", "always"]]));
        let after = plans[0].entity.as_ref().unwrap();
        assert_eq!(
            (plans[0].action, after.publish),
            ("update", Some(ot_store::registry::Publish::Always))
        );
        let plans = run(&sheet(
            &["entity_id", "publish"],
            &[&["ent-ted", "sometimes"]],
        ));
        assert_eq!(plans[0].action, "error");
        let plans = run(&sheet(
            &["entity_id", "name"],
            &[&["ent-ted", "TED STEVENS"]],
        ));
        assert_eq!(plans[0].action, "unchanged");
    }

    fn run(sheet: &Sheet) -> Vec<Planned> {
        let e = ted();
        let mut n = 0;
        plan(
            sheet,
            &mut |id| (id == "ent-ted").then(|| e.clone()),
            &mut |s, v| {
                (e.identifiers.iter().any(|i| i.scheme == s && i.value == v))
                    .then(|| "ent-ted".into())
            },
            &mut || {
                n += 1;
                format!("ent-new{n}")
            },
        )
        .unwrap()
    }

    fn sheet(header: &[&str], rows: &[&[&str]]) -> Sheet {
        Sheet {
            marking: None,
            header: header.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
        }
    }

    #[test]
    fn an_exported_sheet_imports_unchanged() {
        let planned = run(&export(&[ted()]));
        assert_eq!(planned[0].action, "unchanged", "{planned:?}");
    }

    #[test]
    fn rows_update_by_identifier_create_and_report_errors() {
        let planned = run(&sheet(
            &[
                "name",
                "affiliation",
                "id:mmsi",
                "id:elnot",
                "attr:length:number",
                "registry:cot",
                "card:owner",
            ],
            &[
                // Found by MMSI: adds an ELNOT, sets affiliation, a new typed
                // attribute, the CoT type (an earlier sheet's registry:cot) and
                // an earlier sheet's card field.
                &[
                    "",
                    "friend",
                    "338000001",
                    "NL504",
                    "103.5",
                    "a-f-S-C",
                    "MSC",
                ],
                // New entity.
                &["NEW SHIP", "", "999", "", "", "", ""],
                // Bad values.
                &["BAD", "enemy", "998", "", "long", "", ""],
            ],
        ));
        assert_eq!(planned[0].action, "update", "{planned:?}");
        assert_eq!(planned[0].identifiers_added, ["elnot:NL504"]);
        let e = planned[0].entity.as_ref().unwrap();
        assert_eq!(e.field("affiliation"), Some(json!("friend")));
        assert_eq!(e.attribute("length").unwrap().value, json!(103.5));
        assert_eq!(e.cot_type.as_deref(), Some("a-f-S-C"));
        assert_eq!(e.field("owner"), Some(json!("MSC")));
        // Blank cells leave values: the flag is still there.
        assert_eq!(e.field("flag"), Some(json!("US")));
        assert_eq!(
            planned[0].fields,
            ["affiliation", "length", "cot_type", "owner"]
        );

        assert_eq!(planned[1].action, "create");
        assert_eq!(planned[1].entity_id, "ent-new1");

        assert_eq!(planned[2].action, "error");
        assert_eq!(planned[2].errors.len(), 2, "{:?}", planned[2].errors);
    }

    #[test]
    fn identifiers_are_never_taken_and_a_sheet_names_each_once() {
        let planned = run(&sheet(
            &["entity_id", "id:mmsi"],
            &[
                &["ent-other", "338000001"],
                &["ent-x", "5"],
                &["ent-y", "5"],
            ],
        ));
        assert!(planned[0].errors[0].contains("already belongs to entity ent-ted"));
        assert_eq!(planned[1].action, "create");
        assert!(planned[2].errors[0].contains("also in row 3"));
    }

    #[test]
    fn unknown_columns_are_refused() {
        let e = columns(&["name".into(), "colour".into()]).err().unwrap();
        assert!(e.contains("colour"), "{e}");
        assert!(columns(&["attr:x:colour".into()]).is_err());
    }
}
