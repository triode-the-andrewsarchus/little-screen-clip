// SPDX-License-Identifier: MPL-2.0
//! latex.go: bounded MathJax SVG label import and intrinsic placement.

use d2_rs_scene::{
    AspectAlign, AspectFit, AspectRatio, Box as SceneBox, Image, Node, Nrgba, Paint, Primitive,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub(crate) struct LatexAsset {
    pub id: String,
    pub width: f64,
    pub height: f64,
}
use d2_rs_target::Shape;

use crate::golib::geo;
use crate::golib::label::{self, Position};
use crate::golib::shape::Shape as _;
use crate::util::*;
use crate::{Builder, target_geometry};

const MAX_INPUT_BYTES: usize = 4 << 10;
const MAX_GROUP_NESTING_DEPTH: usize = 128;

/// d2latex.ValidateInput.
fn validate_input(s: &str) -> std::result::Result<(), String> {
    if s.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "latex input is {} bytes, exceeding limit {MAX_INPUT_BYTES}",
            s.len()
        ));
    }
    let (mut depth, mut backslashes, mut in_comment) = (0usize, 0usize, false);
    for &c in s.as_bytes() {
        if in_comment {
            if c == b'\n' || c == b'\r' {
                in_comment = false;
            }
            continue;
        }
        if c == b'\\' {
            backslashes += 1;
            continue;
        }
        let escaped = backslashes % 2 != 0;
        backslashes = 0;
        if escaped {
            continue;
        }
        match c {
            b'%' => in_comment = true,
            b'{' => {
                depth += 1;
                if depth > MAX_GROUP_NESTING_DEPTH {
                    return Err(format!(
                        "latex group nesting depth {depth} exceeds limit {MAX_GROUP_NESTING_DEPTH}"
                    ));
                }
            }
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_latex_input(object: &str, formula: &str) -> Result<()> {
    validate_input(formula).map_err(|e| Error(format!("scene: {object} {e}")))
}

/// latexShapeLabelTopLeft: border labels use the inner box.
pub(crate) fn latex_shape_label_top_left(s: &Shape) -> Option<geo::Point> {
    let mut position = Position::from_string(&s.label_position);
    if position == Position::Unset {
        position = Position::InsideMiddleCenter;
    }
    let geometry = target_geometry(s);
    let b = if position.is_outside() {
        geometry.get_box()
    } else {
        geometry.get_inner_box()
    };
    Some(position.get_point_on_box(
        &b,
        label::PADDING,
        s.text.label_width as f64,
        s.text.label_height as f64,
    ))
}

impl Builder<'_> {
    pub(crate) fn validate_latex_import_options(&self, object: &str) -> Result<()> {
        let Some(assets) = &self.options.assets else {
            return Err(unsupported(
                object,
                "label language latex without configured SVG import limits",
            ));
        };
        let l = assets.svg_import_limits;
        if [
            l.max_bytes,
            l.max_depth,
            l.max_elements,
            l.max_attributes,
            l.max_attribute_bytes,
            l.max_path_commands,
            l.max_transform_functions,
            l.max_use_depth,
            l.max_resources,
        ]
        .iter()
        .any(|v| *v <= 0)
        {
            return err(format!(
                "scene: {object} latex label requires positive per-import SVG limits"
            ));
        }
        let b = assets.svg_import_budget;
        if [
            b.max_source_bytes,
            b.max_elements,
            b.max_attributes,
            b.max_attribute_bytes,
            b.max_path_commands,
            b.max_transform_functions,
            b.max_declared_resources,
            b.max_expanded_use_instances,
        ]
        .iter()
        .any(|v| *v <= 0)
        {
            return err(format!(
                "scene: {object} latex label requires positive document-wide SVG import budgets"
            ));
        }
        Ok(())
    }

    /// resolveLatexAsset: validate before cache lookup, then charge the same
    /// document budget used by image assets before publishing the vector.
    pub(crate) fn resolve_latex_asset(
        &mut self,
        object: &str,
        formula: &str,
        raw_color: &str,
    ) -> Result<LatexAsset> {
        validate_latex_input(object, formula)?;
        self.validate_latex_import_options(object)?;
        let current_color = if raw_color.is_empty() {
            Nrgba::new(0, 0, 0, 255)
        } else {
            match self.paint(raw_color, &format!("{object} latex currentColor"))? {
                None => Nrgba::new(0, 0, 0, 255),
                Some(Paint::SolidPaint { color }) => color,
                Some(_) => {
                    return err(format!(
                        "scene: {object} latex currentColor must resolve to a solid color"
                    ));
                }
            }
        };
        let key = (formula.to_owned(), current_color);
        if let Some(asset) = self.latex_assets.get(&key) {
            return Ok(asset.clone());
        }
        let source = d2_rs_math::render_exact(formula)
            .map_err(|e| Error(format!("scene: {object} latex render: {e}")))?;
        let color = [
            current_color.r,
            current_color.g,
            current_color.b,
            current_color.a,
        ];
        let imported = self.import_svg(
            object,
            &format!("{object} latex"),
            source.as_bytes(),
            Some(color),
            true,
        )?;
        let mut digest = Sha256::new();
        digest.update(b"d2-raster-latex\0");
        digest.update(source.as_bytes());
        digest.update(color);
        let id = format!("latex:{:x}", digest.finalize());
        let asset = LatexAsset {
            id: id.clone(),
            width: imported.viewport.width,
            height: imported.viewport.height,
        };
        self.assets
            .insert(id.clone(), crate::svgimport::vector_asset(&id, imported));
        self.latex_assets.insert(key, asset.clone());
        Ok(asset)
    }

    pub(crate) fn build_latex_label_node(
        &mut self,
        object: &str,
        node_id: &str,
        formula: &str,
        raw_color: &str,
        top_left: Option<geo::Point>,
    ) -> Result<Node> {
        if top_left.is_none_or(|p| !finite(p.x) || !finite(p.y)) {
            return Err(invalid_field(
                object,
                "labelPosition",
                None,
                "must resolve to a finite LaTeX label origin",
            ));
        }
        let asset = self.resolve_latex_asset(object, formula, raw_color)?;
        let top_left = top_left.expect("validated origin");
        Ok(node(
            node_id,
            Some(Primitive::Image(Image {
                asset: asset.id,
                r#box: SceneBox {
                    x: top_left.x,
                    y: top_left.y,
                    width: asset.width,
                    height: asset.height,
                },
                aspect: AspectRatio {
                    align: AspectAlign::X_MID_Y_MID,
                    fit: AspectFit::MEET,
                },
            })),
        ))
    }
}
