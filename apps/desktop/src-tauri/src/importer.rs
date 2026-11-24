use crate::{
    catalog::{self, Settings},
    manager::{db_error, insert_workspace},
};
use calamine::{open_workbook_from_rs, Data, DataRef, Reader, Xlsx};
use quick_xml::{events::Event, Reader as XmlReader};
use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{Cursor, Read},
    path::Path,
};
use zip::ZipArchive;

const INPUT_LIMIT: u64 = 2 * 1024 * 1024;
const XLSX_LIMIT: u64 = 16 * 1024 * 1024;
const ROW_LIMIT: usize = 1000;
const COLUMN_LIMIT: usize = 32;
const HEADERS: [&str; 5] = ["name", "start_url", "launch_urls", "icon", "notes"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub number: usize,
    pub name: String,
    pub duplicate: bool,
    pub completed: bool,
    pub error: Option<String>,
    #[serde(skip)]
    pub settings: Settings,
}

#[derive(Serialize)]
pub struct Preview {
    pub fingerprint: String,
    pub rows: Vec<PreviewRow>,
}

pub fn bounded_file(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("Could not open the file: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Could not inspect the file: {e}"))?;
    if !metadata.is_file() || metadata.len() > max {
        return Err(format!(
            "Choose a regular file no larger than {} MiB.",
            max / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read the file: {e}"))?;
    if bytes.len() as u64 > max {
        return Err("The file is too large.".into());
    }
    Ok(bytes)
}

fn check_coordinate(value: &str) -> bool {
    value.split(':').all(|address| {
        let address = address.trim_matches('$');
        let split = address
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(address.len());
        let (letters, digits) = address.split_at(split);
        let col = letters
            .bytes()
            .filter(|c| *c != b'$')
            .try_fold(0usize, |col, c| {
                c.is_ascii_uppercase().then_some(())?;
                col.checked_mul(26)?.checked_add((c - b'A' + 1) as usize)
            });
        col.is_some_and(|col| (1..=COLUMN_LIMIT).contains(&col))
            && digits
                .parse::<usize>()
                .is_ok_and(|row| (1..=ROW_LIMIT + 1).contains(&row))
    })
}

fn verify_xml(name: &str, bytes: &[u8]) -> Result<(), String> {
    let mut reader = XmlReader::from_reader(bytes);
    let worksheet = name.starts_with("xl/worksheets/") && name.ends_with(".xml");
    loop {
        match reader.read_event().map_err(|_| "Invalid workbook XML.")? {
            Event::Eof => return Ok(()),
            Event::DocType(_) => {
                return Err(
                    "Workbook XML declarations with external references are not supported.".into(),
                )
            }
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                if matches!(
                    local.as_ref(),
                    "externalLink"
                        | "externalReference"
                        | "externalReferences"
                        | "f"
                        | "definedName"
                ) {
                    return Err("Workbook formulas, defined names and external references are not supported. Export values only.".into());
                }
                for attr in e.attributes() {
                    let attr = attr.map_err(|_| "Invalid workbook XML attribute.")?;
                    let val = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| "Invalid workbook XML value.")?;
                    let key = attr.key.local_name();
                    if key.as_ref() == "TargetMode" && !val.eq_ignore_ascii_case("internal") {
                        return Err("External workbook relationships are not supported.".into());
                    }
                    if key.as_ref() == "Target"
                        && (val.contains("://") || val.starts_with("//") || val.contains('\\'))
                    {
                        return Err(
                            "External workbook relationship targets are not supported.".into()
                        );
                    }
                    if worksheet
                        && ((local.as_ref() == "c" && key.as_ref() == "r")
                            || (local.as_ref() == "dimension" && key.as_ref() == "ref"))
                        && !check_coordinate(&val)
                    {
                        return Err(
                            "The worksheet exceeds the supported row or column range.".into()
                        );
                    }
                    if worksheet
                        && local.as_ref() == "row"
                        && key.as_ref() == "r"
                        && !val
                            .parse::<usize>()
                            .is_ok_and(|row| (1..=ROW_LIMIT + 1).contains(&row))
                    {
                        return Err("The worksheet exceeds the supported row range.".into());
                    }
                }
            }
            _ => {}
        }
    }
}

fn xlsx(bytes: &[u8]) -> Result<Vec<(usize, Vec<String>)>, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Invalid XLSX archive.")?;
    if archive.len() > 256 {
        return Err("The XLSX archive contains too many files.".into());
    }
    let mut expanded = 0;
    let mut names = HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|_| "Invalid XLSX entry.")?;
        let name = file
            .name()
            .map_err(|_| "Invalid XLSX entry name.")?
            .to_ascii_lowercase();
        if file.enclosed_name().is_none()
            || file.is_symlink()
            || name.contains('\\')
            || name.split('/').any(|part| part == "..")
            || !names.insert(name.clone())
            || name.starts_with("xl/externallinks/")
            || name == "xl/connections.xml"
        {
            return Err("Unsafe workbook archive path or external workbook reference.".into());
        }
        if file.is_dir() {
            continue;
        }
        let capacity = XLSX_LIMIT - expanded;
        if file.size() > capacity {
            return Err("Expanded XLSX data exceeds 16 MiB.".into());
        }
        let mut content = Vec::new();
        file.take(capacity + 1)
            .read_to_end(&mut content)
            .map_err(|_| "Could not read XLSX entry.")?;
        expanded += content.len() as u64;
        if expanded > XLSX_LIMIT {
            return Err("Expanded XLSX data exceeds 16 MiB.".into());
        }
        if name.ends_with(".xml") || name.ends_with(".rels") {
            verify_xml(&name, &content)?;
        }
    }
    let mut book: Xlsx<_> =
        open_workbook_from_rs(Cursor::new(bytes)).map_err(|e| format!("Invalid workbook: {e}"))?;
    worksheet_rows(&mut book, XLSX_LIMIT as usize)
}

fn worksheet_rows(
    book: &mut Xlsx<Cursor<&[u8]>>,
    mut remaining: usize,
) -> Result<Vec<(usize, Vec<String>)>, String> {
    let name = book
        .sheet_names()
        .into_iter()
        .next()
        .ok_or("The workbook has no worksheets.")?;
    let read_error = |e| format!("Could not read the first sheet: {e}");
    let mut sheet = book.worksheet_cells_reader(&name).map_err(read_error)?;
    let mut rows = vec![Vec::new(); ROW_LIMIT + 1];
    let mut cell_count = 0;
    while let Some(cell) = sheet.next_cell().map_err(read_error)? {
        let (row, col) = cell.get_position();
        let (row, col) = (row as usize, col as usize);
        cell_count += 1;
        if row > ROW_LIMIT || col >= COLUMN_LIMIT || cell_count > (ROW_LIMIT + 1) * COLUMN_LIMIT {
            return Err("Worksheet is too large.".into());
        }
        let value = match cell.get_value() {
            DataRef::Empty => continue,
            DataRef::SharedString(value) => {
                if value.len() > remaining {
                    return Err("Expanded worksheet cell data exceeds 16 MiB.".into());
                }
                (*value).to_owned()
            }
            value => Data::from(value.clone()).to_string(),
        };
        remaining = remaining
            .checked_sub(value.len())
            .ok_or("Expanded worksheet cell data exceeds 16 MiB.")?;
        let fields = &mut rows[row];
        if fields.len() <= col {
            fields.resize(col + 1, String::new());
        }
        fields[col] = value;
    }
    Ok(rows
        .into_iter()
        .enumerate()
        .filter(|(_, fields)| !fields.is_empty())
        .map(|(row, fields)| (row + 1, fields))
        .collect())
}

fn text_rows(bytes: &[u8], delimiter: u8) -> Result<Vec<(usize, Vec<String>)>, String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(bytes);
    let mut rows = Vec::new();
    let mut scanned = 0;
    let mut bare_crs = 0;
    for row in reader.records() {
        if rows.len() > ROW_LIMIT {
            return Err("Use at most 1,000 data rows per file.".into());
        }
        let row = row.map_err(|e| format!("Invalid UTF-8 or delimited data: {e}"))?;
        if row.len() > COLUMN_LIMIT {
            return Err("Use at most 32 columns.".into());
        }
        let number = row.position().map_or(rows.len() + 1, |p| {
            let offset = p.byte() as usize;
            bare_crs += (scanned..offset)
                .filter(|&i| bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
                .count();
            scanned = offset;
            p.line() as usize + bare_crs
        });
        rows.push((number, row.iter().map(str::to_owned).collect()));
    }
    Ok(rows)
}

fn preview(db: &Connection, account: &str, bytes: &[u8], format: &str) -> Result<Preview, String> {
    let mut hash = Sha256::new();
    hash.update(format);
    hash.update([0]);
    hash.update(bytes);
    let fingerprint = format!("{:x}", hash.finalize());
    let mut rows = match format {
        "csv" => text_rows(bytes, b',')?,
        "tsv" | "txt" => text_rows(bytes, b'\t')?,
        "xlsx" => xlsx(bytes)?,
        _ => return Err("Choose a CSV, TSV, TXT or XLSX file.".into()),
    }
    .into_iter();
    let headers = rows.next().ok_or("This file is empty.")?.1;
    let mut fields = [None; HEADERS.len()];
    for (index, header) in headers.iter().enumerate() {
        if let Some(pos) = HEADERS
            .iter()
            .position(|name| header.trim().eq_ignore_ascii_case(name))
        {
            if fields[pos].replace(index).is_some() {
                return Err(format!("Repeated header: {}.", HEADERS[pos]));
            }
        }
    }
    if fields[0].is_none() {
        return Err("The first row must have a name column.".into());
    }
    let mut names = db
        .prepare("SELECT name FROM workspaces WHERE account_id=?1")
        .map_err(db_error)?;
    let mut names: HashSet<String> = names
        .query_map([account], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .map(|row| row.map(|name| name.to_lowercase()))
        .collect::<rusqlite::Result<_>>()
        .map_err(db_error)?;
    let mut completed = db
        .prepare(
            "SELECT row_number FROM completed_import_rows WHERE account_id=?1 AND fingerprint=?2",
        )
        .map_err(db_error)?;
    let completed: HashSet<usize> = completed
        .query_map(params![account, fingerprint], |row| row.get(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db_error)?;
    let mut preview = Vec::new();
    for (number, values) in rows {
        if values.iter().all(|v| v.trim().is_empty()) {
            continue;
        }
        let value = |field: usize| {
            fields[field]
                .and_then(|i| values.get(i))
                .map_or("", |s| s.trim())
        };
        let name = value(0).to_owned();
        let mut settings = Settings {
            icon: value(3).into(),
            notes: value(4).into(),
            ..Settings::default()
        };
        settings.launch_urls.extend(
            [value(1)]
                .into_iter()
                .chain(value(2).lines())
                .filter(|url| !url.trim().is_empty())
                .map(str::to_owned),
        );
        let error = catalog::name(&name)
            .and_then(|_| catalog::settings(db, account, settings.clone()).map(|_| ()))
            .err();
        let is_completed = completed.contains(&number);
        let duplicate = error.is_none() && !is_completed && !names.insert(name.to_lowercase());
        preview.push(PreviewRow {
            number,
            name,
            completed: is_completed,
            duplicate,
            error,
            settings,
        });
    }
    Ok(Preview {
        fingerprint,
        rows: preview,
    })
}

pub fn read_preview(db: &Connection, account: &str, path: &Path) -> Result<Preview, String> {
    let format = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // TXT and TSV with the same contents are the same import.
    let format = if format == "txt" { "tsv" } else { &format };
    preview(db, account, &bounded_file(path, INPUT_LIMIT)?, format)
}

pub fn import_row(
    db: &mut Connection,
    account: &str,
    preview: &Preview,
    number: usize,
) -> Result<&'static str, String> {
    let row = preview
        .rows
        .iter()
        .find(|row| row.number == number)
        .ok_or("That row does not exist in the preview.")?;
    if let Some(error) = &row.error {
        return Err(error.clone());
    }
    let tx = db.transaction().map_err(db_error)?;
    if tx.query_row("SELECT EXISTS(SELECT 1 FROM completed_import_rows WHERE account_id=?1 AND fingerprint=?2 AND row_number=?3)",
        params![account, preview.fingerprint, number], |r| r.get::<_, bool>(0),
    ).map_err(db_error)? { return Ok("Already imported"); }
    if row.duplicate {
        return Err("This name was marked as a duplicate in the preview.".into());
    }
    let mut query = tx
        .prepare("SELECT name FROM workspaces WHERE account_id=?1")
        .map_err(db_error)?;
    let mut names = query
        .query_map([account], |r| r.get::<_, String>(0))
        .map_err(db_error)?;
    let name = row.name.to_lowercase();
    if names.any(|value| value.is_ok_and(|v| v.to_lowercase() == name)) {
        return Err("This workspace name now exists.".into());
    }
    drop(names);
    drop(query);
    let id = insert_workspace(&tx, account, &row.name, row.settings.clone())?;
    tx.execute(
        "INSERT INTO completed_import_rows VALUES (?1,?2,?3,?4)",
        params![account, preview.fingerprint, number, id],
    )
    .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok("Imported")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::Manager;
    use std::io::Write;

    #[test]
    fn shared_strings_count_toward_the_budget_per_cell() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let files = [
            ("_rels/.rels", r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#),
            ("xl/workbook.xml", r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Workspaces" sheetId="1" r:id="rId1"></sheet></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", "<sst><si><t>Example</t></si></sst>"),
            ("xl/worksheets/sheet1.xml", r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row><row r="2"><c r="A2" t="s"><v>0</v></c></row></sheetData></worksheet>"#),
        ];
        for (path, xml) in files {
            archive.start_file(path, zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(xml.as_bytes()).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        let mut book = open_workbook_from_rs(Cursor::new(bytes.as_slice())).unwrap();
        assert_eq!(worksheet_rows(&mut book, 14).unwrap(), vec![
            (1, vec!["Example".to_owned()]), (2, vec!["Example".to_owned()]),
        ]);
        assert!(worksheet_rows(&mut book, 13).unwrap_err().contains("Expanded worksheet cell data"));
    }

    #[test]
    fn carriage_return_records_have_independent_import_ids() {
        let root = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(root.path().into()).unwrap();
        let input = b"name,notes\rFirst,\"line 1\rline 2\"\rSecond,\r";
        let doc = preview(manager.db().unwrap(), "local", input, "csv").unwrap();
        assert_eq!(doc.rows.iter().map(|row| row.number).collect::<Vec<_>>(), [2, 4]);
        for row in &doc.rows {
            assert_eq!(import_row(manager.db_mut().unwrap(), "local", &doc, row.number).unwrap(), "Imported");
        }
        let retry = preview(manager.db().unwrap(), "local", input, "csv").unwrap();
        assert!(retry.rows.iter().all(|row| row.completed));
    }

    #[test]
    fn csv_preview_checks_unicode_quotes_duplicates_and_idempotency() {
        let root = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(root.path().into()).unwrap();
        let input = "name,notes,start_url\nCafé,\"one, two\",https://example.test\nCafé,Duplicate,\nBad,,file:///tmp";
        let doc = preview(manager.db().unwrap(), "local", input.as_bytes(), "csv").unwrap();
        assert_eq!(doc.rows[0].settings.notes, "one, two");
        assert!(doc.rows[1].duplicate);
        assert!(doc.rows[2].error.is_some());
        assert_eq!(
            import_row(manager.db_mut().unwrap(), "local", &doc, 2).unwrap(),
            "Imported"
        );
        let retry = preview(manager.db().unwrap(), "local", input.as_bytes(), "csv").unwrap();
        assert!(retry.rows[0].completed);
        assert_eq!(
            import_row(manager.db_mut().unwrap(), "local", &retry, 2).unwrap(),
            "Already imported"
        );
    }

    #[test]
    fn workbook_rejects_external_relationships_and_oversized_coordinates() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "xl/_rels/workbook.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(br#"<Relationships><Relationship TargetMode="External" Target="file:///external.xlsx"/></Relationships>"#).unwrap();
        assert!(xlsx(&archive.finish().unwrap().into_inner())
            .unwrap_err()
            .contains("External"));
        assert!(!check_coordinate("XFD1"));
        assert!(!check_coordinate("A1048576"));
    }
}
