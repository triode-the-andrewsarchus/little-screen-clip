//! Native DBML parsing and field-port layout without database or SQL execution.
use super::graphviz::label;
use dbml_rs::ast::*;
use std::{collections::HashMap, fmt::Write};

struct Tables<'a> {
    nodes: Vec<&'a TableBlock>,
    names: HashMap<(String, String), usize>,
    aliases: HashMap<String, usize>,
}
impl<'a> Tables<'a> {
    fn new(nodes: Vec<&'a TableBlock>) -> Self {
        let mut names = HashMap::new();
        let mut aliases = HashMap::new();
        for (index, table) in nodes.iter().enumerate() {
            names.insert(
                (
                    schema(&table.ident.schema).into(),
                    table.ident.name.to_string.clone(),
                ),
                index,
            );
            if let Some(alias) = &table.ident.alias {
                aliases.insert(alias.to_string.clone(), index);
            }
        }
        Self {
            nodes,
            names,
            aliases,
        }
    }
    fn endpoint(&self, endpoint: &RefIdent) -> Result<Vec<(usize, usize)>, String> {
        let name = &endpoint.table.to_string;
        let index = endpoint
            .schema
            .is_none()
            .then(|| self.aliases.get(name))
            .flatten()
            .or_else(|| {
                self.names
                    .get(&(schema(&endpoint.schema).into(), name.clone()))
            })
            .copied()
            .ok_or_else(|| format!("Unknown DBML table: {name}"))?;
        endpoint
            .compositions
            .iter()
            .map(|column| {
                self.nodes[index]
                    .cols
                    .iter()
                    .position(|c| c.name.to_string == column.to_string)
                    .map(|column| (index, column))
                    .ok_or_else(|| format!("Unknown DBML column: {name}.{}", column.to_string))
            })
            .collect()
    }
}
fn schema(schema: &Option<Ident>) -> &str {
    schema
        .as_ref()
        .map_or(dbml_rs::DEFAULT_SCHEMA, |s| &s.to_string)
}

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let source = source.trim_start_matches('\u{feff}');
    let ast = dbml_rs::parse_dbml(source).map_err(|e| format!("DBML: {e}"))?;
    let tables = Tables::new(ast.tables());
    let enums = ast.enums();
    if tables.nodes.is_empty() && enums.is_empty() {
        return Err("DBML preview requires a table or enum".into());
    }
    if tables.nodes.len() + enums.len() > 128
        || tables.nodes.iter().map(|t| t.cols.len()).sum::<usize>()
            + enums.iter().map(|e| e.values.len()).sum::<usize>()
            > 2048
    {
        return Err("DBML exceeds the table or field budget".into());
    }
    let dot = dot(&ast, &tables)?;
    let (svg, mut warnings) = super::graphviz::svg(&dot)?;
    if ast.blocks.iter().any(|b| {
        matches!(
            b,
            TopLevelBlock::Project(_) | TopLevelBlock::TableGroup(_) | TopLevelBlock::Note(_)
        )
    }) {
        warnings.push("Project metadata, table groups and free-standing notes remain available in source; this preview shows tables, enums and column relationships.".into());
    }
    if tables.nodes.iter().any(|t| t.settings.is_some())
        || ast
            .refs()
            .iter()
            .any(|r| r.name.is_some() || r.settings.is_some())
    {
        warnings.push("Authored presentation settings, relationship names and referential actions remain available in source; edge symbols show maximum one/many cardinality, not minimum participation.".into());
    }
    Ok((svg, warnings))
}

fn dot(ast: &SchemaBlock<'_>, tables: &Tables<'_>) -> Result<String, String> {
    let mut dot = String::from(
        "digraph DBML { graph [rankdir=LR, bgcolor=\"white\", nodesep=0.4, ranksep=0.8]; node [shape=plain, fontname=\"Helvetica\"]; edge [color=\"#506879\", arrowsize=0.7, fontname=\"Helvetica\", fontsize=10];\n",
    );
    for (index, table) in tables.nodes.iter().enumerate() {
        render_table(&mut dot, index, table);
    }
    for (index, enumeration) in ast.enums().iter().enumerate() {
        write!(dot, "e{index} [id=\"dbml-enum-{index}\" label=<").unwrap();
        header(
            &mut dot,
            &format!(
                "{}.{}",
                schema(&enumeration.ident.schema),
                enumeration.ident.name.to_string
            ),
            "ENUM",
        );
        for value in &enumeration.values {
            note(&mut dot, &value.value.to_string);
        }
        dot.push_str("</TABLE>>];\n");
    }
    let mut count = 0;
    for relation in ast.refs() {
        links(
            &mut dot,
            tables,
            &relation.lhs,
            &relation.rhs,
            &relation.rel,
            &mut count,
        )?;
    }
    for table in &tables.nodes {
        for field in &table.cols {
            if let Some(settings) = &field.settings {
                let lhs = RefIdent {
                    // An inline source is the concrete table, never an alias
                    // with the same spelling in another schema.
                    schema: Some(Ident {
                        to_string: schema(&table.ident.schema).into(),
                        ..Default::default()
                    }),
                    table: table.ident.name.clone(),
                    compositions: vec![field.name.clone()],
                    ..Default::default()
                };
                for reference in &settings.refs {
                    links(
                        &mut dot,
                        tables,
                        &lhs,
                        &reference.rhs,
                        &reference.rel,
                        &mut count,
                    )?;
                }
            }
        }
    }
    dot.push_str("}\n");
    if dot.len() > 1024 * 1024 {
        return Err("DBML generated diagram exceeds the byte budget".into());
    }
    Ok(dot)
}
fn render_table(dot: &mut String, index: usize, table: &TableBlock) {
    write!(dot, "t{index} [id=\"dbml-table-{index}\" label=<").unwrap();
    header(
        dot,
        &format!(
            "{}.{}",
            schema(&table.ident.schema),
            table.ident.name.to_string
        ),
        "TABLE",
    );
    for (column, field) in table.cols.iter().enumerate() {
        let settings = field.settings.as_ref();
        let flags = [
            (settings.is_some_and(|s| s.is_pk), "PK"),
            (settings.is_some_and(|s| s.is_unique), "UQ"),
            (
                settings.is_some_and(|s| s.nullable == Some(Nullable::NotNull)),
                "NN",
            ),
            (settings.is_some_and(|s| s.is_incremental), "AUTO"),
        ]
        .into_iter()
        .filter_map(|(yes, flag)| yes.then_some(flag))
        .collect::<Vec<_>>()
        .join(" ");
        write!(dot, "<TR><TD PORT=\"c{column}\" ALIGN=\"LEFT\">{}</TD><TD ALIGN=\"LEFT\">{}</TD><TD PORT=\"r{column}\" ALIGN=\"LEFT\">{}</TD></TR>", label(&field.name.to_string), label(&field.r#type.raw), label(&flags)).unwrap();
        if let Some(default) = settings.and_then(|s| s.default.as_ref()) {
            note(dot, &format!("default: {}", default.to_string()));
        }
        if let Some(text) = settings.and_then(|s| s.note.as_ref()) {
            note(dot, text);
        }
    }
    if let Some(indexes) = &table.indexes {
        for index in &indexes.defs {
            let prefix = if index.settings.as_ref().is_some_and(|s| s.is_pk) {
                "PK"
            } else if index.settings.as_ref().is_some_and(|s| s.is_unique) {
                "UNIQUE"
            } else {
                "INDEX"
            };
            let columns = index
                .cols
                .iter()
                .map(|column| match column {
                    IndexesColumnType::String(c) => c.to_string.clone(),
                    IndexesColumnType::Expr(e) => e.raw.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            note(dot, &format!("{prefix} ({columns})"));
        }
    }
    if let Some(text) = &table.note {
        note(dot, &text.value.value.to_string());
    }
    dot.push_str("</TABLE>>];\n");
}
fn header(dot: &mut String, title: &str, kind: &str) {
    write!(dot, "<TABLE BORDER=\"1\" CELLBORDER=\"0\" CELLSPACING=\"0\" CELLPADDING=\"6\" BGCOLOR=\"#f4f7fa\"><TR><TD COLSPAN=\"3\" BGCOLOR=\"#22384d\"><FONT COLOR=\"white\"><B>{}</B> | {kind}</FONT></TD></TR>", label(title)).unwrap();
}
fn note(dot: &mut String, text: &str) {
    // Keep an empty value from becoming a literal empty <text> node upstream.
    if !text.is_empty() {
        write!(
            dot,
            "<TR><TD COLSPAN=\"3\" ALIGN=\"LEFT\">{}</TD></TR>",
            label(text)
        )
        .unwrap();
    }
}
fn links(
    dot: &mut String,
    tables: &Tables<'_>,
    left: &RefIdent,
    right: &RefIdent,
    relation: &Relation,
    count: &mut usize,
) -> Result<(), String> {
    let left = tables.endpoint(left)?;
    let right = tables.endpoint(right)?;
    if left.len() != right.len() || left.is_empty() {
        return Err("DBML relationship has unequal or empty composite endpoints".into());
    }
    let (tail, head) = match relation {
        Relation::One2One => ("tee", "tee"),
        Relation::One2Many => ("tee", "crow"),
        Relation::Many2One => ("crow", "tee"),
        Relation::Many2Many => ("crow", "crow"),
        Relation::Undef => return Err("Unknown DBML relationship cardinality".into()),
    };
    // Ports on opposite outer cells keep edges outside the type/flags text;
    // a port on the name cell alone would draw through the rest of the row.
    let composite = left.len() > 1;
    for ((a, ac), (b, bc)) in left.into_iter().zip(right) {
        *count += 1;
        if *count > 2048 {
            return Err("DBML exceeds the relationship budget".into());
        }
        writeln!(dot, "t{a}:r{ac}:e -> t{b}:c{bc}:w [id=\"dbml-ref-{count}\", dir=both, arrowtail={tail}, arrowhead={head}, label=\"{}\"] ;", if composite { "composite" } else { "" }).unwrap();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "Table core.users as U {\n id int [pk]\n name varchar(50) [not null]\n}\nTable posts {\n id int [pk]\n user_id int [ref: > U.id]\n title varchar [default: 'draft']\n}\n";
    #[test]
    fn native_tables_fields_and_alias_references_use_the_actual_column_ports() {
        let ast = dbml_rs::parse_dbml(SOURCE).unwrap();
        let tables = Tables::new(ast.tables());
        let dot = dot(&ast, &tables).unwrap();
        assert!(dot.contains("core.users"));
        assert!(dot.contains("t1:r1:e -> t0:c0:w"));
        assert!(dot.contains("arrowtail=crow, arrowhead=tee"));
        assert!(dot.contains("default: draft"));
        let (svg, warnings) = svg(SOURCE).unwrap();
        assert!(svg.contains("dbml-table-0"));
        assert!(svg.contains("dbml-ref-1"));
        assert!(svg.contains("<path"));
        assert!(!svg.contains("<text"));
        assert!(warnings.is_empty(), "{warnings:?}");
        let tree = super::super::retained::prepare(&svg).unwrap().tree;
        let posts = tree
            .node_by_id("graph-0-dbml-table-1")
            .unwrap()
            .abs_bounding_box();
        let users = tree
            .node_by_id("graph-0-dbml-table-0")
            .unwrap()
            .abs_bounding_box();
        let edge = tree
            .node_by_id("graph-0-dbml-ref-1")
            .unwrap()
            .abs_bounding_box();
        assert!(
            edge.left() >= posts.right() - 2.0,
            "relationship must leave outside the field text"
        );
        assert!(
            edge.right() <= users.left() + 2.0,
            "relationship must enter at the outer name edge"
        );
    }
    #[test]
    fn composite_keys_enums_comments_and_quoted_identifiers_preserve_relationships() {
        let source = "Enum status { draft published }\nTable \"parent table\" {\n a int\n b int\n indexes { (a,b) [pk] }\n}\nTable children {\n a int\n b int\n state status\n}\nRef: children.(a,b) > \"parent table\".(a,b)\n";
        let ast = dbml_rs::parse_dbml(source).unwrap();
        let dot = dot(&ast, &Tables::new(ast.tables())).unwrap();
        assert!(dot.contains("PK (a, b)"));
        assert!(dot.contains("t1:r0:e -> t0:c0:w"));
        assert!(dot.contains("t1:r1:e -> t0:c1:w"));
        assert!(dot.contains("composite"));
        assert!(svg(source).is_ok());
    }
    #[test]
    fn all_four_maximum_cardinalities_keep_their_authored_direction() {
        for (operator, tail, head) in [
            ("<", "tee", "crow"),
            (">", "crow", "tee"),
            ("-", "tee", "tee"),
            ("<>", "crow", "crow"),
        ] {
            let source = format!(
                "Table a {{ id int [pk] }}\nTable b {{ id int [pk] }}\nRef: a.id {operator} b.id"
            );
            let ast = dbml_rs::parse_dbml(&source).unwrap();
            let dot = dot(&ast, &Tables::new(ast.tables())).unwrap();
            assert!(dot.contains(&format!("arrowtail={tail}, arrowhead={head}")));
            assert!(svg(&source).is_ok());
        }
    }
    #[test]
    fn unpainted_presentation_and_context_metadata_have_visible_limitations() {
        let source = "Project example { database_type: 'PostgreSQL' }\nTable users [headerColor: #123456] { id int [pk] }\nTableGroup group_a { users }\nNote: 'Context remains in source'";
        let (_, warnings) = svg(source).unwrap();
        assert!(warnings.iter().any(|w| w.contains("table groups")));
        assert!(warnings.iter().any(|w| w.contains("presentation settings")));
    }

    #[test]
    fn invalid_or_unsupported_schema_never_becomes_a_silently_partial_preview() {
        for source in [
            "Table broken {",
            "Table users { id int }\nRef: users.id > missing.id",
            "Table users { id int id text }",
            "TablePartial base { id int }\nTable users { ~base }",
            "Table users { id int }\nRecords users(id) { 1 }",
        ] {
            assert!(svg(source).is_err(), "{source}");
        }
        let source = (0..129)
            .map(|i| format!("Table t{i} {{ id int }}\n"))
            .collect::<String>();
        assert!(svg(&source).unwrap_err().contains("budget"));
    }
    #[test]
    fn expressions_and_markup_in_names_or_notes_are_never_executed() {
        let source = "Table \"<IMG SRC='/etc/passwd'>\" {\n id int [pk, default: `system('no')`]\n Note: '<script>alert(1)</script>'\n}";
        let (svg, _) = svg(source).unwrap();
        assert!(!svg.contains("<script"));
        assert!(!svg.contains("<image"));
    }
}
