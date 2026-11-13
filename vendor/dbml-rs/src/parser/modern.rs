//! Current DBML constructs parsed through Pest, retaining source spans and literals.
use super::*;
use alloc::collections::{BTreeMap, BTreeSet};
use pest::{
  Span,
  error::{Error, ErrorVariant},
};

pub(super) fn unescape(raw: &str) -> String {
  let mut result = String::new();
  let mut chars = raw.chars();
  while let Some(ch) = chars.next() {
    if ch == '\\' {
      match chars.next() {
        Some('n') => result.push('\n'),
        Some('r') => result.push('\r'),
        Some('t') => result.push('\t'),
        Some(next) => {
          if next != '\'' && next != '\\' && next != '"' {
            result.push('\\');
          }
          result.push(next);
        }
        None => result.push('\\'),
      }
    } else {
      result.push(ch);
    }
  }
  result
}

pub(super) fn parse_partial(pair: Pair<Rule>) -> ParserResult<TablePartial> {
  let span_range = s2r(pair.as_span());
  let definition = parse_table_decl(pair)?;
  Ok(TablePartial {
    span_range,
    name: definition.ident.name.clone(),
    definition,
  })
}

pub(super) fn parse_records(pair: Pair<Rule>) -> ParserResult<RecordsBlock> {
  let mut records = RecordsBlock {
    span_range: s2r(pair.as_span()),
    ..Default::default()
  };
  for child in pair.into_inner() {
    match child.as_rule() {
      Rule::decl_ident => {
        let (schema, name) = parse_decl_ident(child)?;
        records.table = Some(TableIdent {
          schema,
          name,
          ..Default::default()
        });
      }
      Rule::records_columns => {
        records.columns = child
          .into_inner()
          .map(parse_ident)
          .collect::<ParserResult<_>>()?;
      }
      Rule::block_settings => {
        records.settings = child
          .into_inner()
          .map(parse_attribute)
          .collect::<ParserResult<_>>()?;
        records.example = records
          .settings
          .iter()
          .any(|a| a.key.to_string == "example");
      }
      Rule::records_body => {
        for row in child.into_inner() {
          if row.as_rule() == Rule::record_row {
            records.rows.push(parse_row(row)?);
          }
        }
      }
      _ => return throw_msg("Unexpected records construct", child),
    }
  }
  Ok(records)
}

fn parse_row(pair: Pair<Rule>) -> ParserResult<Vec<Literal>> {
  let null = Literal {
    span_range: s2r(pair.as_span()),
    raw: String::new(),
    value: Value::Null,
  };
  let mut row = vec![null.clone()];
  for child in pair.into_inner() {
    if matches!(
      child.as_rule(),
      Rule::single_line_comment | Rule::multi_line_comment
    ) {
      continue;
    }
    if child.as_rule() == Rule::record_comma {
      row.push(null.clone());
      continue;
    }
    let span_range = s2r(child.as_span());
    let raw = child.as_str().trim().to_string();
    let value = child.into_inner().next().expect("record literal");
    let parsed = match value.as_rule() {
      Rule::single_quoted_string => Value::String(unescape(value.into_inner().as_str())),
      Rule::backquoted_quoted_string => Value::Expr(value.into_inner().as_str().to_string()),
      Rule::record_enum => Value::Enum(
        value
          .into_inner()
          .map(parse_ident)
          .collect::<ParserResult<Vec<_>>>()?
          .iter()
          .map(|i| i.to_string.as_str())
          .collect::<Vec<_>>()
          .join("."),
      ),
      Rule::record_bare => parse_bare(value)?,
      _ => return throw_msg("Unsupported record literal", value),
    };
    *row.last_mut().expect("record cell") = Literal {
      span_range,
      raw,
      value: parsed,
    };
  }
  Ok(row)
}

fn parse_bare(pair: Pair<Rule>) -> ParserResult<Value> {
  let text = pair.as_str().trim();
  if text.eq_ignore_ascii_case("null") {
    return Ok(Value::Null);
  }
  if text.eq_ignore_ascii_case("true") {
    return Ok(Value::Bool(true));
  }
  if text.eq_ignore_ascii_case("false") {
    return Ok(Value::Bool(false));
  }
  if let Ok(number) = text.parse::<i64>() {
    return Ok(Value::Integer(number));
  }
  if let Ok(number) = text.parse::<f64>() {
    if number.is_finite() {
      return Ok(Value::Decimal(number));
    }
  }
  // Qualified enum values are the only unquoted nonnumeric values in records.
  let parts: Vec<_> = text.split('.').collect();
  if parts.len() >= 2
    && parts
      .iter()
      .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
  {
    return Ok(Value::Enum(text.to_string()));
  }
  throw_msg(
    "Invalid record literal: quote strings and keep expressions in backticks",
    pair,
  )
}

fn fail<T>(input: &str, range: &SpanRange, message: impl ToString) -> ParserResult<T> {
  Err(Error::new_from_span(
    ErrorVariant::CustomError {
      message: message.to_string(),
    },
    Span::new(input, range.start, range.end).expect("authored span"),
  ))
}

pub(crate) fn resolve(schema: &mut SchemaBlock<'_>) -> ParserResult<()> {
  schema.authored_blocks = schema.blocks.clone();
  let mut partials = BTreeMap::new();
  for block in &schema.blocks {
    if let TopLevelBlock::TablePartial(partial) = block {
      validate_partial(partial, schema.input)?;
      if partials
        .insert(partial.name.to_string.clone(), partial.clone())
        .is_some()
      {
        return fail(
          schema.input,
          &partial.span_range,
          "Duplicate TablePartial name",
        );
      }
    }
  }
  let mut total_columns = 0;
  for block in &mut schema.blocks {
    if let TopLevelBlock::Table(table) = block {
      expand(table, &partials, schema.input)?;
      total_columns += table.cols.len();
      if total_columns > 2048 {
        return fail(
          schema.input,
          &table.span_range,
          "DBML expansion exceeds the 2048 field budget",
        );
      }
    }
  }
  attach_records(schema)?;
  for table in schema.tables() {
    validate_records(table, &schema.enums(), schema.input)?;
  }
  Ok(())
}

fn validate_partial(partial: &TablePartial, input: &str) -> ParserResult<()> {
  let definition = &partial.definition;
  if !definition.partials.is_empty() || !definition.records.is_empty() {
    return fail(
      input,
      &partial.span_range,
      "TablePartial cannot contain partial injections or records",
    );
  }
  validate_definition(definition, input)?;
  Ok(())
}

fn unique_columns(columns: &[TableColumn], input: &str) -> ParserResult<()> {
  let mut names = BTreeSet::new();
  for column in columns {
    if !names.insert(&column.name.to_string) {
      return fail(input, &column.span_range, "Duplicate column name");
    }
  }
  Ok(())
}

fn validate_definition(definition: &TableBlock, input: &str) -> ParserResult<()> {
  unique_columns(&definition.cols, input)?;
  if let Some(settings) = &definition.settings {
    let mut keys = BTreeSet::new();
    for attribute in &settings.attributes {
      if !keys.insert(&attribute.key.to_string) {
        return fail(input, &attribute.span_range, "Duplicate table setting");
      }
    }
  }
  Ok(())
}

fn expand(
  table: &mut TableBlock,
  partials: &BTreeMap<String, TablePartial>,
  input: &str,
) -> ParserResult<()> {
  validate_definition(table, input)?;
  if table.partials.len() > 128 {
    return fail(
      input,
      &table.span_range,
      "DBML exceeds 128 partial injections per table",
    );
  }
  let mut injections = BTreeSet::new();
  for injection in &table.partials {
    if !injections.insert(&injection.name.to_string) {
      return fail(
        input,
        &injection.span_range,
        "Duplicate TablePartial injection",
      );
    }
  }
  let local = table.clone();
  let mut indexes = Vec::new();
  let mut attributes = Vec::new();
  let mut note = None;
  // Apply injection precedence in authored order, then local definitions last.
  for injection in &local.partials {
    let partial = match partials.get(&injection.name.to_string) {
      Some(partial) => partial,
      None => return fail(input, &injection.span_range, "Unknown TablePartial"),
    };
    merge_definition(
      &partial.definition,
      &mut indexes,
      &mut attributes,
      &mut note,
    );
  }
  merge_definition(&local, &mut indexes, &mut attributes, &mut note);
  let columns = effective_columns(&local, partials, input)?;
  table.cols = columns;
  table.note = note;
  if !indexes.is_empty() {
    table.indexes = Some(IndexesBlock {
      defs: indexes,
      ..Default::default()
    });
  }
  if !attributes.is_empty() {
    table.settings = Some(TableSettings {
      attributes,
      ..Default::default()
    });
  }
  Ok(())
}

// Select each winning field at its authored injection/local position.
fn effective_columns(
  local: &TableBlock,
  partials: &BTreeMap<String, TablePartial>,
  input: &str,
) -> ParserResult<Vec<TableColumn>> {
  let mut columns = Vec::new();
  let mut winners = BTreeMap::new();
  for (index, injection) in local.partials.iter().enumerate() {
    for column in &partials[&injection.name.to_string].definition.cols {
      winners.insert(column.name.to_string.clone(), Some(index));
    }
  }
  for column in &local.cols {
    winners.insert(column.name.to_string.clone(), None);
  }
  let mut field = 0;
  let mut injection = 0;
  for position in 0..local.cols.len() + local.partials.len() {
    if injection < local.partials.len() && local.partials[injection].order == position {
      let partial = &partials[&local.partials[injection].name.to_string];
      for column in &partial.definition.cols {
        if winners.get(&column.name.to_string) == Some(&Some(injection)) {
          columns.push(column.clone());
        }
      }
      injection += 1;
    } else {
      columns.push(local.cols[field].clone());
      field += 1;
    }
    if columns.len() > 2048 {
      return fail(
        input,
        &local.span_range,
        "DBML partial expansion exceeds 2048 columns per table",
      );
    }
  }
  Ok(columns)
}

fn index_key(index: &IndexesDef) -> Vec<String> {
  index
    .cols
    .iter()
    .map(|c| match c {
      IndexesColumnType::String(c) => c.to_string.clone(),
      IndexesColumnType::Expr(e) => e.raw.clone(),
    })
    .collect()
}

fn merge_definition(
  definition: &TableBlock,
  indexes: &mut Vec<IndexesDef>,
  attributes: &mut Vec<Attribute>,
  note: &mut Option<NoteBlock>,
) {
  if let Some(block) = &definition.indexes {
    indexes.retain(|old| {
      !block
        .defs
        .iter()
        .any(|index| index_key(old) == index_key(index))
    });
    indexes.extend(block.defs.iter().cloned());
  }
  if let Some(settings) = &definition.settings {
    attributes.retain(|old| {
      !settings
        .attributes
        .iter()
        .any(|attribute| old.key.to_string == attribute.key.to_string)
    });
    attributes.extend(settings.attributes.iter().cloned());
  }
  if definition.note.is_some() {
    *note = definition.note.clone();
  }
}

fn table_matches(table: &TableBlock, ident: &TableIdent) -> bool {
  let schema = |s: &Option<Ident>| {
    s.as_ref()
      .map_or(crate::DEFAULT_SCHEMA, |s| s.to_string.as_str())
      .to_string()
  };
  (schema(&table.ident.schema) == schema(&ident.schema)
    && table.ident.name.to_string == ident.name.to_string)
    || (ident.schema.is_none()
      && table
        .ident
        .alias
        .as_ref()
        .is_some_and(|a| a.to_string == ident.name.to_string))
}

fn attach_records(schema: &mut SchemaBlock<'_>) -> ParserResult<()> {
  let records: Vec<_> = schema
    .blocks
    .iter()
    .filter_map(|b| {
      if let TopLevelBlock::Records(r) = b {
        Some(r.clone())
      } else {
        None
      }
    })
    .collect();
  for record in records {
    let ident = record.table.as_ref().expect("top-level record target");
    let table = schema.blocks.iter_mut().find_map(|b| {
      if let TopLevelBlock::Table(t) = b {
        table_matches(t, ident).then_some(t)
      } else {
        None
      }
    });
    match table {
      Some(table) => table.records.push(record),
      None => return fail(schema.input, &record.span_range, "Unknown records table"),
    }
  }
  Ok(())
}

fn validate_records(table: &TableBlock, enums: &[&EnumBlock], input: &str) -> ParserResult<()> {
  if table.records.len() > 1 {
    return fail(
      input,
      &table.records[1].span_range,
      "Only one records block is permitted per table",
    );
  }
  for records in &table.records {
    if records
      .settings
      .iter()
      .any(|a| a.key.to_string != "example" || a.value.is_some())
      || records.settings.len() > 1
    {
      return fail(
        input,
        &records.span_range,
        "Records only supports the [example] flag",
      );
    }
    let columns = record_columns(table, records, input)?;
    if records.rows.len() > 256 || records.rows.iter().map(Vec::len).sum::<usize>() > 4096 {
      return fail(
        input,
        &records.span_range,
        "Records exceeds the 256 row/4096 cell budget",
      );
    }
    for row in &records.rows {
      if row.len() != columns.len() {
        return fail(
          input,
          &records.span_range,
          "Record row has a different number of values than its columns",
        );
      }
      for (value, column) in row.iter().zip(&columns) {
        validate_cell(value, column, enums, input)?;
      }
    }
  }
  Ok(())
}

fn record_columns<'a>(
  table: &'a TableBlock,
  records: &RecordsBlock,
  input: &str,
) -> ParserResult<Vec<&'a TableColumn>> {
  if records.columns.is_empty() {
    return Ok(table.cols.iter().collect());
  }
  let mut names = BTreeSet::new();
  let mut columns = Vec::new();
  for name in &records.columns {
    if !names.insert(&name.to_string) {
      return fail(input, &name.span_range, "Duplicate records column");
    }
    match table
      .cols
      .iter()
      .find(|c| c.name.to_string == name.to_string)
    {
      Some(column) => columns.push(column),
      None => return fail(input, &name.span_range, "Unknown records column"),
    }
  }
  Ok(columns)
}

fn validate_cell(
  value: &Literal,
  column: &TableColumn,
  enums: &[&EnumBlock],
  input: &str,
) -> ParserResult<()> {
  let raw_type = column
    .r#type
    .raw
    .split('(')
    .next()
    .unwrap_or(&column.r#type.raw)
    .trim_matches('"');
  let ty = raw_type.to_ascii_lowercase();
  let string_type = [
    "varchar",
    "char",
    "text",
    "string",
    "character",
    "character varying",
  ]
  .contains(&ty.as_str());
  if matches!(value.value, Value::Null)
    || (matches!(&value.value, Value::String(s) if s.is_empty()) && !string_type)
  {
    if column
      .settings
      .as_ref()
      .is_some_and(|s| s.is_pk || s.nullable == Some(Nullable::NotNull))
    {
      return fail(
        input,
        &value.span_range,
        "Null record value in a required column",
      );
    }
    return Ok(());
  }
  // Expressions deliberately bypass static type checking, as the DBML spec requires.
  if matches!(value.value, Value::Expr(_)) {
    return Ok(());
  }
  validate_enum(value, raw_type, enums, input)?;
  let text = value.value.to_string();
  let valid = match ty.as_str() {
    "int" | "integer" | "bigint" | "smallint" | "int2" | "int4" | "int8" | "serial"
    | "bigserial" | "smallserial" => text.is_empty() || integer_record(&text),
    "float" | "real" | "double" | "double precision" | "decimal" | "numeric" | "float4"
    | "float8" => text.is_empty() || text.parse::<f64>().is_ok_and(|n| n.is_finite()),
    "bool" | "boolean" => {
      text.is_empty()
        || ["true", "false", "y", "n", "t", "f", "1", "0"]
          .contains(&text.to_ascii_lowercase().as_str())
    }
    "date" | "time" | "timestamp" | "timestamptz" | "timetz" | "datetime" => {
      matches!(value.value, Value::String(_))
    }
    _ => true,
  };
  if !valid {
    return fail(
      input,
      &value.span_range,
      "Record literal is incompatible with its numeric/boolean column",
    );
  }
  Ok(())
}

fn integer_record(text: &str) -> bool {
  text.parse::<i64>().is_ok()
    || text.parse::<f64>().is_ok_and(|number| {
      number.is_finite()
        && (i64::MIN as f64..i64::MAX as f64).contains(&number)
        && number == (number as i64) as f64
    })
}

fn validate_enum(
  value: &Literal,
  raw_type: &str,
  enums: &[&EnumBlock],
  input: &str,
) -> ParserResult<()> {
  if let Some(enumeration) = enums.iter().find(|enumeration| {
    let name = &enumeration.ident.name.to_string;
    let qualified = format!(
      "{}.{}",
      enumeration
        .ident
        .schema
        .as_ref()
        .map_or(crate::DEFAULT_SCHEMA, |s| s.to_string.as_str()),
      name
    );
    raw_type == name || raw_type == qualified
  }) {
    let text = value.value.to_string();
    let member = if matches!(value.value, Value::Enum(_)) {
      let prefix = format!("{}.", enumeration.ident.name.to_string);
      let qualified = format!(
        "{}.{}",
        enumeration
          .ident
          .schema
          .as_ref()
          .map_or(crate::DEFAULT_SCHEMA, |s| s.to_string.as_str()),
        prefix
      );
      match text
        .strip_prefix(&prefix)
        .or_else(|| text.strip_prefix(&qualified))
      {
        Some(member) => member,
        None => {
          return fail(
            input,
            &value.span_range,
            "Enum record value names a different enum",
          );
        }
      }
    } else {
      &text
    };
    if !enumeration
      .values
      .iter()
      .any(|entry| entry.value.to_string == member)
    {
      return fail(input, &value.span_range, "Unknown enum record value");
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn partials_expand_in_order_local_wins_and_authored_blocks_survive() {
    let source = "TablePartial base [headerColor: #111111] { id int [pk] title varchar indexes { title [name: 'base'] } }\nTablePartial later [headerColor: #222222] { title text updated timestamp indexes { title [unique] } }\nTable users [headerColor: #333333] { ~base active bool ~later title varchar(20) indexes { title [name: 'local'] } }";
    let ast = crate::parse_dbml(source).unwrap();
    let table = ast.tables()[0];
    assert_eq!(
      table
        .cols
        .iter()
        .map(|c| c.name.to_string.as_str())
        .collect::<Vec<_>>(),
      ["id", "active", "updated", "title"]
    );
    assert_eq!(table.cols[3].r#type.raw, "varchar(20)");
    assert_eq!(
      table.settings.as_ref().unwrap().attributes[0]
        .value
        .as_ref()
        .unwrap()
        .raw,
      "#333333"
    );
    assert_eq!(
      table.indexes.as_ref().unwrap().defs[0]
        .settings
        .as_ref()
        .unwrap()
        .name
        .as_deref(),
      Some("local")
    );
    assert!(
      matches!(&ast.authored_blocks[2], TopLevelBlock::Table(t) if t.cols.len() == 2 && t.partials.len() == 2)
    );
    assert_eq!(ast.input, source);
    let without_local =
      crate::parse_dbml("TablePartial a { x int } TablePartial b { x text } Table t { ~a ~b }")
        .unwrap();
    assert_eq!(without_local.tables()[0].cols[0].r#type.raw, "text");
  }

  #[test]
  fn csv_records_keep_quotes_escapes_empty_cells_and_expressions() {
    let source = "Enum Status { active inactive }\nTablePartial base { id int [pk] }\nTable users {\n ~base\n title varchar\n age int\n enabled boolean\n status Status\n created timestamp\n records [example] {\n 1, 'Alice, O\\'Brien', , 'Y', Status.active, '2024-01-15' // first\n 2, '', -10, FALSE, 'inactive', `now()`\n }\n}";
    let ast = crate::parse_dbml(source).unwrap();
    let record = &ast.tables()[0].records[0];
    assert!(record.example);
    assert_eq!(record.rows.len(), 2);
    assert_eq!(
      record.rows[0][1].value,
      Value::String("Alice, O'Brien".into())
    );
    assert_eq!(record.rows[0][2].value, Value::Null);
    assert_eq!(record.rows[1][2].value, Value::Integer(-10));
    assert_eq!(record.rows[1][5].value, Value::Expr("now()".into()));
    assert!(record.rows[0][1].raw.contains("\\'"));
  }

  #[test]
  fn record_rows_preserve_boundary_cells_crlf_and_literal_whitespace() {
    let source = "Table t {\r\n a int\r\n b int\r\n text varchar\r\n records {\r\n , , ' leading // text ' // comment\r\n 1e2, /* value comment */ '2.0', ''\r\n }\r\n}";
    let ast = crate::parse_dbml(source).unwrap();
    let rows = &ast.tables()[0].records[0].rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0].value, Value::Null);
    assert_eq!(rows[0][1].value, Value::Null);
    assert_eq!(rows[0][2].value, Value::String(" leading // text ".into()));
    assert_eq!(rows[1][1].value, Value::String("2.0".into()));
    assert_eq!(rows[1][2].value, Value::String(String::new()));
    let source =
      "Enum State { \"in.active\" } Table t { state State records {\n State.\"in.active\"\n } }";
    assert!(crate::parse_dbml(source).is_ok());
  }

  #[test]
  fn explicit_record_targets_and_all_optional_operators_keep_endpoints() {
    let source =
      "Table core.users as U { id int [pk] name varchar }\nrecords U(name,id) {\n 'a', 1\n}\n";
    let ast = crate::parse_dbml(source).unwrap();
    assert_eq!(ast.tables()[0].records[0].columns[0].to_string, "name");
    for operator in ["?->", "??>", ">??"] {
      let source =
        format!("Table a {{ id int [pk] }} Table b {{ id int [pk] }} Ref: a.id {operator} b.id");
      assert!(crate::parse_dbml(&source).is_err());
    }
    for op in ["<", ">", "-", "<>"] {
      for (left, right) in [(false, true), (true, false), (true, true)] {
        let operator = format!(
          "{}{op}{}",
          if left { "?" } else { "" },
          if right { "?" } else { "" }
        );
        let source =
          format!("Table a {{ id int [pk] }} Table b {{ id int [pk] }} Ref: a.id {operator} b.id");
        let ast = crate::parse_dbml(&source).unwrap();
        assert_eq!(
          (ast.refs()[0].lhs_optional, ast.refs()[0].rhs_optional),
          (left, right)
        );
      }
    }
  }

  #[test]
  fn invalid_partials_records_and_denied_imports_are_explicit_errors() {
    for source in [
      "Table users { ~missing }",
      "TablePartial a { id int } TablePartial a { x text } Table users { ~a }",
      "TablePartial a { ~a } Table users { ~a }",
      "TablePartial a { x int } Table users { ~a ~a }",
      "TablePartial a { x int x text } Table users { ~a }",
      "Table users { id int records {\n 1\n } records {\n 2\n } }",
      "Table users { id int } records users(id) {\n 1\n } records users(id) {\n 2\n }",
      "Table users { id int } records users {\n 1\n }",
      "Table users { id int } records users(missing) {\n 1\n }",
      "Table users { id int } records users(id,id) {\n 1,2\n }",
      "Table users { id int name text records {\n 1\n } }",
      "Table users { id int records {\n 'bad integer'\n } }",
      "Table users { id int [not null] records {\n ''\n } }",
      "Table users { active bool records {\n 'maybe'\n } }",
      "enum Status { active } Table users { state Status records {\n 'unknown'\n } }",
      "Table users { id int records [invalid] {\n 1\n } }",
      "import { users } from '/etc/passwd'",
    ] {
      assert!(crate::parse_dbml(source).is_err(), "{source}");
    }
  }
}
