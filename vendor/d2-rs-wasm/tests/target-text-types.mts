// SPDX-License-Identifier: MPL-2.0
// D2 v0.9.0 SDK Text/Class/SQLTable declarations and nested SQL member access.
import type { Text, Class, SQLTable, SQLColumn, CompileResponse, D2 } from "../src/index.js";
declare const text: Text;
declare const klass: Class;
declare const table: SQLTable;
declare const column: SQLColumn;
const labels: string[] = [text.label, text.fontFamily, text.language, text.color, column.name.label, column.type.label];
const measurements: number[] = [text.fontSize, text.labelWidth, text.labelHeight, column.name.fontSize, column.type.labelWidth, column.name.labelHeight];
const flags: boolean[] = [text.bold, text.italic, text.underline, column.name.underline, column.type.italic];
const optionalFill: string | undefined = text.labelFill;
const columns: SQLColumn[] | null = table.columns;
const fields: string[] = (klass.fields ?? []).map(field => field.name + field.type + field.visibility);
const methods: string[] = (klass.methods ?? []).map(method => method.name + method.return + method.visibility);
// @ts-expect-error Source text fontSize is numeric.
const wrongTextSize: string = column.name.fontSize;
// @ts-expect-error Source text underline is boolean.
const wrongUnderline: string = column.type.underline;
void [labels, measurements, flags, optionalFill, columns, fields, methods, wrongTextSize, wrongUnderline];

// Go compile responses publish resolved defaults and nullable pointer fields.
declare const compiled: CompileResponse;
declare const d2: D2;
const resolvedPad: number = compiled.renderOptions.pad;
const resolvedTheme: number = compiled.renderOptions.themeID;
const nullableDarkTheme: number | null = compiled.renderOptions.darkThemeID;
const optionalColor: string | null | undefined = compiled.renderOptions.themeOverrides?.n1;
const svg: Promise<string> = d2.render(compiled.diagram, compiled.renderOptions);
// @ts-expect-error A nullable source pointer cannot be used as a definite ID.
const definiteDarkTheme: number = compiled.renderOptions.darkThemeID;
// @ts-expect-error Compile-only layout is absent from returned render options.
const returnedLayout = compiled.renderOptions.layout;
void [resolvedPad, resolvedTheme, nullableDarkTheme, optionalColor, svg, definiteDarkTheme, returnedLayout];

// Public Graph legends use compiled object/edge records, not target Shapes.
const graphLegendLabel: string | undefined = compiled.graph.legend?.label;
const graphLegendIDs: string[] = (compiled.graph.legend?.objects ?? []).map(object => object.id);
const graphLegendRoutes = (compiled.graph.legend?.edges ?? []).map(edge => edge.route);
const graphData: unknown = compiled.graph.data?.["items"];
// @ts-expect-error A compiled legend object has box metadata, not target pos.
const graphLegendTargetPosition = compiled.graph.legend?.objects?.[0]?.pos;
void [graphLegendLabel, graphLegendIDs, graphLegendRoutes, graphData, graphLegendTargetPosition];
