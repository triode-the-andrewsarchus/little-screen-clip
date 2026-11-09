//! Bounded, read-only program-coordinate toolpaths. Never sends commands to a machine.
mod arc;
mod parser;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_MOVES: usize = 200_000;
/// A linear drawing interval, including bounded tessellation of circular moves.
#[derive(Clone, Debug)]
pub struct Move {
    pub from: [f64; 3],
    pub to: [f64; 3],
    /// One-based source line, also retained for tessellated arcs.
    pub line: usize,
    pub rapid: bool,
    pub extruding: bool,
    pub feed_mm_min: Option<f64>,
}
/// Program coordinates and original source, with explicit partial-preview diagnostics.
pub struct Document {
    pub source: String,
    pub moves: Vec<Move>,
    pub notes: Vec<String>,
    pub bounds: [[f64; 3]; 2],
    pub length_mm: f64,
}
impl Document {
    /// Reads common linear and XY circular G-code moves, in millimetres or inches.
    ///
    /// # Errors
    /// Rejects oversized or non-UTF-8 input. Unsupported or malformed instructions stop
    /// the preview at their source line, preserving prior moves and a diagnostic.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("G-code exceeds the 16 MiB opening limit".into());
        }
        let source = std::str::from_utf8(bytes)
            .map_err(|_| "G-code must be UTF-8 text")?
            .to_owned();
        let mut document=Self {source,moves:Vec::new(),notes:vec!["Program coordinates only; initial XYZ/E origin is assumed zero. No machine commands are executed.".into()],bounds:[[0.0;3];2],length_mm:0.0};
        let mut state = parser::State::default();
        for (line, source) in document.source.lines().enumerate() {
            if line >= 500_000 {
                document
                    .notes
                    .push("Preview stops at the 500,000-line limit.".into());
                break;
            }
            match state.line(source, line + 1, &mut document.moves) {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    document
                        .notes
                        .push(format!("Preview stops before line {}: {error}", line + 1));
                    break;
                }
            }
        }
        if state.homed {
            document.notes.push("Homing moves are omitted; the following move establishes a new program-coordinate anchor.".into());
        }
        document.measure();
        Ok(document)
    }
    fn measure(&mut self) {
        if self.moves.is_empty() {
            return;
        }
        self.bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for movement in &self.moves {
            let mut length = 0.0;
            for axis in 0..3 {
                self.bounds[0][axis] = self.bounds[0][axis]
                    .min(movement.from[axis])
                    .min(movement.to[axis]);
                self.bounds[1][axis] = self.bounds[1][axis]
                    .max(movement.from[axis])
                    .max(movement.to[axis]);
                length += (movement.to[axis] - movement.from[axis]).powi(2);
            }
            self.length_mm += length.sqrt();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(source: &str) -> Document {
        Document::parse(source.as_bytes()).unwrap()
    }
    #[test]
    fn printer_fixture_preserves_source_arcs_extrusion_and_end_of_program() {
        let doc = Document::parse(include_bytes!("../tests/fixtures/gcode/frame.gcode")).unwrap();
        assert_eq!(doc.moves.len(), 55);
        assert_eq!(doc.notes.len(), 1);
        assert_eq!(doc.moves.last().unwrap().line, 15);
        assert_eq!(doc.moves.last().unwrap().to, [5.0, 5.0, 0.4]);
        assert_eq!(doc.bounds[1], [45.0, 35.0, 0.4]);
        assert_eq!(doc.moves.iter().filter(|m| m.line == 12).count(), 45);
        assert!(
            doc.moves
                .iter()
                .filter(|m| m.line == 12)
                .all(|m| m.extruding && m.feed_mm_min == Some(1200.0))
        );
        assert!(doc.length_mm > 250.0);
    }
    #[test]
    fn relative_units_resets_and_feed_apply_to_the_whole_block() {
        let doc =
            parse("G1X10Y0F600\nG92X0E0\nG1X2E1\nG91\nM83\nG1X3E1\nF10 G20 G1X1\nG90\nG21G1X0\n");
        assert_eq!(doc.notes.len(), 1);
        let ends: Vec<_> = doc.moves.iter().map(|m| m.to[0]).collect();
        assert_eq!(ends, vec![10.0, 12.0, 15.0, 40.4, 10.0]);
        assert_eq!(doc.moves[3].feed_mm_min, Some(254.0));
        assert!(doc.moves[1].extruding && doc.moves[2].extruding);
    }
    #[test]
    fn radius_arcs_full_circles_and_helices_keep_real_endpoints() {
        for instruction in ["G3X10Y10R10", "G2X10Y10R-10", "G3X0Y0I10J0Z4"] {
            let doc = parse(instruction);
            assert_eq!(doc.notes.len(), 1, "{:?}", doc.notes);
            assert!(doc.moves.len() >= 45);
            assert!(doc.moves.iter().all(|m| m.line == 1));
            assert!(doc.bounds.iter().flatten().all(|v| v.is_finite()));
        }
        let doc = parse("G0X10\nG3X10I-10Z2");
        assert_eq!(doc.moves.len(), 181);
        assert_eq!(doc.moves.last().unwrap().to, [10.0, 0.0, 2.0]);
    }
    #[test]
    fn unsupported_or_bad_blocks_stop_without_publishing_partial_block_moves() {
        for bad in [
            "G18",
            "G1X#1",
            "M98P1",
            "G1XNaN",
            "G1X3X4",
            "G3X10Y10R1",
            "G3X20I1",
            "G1A30",
            "M206X2",
            "G1F0X2",
            "(broken",
            "G1X1.2.3",
        ] {
            let doc = parse(&format!("G1X1\n{bad}\nG1X999"));
            assert_eq!(doc.moves.len(), 1, "{bad}");
            assert!(
                doc.notes.iter().any(|n| n.contains("before line 2")),
                "{bad}"
            );
        }
        let doc = parse("G1X1\nG28\nG0X5Y5Z1\nG1X10\nM2");
        assert_eq!(doc.moves.len(), 2);
        assert_eq!(doc.moves[1].from, [5.0, 5.0, 1.0]);
        assert!(doc.notes.iter().any(|n| n.contains("Homing")));
        assert!(Document::parse(&[0xff]).is_err());
        assert!(Document::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
}
