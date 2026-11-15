//! Bounded direct geometry for live pressure ink, sharing exported freehand paths.
use super::{freedraw, rough::GenerationBudget};
use crate::normalization::NormalizedFreedraw;

/// Native path commands for an already filled pressure stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InkCommand {
    /// Begins an outline contour.
    Move([f64; 2]),
    /// Adds a straight outline edge.
    Line([f64; 2]),
    /// Adds a quadratic outline segment.
    Quadratic {
        /// Quadratic control point.
        control: [f64; 2],
        /// Segment endpoint.
        end: [f64; 2],
    },
    /// Closes the current contour.
    Close,
}

/// A pressure-aware outline without source parsing, font shaping or asset decoding.
#[derive(Clone, Debug, PartialEq)]
pub struct InkPath {
    /// Ordered commands to fill with the stroke color.
    pub commands: Vec<InkCommand>,
}
impl InkPath {
    /// Builds the same outline used by freehand export from local x/y/pressure triples.
    ///
    /// Bounded to 4096 points for interactive use. Coordinates must be finite and
    /// within one million units, pressure normalized to 0..=1, width 0.1..=32.
    /// No simulated pressure is substituted for supplied hardware measurements.
    ///
    /// # Errors
    /// Returns an error for invalid input or exhausted geometry/work budgets.
    pub fn prepare(points: &[[f64; 3]], width: f64) -> Result<Self, String> {
        if !(2..=4096).contains(&points.len())
            || !width.is_finite()
            || !(0.1..=32.0).contains(&width)
            || points.iter().any(|p| {
                !p.iter().all(|v| v.is_finite())
                    || p[0].abs() > 1e6
                    || p[1].abs() > 1e6
                    || !(0.0..=1.0).contains(&p[2])
            })
        {
            return Err("Invalid interactive ink geometry or pressure".into());
        }
        let freedraw = NormalizedFreedraw {
            points: points.iter().map(|p| [p[0], p[1]]).collect(),
            pressures: points.iter().map(|p| p[2]).collect(),
            simulate_pressure: false,
            last_committed_point: None,
            geometry_available: true,
        };
        let mut budget = GenerationBudget::new(65_536, 8_000_000, 0);
        let path = freedraw::generate(&freedraw, width, &mut budget)
            .map_err(|error| format!("Interactive ink generation failed: {error:?}"))?;
        use freedraw::PathOperation;
        Ok(Self {
            commands: path
                .operations
                .into_iter()
                .map(|op| match op {
                    PathOperation::Move(p) => InkCommand::Move([p.x, p.y]),
                    PathOperation::Line(p) => InkCommand::Line([p.x, p.y]),
                    PathOperation::Quadratic { control, end } => InkCommand::Quadratic {
                        control: [control.x, control.y],
                        end: [end.x, end.y],
                    },
                    PathOperation::Close => InkCommand::Close,
                })
                .collect(),
        })
    }
}
