//! The graphic state, as pgf and TikZ keep it: saved when a TeX group starts and put back when it ends.

use std::collections::HashMap;
use std::rc::Rc;

use crate::canvas::{Cap, Join};
use crate::color::Color;
use crate::scene::{Point, Transform};
use crate::tex::Token;
use crate::PT;

/// The topaths library's settings for `to` and `edge`: `out`, `in`, `bend left`, `looseness`,
/// `min distance`, `controls`, `to path`, `loop`.
#[derive(Debug, Clone)]
pub struct ToSettings {
    /// a curve (`\tikz@to@switch@on`) rather than a line
    pub curve: bool,
    /// `out` and `in` taken from the line between the ends (`bend left`, `relative`)
    pub relative: bool,
    pub bend: f64,
    pub out: f64,
    pub inn: f64,
    pub out_looseness: f64,
    pub in_looseness: f64,
    pub out_min: f64,
    pub out_max: f64,
    pub in_min: f64,
    pub in_max: f64,
    pub out_control: Option<Vec<Token>>,
    pub in_control: Option<Vec<Token>>,
    /// `to path={…}` of the user: run with `\tikztostart`, `\tikztotarget`, `\tikztonodes`
    pub path: Option<Vec<Token>>,
    /// `loop`: the target is the start, `looseness=8,min distance=5mm`
    pub is_loop: bool,
}

impl Default for ToSettings {
    fn default() -> Self {
        ToSettings {
            curve: false,
            relative: false,
            bend: 30.0,
            out: 45.0,
            inn: 135.0,
            out_looseness: 1.0,
            in_looseness: 1.0,
            out_min: 0.0,
            out_max: 10000.0 * PT,
            in_min: 0.0,
            in_max: 10000.0 * PT,
            out_control: None,
            in_control: None,
            path: None,
            is_loop: false,
        }
    }
}

/// What a path does once built (`\tikz@mode`): set by `draw`, `fill`… for one path only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mode {
    pub draw: bool,
    pub fill: bool,
    pub clip: bool,
    pub pattern: bool,
    pub shade: bool,
    /// `use as bounding box`
    pub bounding_box: bool,
    /// `path fading`: the path is faded (by the fading named last)
    pub fade: bool,
    /// `path picture={…}`: drawn inside the path, clipped to it
    pub picture: Option<Vec<Token>>,
}

#[derive(Debug, Clone)]
pub struct GState {
    /// pgfkeys' default path (`\pgfkeysdefaultpath`)
    pub key_path: String,
    /// The current transformation (pgf's), from canvas cm.
    pub transform: Transform,
    /// Transformations given in options and not applied yet (`\tikz@transform`).
    pub pending: Transform,
    /// Unit vectors of the xy (and z) coordinate system, in cm.
    pub x: Point,
    pub y: Point,
    pub z: Point,
    pub mode: Mode,
    /// `overlay`: what is drawn does not grow the picture's bounding box.
    pub overlay: bool,

    /// The current color (`color=`, a color name): stroke, fill and text unless they are set apart.
    pub color: Color,
    pub draw_color: Option<Color>,
    pub fill_color: Option<Color>,
    pub text_color: Option<Color>,
    pub pattern_name: Option<String>,
    pub pattern_color: Option<Color>,
    pub line_width: f64,
    pub dash: Option<(Vec<f64>, f64)>,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub draw_opacity: f64,
    pub fill_opacity: f64,
    pub text_opacity: f64,
    pub even_odd: bool,
    /// `rounded corners=` radius, or 0: how far before a corner its arc starts
    pub rounded: f64,
    /// how far after a corner its arc ends (`\pgfsetcornersarced{\pgfpoint{in}{out}}`)
    pub rounded_out: f64,
    /// Arrow specifications at the start and the end (`->`, `-{Latex[length=3mm]}`).
    pub start_arrow: Option<String>,
    pub end_arrow: Option<String>,
    /// Arrow tips defined with `name/.tip=` and `>=`.
    pub tips: Rc<super::arrows::UserTips>,
    /// `tips=`: when tips are drawn
    pub tips_mode: super::arrows::TipsMode,
    pub shorten_start: f64,
    pub shorten_end: f64,
    /// `double`: the color between the two lines, and the distance between them
    pub double: Option<Color>,
    pub double_distance: f64,
    /// Font: size in pt, and the TeX commands that set it (`\bfseries`, `\itshape`)
    pub font_size: f64,
    /// The size (pt) set by a font command (`\Huge`) around the nodes, not by `font=`: `em` and `ex`
    /// in lengths are in it
    pub ambient_size: f64,
    pub font: String,

    // path construction settings
    pub to: ToSettings,
    pub grid_step: (f64, f64),
    pub domain: (f64, f64),
    pub samples: usize,
    /// `samples at={…}`: the values of the plot's variable, instead of the domain and samples
    pub samples_at: Option<Vec<f64>>,
    pub variable: String,
    /// the plot handler: `sharp plot`, `smooth`, `smooth cycle`, `ycomb`, `ybar`, `const plot`…
    pub plot_handler: String,
    /// the basic layer's plot handler (`\pgfplothandlerlineto`) and a mark handler's code
    pub basic_plot: (String, Vec<Token>),
    /// `\pgfsetlinetofirstplotpoint`
    pub plot_first_lineto: bool,
    pub tension: f64,
    pub plot_mark: Option<String>,
    pub only_marks: bool,
    /// `mark size` (the radius), `mark repeat`, `mark phase`, `mark indices`, `mark color`
    pub mark_size: f64,
    pub mark_repeat: usize,
    pub mark_phase: usize,
    pub mark_indices: Option<Vec<usize>>,
    pub mark_color: Option<String>,
    /// `decorate`: the path is decorated when used (reset by each path)
    pub decorate: bool,
    pub decoration: super::decorations::Settings,
    /// `snake=…` (the snakes library of PGF 1.x): the decoration of each straight line of the path
    pub snake: Option<String>,
    /// `\usetikzlibrary{positioning}`: a bare number in `left=…` is in the x and y vectors' units,
    /// not points
    pub positioning: bool,
    /// `preaction=` and `postaction=` options, in order (reset by each path)
    pub preactions: Rc<Vec<Vec<crate::tex::Token>>>,
    pub postactions: Rc<Vec<Vec<crate::tex::Token>>>,
    /// `declare function`
    pub functions: Rc<HashMap<String, (Vec<String>, String)>>,
    /// settings of the node being created
    pub node: super::nodes::NodeSetup,
    /// `baseline=` of the picture
    pub baseline: Option<String>,
    /// `auto`: nodes on a segment go to its left (`'l'`) or right (`'r'`)
    pub auto: Option<char>,
    /// `swap` (or `'`): the other side
    pub swap: bool,
    /// quotes (`"text" above`) in the options being read make nodes on the path, not labels
    pub quotes_on_path: bool,
    /// `execute at begin scope`… code gathered from the options, run when TikZ runs it
    pub at_begin_scope: Vec<crate::tex::Token>,
    pub at_end_scope: Vec<crate::tex::Token>,
    pub at_begin_picture: Vec<crate::tex::Token>,
    pub at_end_picture: Vec<crate::tex::Token>,
    /// pics: the pic's mode (for `pic actions`), its text (quotes), and the prefix of node names
    pub pic_mode: Mode,
    pub pic_text: Option<(Vec<crate::tex::Token>, Vec<crate::tex::Token>)>,
    pub quotes_in_pic: bool,
    /// `edge from parent[options] node{…}` read in a child: options and label nodes
    pub tree_edge: Option<(Vec<crate::tex::Token>, Vec<crate::tex::Token>)>,
    pub name_prefix: String,
    /// `\pgfpositionnodelater`: the nodes made are kept to be placed by `\pgfpositionnodenow`
    pub position_later: bool,
    /// what the options of the matrix being read say about its cells
    pub matrix: super::matrix::MatrixSettings,
    /// shadings: which one (`axis`, `radial`, `ball`), its angle and its colors
    pub shading: String,
    pub shade_angle: f64,
    pub axis_top: Color,
    pub axis_middle: Color,
    pub axis_bottom: Color,
    pub ball: Color,
    pub radial_inner: Color,
    pub radial_outer: Color,
    /// `path fading=`: the fading's name; `transparency group` for the scope being opened
    pub path_fading: Option<String>,
    /// `fading transform` (and `fading angle`): the path fading transformed in the path's box.
    pub fading_transform: Transform,
    /// `handle active characters in code` (and `… in nodes`): active characters in options and
    /// coordinates read as the characters they are
    pub active_in_code: bool,
    pub active_in_nodes: bool,
    pub transparency_group: bool,
    /// `blend mode`: how what is drawn mixes with what is under it (SVG's name), if not normally
    pub blend: Option<String>,
    /// trees: the level being grown, the distances, the direction and the anchors
    pub tree_level: usize,
    pub level_distance: f64,
    pub sibling_distance: f64,
    pub grow_angle: f64,
    pub grow_swapped: bool,
    pub special_level: Option<usize>,
    pub growth_anchor: String,
    pub parent_anchor: String,
    pub child_anchor: String,
    pub child_missing: bool,
    pub edge_from_parent_path: Option<Vec<crate::tex::Token>>,
    /// the box of the path being used (`current path bounding box`), for its actions
    pub path_box: Option<(Point, Point)>,
    /// the perspective library's matrix, and the points of `plane origin`, `plane x`, `plane y`
    pub perspective: super::three_d::Perspective,
    pub plane: [Option<String>; 3],
    /// `insert path={…}`: path operations the options put where they are
    pub insert_path: Vec<Token>,
    /// `growth function=\macro`: places the children instead of `grow` (the petri library's tokens)
    pub growth_function: Option<Vec<crate::tex::Token>>,
    /// `name path=`: the path is kept under this name (intersections)
    pub name_path: Option<String>,
    /// the chains library: the current chain, the direction of new ones, the node's chain
    pub current_chain: String,
    pub chain_direction: super::chains::Direction,
    pub on_chain: Option<super::chains::OnChain>,
    pub pending_joins: Vec<(Option<String>, String)>,
    /// `after node path={…}`: path operations after the node
    pub after_node_path: Vec<crate::tex::Token>,
    /// `alias=name`: more names for the node
    pub aliases: Vec<String>,
    /// `trim left` and `trim right` of the picture
    pub trim_left: Option<String>,
    /// `allow upside down`: sloped nodes are not turned the right way up
    pub allow_upside_down: bool,
    /// `make snapshot of`: the time (s) the animations are shown at
    pub snapshot: Option<f64>,
    /// `current point is local`: a group of the path leaves the current point where it found it
    pub current_point_local: bool,
    /// `trig format=rad`: the angles of polar coordinates are in radians
    pub trig_rad: bool,
    /// `\pgfsetinnerlinewidth` and `\pgfsetinnerstrokecolor`: a second, inner line over each
    /// stroke (cm; white unless a color is set)
    pub inner_line: (f64, Option<crate::color::Color>),
    /// `fit fading`: a path fading fitted to the path's box
    pub fit_fading: bool,
    /// `\pgfsetfillpattern`: the basic layer's fills use the pattern
    pub fill_pattern: bool,
    /// `save path=\macro`, `use path=\macro` (the path is kept under the macro's name)
    pub save_path: Option<Token>,
    pub use_saved_path: Option<Token>,
    /// `\pgftransformnonlinear`: the code of each, in order
    pub nonlinear: std::rc::Rc<Vec<Vec<Token>>>,
    /// `\pgfsetcurvilinearbeziercurve`
    pub curvilinear: Option<std::rc::Rc<super::arrows::Curvilinear>>,
    pub trim_right: Option<String>,
    /// nodes made by quotes in `to`/`edge` options: text and options
    pub quoted: Vec<(Vec<crate::tex::Token>, Vec<crate::tex::Token>)>,
    /// `/pgf/number format`: how `\pgfmathprintnumber` writes numbers
    pub number_format: super::pgfplots::number::NumberFormat,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            key_path: "/".into(),
            transform: Transform::default(),
            pending: Transform::default(),
            overlay: false,
            x: Point::new(1.0, 0.0),
            y: Point::new(0.0, 1.0),
            z: Point::new(-0.3851, -0.3851),
            mode: Mode::default(),
            color: Color::BLACK,
            draw_color: None,
            fill_color: None,
            text_color: None,
            pattern_name: None,
            pattern_color: None,
            line_width: 0.4 * PT,
            dash: None,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 10.0,
            draw_opacity: 1.0,
            fill_opacity: 1.0,
            text_opacity: 1.0,
            even_odd: false,
            rounded: 0.0,
            rounded_out: 0.0,
            start_arrow: None,
            end_arrow: None,
            tips: Rc::default(),
            tips_mode: Default::default(),
            shorten_start: 0.0,
            shorten_end: 0.0,
            double: None,
            double_distance: 0.6 * PT,
            font_size: 10.0,
            ambient_size: 10.0,
            font: String::new(),
            to: ToSettings::default(),
            grid_step: (1.0, 1.0),
            domain: (-5.0, 5.0),
            samples: 25,
            samples_at: None,
            variable: "x".into(),
            plot_handler: "sharp plot".into(),
            basic_plot: ("sharp plot".into(), Vec::new()),
            plot_first_lineto: false,
            tension: 0.5,
            plot_mark: None,
            only_marks: false,
            mark_size: 2.0 * PT,
            mark_repeat: 1,
            mark_phase: 1,
            mark_indices: None,
            mark_color: None,
            decorate: false,
            decoration: Default::default(),
            snake: None,
            positioning: false,
            preactions: Rc::default(),
            postactions: Rc::default(),
            functions: Rc::new(HashMap::new()),
            node: Default::default(),
            baseline: None,
            auto: None,
            swap: false,
            quotes_on_path: false,
            name_path: None,
            current_chain: "chain".into(),
            chain_direction: super::chains::Direction::Going("right".into()),
            on_chain: None,
            pending_joins: Vec::new(),
            after_node_path: Vec::new(),
            aliases: Vec::new(),
            trim_left: None,
            allow_upside_down: false,
            snapshot: None,
            current_point_local: false,
            trig_rad: false,
            inner_line: (0.0, None),
            fit_fading: true,
            fill_pattern: false,
            save_path: None,
            use_saved_path: None,
            nonlinear: Default::default(),
            curvilinear: None,
            trim_right: None,
            matrix: Default::default(),
            path_fading: None,
            fading_transform: Transform::default(),
            active_in_code: false,
            active_in_nodes: false,
            transparency_group: false,
            blend: None,
            shading: "axis".into(),
            shade_angle: 0.0,
            axis_top: Color::rgb(0.5, 0.5, 0.5),
            axis_middle: Color::rgb(0.75, 0.75, 0.75),
            axis_bottom: Color::WHITE,
            ball: Color::rgb(0.0, 0.0, 1.0),
            radial_inner: Color::rgb(0.5, 0.5, 0.5),
            radial_outer: Color::WHITE,
            tree_level: 0,
            level_distance: 1.5,
            sibling_distance: 1.5,
            grow_angle: -90.0,
            grow_swapped: false,
            special_level: None,
            growth_anchor: "center".into(),
            parent_anchor: String::new(),
            child_anchor: String::new(),
            child_missing: false,
            edge_from_parent_path: None,
            growth_function: None,
            insert_path: Vec::new(),
            perspective: super::three_d::NO_PERSPECTIVE,
            plane: [None, None, None],
            path_box: None,
            pic_mode: Mode::default(),
            pic_text: None,
            quotes_in_pic: false,
            tree_edge: None,
            name_prefix: String::new(),
            position_later: false,
            at_begin_scope: Vec::new(),
            at_end_scope: Vec::new(),
            at_begin_picture: Vec::new(),
            at_end_picture: Vec::new(),
            quoted: Vec::new(),
            number_format: Default::default(),
        }
    }
}

impl GState {
    pub fn stroke_color(&self) -> Color {
        self.draw_color.unwrap_or(self.color)
    }
    pub fn fill_paint_color(&self) -> Color {
        self.fill_color.unwrap_or(self.color)
    }
    pub fn text_paint_color(&self) -> Color {
        self.text_color.unwrap_or(self.color)
    }
    /// Applies the pending transformations (`\tikz@transform`) and clears them.
    pub fn apply_pending(&mut self) {
        self.transform = self.transform.compose(&self.pending);
        self.pending = Transform::default();
    }
}
