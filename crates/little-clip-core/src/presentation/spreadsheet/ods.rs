//! Reads OpenDocument merge geometry alongside Calamine's worksheet values.

use super::check_cancelled;
use calamine::Dimensions;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use std::{collections::HashMap, sync::atomic::AtomicBool};

const MAX_ROWS: u32 = 1_048_576;
const MAX_COLUMNS: u32 = 16_384;
const MAX_MERGES: usize = 100_000;

pub(super) fn read_merges(
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> Result<HashMap<String, Vec<Dimensions>>, String> {
    let mut archive = crate::zip_content::archive(bytes)?;
    let xml = crate::zip_content::entry(&mut archive, "content.xml", 64 * 1024 * 1024)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    reader.config_mut().expand_empty_elements = true;
    let mut sheets = HashMap::new();
    let mut remaining = MAX_MERGES;
    while remaining > 0 {
        check_cancelled(cancelled)?;
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(tag) if tag.name().as_ref() == b"table:table" => {
                let name = tag
                    .try_get_attribute("table:name")
                    .map_err(|err| err.to_string())?
                    .map(|name| {
                        name.decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map(|name| name.into_owned())
                    })
                    .transpose()
                    .map_err(|err| err.to_string())?;
                let merges = read_table(&mut reader, remaining, cancelled)?;
                remaining -= merges.len();
                if let Some(name) = name {
                    sheets.insert(name, merges);
                }
            }
            Event::Eof => break,
            _ => (),
        }
    }
    Ok(sheets)
}

fn read_table(
    reader: &mut Reader<&[u8]>,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<Dimensions>, String> {
    let mut position = Position::default();
    let mut merges = Vec::new();
    loop {
        check_cancelled(cancelled)?;
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(tag) if tag.name().as_ref() == b"table:table-row" => {
                position.begin_row(&tag)?;
            }
            Event::Start(tag) if position.row_repeat > 0 && is_cell(&tag) => {
                position.read_cell(&tag, &mut merges, limit)?;
                reader
                    .read_to_end(tag.name())
                    .map_err(|err| err.to_string())?;
            }
            Event::End(tag) if tag.name().as_ref() == b"table:table-row" => {
                position.end_row();
            }
            Event::End(tag) if tag.name().as_ref() == b"table:table" => return Ok(merges),
            Event::Eof => return Err("Incomplete OpenDocument table".into()),
            _ => (),
        }
    }
}

fn is_cell(tag: &BytesStart<'_>) -> bool {
    matches!(
        tag.name().as_ref(),
        b"table:table-cell" | b"table:covered-table-cell"
    )
}

fn count(tag: &BytesStart<'_>, attribute: &str) -> Result<u32, String> {
    let value = tag
        .try_get_attribute(attribute)
        .map_err(|err| err.to_string())?;
    Ok(value
        .and_then(|value| std::str::from_utf8(&value.value).ok()?.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1))
}

#[derive(Default)]
struct Position {
    row: u32,
    column: u32,
    row_repeat: u32,
}

impl Position {
    fn begin_row(&mut self, tag: &BytesStart<'_>) -> Result<(), String> {
        self.column = 0;
        self.row_repeat = count(tag, "table:number-rows-repeated")?.min(MAX_ROWS - self.row);
        Ok(())
    }

    fn end_row(&mut self) {
        self.row += self.row_repeat;
        self.row_repeat = 0;
    }

    fn read_cell(
        &mut self,
        tag: &BytesStart<'_>,
        merges: &mut Vec<Dimensions>,
        limit: usize,
    ) -> Result<(), String> {
        let repeat = count(tag, "table:number-columns-repeated")?.min(MAX_COLUMNS - self.column);
        if tag.name().as_ref() == b"table:table-cell" {
            let rows = count(tag, "table:number-rows-spanned")?;
            let columns = count(tag, "table:number-columns-spanned")?;
            if rows > 1 || columns > 1 {
                let origins = (self.row..self.row + self.row_repeat).flat_map(|row| {
                    (self.column..self.column + repeat).map(move |column| (row, column))
                });
                merges.extend(origins.take(limit.saturating_sub(merges.len())).map(
                    |(row, column)| Dimensions {
                        start: (row, column),
                        end: (
                            row.saturating_add(rows - 1).min(MAX_ROWS - 1),
                            column.saturating_add(columns - 1).min(MAX_COLUMNS - 1),
                        ),
                    },
                ));
            }
        }
        self.column += repeat;
        Ok(())
    }
}
