//! The drawing layer: TikZ on top of [`crate::tex`]. Commands such as `\draw` read their arguments
//! from the token stream, the way `tikz.code.tex` does; options are pgfkeys; the graphic state is
//! saved with TeX groups; what is drawn goes to a [`Canvas`].

pub mod animations;
pub mod arrows;
pub mod basic;
pub mod calendar;
pub mod chains;
pub mod circuits;
pub mod coords;
pub mod datavis;
pub mod decorations;
pub mod fadings;
pub mod folding;
pub mod functional;
pub mod graphs;
pub mod intersections;
pub mod libraries;
pub mod lsystem;
pub mod marks;
pub mod matrix;
pub mod mindmap;
pub mod nodes;
pub mod nonlinear;
pub mod options;
pub mod output;
pub mod overlays;
pub mod path;
pub mod patterns;
pub mod pgfplots;
pub mod pics;
pub mod saveboxes;
pub mod shadings;
pub mod shapes;
pub mod spy;
pub mod state;
pub mod three_d;
pub mod tikzmath;
pub mod trees;
pub mod turtle;
pub mod user_shapes;
pub mod views;

use std::collections::HashMap;
use std::rc::Rc;

use crate::canvas::{Canvas, Item};
use crate::color::{self, Color};
use crate::scene::{Point, Transform};
use crate::tex::{self, Cat, Meaning, Prim, Tex, Token};
use crate::Error;

pub use state::GState;

/// Commands of the drawing layer (the number is the `Meaning::Native` id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Cmd {
    Only,
    Uncover,
    Visible,
    Invisible,
    Onslide,
    Alt,
    Pause,
    Path,
    Draw,
    Fill,
    Filldraw,
    Clip,
    Shade,
    Shadedraw,
    Pattern,
    Node,
    Coordinate,
    Useasboundingbox,
    ResetBoundingBox,
    Matrix,
    PgfMatrix,
    Chainin,
    TikzMath,
    DeclareFading,
    /// `\pgfdeclareimage[options]{name}{file}`
    DeclareImage,
    DeclarePatternForm,
    DeclarePatternColored,
    SetFillPattern,
    TikzDeclarePattern,
    PgfDeclarePattern,
    ArrowsDeclare,
    DeclareShape,
    PositionNodeLater,
    PositionNodeNow,
    DeclareShadingH,
    DeclareShadingV,
    DeclareShadingR,
    UseShading,
    ShadePath,
    SetFading,
    TransformNonlinear,
    ApproximateNonlinear,
    ApproximateNonlinearTranslation,
    SetCurvilinear,
    PointCurvilinear,
    PointCurvilinearPolar,
    MathSinCos,
    SetFadingForPath,
    SetFadingForPathStroked,
    SetSeed,
    /// `\pgfmathdeclarerandomlist{name}{{a}{b}…}`
    DeclareRandomList,
    Pic,
    Graph,
    Tikz,
    Begin,
    End,
    Scoped,
    Tikzset,
    Pgfkeys,
    Pgfqkeys,
    Pgfkeysalso,
    Pgfset,
    Pgfplotsset,
    Tikzstyle,
    Key,
    SetPath,
    Foreach,
    Definecolor,
    Colorlet,
    Color,
    Textcolor,
    Pagecolor,
    Nopagecolor,
    Pgfqbox,
    Pgfmathsetmacro,
    Pgfmathparse,
    Pgfmathtruncatemacro,
    Pgfmathsetlengthmacro,
    Pgfmathsetcount,
    Pgfmathsetlength,
    Pgfmathaddtolength,
    Pgfmathsetcounter,
    Pgfmathaddtocounter,
    Pgfmathdeclarefunction,
    Pgfmathprintnumber,
    Usetikzlibrary,
    Usepackage,
    Documentclass,
    PassOptionsToPackage,
    Ignore1,
    Ignore2,
    Hspace,
    Fbox,
    Scalebox,
    Resizebox,
    Hskip,
    /// `\vskip 1em plus 2pt`: its glue read, and dropped (the pictures are not set in lines by height)
    Vskip,
    /// `\newframe[rate]` (animate package): the frames after the first are not drawn
    NewFrame,
    /// `\setbeamertemplate{element}[option]…{code}`: read, and dropped
    SetBeamerTemplate,
    FontSize,
    FontSeries,
    OnLayer,
    EndOnLayer,
    NamePrefix,
    Tikzfading,
    /// LaTeX's boxes: `\newsavebox`, `\savebox` and `\sbox`, `\usebox`
    Newsavebox,
    Savebox,
    Usebox,
    /// the active preview package's `\PreviewEnvironment`, and `\scalebox`, `\resizebox` and
    /// `\fbox` when it takes the pictures out of them
    PreviewEnvironment,
    Unboxed,
    SetLayers,
    Frame,
    /// `\arrow` and `\arrowreversed` of markings
    Arrow,
    ArrowReversed,
    /// the basic layer's decorations: `\pgfdecoration` … `\endpgfdecoration`, `\pgfdecoratecurrentpath`
    DecorationBegin,
    DecorationEnd,
    DecorateCurrentPath,
    /// the commands of data visualizations, dispatched by name (`datavis`)
    DataVis,
    /// the spy library: `\spy`, and what a spy scope does at its start and end
    Spy,
    SpyBegin,
    SpyEnd,
    /// `\quad` and `\qquad` between pictures
    Quad,
    /// `\tikz@checkunit{value}`
    CheckUnit,
}

const COMMANDS: &[(&str, Cmd)] = &[
    ("tikra@arrow", Cmd::Arrow),
    ("tikra@arrowreversed", Cmd::ArrowReversed),
    ("tikra@decoration", Cmd::DecorationBegin),
    ("tikra@enddecoration", Cmd::DecorationEnd),
    ("tikra@decoratecurrentpath", Cmd::DecorateCurrentPath),
    ("path", Cmd::Path),
    ("draw", Cmd::Draw),
    ("fill", Cmd::Fill),
    ("filldraw", Cmd::Filldraw),
    ("clip", Cmd::Clip),
    ("shade", Cmd::Shade),
    ("shadedraw", Cmd::Shadedraw),
    ("pattern", Cmd::Pattern),
    ("node", Cmd::Node),
    ("coordinate", Cmd::Coordinate),
    ("useasboundingbox", Cmd::Useasboundingbox),
    ("pgfresetboundingbox", Cmd::ResetBoundingBox),
    ("matrix", Cmd::Matrix),
    ("pgfmatrix", Cmd::PgfMatrix),
    ("chainin", Cmd::Chainin),
    ("tikzmath", Cmd::TikzMath),
    ("pgfdeclarefading", Cmd::DeclareFading),
    ("pgfdeclareimage", Cmd::DeclareImage),
    ("pgfdeclarepatternformonly", Cmd::DeclarePatternForm),
    ("pgfdeclarepatterninherentlycolored", Cmd::DeclarePatternColored),
    ("pgfsetfillpattern", Cmd::SetFillPattern),
    ("tikzdeclarepattern", Cmd::TikzDeclarePattern),
    ("pgfdeclarepattern", Cmd::PgfDeclarePattern),
    ("pgfarrowsdeclare", Cmd::ArrowsDeclare),
    ("pgfdeclareshape", Cmd::DeclareShape),
    ("pgfpositionnodelater", Cmd::PositionNodeLater),
    ("pgfpositionnodenow", Cmd::PositionNodeNow),
    ("pgfdeclarehorizontalshading", Cmd::DeclareShadingH),
    ("pgfdeclareverticalshading", Cmd::DeclareShadingV),
    ("pgfdeclareradialshading", Cmd::DeclareShadingR),
    ("pgfuseshading", Cmd::UseShading),
    ("pgfshadepath", Cmd::ShadePath),
    ("pgfsetfading", Cmd::SetFading),
    ("pgftransformnonlinear", Cmd::TransformNonlinear),
    ("pgfapproximatenonlineartransformation", Cmd::ApproximateNonlinear),
    ("pgfapproximatenonlineartranslation", Cmd::ApproximateNonlinearTranslation),
    ("pgfsetcurvilinearbeziercurve", Cmd::SetCurvilinear),
    ("pgfpointcurvilinearbezierorthogonal", Cmd::PointCurvilinear),
    ("pgfpointcurvilinearbezierpolar", Cmd::PointCurvilinearPolar),
    ("tikz@checkunit", Cmd::CheckUnit),
    ("pgfmathsincos@", Cmd::MathSinCos),
    ("pgfsetfadingforcurrentpath", Cmd::SetFadingForPath),
    ("pgfsetfadingforcurrentpathstroked", Cmd::SetFadingForPathStroked),
    ("tikra@setseed", Cmd::SetSeed),
    ("pgfmathdeclarerandomlist", Cmd::DeclareRandomList),
    ("pic", Cmd::Pic),
    ("graph", Cmd::Graph),
    ("tikz", Cmd::Tikz),
    ("begin", Cmd::Begin),
    ("end", Cmd::End),
    ("scoped", Cmd::Scoped),
    ("tikzset", Cmd::Tikzset),
    ("pgfkeys", Cmd::Pgfkeys),
    ("pgfqkeys", Cmd::Pgfqkeys),
    ("pgfkeysalso", Cmd::Pgfkeysalso),
    ("pgfset", Cmd::Pgfset),
    ("pgfplotsset", Cmd::Pgfplotsset),
    ("tikzstyle", Cmd::Tikzstyle),
    ("tikra@key", Cmd::Key),
    ("tikra@setpath", Cmd::SetPath),
    ("foreach", Cmd::Foreach),
    ("definecolor", Cmd::Definecolor),
    ("providecolor", Cmd::Definecolor),
    ("xdefinecolor", Cmd::Definecolor),
    ("colorlet", Cmd::Colorlet),
    ("color", Cmd::Color),
    ("textcolor", Cmd::Textcolor),
    ("pagecolor", Cmd::Pagecolor),
    ("nopagecolor", Cmd::Nopagecolor),
    ("pgfqbox", Cmd::Pgfqbox),
    ("pgfmathsetmacro", Cmd::Pgfmathsetmacro),
    ("pgfmathparse", Cmd::Pgfmathparse),
    ("pgfmathtruncatemacro", Cmd::Pgfmathtruncatemacro),
    ("pgfmathsetlengthmacro", Cmd::Pgfmathsetlengthmacro),
    ("pgfmathsetcount", Cmd::Pgfmathsetcount),
    ("pgfmathsetlength", Cmd::Pgfmathsetlength),
    ("pgfmathaddtolength", Cmd::Pgfmathaddtolength),
    ("pgfmathsetcounter", Cmd::Pgfmathsetcounter),
    ("pgfmathaddtocounter", Cmd::Pgfmathaddtocounter),
    ("pgfmathdeclarefunction", Cmd::Pgfmathdeclarefunction),
    ("pgfmathprintnumber", Cmd::Pgfmathprintnumber),
    ("usetikzlibrary", Cmd::Usetikzlibrary),
    ("usepgflibrary", Cmd::Usetikzlibrary),
    ("usepgfplotslibrary", Cmd::Usetikzlibrary),
    ("usepgfmodule", Cmd::Usetikzlibrary),
    ("usepackage", Cmd::Usepackage),
    ("RequirePackage", Cmd::Usepackage),
    ("documentclass", Cmd::Documentclass),
    ("PassOptionsToPackage", Cmd::PassOptionsToPackage),
    ("pgfplotsset", Cmd::Pgfplotsset),
    ("label", Cmd::Ignore1),
    ("hspace", Cmd::Hspace),
    ("fbox", Cmd::Fbox),
    ("scalebox", Cmd::Scalebox),
    ("resizebox", Cmd::Resizebox),
    ("hskip", Cmd::Hskip),
    ("vskip", Cmd::Vskip),
    ("newframe", Cmd::NewFrame),
    // beamer's themes and its frame's title: the page around the pictures, not drawn
    ("usetheme", Cmd::Ignore1),
    ("usecolortheme", Cmd::Ignore1),
    ("usefonttheme", Cmd::Ignore1),
    ("useinnertheme", Cmd::Ignore1),
    ("useoutertheme", Cmd::Ignore1),
    ("frametitle", Cmd::Ignore1),
    ("framesubtitle", Cmd::Ignore1),
    ("setbeamercolor", Cmd::Ignore2),
    ("setbeamerfont", Cmd::Ignore2),
    ("setbeamertemplate", Cmd::SetBeamerTemplate),
    ("vspace", Cmd::Ignore1),
    ("pgfdeclarelayer", Cmd::Ignore1),
    ("pgfsetlayers", Cmd::SetLayers),
    ("pgfonlayer", Cmd::OnLayer),
    ("endpgfonlayer", Cmd::EndOnLayer),
    ("tikra@frame", Cmd::Frame),
    ("tikra@nameprefix", Cmd::NamePrefix),
    ("tikzfading", Cmd::Tikzfading),
    ("newsavebox", Cmd::Newsavebox),
    ("savebox", Cmd::Savebox),
    ("sbox", Cmd::Savebox),
    ("usebox", Cmd::Usebox),
    ("tikra@previewenvironment", Cmd::PreviewEnvironment),
    ("tikra@unboxed", Cmd::Unboxed),
    ("tikzexternalize", Cmd::Ignore1),
    ("pgfrealjobname", Cmd::Ignore1),
    ("pagestyle", Cmd::Ignore1),
    // the geometry package's page margins: no page here
    ("geometry", Cmd::Ignore1),
    ("newgeometry", Cmd::Ignore1),
    ("thispagestyle", Cmd::Ignore1),
    ("caption", Cmd::Ignore1),
    // what `\maketitle` would set: tikra sets no title
    ("title", Cmd::Ignore1),
    ("author", Cmd::Ignore1),
    ("date", Cmd::Ignore1),
    ("tikzsetnextfilename", Cmd::Ignore1),
    ("pgfplotstableset", Cmd::Ignore1),
    ("tiny", Cmd::FontSize),
    ("scriptsize", Cmd::FontSize),
    ("footnotesize", Cmd::FontSize),
    ("small", Cmd::FontSize),
    ("normalsize", Cmd::FontSize),
    ("large", Cmd::FontSize),
    ("Large", Cmd::FontSize),
    ("LARGE", Cmd::FontSize),
    ("huge", Cmd::FontSize),
    ("Huge", Cmd::FontSize),
    ("bfseries", Cmd::FontSeries),
    ("mdseries", Cmd::FontSeries),
    ("itshape", Cmd::FontSeries),
    ("upshape", Cmd::FontSeries),
    ("slshape", Cmd::FontSeries),
    ("scshape", Cmd::FontSeries),
    ("ttfamily", Cmd::FontSeries),
    ("sffamily", Cmd::FontSeries),
    ("rmfamily", Cmd::FontSeries),
    ("centering", Cmd::FontSeries),
    ("raggedright", Cmd::FontSeries),
    ("raggedleft", Cmd::FontSeries),
    ("noindent", Cmd::FontSeries),
    ("maketitle", Cmd::FontSeries),
    ("selectfont", Cmd::FontSeries),
    ("quad", Cmd::Quad),
    ("qquad", Cmd::Quad),
    ("spy", Cmd::Spy),
    ("tikra@spybegin", Cmd::SpyBegin),
    ("tikra@spyend", Cmd::SpyEnd),
];

/// pgfmath's functions as commands (`\pgfmathadd{a}{b}`…), numbered from
/// `PGFMATH_FUNCTION_BASE` in this order: the number, not the name a command is called by, says which
/// function it is (`\let\plus\pgfmathadd`).
const PGFMATH_FUNCTION_BASE: u16 = 400;
const _: () = assert!((Cmd::CheckUnit as u16) < PGFMATH_FUNCTION_BASE && PGFMATH_FUNCTION_BASE as usize + PGFMATH_FUNCTIONS.len() <= basic::BASE as usize);
const PGFMATH_FUNCTIONS: &[&str] = &[
    "add", "subtract", "multiply", "divide", "max", "min", "mod", "Mod", "pow", "veclen", "atan2", "abs", "neg", "sqrt", "sin", "cos", "tan", "exp", "ln", "round", "floor",
    "ceil", "int", "deg", "rad", "sign",
];

/// Commands that the drawing layer turns into a path statement, with their default options.
fn path_command(cmd: Cmd) -> Option<&'static str> {
    Some(match cmd {
        Cmd::Path => "",
        Cmd::Draw => "draw",
        Cmd::Fill => "fill",
        Cmd::Filldraw => "fill,draw",
        Cmd::Clip => "clip",
        Cmd::Shade => "shade",
        Cmd::Shadedraw => "shade,draw",
        Cmd::Pattern => "pattern",
        Cmd::Useasboundingbox => "use as bounding box",
        _ => return None,
    })
}

/// A pgf scope (picture, `scope`, node): what must be closed in the canvas when it ends.
#[derive(Debug, Clone, Default)]
struct PgfScope {
    ends: usize,
    /// the item opened last in it is a fading: a new fading takes its place
    fading_last: bool,
    /// a path clipped it: what it draws after does not make the picture larger
    pub clipped: bool,
}

/// Cap on statements per figure: nested loops do not hang the caller.
pub const MAX_STEPS: usize = 100_000;
/// Cap on nesting of scopes, loops and environments.
pub const MAX_DEPTH: usize = 64;

pub struct Engine {
    pub tex: Tex,
    /// The source map: the statements' spans in characters of the code (none: not asked for), and
    /// the one being run, an index in it.
    pub spans: Option<Vec<(usize, usize)>>,
    pub source: Option<usize>,
    /// Where the outermost picture starts, in those spans.
    picture_source: Option<usize>,
    /// The slide being drawn (see `overlays`), how many the code has shown so far, and Beamer's
    /// counter of pauses.
    pub slide: usize,
    pub slides: usize,
    pub pauses: usize,
    /// The animations library is played, not shown still.
    pub play: bool,
    /// Each bar and mark of a plot is an element of its own, with its values.
    pub points: bool,
    /// The values of the plot's points, by where they are on the canvas.
    pub point_values: HashMap<(i64, i64), (String, String)>,
    /// The animations read for the object being drawn, what they turn around, and the group depths
    /// of the scopes with animations.
    pub animations: Vec<crate::canvas::Animation>,
    pub animation_center: Option<Point>,
    pub animated_scopes: Vec<usize>,
    /// A `\\pause` or a bare `\\onslide` hides what follows on this slide.
    slide_hidden: bool,
    /// Inside what is being hidden.
    hiding: usize,
    pub st: GState,
    saved: Vec<GState>,
    pub canvas: Canvas,
    pub nodes: HashMap<String, nodes::NodeInfo>,
    pub colors: HashMap<String, Color>,
    /// `\pagecolor`: the color of the page behind the figure
    page_color: Option<Color>,
    /// `varwidth=…` of the `standalone` class: the widest the page gets (cm)
    varwidth: Option<f64>,
    /// the group depth inside `\begin{document}`, where a `\color` lasts to the page's end
    document_depth: Option<usize>,
    scopes: Vec<PgfScope>,
    /// how many pictures are open (`tikzpicture` inside `tikzpicture` is a scope)
    pub picture_depth: usize,
    pub pictures: usize,
    pub steps: usize,
    pub depth: usize,
    /// text outside nodes was seen and reported
    text_reported: bool,
    /// the native keys, by id − NATIVE_KEY_BASE
    key_fns: Vec<options::KeyFn>,
    /// the keys of libraries from outside ([`crate::extension`]), by id − EXTENSION_KEY_BASE
    extension_fns: Vec<crate::extension::KeyFn>,
    /// `use as bounding box` was used: the bounding box no longer grows
    pub canvas_frozen: bool,
    /// nested runs of token lists (styles, loop bodies)
    nesting: usize,
    /// the input depth below which the token list being run (`run_tokens`) ends: a statement
    /// skipped in it stops there
    floor: usize,
    /// the basic layer's path being built
    pub pgf_path: basic::PgfPath,
    /// the points of `\pgfplotstreampoint` since `\pgfplotstreamstart`, on the canvas
    pub plot_stream: Vec<crate::scene::Point>,
    /// `execute at end scope` code of the open scopes, innermost last
    scope_ends: Vec<Vec<Token>>,
    /// for each `{` in a picture: whether it opened a scope (`{[options] …}`, the scopes library)
    brace_scopes: Vec<bool>,
    /// layers: their order, the items of each but main, and what `\pgfonlayer` set aside
    layer_order: Vec<String>,
    layers: HashMap<String, Vec<Item>>,
    on_layer: Vec<(String, Vec<Item>)>,
    /// the last node made (a tree's parent), its name, and how many tree parents were named
    pub last_node: Option<nodes::NodeInfo>,
    /// the shifts a node's animations move it by (canvas), which its picture's size makes room for
    pub anim_shifts: Vec<Point>,
    /// The options the system layer's animations give, at the snapshot time, to the next object
    /// with each name (`\pgfidrefnextuse`, `\pgfsysanimate`).
    pub sys_animations: HashMap<String, Vec<Token>>,
    pub last_node_name: Option<String>,
    tree_count: usize,
    /// the chains of the chains library (global, as TikZ keeps their counts)
    pub chain_list: std::collections::HashMap<String, chains::Chain>,
    /// the types `\tikzmath` declarations gave variables
    pub math_kinds: HashMap<String, tikzmath::Kind>,
    /// path operations to run after the node just made (`after node path`, joins)
    pub after_node_path: Vec<Token>,
    /// The open `local bounding box`es: their names and the group depth they were opened at.
    pub local_boxes: Vec<(String, usize)>,
    /// The picture's size when each `pgfinterruptboundingbox` opened, restored when it closes.
    interrupted_boxes: Vec<(Option<Point>, Option<Point>)>,
    /// The path being built when each `\pgfinterruptpath` opened, given back when it closes.
    pub interrupted_paths: Vec<basic::PgfPath>,
    /// The open `animateinline`s (animate package): whether a `\newframe` has passed in each, so that
    /// only the first frame, its poster, is drawn.
    animate_frames: Vec<AnimateFrames>,
    /// For each path statement being read, its `scope fading`s.
    pub path_fadings: Vec<Vec<fadings::PathFading>>,
    /// The `scope fading`s of a path that drew nothing: the next path is faded, the fading fitted
    /// to the box of the path that gave it (none: at its natural size).
    pub carried_fadings: Vec<fadings::CarriedFading>,
    /// How many `transform canvas` groups have been opened: a path statement closes its own.
    pub canvas_transforms: usize,
    /// For each path statement being read, the `fading transform` and `fit fading` its options (and
    /// its nodes') set: what its `scope fading`s use.
    pub path_fading_settings: Vec<(Transform, bool)>,
    /// the path's last point and where it came from, for `turn` in the next coordinate
    pub turn_from: (crate::scene::Point, crate::scene::Point),
    /// where the path's subpath started (`current subpath start`)
    pub subpath_start: crate::scene::Point,
    /// the last point of the last path, on the canvas (`\pgfgetlastxy`); it outlives scopes
    pub last_xy: crate::scene::Point,
    /// fadings defined by `\tikzfading`, in their 100 bp box
    pub fadings: HashMap<String, crate::canvas::Shading>,
    /// fadings drawn as pictures (`\pgfdeclarefading`, `tikzfadingfrompicture`)
    pub picture_fadings: HashMap<String, fadings::PictureFading>,
    /// the basic layer's declared shadings
    pub basic_shadings: HashMap<String, shadings::BasicShading>,
    /// the declared functional shadings
    pub functional_shadings: HashMap<String, functional::Functional>,
    /// marks declared by `\pgfdeclareplotmark`: their code
    pub plot_marks_declared: HashMap<String, Vec<Token>>,
    /// plot handlers declared by `\pgfdeclareplothandler`: the code for each point (`##1` the
    /// point, `#1`… the handler's arguments)
    pub plot_handlers: HashMap<String, String>,
    /// paths kept by `save path`
    pub saved_paths: HashMap<String, Vec<crate::scene::Segment>>,
    /// the points `\pgfintersectionofpaths` found (canvas)
    pub pgf_solutions: Vec<Point>,
    /// patterns declared by the basic layer
    pub user_patterns: HashMap<String, patterns::UserPattern>,
    /// shapes declared by `\pgfdeclareshape`
    pub user_shapes: HashMap<String, user_shapes::UserShape>,
    /// while a declared shape's path code runs: what it draws, kept for the shape
    pub shape_capture: Option<user_shapes::ShapeCapture>,
    /// the node shapes libraries declared, by name
    pub extension_shapes: HashMap<String, crate::extension::ShapeFn>,
    /// the node `\pgfpositionnodelater` kept: what it draws, where its anchors are, its name
    pub later_node: Option<(Vec<Item>, nodes::NodeInfo, Option<String>)>,
    /// the text box a node must take (a matrix's cells), instead of its text's
    pub forced_box: Option<(f64, f64, f64)>,
    /// the shading a node's text is (`\pgfuseshading{name}`), with its half width and height (cm)
    pub text_shading: Option<(String, f64, f64)>,
    /// the picture a node's text is (`\begin{tikzpicture}…\end{tikzpicture}`, `\tikz…`,
    /// `\usebox`): what it draws, its box's corners, and its depth below the baseline (cm)
    pub text_picture: Option<(Vec<Item>, Point, Point, f64)>,
    /// the name of each child being grown, set by the first node its statement makes
    pub tree_child_names: Vec<Option<String>>,
    /// a child's `edge from parent[…]`, read inside its statement
    pub tree_edge_out: Option<(Vec<Token>, Vec<Token>)>,
    /// what the `behind path` nodes of the path being built draw, drawn before it
    pub behind_path: Vec<Item>,
    /// open `\pgfdecoration`s
    pub decoration_envs: Vec<decorations::Env>,
    /// paths kept by `name path`
    pub named_paths: HashMap<String, Vec<crate::scene::Segment>>,
    /// pictures already drawn, and how the document lays them out
    boxes: Vec<PictureBox>,
    layout: Vec<Layout>,
    /// the words of the text around the pictures, each with the font it starts in
    words: Vec<(String, String)>,
    /// the boxes `\savebox`, `\sbox` and `lrbox` keep, by their names (`\newsavebox`)
    saved_boxes: HashMap<String, PictureBox>,
    /// keys run so far
    /// `/handlers/first char syntax` was set: keys are checked for a handler of their first character.
    pub first_char_syntax: bool,
    pub keys_run: usize,
    /// the data visualizations being run, innermost last
    pub datavis: Vec<datavis::Dv>,
    /// the full path of the native key being run (natives that share code tell keys apart by it)
    pub current_key: String,
    /// the `axis` environment being read, and the one being drawn
    pub axis: Option<Box<pgfplots::Axis>>,
    pub frame: Option<std::rc::Rc<pgfplots::Frame>>,
    /// the `groupplot` environment being read
    pub plot_group: Option<Box<pgfplots::group::Group>>,
    /// the graphs being read (`\graph`)
    pub graphs: graphs::State,
    /// the open spy scopes, innermost last
    pub spy_scopes: Vec<spy::SpyScope>,
    /// what the figure shows, noted as it is drawn, for its description in words
    pub facts: crate::describe::Facts,
}

/// A finished picture: its canvas and the height of its baseline in it (cm).
#[derive(Debug, Clone)]
struct PictureBox {
    canvas: Canvas,
    baseline: f64,
}

/// What a part of the document typesets on its own: its pictures, how they are laid out, its words.
type Apart = (Vec<PictureBox>, Vec<Layout>, Vec<(String, String)>);

/// What a document puts between pictures: TeX typesets them in lines, like words.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Layout {
    Picture(usize),
    /// a word of the text around the pictures (in `words`)
    Word(usize),
    Space,
    /// `\quad`, `\qquad`, `\hspace{…}`: glue of that width (cm)
    Skip(f64),
    Par,
    /// `\\`: the line ends there
    Break,
    /// `center` begins (true) or ends: its lines are centered in the line's width
    Center(bool),
    /// a `tabular` begins: its columns' alignments, two bits each (`l` 0, `c` 1, `r` 2), with
    /// their count in the low byte
    TabStart(u32),
    /// `&`, `\\` and the end of a `tabular`
    Cell,
    Row,
    TabEnd,
    /// the width of the line (cm), in a `\vbox` of that `\hsize`
    Width(f64),
    /// the pictures are in a `\vbox` (not a `minipage`): glue at its end is part of its height
    Vbox,
}

/// An open `animateinline`: whether its first frame is over (a `\newframe` has passed), and the
/// code its `end` option closes each frame with.
#[derive(Clone, Debug, Default)]
struct AnimateFrames {
    past_first: bool,
    end: Vec<Token>,
}

/// Cap on nested runs (a style calling itself).
pub const MAX_NESTING: usize = 150;
/// Cap on keys per figure (styles that multiply: `s/.style={t,t,t,t}`…). The figures of the manuals
/// and the corpus run at most about 10,000.
pub const MAX_KEYS: usize = 50_000;

/// Ids at and above this are native keys (`\pgfk@…/.@cmd`), below are commands.
pub const NATIVE_KEY_BASE: u16 = 1000;

/// Ids at and above this are the keys of libraries from outside ([`crate::extension`]).
pub const EXTENSION_KEY_BASE: u16 = 40000;

thread_local! {
    /// The engine after defining every command and key: cloned for each figure.
    static PROTOTYPE: std::cell::RefCell<Option<Engine>> = const { std::cell::RefCell::new(None) };
    /// The engine with libraries from outside, by their names: cloned for each figure that uses them.
    static WITH_LIBRARIES: std::cell::RefCell<Vec<(Vec<&'static str>, Engine)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Engine {
            tex: self.tex.clone(),
            spans: self.spans.clone(),
            source: self.source,
            picture_source: self.picture_source,
            slide: self.slide,
            slides: self.slides,
            pauses: self.pauses,
            play: self.play,
            points: self.points,
            point_values: self.point_values.clone(),
            animations: self.animations.clone(),
            animation_center: self.animation_center,
            animated_scopes: self.animated_scopes.clone(),
            slide_hidden: self.slide_hidden,
            hiding: self.hiding,
            st: self.st.clone(),
            saved: self.saved.clone(),
            canvas: self.canvas.clone(),
            nodes: self.nodes.clone(),
            colors: self.colors.clone(),
            page_color: self.page_color,
            varwidth: self.varwidth,
            document_depth: self.document_depth,
            scopes: self.scopes.clone(),
            picture_depth: self.picture_depth,
            pictures: self.pictures,
            steps: self.steps,
            depth: self.depth,
            text_reported: self.text_reported,
            key_fns: self.key_fns.clone(),
            extension_fns: self.extension_fns.clone(),
            canvas_frozen: self.canvas_frozen,
            nesting: self.nesting,
            floor: 0,
            keys_run: self.keys_run,
            first_char_syntax: self.first_char_syntax,
            datavis: self.datavis.clone(),
            pgf_path: self.pgf_path.clone(),
            plot_stream: Vec::new(),
            decoration_envs: self.decoration_envs.clone(),
            named_paths: self.named_paths.clone(),
            last_node: self.last_node.clone(),
            last_node_name: self.last_node_name.clone(),
            tree_count: self.tree_count,
            chain_list: self.chain_list.clone(),
            math_kinds: self.math_kinds.clone(),
            after_node_path: Vec::new(),
            local_boxes: Vec::new(),
            interrupted_boxes: Vec::new(),
            interrupted_paths: Vec::new(),
            animate_frames: Vec::new(),
            path_fadings: Vec::new(),
            carried_fadings: Vec::new(),
            canvas_transforms: 0,
            path_fading_settings: Vec::new(),
            turn_from: Default::default(),
            subpath_start: Default::default(),
            last_xy: Default::default(),
            tree_edge_out: self.tree_edge_out.clone(),
            behind_path: Vec::new(),
            tree_child_names: self.tree_child_names.clone(),
            forced_box: self.forced_box,
            text_shading: None,
            text_picture: None,
            fadings: self.fadings.clone(),
            picture_fadings: self.picture_fadings.clone(),
            basic_shadings: self.basic_shadings.clone(),
            functional_shadings: self.functional_shadings.clone(),
            plot_marks_declared: self.plot_marks_declared.clone(),
            plot_handlers: self.plot_handlers.clone(),
            saved_paths: self.saved_paths.clone(),
            pgf_solutions: self.pgf_solutions.clone(),
            anim_shifts: Vec::new(),
            sys_animations: HashMap::new(),
            user_patterns: self.user_patterns.clone(),
            user_shapes: self.user_shapes.clone(),
            shape_capture: None,
            extension_shapes: self.extension_shapes.clone(),
            later_node: None,
            scope_ends: self.scope_ends.clone(),
            brace_scopes: Vec::new(),
            layer_order: self.layer_order.clone(),
            layers: self.layers.clone(),
            on_layer: self.on_layer.clone(),
            boxes: self.boxes.clone(),
            layout: self.layout.clone(),
            words: self.words.clone(),
            saved_boxes: self.saved_boxes.clone(),
            current_key: self.current_key.clone(),
            axis: self.axis.clone(),
            frame: self.frame.clone(),
            plot_group: self.plot_group.clone(),
            graphs: self.graphs.clone(),
            spy_scopes: self.spy_scopes.clone(),
            facts: self.facts.clone(),
        }
    }
}

impl Engine {
    /// A fresh engine with TikZ defined (built once per thread, then cloned).
    pub fn new() -> Engine {
        arrows::clear_declared();
        PROTOTYPE.with(|p| {
            let mut p = p.borrow_mut();
            if p.is_none() {
                *p = Some(Engine::build());
            }
            p.as_ref().expect("built").clone()
        })
    }

    /// A fresh engine with TikZ and these libraries defined (built once per thread for each set of
    /// libraries, then cloned).
    pub fn with_libraries(libraries: &[&'static crate::extension::Library]) -> Engine {
        if libraries.is_empty() {
            return Engine::new();
        }
        let names: Vec<&'static str> = libraries.iter().map(|l| l.name).collect();
        let found = WITH_LIBRARIES.with(|w| w.borrow().iter().find(|(n, _)| *n == names).map(|(_, e)| e.clone()));
        if let Some(e) = found {
            arrows::clear_declared();
            return e;
        }
        let mut e = Engine::new();
        for library in libraries {
            e.register(library);
        }
        // (what the definitions warned stays with the engine: each figure that uses them says so)
        WITH_LIBRARIES.with(|w| w.borrow_mut().push((names, e.clone())));
        e
    }

    /// Defines a library's keys, then runs its definitions as TeX with `@` a letter.
    fn register(&mut self, library: &crate::extension::Library) {
        self.register_keys(library);
        let path = std::mem::replace(&mut self.st.key_path, "/tikz".into());
        let mut lexer = Tex::new_lexer();
        lexer.push_text(&(library.definitions)());
        let mut tokens = Vec::new();
        while let Some(t) = lexer.get_token() {
            tokens.push(t);
        }
        if let Err(err) = self.run_tokens(tokens) {
            self.warn(format!("library {}: definitions failed: {err}", library.name));
        }
        self.st.key_path = path;
    }

    /// Defines a library's keys only (TikZ's own libraries run their definitions with TikZ's).
    pub(crate) fn register_keys(&mut self, library: &crate::extension::Library) {
        let mut left_out = library.keys.iter().skip_while(|(path, f)| {
            let id = EXTENSION_KEY_BASE as usize + self.extension_fns.len();
            if id > u16::MAX as usize {
                return false;
            }
            self.extension_fns.push(*f);
            self.tex.define(&crate::keys::macro_name(&format!("{path}/.@cmd")), Meaning::Native(id as u16));
            true
        });
        // past the room for native keys, one warning for all the keys left out
        if let Some((first, _)) = left_out.next() {
            let n = 1 + left_out.count();
            self.warn(format!("library {}: too many keys, {n} left out from {first} on", library.name));
        }
    }

    fn build() -> Engine {
        let mut e = Engine {
            tex: Tex::new(),
            st: GState::default(),
            saved: Vec::new(),
            canvas: Canvas::default(),
            nodes: HashMap::new(),
            colors: HashMap::new(),
            page_color: None,
            varwidth: None,
            document_depth: None,
            scopes: Vec::new(),
            picture_depth: 0,
            pictures: 0,
            steps: 0,
            spans: None,
            source: None,
            picture_source: None,
            slide: 1,
            slides: 1,
            pauses: 1,
            play: false,
            points: false,
            point_values: HashMap::new(),
            animations: Vec::new(),
            animation_center: None,
            animated_scopes: Vec::new(),
            slide_hidden: false,
            hiding: 0,
            depth: 0,
            text_reported: false,
            key_fns: Vec::new(),
            extension_fns: Vec::new(),
            canvas_frozen: false,
            nesting: 0,
            floor: 0,
            keys_run: 0,
            first_char_syntax: false,
            datavis: Vec::new(),
            pgf_path: basic::PgfPath::default(),
            plot_stream: Vec::new(),
            decoration_envs: Vec::new(),
            named_paths: HashMap::new(),
            last_node: None,
            last_node_name: None,
            tree_count: 0,
            chain_list: Default::default(),
            math_kinds: Default::default(),
            after_node_path: Vec::new(),
            local_boxes: Vec::new(),
            interrupted_boxes: Vec::new(),
            interrupted_paths: Vec::new(),
            animate_frames: Vec::new(),
            path_fadings: Vec::new(),
            carried_fadings: Vec::new(),
            canvas_transforms: 0,
            path_fading_settings: Vec::new(),
            turn_from: Default::default(),
            subpath_start: Default::default(),
            last_xy: Default::default(),
            tree_edge_out: None,
            behind_path: Vec::new(),
            tree_child_names: Vec::new(),
            forced_box: None,
            text_shading: None,
            text_picture: None,
            fadings: HashMap::new(),
            picture_fadings: HashMap::new(),
            basic_shadings: HashMap::new(),
            functional_shadings: HashMap::new(),
            plot_marks_declared: HashMap::new(),
            plot_handlers: HashMap::new(),
            saved_paths: HashMap::new(),
            pgf_solutions: Vec::new(),
            anim_shifts: Vec::new(),
            sys_animations: HashMap::new(),
            user_patterns: HashMap::new(),
            user_shapes: HashMap::new(),
            shape_capture: None,
            extension_shapes: HashMap::new(),
            later_node: None,
            scope_ends: Vec::new(),
            brace_scopes: Vec::new(),
            layer_order: vec!["background".into(), "main".into()],
            layers: HashMap::new(),
            on_layer: Vec::new(),
            boxes: Vec::new(),
            layout: Vec::new(),
            words: Vec::new(),
            saved_boxes: HashMap::new(),
            current_key: String::new(),
            axis: None,
            frame: None,
            plot_group: None,
            graphs: graphs::State::default(),
            spy_scopes: Vec::new(),
            facts: Default::default(),
        };
        for (name, cmd) in COMMANDS {
            e.tex.define(name, Meaning::Native(*cmd as u16));
        }
        for (k, name) in PGFMATH_FUNCTIONS.iter().enumerate() {
            let id = PGFMATH_FUNCTION_BASE + k as u16;
            e.tex.define(&format!("pgfmath{name}"), Meaning::Native(id));
            // pgfmath's inner forms (`\pgfmathadd@`), used by documents with @ a letter
            e.tex.define(&format!("pgfmath{name}@"), Meaning::Native(id));
        }
        // in a fading, transparent is black (`\colorlet{pgftransparent}{black}`)
        e.colors.insert("transparent".into(), Color::BLACK);
        e.colors.insert("pgftransparent".into(), Color::BLACK);
        e.tex.define_text(r"\let\bgroup={\let\egroup=}\def\pgfkeysnovalue{\pgfkeys@novalue}\def\pgfkeys@novalue{}\def\pgfkeysvalueof#1{\csname pgfk@#1\endcsname}\def\pgfkeysgetvalue#1#2{\expandafter\let\expandafter#2\csname pgfk@#1\endcsname}\def\pgfkeyssetvalue#1#2{\expandafter\def\csname pgfk@#1\endcsname{#2}}\def\pgfkeysifdefined#1{\ifcsname pgfk@#1\endcsname\expandafter\@firstoftwo\else\expandafter\@secondoftwo\fi}\def\pgfmathresult{}\def\tikzlastnode{}");
        basic::define(&mut e);
        nonlinear::define(&mut e);
        calendar::define(&mut e);
        folding::define(&mut e);
        mindmap::define(&mut e);
        lsystem::define(&mut e);
        options::define_keys(&mut e);
        datavis::define(&mut e);
        pgfplots::define(&mut e);
        graphs::define(&mut e);
        e
    }

    // ---------------------------------------------------------------- running

    pub fn warn(&mut self, s: impl Into<String>) {
        self.tex.warn(s);
    }

    /// Runs the input until it ends.
    pub fn run(&mut self) -> Result<(), Error> {
        while let Some(t) = self.tex.get_x_token()? {
            let statement = self.spans.is_some() && (matches!(self.tex.meaning(&t), Meaning::Native(id) if id < NATIVE_KEY_BASE) || overlays::named(&self.tex, &t).is_some());
            if statement {
                self.open_source();
            }
            self.execute(t)?;
            if statement {
                self.close_source();
            } else {
                self.tex.end_statement();
            }
        }
        Ok(())
    }

    /// The source map: what is drawn from here on came from the statement that starts here.
    fn open_source(&mut self) {
        let Some(span) = self.tex.statement_span() else { return };
        let Some(spans) = &mut self.spans else { return };
        spans.push(span);
        self.source = Some(spans.len() - 1);
        self.mark_source(self.source);
    }

    /// The source map: the statement ended where the reading is now.
    fn close_source(&mut self) {
        if let (Some(id), Some((_, end)), Some(spans)) = (self.source, self.tex.statement_span(), &mut self.spans) {
            spans[id].1 = end;
        }
        self.tex.end_statement();
    }

    /// What is drawn from here on came from this statement, kept for later (an `\\addplot`).
    pub fn mark_source(&mut self, source: Option<usize>) {
        if let Some(id) = source {
            self.canvas.push(Item::Source(id));
        }
    }

    /// Runs `tokens` to completion (a loop body, a style's code), then returns.
    pub fn run_tokens(&mut self, tokens: Vec<Token>) -> Result<(), Error> {
        if self.nesting >= MAX_NESTING {
            // a style or macro that calls itself: TeX would run out of memory
            self.warn(format!("styles or macros nested more than {MAX_NESTING} levels (one calls itself?): the rest ignored"));
            return Ok(());
        }
        self.nesting += 1;
        let depth = self.tex.depth();
        let outer_floor = std::mem::replace(&mut self.floor, depth);
        self.tex.push_frame(tokens);
        // the code ends where its tokens end: a number or a word at its end reads no further
        let floor = self.tex.seal(depth);
        let r = (|| {
            while let Some(t) = self.tex.get_x_token_until(depth)? {
                self.execute(t)?;
            }
            Ok(())
        })();
        self.tex.unseal(floor);
        if r.is_err() {
            self.tex.drain_to(depth);
        }
        self.floor = outer_floor;
        self.nesting -= 1;
        r
    }

    fn count_step(&mut self) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(Error::Figure(format!("figure too large: more than {MAX_STEPS} statements")));
        }
        Ok(())
    }

    pub fn execute(&mut self, t: Token) -> Result<(), Error> {
        let m = self.tex.meaning(&t);
        match m {
            Meaning::Char(_, Cat::Begin) if self.picture_depth > 0 => {
                // the scopes library: `{[options] …}` is a scope
                match self.tex.next_nonspace() {
                    Some(n) if n.is_char('[') => {
                        let o = self.tex.read_until_char(']', false)?;
                        self.brace_scopes.push(true);
                        let r = self.open_tikz_scope(Some(o), false);
                        self.recover(r, &t)?;
                    }
                    other => {
                        if let Some(n) = other {
                            self.tex.back(n);
                        }
                        self.brace_scopes.push(false);
                        self.begin_group();
                    }
                }
            }
            Meaning::Char(_, Cat::End) if self.picture_depth > 0 => {
                if self.brace_scopes.pop() == Some(true) {
                    let r = self.close_tikz_scope();
                    self.recover(r, &t)?;
                } else {
                    self.end_group();
                }
            }
            Meaning::Char(_, Cat::Begin) => self.begin_group(),
            Meaning::Char(_, Cat::End) => self.end_group(),
            // `$=\implies$` in the text around the pictures: a formula, set as a word
            Meaning::Char(_, Cat::Math) if self.picture_depth == 0 => {
                let mut formula = Vec::new();
                while let Some(n) = self.tex.get_token() {
                    if matches!(self.tex.meaning(&n), Meaning::Char(_, Cat::Math)) {
                        break;
                    }
                    formula.push(n);
                }
                if formula.iter().any(|x| x.is_cs("tikz") || x.is_cs("begin")) {
                    // a picture inside: read as it comes
                    self.tex.push_tokens(formula);
                    return Ok(());
                }
                let text = format!("${}$", tex::tokens_to_string(&formula));
                match self.layout.last() {
                    Some(Layout::Word(i)) => self.words[*i].1.push_str(&text),
                    _ => {
                        self.layout.push(Layout::Word(self.words.len()));
                        self.words.push((self.st.font.clone(), text));
                    }
                }
            }
            Meaning::Prim(Prim::Par) if self.picture_depth == 0 => {
                if !self.layout.is_empty() && self.layout.last() != Some(&Layout::Par) {
                    self.layout.push(Layout::Par);
                }
            }
            Meaning::Prim(Prim::Begingroup) => self.begin_group(),
            Meaning::Prim(Prim::Endgroup) => self.end_group(),
            Meaning::Native(id) if id >= NATIVE_KEY_BASE => {
                // a key's code used as a command: nothing to do outside key processing
            }
            Meaning::Native(id) => {
                // keys run by a key list are not statements, and their errors go to whoever runs the list
                if id == Cmd::Key as u16 || id == Cmd::SetPath as u16 {
                    return self.command(id, &t);
                }
                self.count_step()?;
                self.tex.new_statement();
                if overlays::takes_a_specification(id) {
                    let off = self.statement_is_off();
                    if matches!(off, Ok(true)) {
                        return Ok(());
                    }
                }
                let start = self.canvas.items.len();
                // (a statement run inside this one, as a node's text is, places only its own)
                let animations_before = self.animations.len();
                let r = if self.slide_hidden && self.hiding == 0 && self.picture_depth > 0 && !overlays::stays_out(id) {
                    self.hiding_around(|e| e.command(id, &t))
                } else {
                    self.command(id, &t)
                };
                if self.play {
                    if self.animations.len() > animations_before {
                        self.place_animations(start, id == Cmd::Begin as u16, animations_before);
                    }
                    if id == Cmd::End as u16 {
                        self.close_animated_scopes();
                    }
                }
                self.recover(r, &t)?;
            }
            Meaning::Char(c, _) if matches!(t, Token::Char(..)) => self.text(c),
            Meaning::Undefined => {
                if let Some(cmd) = overlays::named(&self.tex, &t) {
                    self.count_step()?;
                    self.tex.new_statement();
                    let r = self.overlay_command(cmd);
                    self.recover(r, &t)?;
                } else if let Token::Cs(name) = &t {
                    self.unknown_command(name)?;
                } else if matches!(t, Token::Char('~', Cat::Active)) && self.picture_depth == 0 {
                    // `~`: an interword space where the line cannot break
                    self.layout.push(Layout::Skip(SPACE));
                }
            }
            _ => {
                if !self.tex.execute_basic(t.clone())? {
                    if let Meaning::Char(c, _) = self.tex.meaning(&t) {
                        self.text(c);
                    }
                }
            }
        }
        Ok(())
    }

    /// A box of text in the paragraph, a word of its own (a `tabular`).
    pub fn typeset_word(&mut self, text: String) {
        self.layout.push(Layout::Word(self.words.len()));
        self.words.push((self.st.font.clone(), text));
    }

    /// Whether a `tabular` around the pictures is open.
    fn in_table(&self) -> bool {
        let opened = self.layout.iter().filter(|l| matches!(l, Layout::TabStart(_))).count();
        opened > self.layout.iter().filter(|l| matches!(l, Layout::TabEnd)).count()
    }

    /// A character outside any statement: TeX would typeset it; in a picture TikZ ignores it.
    fn text(&mut self, c: char) {
        if self.picture_depth == 0 && c == '&' && self.in_table() {
            self.layout.push(Layout::Cell);
            return;
        }
        if self.picture_depth == 0 && c.is_whitespace() {
            // a space after a picture or a word is TeX's interword space; spaces with commands
            // between them (`\tikzset{…}` on its own line) are glue each, so is one after `\hspace{…}`
            if matches!(self.layout.last(), Some(Layout::Picture(_) | Layout::Word(_) | Layout::Space | Layout::Skip(_))) {
                self.layout.push(Layout::Space);
            }
            return;
        }
        if self.picture_depth == 0 {
            // text of the paragraph, set word by word around the pictures
            match self.layout.last() {
                Some(Layout::Word(i)) => self.words[*i].1.push(c),
                _ => {
                    self.layout.push(Layout::Word(self.words.len()));
                    self.words.push((self.st.font.clone(), c.to_string()));
                }
            }
            return;
        }
        if c.is_whitespace() || c == ';' || self.text_reported {
            return;
        }
        if self.picture_depth > 0 {
            self.text_reported = true;
            self.warn("text outside a statement ignored");
        }
    }

    /// An undefined command: in a picture, the statement it starts is skipped to its `;`.
    fn unknown_command(&mut self, name: &str) -> Result<(), Error> {
        if self.picture_depth == 0 && name == "\\" {
            // `\\[2pt]`: the space it asks for is not kept
            if let Some(t) = self.tex.next_nonspace() {
                if t.is_char('[') {
                    let _ = self.tex.read_until_char(']', false)?;
                } else {
                    self.tex.back(t);
                }
            }
            self.layout.push(if self.in_table() { Layout::Row } else { Layout::Break });
            return Ok(());
        }
        if self.picture_depth == 0 && name == " " {
            // `\ `: an interword space, also after a command that eats the spaces after it
            self.layout.push(Layout::Space);
            return Ok(());
        }
        if self.picture_depth == 0 && TEXT_STYLES.contains(&name) {
            // `Ti\emph{k}Z`: set with the word it is in, as a node's text is
            let arg = self.argument()?;
            let text = format!("\\{name}{{{}}}", tex::tokens_to_string(&self.tex.expand_fully(arg)?));
            match self.layout.last() {
                Some(Layout::Word(i)) => self.words[*i].1.push_str(&text),
                _ => {
                    self.layout.push(Layout::Word(self.words.len()));
                    self.words.push((self.st.font.clone(), text));
                }
            }
            return Ok(());
        }
        if self.picture_depth > 0 {
            let rest = self.skip_statement()?;
            self.warn(format!("\\{name} not supported: '{}' ignored", rest.chars().take(40).collect::<String>().trim()));
        }
        Ok(())
    }

    /// Reads up to the `;` that ends a statement (outside braces), and returns it as text.
    pub fn skip_statement(&mut self) -> Result<String, Error> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        // not past the end of the token list being run
        while let Some(t) = self.tex.get_token_until(self.floor) {
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => {
                    if depth == 0 {
                        self.tex.back(t);
                        break;
                    }
                    depth -= 1;
                }
                Token::Char(';', _) if depth == 0 => break,
                _ => {}
            }
            out.push(t);
            if out.len() > tex::MAX_TOKENS {
                break;
            }
        }
        Ok(tex::tokens_to_string(&out))
    }

    // ---------------------------------------------------------------- groups and scopes

    pub fn begin_group(&mut self) {
        self.tex.begin_group();
        self.saved.push(self.st.clone());
    }

    pub fn end_group(&mut self) {
        if self.tex.end_group() {
            if let Some(s) = self.saved.pop() {
                self.st = s;
            }
            self.canvas.overlay = self.st.overlay;
            self.close_local_boxes();
        } else {
            self.warn("extra '}'");
        }
    }

    /// `local bounding box=name`: a rectangle node `name` over what is drawn from here to the end
    /// of the group.
    pub fn open_local_box(&mut self, name: String) {
        self.local_boxes.push((name, self.saved.len()));
        self.canvas.local.push((None, None));
    }

    /// The local bounding boxes of the groups that ended, as nodes.
    fn close_local_boxes(&mut self) {
        while self.local_boxes.last().is_some_and(|b| b.1 > self.saved.len()) {
            let (name, _) = self.local_boxes.pop().expect("checked");
            if let Some((Some(lo), Some(hi))) = self.canvas.local.pop() {
                self.nodes.insert(name, libraries::box_node(lo, hi));
            }
        }
    }

    pub fn begin_scope(&mut self) {
        self.scopes.push(PgfScope::default());
    }

    pub fn end_scope(&mut self) {
        if let Some(s) = self.scopes.pop() {
            for _ in 0..s.ends {
                self.canvas.push(Item::End);
            }
        }
    }

    /// Runs `code` unseen, as [`crate::extension::Context::measure`]: in a group and a scope of its
    /// own, on a canvas apart, `read` called at its end, and then the nodes, the named paths and the
    /// layers as they were before. The box is that of what `code` drew, on the canvas.
    pub fn run_unseen<T>(&mut self, code: Vec<Token>, read: impl FnOnce(&mut Engine) -> T) -> Result<(T, Option<(Point, Point)>), Error> {
        let saved_canvas = std::mem::take(&mut self.canvas);
        let saved_frozen = std::mem::replace(&mut self.canvas_frozen, false);
        let saved_layers = std::mem::take(&mut self.layers);
        let saved_behind = std::mem::take(&mut self.behind_path);
        let nodes = self.nodes.clone();
        let named_paths = self.named_paths.clone();
        let last_node = (self.last_node.clone(), self.last_node_name.clone());
        self.begin_group();
        self.begin_scope();
        let r = self.run_tokens(code);
        let value = r.is_ok().then(|| read(self));
        self.end_scope();
        self.end_group();
        let drawn = std::mem::replace(&mut self.canvas, saved_canvas);
        self.canvas_frozen = saved_frozen;
        self.layers = saved_layers;
        self.behind_path = saved_behind;
        self.nodes = nodes;
        self.named_paths = named_paths;
        (self.last_node, self.last_node_name) = last_node;
        r?;
        Ok((value.expect("read after the code ran"), drawn.min.zip(drawn.max)))
    }

    /// Opens a canvas item that the current pgf scope closes (a clip, a transparency group).
    pub fn open_in_scope(&mut self, item: Item) {
        let fading = matches!(item, Item::Fade { .. } | Item::FadePicture { .. });
        // a fading replaces the scope's fading, as pgf's graphic state holds one
        if fading && self.scopes.last().is_some_and(|s| s.fading_last && s.ends > 0) {
            self.canvas.push(Item::End);
            if let Some(s) = self.scopes.last_mut() {
                s.ends -= 1;
            }
        }
        self.canvas.push(item);
        match self.scopes.last_mut() {
            Some(s) => {
                s.ends += 1;
                s.fading_last = fading;
            }
            None => self.canvas.push(Item::End),
        }
    }

    // ---------------------------------------------------------------- reading arguments

    pub fn argument(&mut self) -> Result<Vec<Token>, Error> {
        self.tex.read_undelimited()
    }

    pub fn argument_string(&mut self) -> Result<String, Error> {
        let a = self.argument()?;
        Ok(self.tex.expand_to_string(a)?.trim().to_string())
    }

    /// `[…]` if it comes next (after spaces), without the brackets.
    pub fn optional(&mut self) -> Result<Option<Vec<Token>>, Error> {
        match self.tex.next_nonspace() {
            Some(t) if t.is_char('[') => Ok(Some(self.tex.read_until_char(']', false)?)),
            Some(t) => {
                self.tex.back(t);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// Text for pgfmath: macros expanded, registers replaced by their values.
    pub fn math_string(&mut self, tokens: Vec<Token>) -> Result<String, Error> {
        let mut t = self.tex.expand_fully(tokens)?;
        // `\numexpr …\relax` and `\dimexpr …\relax` read as TeX reads them where pgfmath reads a number
        while let Some(i) = t.iter().position(|x| matches!(x, Token::Cs(_)) && matches!(self.tex.meaning(x), Meaning::Prim(Prim::Numexpr | Prim::Dimexpr))) {
            let dim = self.tex.meaning(&t[i]) == Meaning::Prim(Prim::Dimexpr);
            let (v, rest) = self.tex.expr_of(t.split_off(i + 1), dim)?;
            t.pop();
            t.extend(tex::string_tokens(&if dim { tex::dimen::show(v) } else { v.to_string() }));
            t.extend(rest);
        }
        let mut s = String::new();
        for (i, x) in t.iter().enumerate() {
            match self.tex.meaning(x) {
                Meaning::Count(r) if matches!(x, Token::Cs(_)) => s.push_str(&self.tex.counts.get(&r).copied().unwrap_or(0).to_string()),
                Meaning::CharDef(v) if matches!(x, Token::Cs(_)) => s.push_str(&v.to_string()),
                Meaning::Dimen(r) | Meaning::Skip(r) if matches!(x, Token::Cs(_)) => {
                    // `2\baselineskip` is a product
                    if s.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
                        s.push('*');
                    }
                    let sp = self.tex.dimens.get(&r).copied().unwrap_or(0);
                    s.push_str(&format!("{}pt", sp as f64 / 65536.0));
                }
                // pgf's `\pgf@xx`, `\pgf@xy`, `\pgf@yx`, `\pgf@yy`: the parts of the x and y vectors, in pt
                Meaning::Undefined if matches!(x, Token::Cs(n) if matches!(&**n, "pgf@xx" | "pgf@xy" | "pgf@yx" | "pgf@yy")) => {
                    let Token::Cs(n) = x else { unreachable!() };
                    let v = if n.starts_with("pgf@x") { self.st.x } else { self.st.y };
                    let v = if n.ends_with('x') { v.x } else { v.y };
                    if s.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
                        s.push('*');
                    }
                    s.push_str(&format!("{}pt", v / crate::PT));
                }
                _ => s.push_str(&tex::tokens_to_string(&t[i..i + 1])),
            }
            // keep `\x y` apart when a control word is followed by a letter
            if let (Token::Cs(_), Some(Token::Char(c, _))) = (x, t.get(i + 1)) {
                if c.is_alphabetic() && !s.ends_with(' ') {
                    s.push(' ');
                }
            }
        }
        Ok(s)
    }

    pub fn vars(&self) -> crate::expr::Variables {
        let sans = self.st.font.contains("\\sffamily");
        crate::expr::Variables {
            values: HashMap::new(),
            functions: self.functions_now(),
            font_scale: Some(self.st.ambient_size / 10.0),
            in_pt: false,
            sans,
            trig_rad: self.st.trig_rad,
            fpu: false,
        }
    }

    /// The declared functions, the macros in their bodies replaced by what they hold now, as pgfmath
    /// reads a body when the function is used (`f(\q)=\q*\total;`, `\total` set later).
    fn functions_now(&self) -> HashMap<String, (Vec<String>, String)> {
        let mut functions = (*self.st.functions).clone();
        for (params, body) in functions.values_mut() {
            // a macro may hold others: a few rounds
            for _ in 0..8 {
                let mut out = String::new();
                let mut changed = false;
                let mut rest = body.as_str();
                while let Some(i) = rest.find('\\') {
                    out.push_str(&rest[..i]);
                    let name: String = rest[i + 1..].chars().take_while(|c| c.is_ascii_alphabetic() || *c == '@').collect();
                    match self.tex.meaning_of(&name) {
                        tex::Meaning::Macro(m) if !name.is_empty() && m.params.is_empty() && !params.contains(&name) => {
                            out.push_str(&tex::tokens_to_string(&tex::substitute(&m.body, &[])));
                            changed = true;
                        }
                        _ => {
                            out.push('\\');
                            out.push_str(&name);
                        }
                    }
                    rest = &rest[i + 1 + name.len()..];
                }
                out.push_str(rest);
                *body = out;
                if !changed {
                    break;
                }
            }
        }
        functions
    }

    /// A pgfmath number from text (a length in it counts in pt, as pgfmath counts it: `30pt` is 30).
    pub fn number(&mut self, text: &str) -> Result<f64, Error> {
        let t = tex::string_tokens(text);
        let s = self.math_string(t)?;
        let mut vars = self.vars();
        vars.in_pt = true;
        crate::expr::eval(&s, &vars)
    }

    /// A pgfmath length in cm (a number without a unit is in pt, as pgf reads lengths).
    pub fn length(&mut self, text: &str) -> Result<f64, Error> {
        let s = self.math_string(tikra_tokens(text))?;
        let (v, unit) = crate::expr::eval_length(&s, &self.vars())?;
        Ok(if unit { v } else { v * crate::PT })
    }

    pub fn number_of(&mut self, tokens: Vec<Token>) -> Result<f64, Error> {
        let s = self.math_string(tokens)?;
        crate::expr::eval(&s, &self.vars())
    }

    pub fn length_of(&mut self, tokens: Vec<Token>) -> Result<f64, Error> {
        let s = self.math_string(tokens)?;
        let (v, unit) = crate::expr::eval_length(&s, &self.vars())?;
        Ok(if unit { v } else { v * crate::PT })
    }

    pub fn color(&self, text: &str) -> Option<Color> {
        // `.` is xcolor's current color (`draw=.!60!black`)
        if text.split('!').any(|p| p.trim() == ".") {
            let mut colors = self.colors.clone();
            colors.insert(".".into(), self.st.color);
            return color::parse(text.trim(), &colors);
        }
        color::parse(text.trim(), &self.colors)
    }

    // ---------------------------------------------------------------- commands

    /// Whether the command `id` only computes with pgfmath (`\pgfmathparse`, `\pgfmathadd`…): code
    /// that may come before a point, as in `{\pgfmathrand\pgfpoint{…}{\pgfmathresult pt}}`.
    fn is_pgfmath(id: u16) -> bool {
        Self::pgfmath_function(id).is_some() || [Cmd::Pgfmathparse, Cmd::Pgfmathsetmacro, Cmd::Pgfmathtruncatemacro, Cmd::Pgfmathsetlengthmacro].iter().any(|c| *c as u16 == id)
    }

    /// The pgfmath function the command `id` computes, if it is one.
    fn pgfmath_function(id: u16) -> Option<&'static str> {
        PGFMATH_FUNCTIONS.get(usize::from(id.checked_sub(PGFMATH_FUNCTION_BASE)?)).copied()
    }

    /// `\pgfmathadd{a}{b}` and the other functions as commands: `\pgfmathresult` is their value.
    fn pgfmath_function_command(&mut self, name: &str) -> Result<(), Error> {
        let n = if matches!(name, "add" | "subtract" | "multiply" | "divide" | "max" | "min" | "mod" | "Mod" | "pow" | "veclen" | "atan2") { 2 } else { 1 };
        let mut args = Vec::new();
        for _ in 0..n {
            let a = self.argument()?;
            args.push(format!("({})", self.math_string(a)?));
        }
        let text = match name {
            "add" => format!("{}+{}", args[0], args[1]),
            "subtract" => format!("{}-{}", args[0], args[1]),
            "multiply" => format!("{}*{}", args[0], args[1]),
            "divide" => format!("{}/{}", args[0], args[1]),
            "neg" => format!("-{}", args[0]),
            other => format!("{other}({})", args.join(",")),
        };
        match crate::expr::eval_length(&text, &self.vars()).map(|(v, u)| if u { v / crate::PT } else { v }) {
            Ok(v) => self.tex.define_string("pgfmathresult", &pgf_number(v), false),
            Err(e) => self.warn(format!("\\pgfmath{name}: {e}")),
        }
        Ok(())
    }

    fn command(&mut self, id: u16, t: &Token) -> Result<(), Error> {
        if basic::is_basic(id) {
            return self.basic_command(id);
        }
        if id == Cmd::DataVis as u16 {
            return self.datavis_command(t);
        }
        if calendar::is_calendar(id) {
            return self.calendar_command(id);
        }
        if folding::is_folding(id) {
            return self.folding_command(id);
        }
        if mindmap::is_mindmap(id) {
            return self.mindmap_command(id);
        }
        if lsystem::is_lsystem(id) {
            return self.lsystem_command(id);
        }
        if pgfplots::is_command(id) {
            return self.pgfplots_command(id);
        }
        if let Some(name) = Self::pgfmath_function(id) {
            return self.pgfmath_function_command(name);
        }
        let Some(&(name, cmd)) = COMMANDS.iter().find(|(_, c)| *c as u16 == id) else { return Ok(()) };
        if self.axis.is_some() && pgfplots::is_deferred(name) {
            // inside an axis, TikZ statements wait until the axis is set up
            return self.defer_statement(t);
        }
        // in a frame of an animation after its first, a statement outside a picture is not drawn
        // (nor a `\tikz` picture: its options, then its statement)
        if self.picture_depth == 0
            && self.animate_frames.last().is_some_and(|a| a.past_first)
            && (path_command(cmd).is_some() || matches!(cmd, Cmd::Node | Cmd::Coordinate | Cmd::Matrix | Cmd::Pic | Cmd::Graph | Cmd::Tikz))
        {
            if cmd == Cmd::Tikz {
                let _ = self.optional()?;
            }
            let _ = self.next_statement()?;
            return Ok(());
        }
        if let Some(defaults) = path_command(cmd) {
            let r = path::statement(self, defaults);
            return self.recover(r, t);
        }
        match cmd {
            Cmd::Node => {
                // `\node[matrix]` is a `\matrix`
                if let Some(o) = self.tex.next_nonspace() {
                    let mut back = vec![o.clone()];
                    let mut matrix = false;
                    if o.is_char('[') {
                        let options = self.tex.read_until_char(']', false)?;
                        matrix = crate::keys::split_list(&options).iter().any(|i| i.key == "matrix" && i.value.is_none());
                        back.extend(options);
                        back.push(Token::other(']'));
                    }
                    self.tex.push_tokens(back);
                    if matrix {
                        return self.matrix();
                    }
                }
                self.tex.push_tokens(tikra_tokens("node "));
                let r = path::statement(self, "");
                self.recover(r, t)
            }
            Cmd::Coordinate => {
                self.tex.push_tokens(tikra_tokens("coordinate "));
                let r = path::statement(self, "");
                self.recover(r, t)
            }
            Cmd::Pic => {
                self.tex.push_tokens(tikra_tokens("pic "));
                let r = path::statement(self, "");
                self.recover(r, t)
            }
            Cmd::NamePrefix => {
                let p = self.argument_string()?;
                self.st.name_prefix = if p == ".." { String::new() } else { p };
                Ok(())
            }
            Cmd::Matrix => self.matrix(),
            Cmd::PgfMatrix => self.pgf_matrix(),
            Cmd::Chainin => self.chainin(),
            Cmd::TikzMath => self.tikzmath(),
            Cmd::DeclareFading => self.declare_fading(),
            Cmd::DeclareImage => self.declare_image(),
            Cmd::DeclarePatternForm => self.declare_pattern(false),
            Cmd::DeclarePatternColored => self.declare_pattern(true),
            Cmd::SetFillPattern => self.set_fill_pattern(),
            Cmd::TikzDeclarePattern => self.tikz_declare_pattern(true),
            Cmd::PgfDeclarePattern => self.tikz_declare_pattern(false),
            Cmd::ArrowsDeclare => self.arrows_declare(),
            Cmd::DeclareShape => self.declare_shape(),
            // `\pgfpositionnodelater{macro}`: the nodes of the group kept, not drawn (the macro,
            // which would save them, is not run: `\pgfpositionnodenow` places the last one)
            Cmd::PositionNodeLater => {
                let _ = self.argument()?;
                self.st.position_later = true;
                Ok(())
            }
            Cmd::PositionNodeNow => self.position_node_now(),
            Cmd::DeclareShadingH => self.declare_shading('h'),
            Cmd::DeclareShadingV => self.declare_shading('v'),
            Cmd::DeclareShadingR => self.declare_shading('r'),
            Cmd::UseShading => self.use_shading(),
            Cmd::ShadePath => self.shade_path(),
            Cmd::SetFading => self.set_fading(),
            Cmd::TransformNonlinear => self.transform_nonlinear(),
            Cmd::ApproximateNonlinear => self.approximate_nonlinear(true),
            Cmd::ApproximateNonlinearTranslation => self.approximate_nonlinear(false),
            Cmd::SetCurvilinear => self.set_curvilinear(),
            Cmd::PointCurvilinear => self.point_curvilinear(),
            Cmd::PointCurvilinearPolar => self.point_curvilinear_polar(),
            Cmd::MathSinCos => self.math_sincos(),
            Cmd::SetFadingForPath => self.set_fading_for_path(false),
            Cmd::SetFadingForPathStroked => self.set_fading_for_path(true),
            Cmd::SetSeed => {
                // \pgfmathsetseed: the integer part of the value
                let v = self.argument_string()?;
                let n: f64 = v.trim().parse().unwrap_or(1.0);
                crate::expr::set_seed(n.trunc() as i64);
                Ok(())
            }
            Cmd::DeclareRandomList => {
                // global, as pgf has it: the count in `\pgfmath@randomlist@name`, the items in
                // `\pgfmath@randomlist@name@1`… (`\pgfmathrandomitem` picks one)
                let name = self.argument_string()?;
                let list = self.argument()?;
                let mut items: Vec<Vec<Token>> = Vec::new();
                let mut depth = 0;
                for x in list {
                    match &x {
                        Token::Char(_, Cat::Begin) => {
                            depth += 1;
                            if depth == 1 {
                                items.push(Vec::new());
                                continue;
                            }
                        }
                        Token::Char(_, Cat::End) => {
                            depth -= 1;
                            if depth == 0 {
                                continue;
                            }
                        }
                        _ if depth == 0 => {
                            // an item without braces is its one token
                            if !x.is_space() {
                                items.push(vec![x]);
                            }
                            continue;
                        }
                        _ => {}
                    }
                    if let Some(last) = items.last_mut() {
                        last.push(x);
                    }
                }
                for (k, item) in items.iter().enumerate() {
                    self.tex.set_meaning(&Token::cs(&format!("pgfmath@randomlist@{name}@{}", k + 1)), macro_tokens(item.clone()), true);
                }
                self.tex.define_string(&format!("pgfmath@randomlist@{name}"), &items.len().to_string(), true);
                Ok(())
            }
            Cmd::Graph => {
                self.tex.push_tokens(tikra_tokens("graph "));
                let r = path::statement(self, "");
                self.recover(r, t)
            }
            Cmd::Tikz => self.tikz_command(),
            Cmd::Arrow => self.arrow_command(false),
            Cmd::ArrowReversed => self.arrow_command(true),
            Cmd::DecorationBegin => self.decoration_begin(),
            Cmd::DecorationEnd => self.decoration_end(),
            Cmd::DecorateCurrentPath => self.decorate_current_path(),
            Cmd::Spy => self.spy_command(),
            Cmd::SpyBegin => {
                self.spy_begin();
                Ok(())
            }
            Cmd::SpyEnd => self.spy_end(),
            Cmd::Begin => self.begin_environment(),
            Cmd::End => self.end_environment(),
            Cmd::Scoped => {
                let mut o = self.optional()?;
                // `\scoped :rotate = {…}`: animated options of the scope
                while let Some(t) = self.tex.next_nonspace() {
                    if !t.is_char(':') {
                        self.tex.back(t);
                        break;
                    }
                    if let Some(a) = self.read_animation()? {
                        let v = o.get_or_insert_with(Vec::new);
                        if !v.is_empty() {
                            v.push(Token::other(','));
                        }
                        // a scope's transformation is animated on the canvas, text and all
                        let name = tex::tokens_to_string(&a);
                        if ["rotate", "shift", "xshift", "yshift", "scale", "xscale", "yscale", "xslant", "yslant"].iter().any(|k| name.starts_with(&format!("{k}="))) {
                            v.extend(tikra_tokens("transform canvas="));
                            v.push(Token::Char('{', Cat::Begin));
                            v.extend(a);
                            v.push(Token::Char('}', Cat::End));
                        } else {
                            v.extend(a);
                        }
                    }
                }
                self.open_tikz_scope(o, false)?;
                // the next statement or group is the content
                let body = self.next_statement()?;
                self.run_tokens(body)?;
                self.close_tikz_scope()
            }
            Cmd::Tikzset => {
                let a = self.argument()?;
                self.keys_command(a, Some("/tikz"), true)?;
                // `\tikzset{xshift=1cm}` in a picture transforms what follows in the group
                if self.picture_depth > 0 {
                    self.st.apply_pending();
                }
                Ok(())
            }
            Cmd::Pgfset => {
                let a = self.argument()?;
                self.keys_command(a, Some("/pgf"), true)
            }
            Cmd::Pgfplotsset => {
                let a = self.argument()?;
                self.keys_command(a, Some("/pgfplots"), true)
            }
            Cmd::Pgfkeys => {
                let a = self.argument()?;
                self.keys_command(a, Some("/"), true)
            }
            Cmd::Pgfqkeys => {
                let p = self.argument_string()?;
                let a = self.argument()?;
                self.keys_command(a, Some(&p), true)
            }
            Cmd::Pgfkeysalso => {
                let a = self.argument()?;
                self.keys_command(a, None, false)
            }
            Cmd::Tikzstyle => {
                // \tikzstyle{name}=[options], \tikzstyle{name}+=[options]; without braces the
                // name runs to the `=` (a `+` before it is part of the name)
                let name = match self.tex.next_nonspace() {
                    Some(t) if !matches!(t, Token::Char(_, Cat::Begin)) => {
                        let mut name = vec![t];
                        while let Some(t) = self.tex.get_token() {
                            if t.is_char('=') {
                                self.tex.back(t);
                                break;
                            }
                            name.push(t);
                        }
                        tex::tokens_to_string(&name)
                    }
                    t => {
                        if let Some(t) = t {
                            self.tex.back(t);
                        }
                        self.argument_string()?
                    }
                };
                let mut append = false;
                if let Some(x) = self.tex.next_nonspace() {
                    if x.is_char('+') {
                        append = true;
                    } else if !x.is_char('=') {
                        self.tex.back(x);
                    }
                }
                if append {
                    if let Some(x) = self.tex.next_nonspace() {
                        if !x.is_char('=') {
                            self.tex.back(x);
                        }
                    }
                }
                let o = self.optional()?.unwrap_or_default();
                let mut v = tikra_tokens(&format!("{name}/.{}=", if append { "append style" } else { "style" }));
                v.push(Token::Char('{', Cat::Begin));
                v.extend(o);
                v.push(Token::Char('}', Cat::End));
                self.keys_command(v, Some("/tikz"), true)
            }
            Cmd::Key => self.invoke_key_command(),
            Cmd::SetPath => {
                let p = self.argument_string()?;
                self.st.key_path = p;
                Ok(())
            }
            Cmd::Foreach => self.foreach(),
            Cmd::Definecolor => {
                // \definecolor{name}{model}{values}, with an optional [type] first
                let _ = self.optional()?;
                let name = self.argument_string()?;
                let model = self.argument_string()?;
                let values = self.argument_string()?;
                match color::define(&model, &values) {
                    Some(c) => {
                        self.colors.insert(name, c);
                    }
                    None => self.warn(format!("\\definecolor{{{name}}}{{{model}}}: model not supported")),
                }
                Ok(())
            }
            Cmd::Colorlet => {
                let _ = self.optional()?;
                let name = self.argument_string()?;
                let e = self.argument_string()?;
                match self.color(&e) {
                    Some(c) => {
                        self.colors.insert(name, c);
                    }
                    None => self.warn(format!("\\colorlet{{{name}}}: unknown color '{e}'")),
                }
                Ok(())
            }
            Cmd::Color => {
                let _ = self.optional()?;
                let e = self.argument_string()?;
                if let Some(c) = self.color(&e) {
                    self.st.color = c;
                    self.st.draw_color = None;
                    self.st.fill_color = None;
                    self.st.text_color = None;
                }
                // a `\color` of the page's text, in no group: it is still on at the paragraph's end,
                // and `varwidth` takes its whole width (a color's end is not text it can measure)
                if let (0, Some(w), Some(d)) = (self.picture_depth, self.varwidth, self.document_depth) {
                    if self.tex.group_depth() == d && !self.layout.contains(&Layout::Width(w)) {
                        self.layout.push(Layout::Width(w));
                    }
                }
                Ok(())
            }
            Cmd::Pgfqbox => {
                // `\pgfqbox{\box}`: a box put in the picture, outside its size
                let b = self.argument()?;
                self.warn(format!("\\pgfqbox{{{}}}: the box is not drawn", tex::tokens_to_string(&b).trim()));
                Ok(())
            }
            Cmd::Pagecolor => {
                // `\pagecolor{olive!50}` or `\pagecolor[RGB]{200,30,30}`
                let model = self.optional()?.map(|m| tex::tokens_to_string(&m));
                let spec = self.argument_string()?;
                match model.map_or_else(|| self.color(&spec), |m| color::define(&m, &spec)) {
                    Some(c) => self.page_color = Some(c),
                    None => self.warn(format!("\\pagecolor: unknown color '{spec}'")),
                }
                Ok(())
            }
            Cmd::Nopagecolor => {
                self.page_color = None;
                Ok(())
            }
            Cmd::Textcolor => {
                // text, typeset by TeX: outside nodes it is ignored
                let _ = self.argument()?;
                let _ = self.argument()?;
                Ok(())
            }
            Cmd::Pgfmathsetmacro | Cmd::Pgfmathtruncatemacro | Cmd::Pgfmathsetlengthmacro => {
                let name = self.argument()?;
                let expr = self.argument()?;
                let Some(name) = name.into_iter().find(|x| !x.is_space()) else { return Ok(()) };
                let text = self.math_string(expr)?;
                // lengths in pt, as pgfmath counts them
                let mut vars = self.vars();
                vars.in_pt = true;
                if cmd == Cmd::Pgfmathsetmacro {
                    if let Ok(Some(s)) = crate::expr::eval_string(&text, &vars) {
                        self.tex.set_meaning(&name, macro_from(&s), false);
                        self.tex.define_string("pgfmathresult", &s, false);
                        return Ok(());
                    }
                }
                let v = crate::expr::eval_length(&text, &vars).map(|(v, _)| v);
                match v {
                    Ok(v) if v.is_finite() => {
                        let s = match cmd {
                            Cmd::Pgfmathtruncatemacro => format!("{}", v.trunc() as i64),
                            Cmd::Pgfmathsetlengthmacro => format!("{}pt", pgf_number(v)),
                            _ => pgf_number(v),
                        };
                        self.tex.set_meaning(&name, macro_from(&s), false);
                        self.tex.define_string("pgfmathresult", &s, false);
                    }
                    Ok(v) => self.warn(format!("\\pgfmathsetmacro{{{}}}: non-finite result ({v}), macro not defined", tex::show_token(&name))),
                    Err(e) => self.warn(format!("\\pgfmathsetmacro{{{}}}: {e}", tex::show_token(&name))),
                }
                Ok(())
            }
            Cmd::Pgfmathparse => {
                let expr = self.argument()?;
                let text = self.math_string(expr)?;
                let mut vars = self.vars();
                vars.in_pt = true;
                let value = match crate::expr::eval_string(&text, &vars) {
                    Ok(Some(s)) => Ok(s),
                    Ok(None) => crate::expr::eval_length(&text, &vars).map(|(v, _)| pgf_number(v)),
                    Err(e) => Err(e),
                };
                match value {
                    Ok(v) => self.tex.define_string("pgfmathresult", &v, false),
                    Err(e) => self.warn(format!("\\pgfmathparse: {e}")),
                }
                Ok(())
            }
            Cmd::Pgfmathsetcount | Cmd::Pgfmathsetlength | Cmd::Pgfmathaddtolength | Cmd::Pgfmathsetcounter | Cmd::Pgfmathaddtocounter => {
                let target = self.argument()?;
                let expr = self.argument()?;
                let text = self.math_string(expr)?;
                let add = matches!(cmd, Cmd::Pgfmathaddtolength | Cmd::Pgfmathaddtocounter);
                // lengths in pt, as pgfmath counts them
                let mut vars = self.vars();
                vars.in_pt = true;
                // a LaTeX counter is the register `\c@name`; pgfmath sets it locally, unlike `\setcounter`
                let m = if matches!(cmd, Cmd::Pgfmathsetcounter | Cmd::Pgfmathaddtocounter) {
                    self.tex.meaning_of(&format!("c@{}", tex::tokens_to_string(&target).trim()))
                } else {
                    let Some(name) = target.iter().find(|x| !x.is_space()) else { return Ok(()) };
                    self.tex.meaning(name)
                };
                match m {
                    Meaning::Count(r) => {
                        // the value is truncated before it is added
                        let v = crate::expr::eval(&text, &vars)?.trunc() as i64;
                        let old = if add { self.tex.counts.get(&r).copied().unwrap_or(0) } else { 0 };
                        self.tex.set_count(r, old + v, false);
                    }
                    Meaning::Dimen(r) | Meaning::Skip(r) => {
                        let pt = crate::expr::eval_length(&text, &vars)?.0;
                        let old = if add { self.tex.dimens.get(&r).copied().unwrap_or(0) } else { 0 };
                        self.tex.set_dimen(r, old + (pt * 65536.0).round() as i64, false);
                    }
                    _ => self.warn(format!("{}: no register or counter '{}'", tex::show_token(t), tex::tokens_to_string(&target).trim())),
                }
                Ok(())
            }
            Cmd::Pgfmathdeclarefunction => {
                // \pgfmathdeclarefunction{name}{n}{code}: code sets \pgfmathresult; only `\pgfmathparse{expr}` bodies are understood
                let name = self.argument_string()?;
                let n: usize = self.argument_string()?.parse().unwrap_or(0);
                let body = self.argument()?;
                let text = tex::tokens_to_string(&body);
                let expr = text.split("\\pgfmathparse").nth(1).map(|r| r.trim().trim_start_matches('{').trim_end_matches('}').to_string());
                match expr {
                    Some(e) => {
                        // #1, #2… become variables the expressions read (`\tikraarga`…)
                        let params: Vec<String> = (1..=n.min(9)).map(|k| format!("tikraarg{}", (b'a' + k as u8 - 1) as char)).collect();
                        let mut e = e;
                        for (k, p) in params.iter().enumerate() {
                            let var = format!("\\{p}");
                            e = e.replace(&format!("##{}", k + 1), &var).replace(&format!("#{}", k + 1), &var);
                        }
                        Rc::make_mut(&mut self.st.functions).insert(name, (params, e));
                    }
                    None => self.warn(format!("\\pgfmathdeclarefunction{{{name}}}: only a \\pgfmathparse body is supported")),
                }
                Ok(())
            }
            Cmd::Pgfmathprintnumber => {
                let o = self.optional()?;
                let a = self.argument()?;
                let v = self.number_of(a)?;
                let saved = self.st.number_format.clone();
                if let Some(o) = o {
                    let path = self.st.key_path.clone();
                    self.process_keys(o, Some("/pgf/number format"))?;
                    self.st.key_path = path;
                }
                let s = self.st.number_format.print(v);
                self.st.number_format = saved;
                self.tex.push_tokens(tex::string_tokens(&s));
                Ok(())
            }
            Cmd::Usetikzlibrary | Cmd::Usepackage | Cmd::Documentclass => {
                let o = if cmd == Cmd::Documentclass { self.documentclass_options() } else { self.optional()? };
                // the name in braces (a broken \documentclass[ may have none)
                let a = match self.tex.next_nonspace() {
                    Some(Token::Char(_, Cat::Begin)) => {
                        let body = self.tex.read_group_body()?;
                        self.tex.expand_to_string(body)?
                    }
                    Some(t) => {
                        self.tex.back(t);
                        String::new()
                    }
                    None => String::new(),
                };
                // the mindmap library sets `/pgf/decoration/angle=20` as it loads, for every decoration after
                if cmd == Cmd::Usetikzlibrary && a.split(',').any(|l| l.trim() == "mindmap") {
                    self.st.decoration.angle = 20.0;
                }
                if cmd == Cmd::Usetikzlibrary && a.split(',').any(|l| l.trim() == "positioning") {
                    self.st.positioning = true;
                }
                // `\usepackage[T1]{fontenc}`: text kerned as the T1 fonts kern it (the last encoding
                // listed is the one in force)
                if cmd == Cmd::Usepackage && a.split(',').any(|p| p.trim() == "fontenc") {
                    let encodings = o.as_ref().map(|o| self.tex.expand_to_string(o.clone())).transpose()?.unwrap_or_default();
                    let last = encodings.split(',').map(str::trim).rfind(|e| !e.is_empty()).unwrap_or("OT1");
                    crate::label::set_kerning(if last == "T1" { mathra::layout::kerning::TextKerning::T1 } else { mathra::layout::kerning::TextKerning::Ot1 });
                }
                // xcolor takes the class's options too (tikz loads xcolor)
                if cmd == Cmd::Documentclass || a.split(',').any(|p| p.trim() == "xcolor") {
                    if let Some(o) = &o {
                        let options = self.tex.expand_to_string(o.clone())?;
                        self.load_color_sets(&options);
                    }
                }
                // the paper's size, from the class options or geometry's
                if cmd == Cmd::Documentclass || (cmd == Cmd::Usepackage && a.split(',').any(|p| p.trim() == "geometry")) {
                    if let Some(o) = &o {
                        self.tex.paper_options(&tex::tokens_to_string(o));
                    }
                }
                // `\usepackage[active]{preview}`, then `\PreviewEnvironment{tikzpicture}`: each picture
                // is a page of its own, out of the boxes around it (`\scalebox`, `\resizebox`, `\fbox`)
                if cmd == Cmd::Usepackage && a.split(',').any(|p| p.trim() == "preview") {
                    let options = o.as_ref().map(|o| self.tex.expand_to_string(o.clone())).transpose()?.unwrap_or_default();
                    if options.split(',').any(|o| o.trim() == "active") {
                        let m = self.tex.meaning_of("tikra@previewenvironment");
                        self.tex.set_meaning(&Token::cs("PreviewEnvironment"), m, false);
                    }
                }
                if cmd == Cmd::Documentclass && a.contains("standalone") {
                    if let Some(o) = o {
                        self.standalone_options(o)?;
                    }
                }
                Ok(())
            }
            Cmd::PassOptionsToPackage => {
                let options = self.argument_string()?;
                let packages = self.argument_string()?;
                if packages.split(',').any(|p| p.trim() == "xcolor") {
                    self.load_color_sets(&options);
                }
                Ok(())
            }
            Cmd::OnLayer => {
                let name = self.argument_string()?;
                self.begin_on_layer(name);
                Ok(())
            }
            Cmd::EndOnLayer => {
                self.end_on_layer();
                Ok(())
            }
            Cmd::SetLayers => {
                let list = self.argument_string()?;
                self.layer_order = list.split(',').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                Ok(())
            }
            Cmd::Frame => self.frame_macros(),
            Cmd::Tikzfading => self.tikzfading(),
            Cmd::Newsavebox => self.new_save_box(),
            Cmd::Savebox => self.save_box(t.is_cs("savebox")),
            Cmd::Usebox => self.use_box(),
            Cmd::PreviewEnvironment => self.preview_environment(),
            Cmd::Unboxed => self.unboxed(t),
            Cmd::Ignore1 => {
                let _ = self.optional()?;
                let _ = self.argument()?;
                Ok(())
            }
            Cmd::SetBeamerTemplate => {
                let _ = self.argument()?;
                while self.optional()?.is_some() {}
                if let Some(t) = self.tex.next_nonspace() {
                    let group = matches!(t, Token::Char(_, tex::Cat::Begin));
                    self.tex.back(t);
                    if group {
                        let _ = self.argument()?;
                    }
                }
                Ok(())
            }
            Cmd::NewFrame => {
                // `\newframe*` pauses there, `\newframe[5]` changes the frame rate
                if let Some(t) = self.tex.next_nonspace() {
                    if !t.is_char('*') {
                        self.tex.back(t);
                    }
                }
                let _ = self.optional()?;
                self.end_first_frame()
            }
            // `\hspace{1cm}` (or `\hspace*`) between pictures: the space TeX leaves there
            // `\fbox{…}`: a frame of `\fboxrule` around the pictures, `\fboxsep` away
            Cmd::Fbox => {
                let arg = self.argument()?;
                let start = self.boxes.len();
                self.begin_group();
                let r = self.run_tokens(arg);
                self.end_group();
                r?;
                if self.picture_depth == 0 {
                    let sep = self.tex.dimens.get(&247).copied().unwrap_or(0) as f64 / 65536.0 * crate::PT;
                    let rule = self.tex.dimens.get(&246).copied().unwrap_or(0) as f64 / 65536.0 * crate::PT;
                    for b in &mut self.boxes[start..] {
                        let (Some(lo), Some(hi)) = (b.canvas.min, b.canvas.max) else { continue };
                        let d = sep + rule / 2.0;
                        let (lo, hi) = (Point::new(lo.x - d, lo.y - d), Point::new(hi.x + d, hi.y + d));
                        use crate::scene::Segment as S;
                        let segments = vec![S::MoveTo(lo), S::LineTo(Point::new(hi.x, lo.y)), S::LineTo(hi), S::LineTo(Point::new(lo.x, hi.y)), S::Close];
                        let stroke = crate::canvas::Stroke {
                            color: crate::color::Color::BLACK,
                            width: rule,
                            dash: None,
                            cap: crate::canvas::Cap::Butt,
                            join: crate::canvas::Join::Miter,
                            miter_limit: 10.0,
                            opacity: 1.0,
                        };
                        b.canvas.items.push(Item::Path { segments, fill: None, stroke: Some(stroke) });
                        b.canvas.include(Point::new(lo.x - rule / 2.0, lo.y - rule / 2.0));
                        b.canvas.include(Point::new(hi.x + rule / 2.0, hi.y + rule / 2.0));
                    }
                }
                Ok(())
            }
            // `\scalebox{0.7}[1]{…}`: the pictures in it scaled, by the second factor up (the first
            // without it), around their origin
            Cmd::Scalebox => {
                let fx = self.argument_string()?;
                let fx = self.number(&fx)?;
                let fy = match self.optional()? {
                    Some(t) => {
                        let t = self.tex.expand_to_string(t)?;
                        self.number(&t)?
                    }
                    None => fx,
                };
                let arg = self.argument()?;
                let start = self.boxes.len();
                self.begin_group();
                let r = self.run_tokens(arg);
                self.end_group();
                r?;
                if self.picture_depth == 0 {
                    self.scale_boxes(start, |_, _, _| (fx, fy));
                }
                Ok(())
            }
            // `\resizebox{7cm}{!}{…}`: the pictures in it scaled to that width and height (`!`
            // keeps the other's ratio); starred, the height is the height plus the depth
            Cmd::Resizebox => {
                let next = self.tex.next_nonspace();
                let star = next.as_ref().is_some_and(|t| t.is_char('*'));
                if let Some(t) = next.filter(|_| !star) {
                    self.tex.back(t);
                }
                let mut size = [None, None];
                for s in &mut size {
                    let t = self.argument_string()?;
                    if t.trim() != "!" {
                        match self.length(&t) {
                            Ok(v) => *s = Some(v),
                            // (`\width`, the box's own size, is not known here)
                            Err(_) => self.warn(format!("\\resizebox: size '{t}' not understood: not resized to it")),
                        }
                    }
                }
                let arg = self.argument()?;
                let start = self.boxes.len();
                self.begin_group();
                let r = self.run_tokens(arg);
                self.end_group();
                r?;
                if self.picture_depth == 0 {
                    self.scale_boxes(start, |width, height, depth| {
                        let fx = size[0].map(|x| x / width);
                        let fy = size[1].map(|y| y / if star { height + depth } else { height });
                        (fx.or(fy).unwrap_or(1.0), fy.or(fx).unwrap_or(1.0))
                    });
                }
                Ok(())
            }
            Cmd::Hspace => {
                match self.tex.next_nonspace() {
                    Some(t) if t.is_char('*') => {}
                    Some(t) => self.tex.back(t),
                    None => {}
                }
                let arg = self.argument()?;
                if self.picture_depth == 0 {
                    let w = self.length_of(arg)?;
                    self.layout.push(Layout::Skip(w));
                }
                Ok(())
            }
            // `\hskip 1cm plus 1fil`: glue of its natural width between pictures
            Cmd::Hskip | Cmd::Vskip => {
                let w = self.tex.scan_dimen()?;
                for k in ["plus", "minus"] {
                    if self.tex.scan_keyword(k)? {
                        let _ = self.tex.scan_dimen();
                        let _ = self.tex.scan_keyword("fill")? || self.tex.scan_keyword("fil")?;
                    }
                }
                if self.picture_depth == 0 && matches!(cmd, Cmd::Hskip) {
                    self.layout.push(Layout::Skip(w as f64 / 65536.0 * crate::PT));
                }
                // vertical glue in a paragraph ends it first, as `\par` does
                if self.picture_depth == 0 && matches!(cmd, Cmd::Vskip) && !self.layout.is_empty() && self.layout.last() != Some(&Layout::Par) {
                    self.layout.push(Layout::Par);
                }
                Ok(())
            }
            // the picture's size forgotten: what follows makes it anew
            Cmd::ResetBoundingBox => {
                (self.canvas.min, self.canvas.max) = (None, None);
                Ok(())
            }
            Cmd::Ignore2 => {
                let _ = self.argument()?;
                let _ = self.argument()?;
                Ok(())
            }
            Cmd::FontSize => {
                if let Token::Cs(n) = t {
                    self.st.font_size = font_size(n);
                    self.st.ambient_size = self.st.font_size;
                    self.st.font.push_str(&format!("\\{n}"));
                }
                Ok(())
            }
            Cmd::CheckUnit => {
                // \tikz@checkunit{value}: \iftikz@isdimension is true when the value, read by
                // pgfmath, has a unit (`1cm`, `2*3pt`, a length register), false for a plain number
                let a = self.argument()?;
                let s = self.math_string(a)?;
                let unit = crate::expr::eval_length(&s, &self.vars()).is_ok_and(|(_, unit)| unit);
                self.tex.set_meaning(&Token::cs("iftikz@isdimension"), Meaning::Prim(if unit { Prim::Iftrue } else { Prim::Iffalse }), false);
                Ok(())
            }
            Cmd::Quad => {
                // 1em and 2em of the 10pt font, as glue between the pictures
                if self.picture_depth == 0 {
                    let em = if t.is_cs("qquad") { 2.0 } else { 1.0 };
                    self.layout.push(Layout::Skip(em * 10.0 * crate::PT));
                }
                Ok(())
            }
            // the path commands are run above; the overlay commands are read by name, never defined:
            // what is left is `\bfseries` and the other font series
            other => {
                assert!(other == Cmd::FontSeries, "a path or overlay command");
                if let Token::Cs(n) = t {
                    self.st.font.push_str(&format!("\\{n}"));
                }
                Ok(())
            }
        }
    }

    /// An error in a statement: reported, and the rest of the statement skipped, as TeX goes on after an
    /// error. Limits (endless loops, a figure too large) stop the figure.
    fn recover(&mut self, r: Result<(), Error>, t: &Token) -> Result<(), Error> {
        match r {
            Ok(()) => Ok(()),
            Err(e) if is_limit(&e) => Err(e),
            Err(e) => {
                self.warn(format!("{}: {e}; statement ignored", tex::show_token(t)));
                let _ = self.skip_statement();
                Ok(())
            }
        }
    }

    /// `\documentclass[…]`, read with tolerance: a bracket left open ends before the next `\begin`.
    fn documentclass_options(&mut self) -> Option<Vec<Token>> {
        let t = self.tex.next_nonspace()?;
        if !t.is_char('[') {
            self.tex.back(t);
            return None;
        }
        let mut out = Vec::new();
        let mut depth = 0usize;
        while let Some(x) = self.tex.get_token() {
            match &x {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                x if depth == 0 && x.is_char(']') => return Some(out),
                x if x.is_cs("begin") => {
                    self.tex.back(x.clone());
                    return Some(out);
                }
                _ => {}
            }
            out.push(x);
        }
        Some(out)
    }

    /// The `border` option of `\documentclass[border=…]{standalone}`.
    /// xcolor's `dvipsnames`, `svgnames` and `x11names`: each defines its colors, over those of
    /// the same name before it (the options are taken in their order, the class's first).
    fn load_color_sets(&mut self, options: &str) {
        for set in options.split(',').filter_map(color::sets::Set::of_option) {
            self.colors.extend(set.colors().into_iter().map(|(n, c)| (n.to_string(), c)));
        }
    }

    fn standalone_options(&mut self, o: Vec<Token>) -> Result<(), Error> {
        for item in crate::keys::split_list(&o) {
            if item.key == "varwidth" {
                if let Some(v) = item.value {
                    let s = self.tex.expand_to_string(v)?;
                    if let Ok(w) = self.length(s.trim()) {
                        self.varwidth = Some(w);
                    }
                }
            } else if item.key == "border" {
                if let Some(v) = item.value {
                    let s = self.tex.expand_to_string(v)?;
                    let first = s.split_whitespace().next().unwrap_or("0pt").to_string();
                    if let Ok(m) = self.length(&first) {
                        self.canvas.margin = m;
                    }
                }
            }
        }
        Ok(())
    }

    /// The next statement in a picture: a `{…}` group, or tokens up to and including `;`.
    pub fn next_statement(&mut self) -> Result<Vec<Token>, Error> {
        match self.tex.next_nonspace() {
            Some(Token::Char(_, Cat::Begin)) => self.tex.read_group_body(),
            // a \foreach as the statement: its variables, options and list, then its own body
            // (`\pgffor@collectforeach`)
            Some(t) if t.is_cs("foreach") => {
                let mut out = vec![t];
                while let Some(x) = self.tex.get_token() {
                    if x.is_char('[') {
                        out.push(x);
                        out.extend(self.tex.read_until_char(']', false)?);
                        out.push(Token::other(']'));
                        continue;
                    }
                    let is_i = x.is_char('i');
                    out.push(x);
                    if is_i {
                        match self.tex.get_token() {
                            Some(n) if n.is_char('n') => {
                                out.push(n);
                                break;
                            }
                            Some(n) => self.tex.back(n),
                            None => break,
                        }
                    }
                }
                // the list (a `{…}` group or one token, a macro), and options after it
                match self.tex.next_nonspace() {
                    Some(b @ Token::Char(_, Cat::Begin)) => {
                        out.push(b);
                        out.extend(self.tex.read_group_body()?);
                        out.push(Token::Char('}', Cat::End));
                        // options after the list (`{1,2} [count=\i]`)
                        match self.tex.next_nonspace() {
                            Some(x) if x.is_char('[') => {
                                out.push(x);
                                out.extend(self.tex.read_until_char(']', false)?);
                                out.push(Token::other(']'));
                            }
                            Some(x) => self.tex.back(x),
                            None => {}
                        }
                    }
                    Some(x) => out.push(x),
                    None => {}
                }
                out.push(Token::Char(' ', Cat::Space));
                out.extend(self.next_statement_braced()?);
                Ok(out)
            }
            Some(t) => {
                self.tex.back(t);
                let mut out = Vec::new();
                let mut depth = 0usize;
                while let Some(x) = self.tex.get_token() {
                    match x {
                        Token::Char(_, Cat::Begin) => depth += 1,
                        Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    let end = depth == 0 && x.is_char(';');
                    out.push(x);
                    if end || out.len() > tex::MAX_TOKENS {
                        break;
                    }
                }
                Ok(out)
            }
            None => Ok(Vec::new()),
        }
    }

    /// The next statement, kept in its braces when it is a group (the body of a `\foreach`).
    fn next_statement_braced(&mut self) -> Result<Vec<Token>, Error> {
        match self.tex.next_nonspace() {
            Some(b @ Token::Char(_, Cat::Begin)) => {
                let mut out = vec![b];
                out.extend(self.tex.read_group_body()?);
                out.push(Token::Char('}', Cat::End));
                Ok(out)
            }
            Some(t) => {
                self.tex.back(t);
                self.next_statement()
            }
            None => Ok(Vec::new()),
        }
    }

    // ---------------------------------------------------------------- pictures and environments

    fn begin_picture(&mut self, o: Option<Vec<Token>>) -> Result<(), Error> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::Figure(format!("scope/\\foreach nesting went past {MAX_DEPTH} levels")));
        }
        self.depth += 1;
        if self.picture_depth == 0 {
            self.pictures += 1;
            // each picture is drawn on its own canvas, laid out with the others at the end
            let margin = self.canvas.margin;
            self.canvas = Canvas { margin, ..Canvas::default() };
            self.mark_source(self.source);
            self.picture_source = self.source;
            self.canvas_frozen = false;
            self.last_xy = Point::default();
        }
        self.picture_depth += 1;
        self.begin_group();
        self.begin_scope();
        self.run_tokens(tikra_tokens("\\tikz@picture@start"))?;
        if let Some(o) = o {
            // the picture's own options failing bring the figure down, as in TeX
            self.process_keys(o, Some("/tikz")).map_err(|e| Error::Figure(format!("picture options: {e}")))?;
        }
        self.st.apply_pending();
        self.open_transparency_group();
        // a picture is a scope too: `execute at begin scope` first, its end last
        let mut begin = std::mem::take(&mut self.st.at_begin_scope);
        begin.append(&mut self.st.at_begin_picture);
        let mut end = std::mem::take(&mut self.st.at_end_picture);
        end.append(&mut self.st.at_end_scope);
        self.scope_ends.push(end);
        self.run_tokens(begin)
    }

    /// `transparency group` given to a scope or a picture: what it draws is a group of its own.
    fn open_transparency_group(&mut self) {
        if std::mem::take(&mut self.st.transparency_group) {
            // the scope's opacity is the group's; inside, everything is opaque
            let opacity = self.st.fill_opacity.min(self.st.draw_opacity);
            self.open_in_scope(Item::Group { opacity });
            self.st.draw_opacity = 1.0;
            self.st.fill_opacity = 1.0;
            self.st.text_opacity = 1.0;
        }
    }

    /// `\begin{scope}[options]` and `\scoped`: a group, a pgf scope, the options, `execute at begin scope`.
    fn open_tikz_scope(&mut self, o: Option<Vec<Token>>, fatal: bool) -> Result<(), Error> {
        self.begin_group();
        self.begin_scope();
        self.run_tokens(tikra_tokens("\\tikz@scope@options"))?;
        if let Some(o) = o {
            let r = self.process_keys(o, Some("/tikz"));
            if fatal {
                r.map_err(|e| Error::Figure(format!("scope options: {e}")))?;
            } else {
                r?;
            }
        }
        self.st.apply_pending();
        self.open_transparency_group();
        let begin = std::mem::take(&mut self.st.at_begin_scope);
        let end = std::mem::take(&mut self.st.at_end_scope);
        self.scope_ends.push(end);
        self.run_tokens(begin)
    }

    /// The first frame of the open animation ends (at its first `\newframe`, or at its end): its
    /// `end` code runs, and the frames after it are not drawn.
    fn end_first_frame(&mut self) -> Result<(), Error> {
        let Some(a) = self.animate_frames.last_mut().filter(|a| !a.past_first) else { return Ok(()) };
        a.past_first = true;
        let end = std::mem::take(&mut a.end);
        self.run_tokens(end)
    }

    /// The rest of an environment `name` read and dropped, up to its `\end` (those nested in it
    /// skipped with it).
    fn skip_environment(&mut self, name: &str) -> Result<(), Error> {
        let mut depth = 0usize;
        while let Some(t) = self.tex.get_token() {
            let (open, close) = (t.is_cs("begin"), t.is_cs("end"));
            if !open && !close {
                continue;
            }
            let Some(g) = self.tex.next_nonspace() else { break };
            if !matches!(g, Token::Char(_, Cat::Begin)) {
                self.tex.back(g);
                continue;
            }
            let body = self.tex.read_group_body()?;
            if tex::tokens_to_string(&body).trim() != name {
                continue;
            }
            if open {
                depth += 1;
            } else if depth == 0 {
                return Ok(());
            } else {
                depth -= 1;
            }
        }
        self.warn(format!("\\begin{{{name}}} without \\end: skipped to the end"));
        Ok(())
    }

    /// `\pgfinterruptpath`: a group with no path being built, as if none had begun.
    pub fn interrupt_path(&mut self) {
        self.begin_group();
        let path = std::mem::take(&mut self.pgf_path);
        self.interrupted_paths.push(path);
    }

    /// `\endpgfinterruptpath`: the path that was being built comes back, whatever was built inside.
    pub fn end_interrupt_path(&mut self) {
        if let Some(path) = self.interrupted_paths.pop() {
            self.pgf_path = path;
            self.end_group();
        } else {
            self.warn("\\endpgfinterruptpath without \\pgfinterruptpath: ignored");
        }
    }

    fn close_tikz_scope(&mut self) -> Result<(), Error> {
        let end = self.scope_ends.pop().unwrap_or_default();
        let r = self.run_tokens(end);
        self.end_scope();
        self.end_group();
        r
    }

    /// `\tikzfading[name=…, top color=transparent!0, bottom color=transparent!100]`: a fading from
    /// shading options, with `transparent!N` meaning N percent transparent.
    fn tikzfading(&mut self) -> Result<(), Error> {
        let o = self.optional()?.unwrap_or_default();
        let mut name = String::new();
        let mut opts = Vec::new();
        for item in crate::keys::split_list(&o) {
            if item.key == "name" {
                name = self.tex.expand_to_string(item.value.unwrap_or_default())?.trim().to_string();
            } else {
                if !opts.is_empty() {
                    opts.push(Token::other(','));
                }
                opts.extend(tikra_tokens(&item.key));
                if let Some(v) = item.value {
                    opts.push(Token::other('='));
                    opts.push(Token::Char('{', Cat::Begin));
                    opts.extend(v);
                    opts.push(Token::Char('}', Cat::End));
                }
            }
        }
        self.begin_group();
        // transparent is black in a fading: N percent of it, the rest white
        let r = self.process_keys(opts, Some("/tikz"));
        // a unit square, so the shading's box is the identity once fitted
        let square = [crate::scene::Segment::MoveTo(Point::new(-25.0, -25.0)), crate::scene::Segment::LineTo(Point::new(25.0, 25.0))];
        let sh = self.shading_of(&square);
        self.end_group();
        r?;
        if !name.is_empty() {
            self.fadings.insert(name, sh);
        }
        Ok(())
    }

    /// `\pgfonlayer{name}`: what is drawn until `\endpgfonlayer` goes to that layer.
    fn begin_on_layer(&mut self, name: String) {
        self.begin_group();
        let main = std::mem::take(&mut self.canvas.items);
        self.on_layer.push((name, main));
        self.mark_source(self.source);
    }

    fn end_on_layer(&mut self) {
        let Some((name, main)) = self.on_layer.pop() else {
            self.warn("\\endpgfonlayer without \\pgfonlayer");
            return;
        };
        let drawn = std::mem::replace(&mut self.canvas.items, main);
        self.layers.entry(name).or_default().extend(drawn);
        self.end_group();
    }

    /// `\tikra@frame`: the picture's box, and with the frame seps around it, as macros in pt
    /// (`\tikra@xa`… inner, `\tikra@oxa`… outer) for the backgrounds library.
    fn frame_macros(&mut self) -> Result<(), Error> {
        // once per picture: the frame is around what the picture drew, not around another frame
        if matches!(self.tex.meaning_of("tikra@xa"), Meaning::Macro(_)) {
            return Ok(());
        }
        let (Some(lo), Some(hi)) = (self.canvas.min, self.canvas.max) else { return Ok(()) };
        let sep = |e: &mut Engine, name: &str| -> f64 {
            let v = match e.tex.meaning_of(name) {
                Meaning::Macro(m) => tex::tokens_to_string(&tex::substitute(&m.body, &[])),
                _ => "0pt".into(),
            };
            e.length(&v).unwrap_or(0.0)
        };
        let (fx, fy) = (sep(self, "tikz@framexsep"), sep(self, "tikz@frameysep"));
        let (ox, oy) = (sep(self, "tikz@outerframexsep"), sep(self, "tikz@outerframeysep"));
        let pt = |v: f64| format!("{}pt", v / crate::PT);
        for (name, v) in [
            ("tikra@xa", lo.x - fx),
            ("tikra@ya", lo.y - fy),
            ("tikra@xb", hi.x + fx),
            ("tikra@yb", hi.y + fy),
            ("tikra@oxa", lo.x - fx - ox),
            ("tikra@oya", lo.y - fy - oy),
            ("tikra@oxb", hi.x + fx + ox),
            ("tikra@oyb", hi.y + fy + oy),
        ] {
            self.tex.define_string(name, &pt(v), false);
        }
        Ok(())
    }

    fn end_picture(&mut self) {
        // execute at end picture, while the picture is still open
        let end = self.scope_ends.pop().unwrap_or_default();
        // what the picture's options ask for at its end comes from where they are
        let source = self.source;
        if self.picture_depth == 1 {
            self.source = self.picture_source;
            self.mark_source(self.source);
        }
        if let Err(e) = self.run_tokens(end) {
            self.warn(format!("execute at end picture: {e}"));
        }
        self.source = source;
        while !self.on_layer.is_empty() {
            self.end_on_layer();
        }
        self.end_scope();
        let outermost = self.picture_depth == 1;
        if outermost && !self.layers.is_empty() {
            // the layers, in the order of \pgfsetlayers (main where it is listed)
            let main = std::mem::take(&mut self.canvas.items);
            let mut layers = std::mem::take(&mut self.layers);
            let mut order = self.layer_order.clone();
            if !order.iter().any(|l| l == "main") {
                order.push("main".into());
            }
            let mut main = Some(main);
            for name in order {
                if name == "main" {
                    self.canvas.items.extend(main.take().unwrap_or_default());
                } else if let Some(v) = layers.remove(&name) {
                    self.canvas.items.extend(v);
                }
            }
            for (_, v) in layers {
                self.canvas.items.extend(v);
            }
        }
        let baseline = if outermost { self.picture_baseline() } else { 0.0 };
        if outermost {
            self.trim_picture();
        }
        self.end_group();
        self.picture_depth = self.picture_depth.saturating_sub(1);
        self.depth = self.depth.saturating_sub(1);
        if outermost {
            let margin = self.canvas.margin;
            let canvas = std::mem::replace(&mut self.canvas, Canvas { margin, ..Canvas::default() });
            // in a `\vbox` of another `\hsize`, the line is that wide
            let hsize = self.tex.dimens.get(&255).copied().unwrap_or(0);
            if hsize != 345 * 65536 && hsize > 0 {
                self.layout.push(Layout::Width(hsize as f64 / 65536.0 * crate::PT));
                if !self.tex.env_stack.iter().any(|n| n == "minipage") {
                    self.layout.push(Layout::Vbox);
                }
            }
            self.layout.push(Layout::Picture(self.boxes.len()));
            self.boxes.push(PictureBox { canvas, baseline });
        }
    }

    /// `trim left=x`, `trim right=x`: the picture's box starts or ends there (what is drawn beyond
    /// it sticks out, cut off with the page).
    fn trim_picture(&mut self) {
        for (spec, left) in [(self.st.trim_left.clone(), true), (self.st.trim_right.clone(), false)] {
            let Some(spec) = spec else { continue };
            let s = spec.trim();
            let x = match s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
                Some(inner) => self.coordinate(inner).map(|c| c.p.x),
                None if s.is_empty() => Ok(0.0),
                None => self.length(s),
            };
            let Ok(x) = x else { continue };
            match (left, self.canvas.min.as_mut(), self.canvas.max.as_mut()) {
                (true, Some(m), _) => m.x = x,
                (false, _, Some(m)) => m.x = x,
                _ => {}
            }
        }
    }

    /// Where the picture's baseline is (`baseline=`): by default its lower edge, as TikZ sets it.
    fn picture_baseline(&mut self) -> f64 {
        let lower = self.canvas.min.map_or(0.0, |m| m.y);
        let Some(spec) = self.st.baseline.clone() else { return lower };
        let s = spec.trim();
        if s.is_empty() {
            return 0.0;
        }
        if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
            return self.coordinate(inner).map(|c| c.p.y).unwrap_or(lower);
        }
        // \pgfsetbaselinepointlater{point}: the point's height once the picture is drawn
        if let Some(point) = s.strip_prefix("tikra@point:") {
            let t = self.st.transform;
            return self.point_of(tikra_tokens(point)).map(|p| t.apply(p).y).unwrap_or(lower);
        }
        self.length(s).unwrap_or(lower)
    }

    /// The pictures from `start` on scaled around their origins, by the factors `factors` gives
    /// for the size of all of them together: its width, and its height and depth at the first
    /// one's baseline.
    fn scale_boxes(&mut self, start: usize, factors: impl Fn(f64, f64, f64) -> (f64, f64)) {
        let corners: Vec<(Point, Point)> = self.boxes[start..].iter().filter_map(|b| b.canvas.min.zip(b.canvas.max)).collect();
        let Some(&(first_lo, first_hi)) = corners.first() else { return };
        let (lo, hi) = corners.iter().fold((first_lo, first_hi), |(a, b), (l, h)| (Point::new(a.x.min(l.x), a.y.min(l.y)), Point::new(b.x.max(h.x), b.y.max(h.y))));
        let baseline = self.boxes[start].baseline;
        let (fx, fy) = factors(hi.x - lo.x, hi.y - baseline, baseline - lo.y);
        let t = crate::scene::Transform::scaling(fx, fy);
        for b in &mut self.boxes[start..] {
            b.canvas.items.iter_mut().for_each(|i| i.transform(&t));
            let corners = [b.canvas.min, b.canvas.max];
            if let [Some(lo), Some(hi)] = corners {
                let (a, c) = (t.apply(lo), t.apply(hi));
                b.canvas.min = Some(Point::new(a.x.min(c.x), a.y.min(c.y)));
                b.canvas.max = Some(Point::new(a.x.max(c.x), a.y.max(c.y)));
            }
            b.baseline *= fy;
        }
    }

    /// `\tikz[options]{…}` or `\tikz[options] statement;`
    fn tikz_command(&mut self) -> Result<(), Error> {
        let o = self.optional()?;
        // a data visualization reads its data line by line: it reads its own input
        if let Some(t) = self.tex.next_nonspace() {
            let dv = t.is_cs("datavisualization");
            self.tex.back(t);
            if dv {
                let _ = self.tex.get_token();
                self.begin_picture(o)?;
                let r = self.datavisualization();
                self.end_picture();
                return r;
            }
        }
        let body = self.next_statement()?;
        self.begin_picture(o)?;
        self.run_tokens(body)?;
        self.end_picture();
        Ok(())
    }

    fn begin_environment(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        self.tex.env_stack.push(name.clone());
        match name.as_str() {
            // a frame of an animation after its first: not drawn, and its code not run
            "tikzpicture" if self.animate_frames.last().is_some_and(|a| a.past_first) => {
                self.tex.env_stack.pop();
                self.skip_environment("tikzpicture")
            }
            "tikzpicture" => {
                let o = self.optional()?;
                self.begin_picture(o)
            }
            // animate's `animateinline[options]{frame rate}`: its first frame is the picture, as in a
            // PDF before it plays (TeX's `poster`, which is the first frame unless said otherwise)
            // (its `begin` and `end` options open and close each frame: a picture, often)
            "animateinline" => {
                let o = self.optional()?.unwrap_or_default();
                let _ = self.argument()?;
                let (mut begin, mut end) = (Vec::new(), Vec::new());
                for item in crate::keys::split_list(&o) {
                    match item.key.as_str() {
                        "begin" => begin = tex::strip_outer_braces(item.value.unwrap_or_default()),
                        "end" => end = tex::strip_outer_braces(item.value.unwrap_or_default()),
                        _ => {}
                    }
                }
                self.animate_frames.push(AnimateFrames { past_first: false, end });
                self.begin_group();
                self.tex.push_tokens(begin);
                Ok(())
            }
            "pgfpicture" => self.begin_picture(None),
            "pgfscope" => {
                self.begin_group();
                self.begin_scope();
                Ok(())
            }
            "scope" => {
                if self.depth >= MAX_DEPTH {
                    return Err(Error::Figure(format!("scope/\\foreach nesting went past {MAX_DEPTH} levels")));
                }
                self.depth += 1;
                let o = self.optional()?;
                self.open_tikz_scope(o, true)
            }
            n if pgfplots::is_axis(n) => self.begin_axis(n),
            "groupplot" => self.begin_groupplot(),
            "pgfonlayer" => {
                let layer = self.argument_string()?;
                self.begin_on_layer(layer);
                Ok(())
            }
            "pgfviewboxscope" => self.begin_view_box_scope(),
            // a path of its own, the one being built kept for after it
            "pgfinterruptpath" => {
                self.interrupt_path();
                Ok(())
            }
            // what is drawn inside does not make the picture larger
            "pgfinterruptboundingbox" => {
                self.interrupted_boxes.push((self.canvas.min, self.canvas.max));
                self.begin_group();
                Ok(())
            }
            "tikzfadingfrompicture" => {
                // its \end is read with its body
                self.tex.env_stack.pop();
                self.fading_from_picture()
            }
            "lrbox" => {
                self.tex.env_stack.pop();
                self.lrbox()
            }
            "tabular" if self.picture_depth == 0 => {
                // `\begin{tabular}[pos]{rl}`: the pictures in the cells, set as TeX sets an \halign
                let _ = self.optional()?;
                let spec = self.argument_string()?;
                let cols: Vec<u32> = spec
                    .chars()
                    .filter_map(|c| match c {
                        'l' => Some(0),
                        'c' => Some(1),
                        'r' => Some(2),
                        _ => None,
                    })
                    .take(12)
                    .collect();
                let code = cols.iter().enumerate().fold(cols.len() as u32, |acc, (k, a)| acc | (a << (8 + 2 * k)));
                self.layout.push(Layout::TabStart(code));
                self.begin_group();
                Ok(())
            }
            // beamer's `frame<overlay>[options]{title}{subtitle}`: the title is the theme's to draw
            "frame" => {
                if let Some(t) = self.tex.next_nonspace() {
                    if t.is_char('<') {
                        let _ = self.tex.read_until_char('>', false)?;
                    } else {
                        self.tex.back(t);
                    }
                }
                while self.optional()?.is_some() {}
                for _ in 0..2 {
                    let Some(t) = self.tex.next_nonspace() else { break };
                    let group = matches!(t, Token::Char(_, tex::Cat::Begin));
                    self.tex.back(t);
                    if !group {
                        break;
                    }
                    let _ = self.argument()?;
                }
                self.begin_group();
                Ok(())
            }
            // `preview` (the preview package) only marks what goes on the page
            "document" | "center" | "figure" | "flushleft" | "flushright" | "minipage" | "preview" => {
                if name == "center" && self.picture_depth == 0 {
                    self.layout.extend([Layout::Par, Layout::Center(true)]);
                }
                if name == "minipage" {
                    let _ = self.optional()?;
                    let width = self.argument()?;
                    // the page is as wide as the minipage
                    if self.picture_depth == 0 {
                        let w = self.length_of(width)?;
                        self.layout.push(Layout::Width(w));
                    }
                } else if name == "figure" {
                    let _ = self.optional()?;
                }
                self.begin_group();
                if name == "document" {
                    self.document_depth = Some(self.tex.group_depth());
                }
                Ok(())
            }
            _ => {
                if let Some(code) = self.tex.environment_begin(&name)? {
                    self.begin_group();
                    self.tex.push_tokens(code);
                    return Ok(());
                }
                self.warn(format!("environment {name} not supported: content read as statements"));
                self.begin_group();
                Ok(())
            }
        }
    }

    fn end_environment(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        // the last frame of an animation ends as each does, before the animation
        if name == "animateinline" {
            self.end_first_frame()?;
        }
        let Some(pos) = self.tex.env_stack.iter().rposition(|n| *n == name) else {
            self.warn(format!("\\end{{{name}}} without a matching \\begin: ignored"));
            return Ok(());
        };
        // environments left open inside are closed with it
        while self.tex.env_stack.len() > pos + 1 {
            let inner = self.tex.env_stack.pop().unwrap_or_default();
            self.warn(format!("\\begin{{{inner}}} without \\end: closed at \\end{{{name}}}"));
            self.close_environment(&inner)?;
        }
        self.tex.env_stack.pop();
        self.close_environment(&name)
    }

    fn close_environment(&mut self, name: &str) -> Result<(), Error> {
        match name {
            "tikzpicture" | "pgfpicture" => self.end_picture(),
            n if pgfplots::is_axis(n) => self.end_axis()?,
            "groupplot" => self.end_groupplot()?,
            "pgfscope" | "pgfviewboxscope" => {
                self.end_scope();
                self.end_group();
            }
            "pgfonlayer" => self.end_on_layer(),
            "pgfinterruptpath" => self.end_interrupt_path(),
            "animateinline" => {
                self.animate_frames.pop();
                self.end_group();
            }
            "pgfinterruptboundingbox" => {
                self.end_group();
                if let Some((min, max)) = self.interrupted_boxes.pop() {
                    (self.canvas.min, self.canvas.max) = (min, max);
                }
            }
            "scope" => {
                self.close_tikz_scope()?;
                self.depth = self.depth.saturating_sub(1);
            }
            _ => {
                if let Some((_, end)) = self.tex.environments.get(name).cloned() {
                    self.run_tokens(end)?;
                }
                self.end_group();
                if name == "center" && self.picture_depth == 0 {
                    self.layout.extend([Layout::Par, Layout::Center(false)]);
                }
                if name == "tabular" && self.picture_depth == 0 {
                    self.layout.push(Layout::TabEnd);
                }
                // what follows `\end{document}` is never typeset
                if name == "document" {
                    self.tex.clear_input();
                }
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- \foreach

    /// A pgfmath expression whose value is text: a quoted string (`"-- "`), or `ifthenelse` (or
    /// `?:`) choosing between two. `None` for a number.
    fn math_text_value(&mut self, text: &str) -> Result<Option<String>, Error> {
        let t = text.trim();
        let mut t = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')).unwrap_or(t).trim();
        if !t.contains('"') {
            return Ok(None);
        }
        // `("a")`: the parentheses around the whole
        while let Some(inner) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
            let mut depth = 0i32;
            if inner.chars().any(|c| {
                depth += i32::from(c == '(') - i32::from(c == ')');
                depth < 0
            }) {
                break;
            }
            t = inner.trim();
        }
        if let Some(q) = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')).filter(|q| !q.contains('"')) {
            return Ok(Some(q.to_string()));
        }
        // `condition ? a : b`: the first `?` outside quotes and parentheses, and its `:` (a `?:` in
        // the second part nests)
        let (mut depth, mut quoted, mut question, mut nested) = (0, false, None, 0);
        for (i, c) in t.char_indices() {
            match c {
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth -= 1,
                '?' if !quoted && depth == 0 => {
                    if question.is_none() {
                        question = Some(i);
                    } else {
                        nested += 1;
                    }
                }
                ':' if !quoted && depth == 0 && question.is_some() => {
                    if nested > 0 {
                        nested -= 1;
                        continue;
                    }
                    let q = question.unwrap_or_default();
                    let pick = if self.number(&t[..q])? != 0.0 { &t[q + 1..i] } else { &t[i + 1..] };
                    return Ok(Some(self.math_text_value(pick)?.unwrap_or_else(|| pick.trim().to_string())));
                }
                _ => {}
            }
        }
        // `ifthenelse(condition, a, b)`: its arguments at the commas outside quotes and parentheses
        // (the closing parenthesis may be missing at the end, as pgfmath lets it)
        let Some(args) = t.strip_prefix("ifthenelse").map(str::trim).and_then(|a| a.strip_prefix('(')).map(|a| a.strip_suffix(')').unwrap_or(a)) else { return Ok(None) };
        let mut parts = vec![String::new()];
        let (mut depth, mut quoted) = (0, false);
        for c in args.chars() {
            match c {
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth -= 1,
                ',' if !quoted && depth == 0 => {
                    parts.push(String::new());
                    continue;
                }
                _ => {}
            }
            parts.last_mut().unwrap().push(c);
        }
        let [cond, a, b] = &parts[..] else { return Ok(None) };
        let pick = if self.number(cond)? != 0.0 { a } else { b };
        Ok(Some(self.math_text_value(pick)?.unwrap_or_else(|| pick.trim().to_string())))
    }

    /// The list of a `\foreach`, for `foreach_items`: a macro that holds it, expanded; or a group,
    /// whose items stay as they are written (`{\a,\b}` is two items, whatever `\a` holds) but for
    /// the ends of a `...` range.
    pub fn foreach_list(&mut self) -> Result<Vec<Token>, Error> {
        let list = match self.tex.next_nonspace() {
            Some(Token::Char(_, Cat::Begin)) => self.tex.read_group_body()?,
            Some(t) => return self.tex.expand_fully(vec![t]),
            None => return Ok(Vec::new()),
        };
        let items = foreach_split(&list);
        let dots = |i: usize| items.get(i).is_some_and(|p| tex::tokens_to_string(p).trim() == "...");
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push(Token::other(','));
            }
            // `\a,\b,...,\c`: the step comes from the two before the dots
            if dots(i + 1) || dots(i + 2) || (i > 0 && dots(i - 1)) {
                out.extend(self.tex.expand_fully(item.clone())?);
            } else {
                out.extend(item.iter().cloned());
            }
        }
        Ok(out)
    }

    /// `\foreach \x/\y [count=\i, evaluate=\x as \v using …, remember=\x as \lastx (initially 0)] in {list} body`
    fn foreach(&mut self) -> Result<(), Error> {
        // variables up to `[` or `in`
        let mut vars: Vec<Token> = Vec::new();
        let mut opts: Vec<Token> = Vec::new();
        loop {
            let Some(t) = self.tex.get_token() else { return Err(Error::Figure("\\foreach without 'in'".into())) };
            if t.is_char('[') {
                opts.extend(self.tex.read_until_char(']', false)?);
                continue;
            }
            if t.is_char('i') {
                if let Some(n) = self.tex.get_token() {
                    if n.is_char('n') {
                        break;
                    }
                    self.tex.back(n);
                }
            }
            vars.push(t);
            if vars.len() > 1000 {
                return Err(Error::Figure("\\foreach without 'in'".into()));
            }
        }
        let names: Vec<Token> = vars.iter().filter(|t| matches!(t, Token::Cs(_))).cloned().collect();
        let list = self.foreach_list()?;
        let items = foreach_items(&list);
        // options after the list too: \foreach \x in {…} [count=\i]
        if let Some(t) = self.tex.next_nonspace() {
            if t.is_char('[') {
                opts.extend(tex::string_tokens(","));
                opts.extend(self.tex.read_until_char(']', false)?);
            } else {
                self.tex.back(t);
            }
        }
        let body = self.next_statement()?;
        let o = foreach_options(&opts);
        if self.depth >= MAX_DEPTH {
            return Err(Error::Figure(format!("scope/\\foreach nesting went past {MAX_DEPTH} levels")));
        }
        self.depth += 1;
        // remembered values live across iterations
        let mut memory = o.initial_memory();
        for (k, item) in items.iter().enumerate() {
            self.count_step()?;
            self.begin_group();
            let parts = split_slash(item);
            for (i, name) in names.iter().enumerate() {
                let v = parts.get(i).or(parts.last()).cloned().unwrap_or_default();
                self.tex.set_meaning(name, macro_tokens(v), false);
            }
            self.foreach_iteration(&o, k, &memory)?;
            let r = self.run_tokens(body.clone());
            self.foreach_remember(&o, &mut memory);
            self.end_group();
            r?;
            // `\breakforeach`: this was the last iteration
            if matches!(self.tex.meaning_of("tikra@foreach@break"), Meaning::Macro(m) if !m.body.is_empty()) {
                self.tex.set_meaning(&Token::cs("tikra@foreach@break"), macro_tokens(Vec::new()), true);
                break;
            }
        }
        self.depth -= 1;
        Ok(())
    }

    /// What `\foreach`'s options set for iteration `k` (from 0): the count, the remembered values
    /// (`memory`, the last iteration's), the evaluated variables.
    pub fn foreach_iteration(&mut self, o: &ForeachOptions, k: usize, memory: &[Option<Vec<Token>>]) -> Result<(), Error> {
        if let Some((n, start)) = &o.count {
            self.tex.set_meaning(n, macro_from(&(start + k as i64).to_string()), false);
        }
        for ((_, as_name, _), m) in o.remember.iter().zip(memory) {
            if let Some(v) = m {
                self.tex.set_meaning(as_name, macro_tokens(v.clone()), false);
            }
        }
        for (var, target, using) in &o.evaluate {
            let expr = match using {
                Some(u) => u.clone(),
                None => vec![var.clone()],
            };
            let dest = target.clone().unwrap_or_else(|| var.clone());
            let text = self.tex.expand_to_string(expr.clone())?;
            if let Some(s) = self.math_text_value(&text)? {
                self.tex.set_meaning(&dest, macro_tokens(tikra_tokens(&s)), false);
                continue;
            }
            let v = self.number_of(expr)?;
            self.tex.set_meaning(&dest, macro_from(&pgf_number(v)), false);
        }
        Ok(())
    }

    /// The values to remember, after an iteration.
    pub fn foreach_remember(&self, o: &ForeachOptions, memory: &mut [Option<Vec<Token>>]) {
        for (i, (var, _, _)) in o.remember.iter().enumerate() {
            if let Meaning::Macro(m) = self.tex.meaning(var) {
                memory[i] = Some(tex::substitute(&m.body, &[]));
            }
        }
    }
}

/// `\foreach`'s options: `count=\i from 1`, `remember=\x as \last (initially v)`, `evaluate=\x as
/// \v using expr` (or `evaluate={\v=expr;}`).
#[derive(Default)]
pub struct ForeachOptions {
    count: Option<(Token, i64)>,
    remember: Vec<(Token, Token, Option<Vec<Token>>)>,
    evaluate: Vec<(Token, Option<Token>, Option<Vec<Token>>)>,
}

impl ForeachOptions {
    /// What the remembered variables start with (`initially`).
    pub fn initial_memory(&self) -> Vec<Option<Vec<Token>>> {
        self.remember.iter().map(|r| r.2.clone()).collect()
    }
}

/// The options of a `\foreach` (between its brackets).
pub fn foreach_options(opts: &[Token]) -> ForeachOptions {
    let options = crate::keys::split_list(opts);
    let mut count: Option<(Token, i64)> = None;
    let mut remember: Vec<(Token, Token, Option<Vec<Token>>)> = Vec::new();
    let mut evaluate: Vec<(Token, Option<Token>, Option<Vec<Token>>)> = Vec::new();
    for o in &options {
        let v = o.value.clone().unwrap_or_default();
        let parts: Vec<Token> = v.iter().filter(|t| !t.is_space()).cloned().collect();
        match o.key.as_str() {
            "count" => {
                // count=\i from 0
                let name = parts.first().cloned();
                let s = tex::tokens_to_string(&v);
                let start = s.split("from").nth(1).and_then(|x| x.trim().parse().ok()).unwrap_or(1);
                if let Some(n) = name {
                    count = Some((n, start));
                }
            }
            "remember" => {
                let s = tex::tokens_to_string(&v);
                let initially = s.split("initially").nth(1).map(|x| tex::string_tokens(x.trim().trim_start_matches('(').trim_end_matches(')')));
                let cs: Vec<Token> = v.iter().filter(|t| matches!(t, Token::Cs(_))).cloned().collect();
                if cs.len() >= 2 {
                    remember.push((cs[0].clone(), cs[1].clone(), initially));
                }
            }
            // `evaluate={\c=\x*10;}`: assignments, each a variable and its expression
            "evaluate" if !tex::tokens_to_string(&v).contains(" as ") && tex::tokens_to_string(&v).contains('=') => {
                let s = tex::tokens_to_string(&v);
                for statement in s.split(';') {
                    if let Some((name, expr)) = statement.split_once('=') {
                        if let Some(var) = tikra_tokens(name.trim()).into_iter().find(|t| matches!(t, Token::Cs(_))) {
                            evaluate.push((var.clone(), Some(var), Some(tikra_tokens(expr.trim()))));
                        }
                    }
                }
            }
            "evaluate" => {
                let s = tex::tokens_to_string(&v);
                let cs: Vec<Token> = v.iter().filter(|t| matches!(t, Token::Cs(_))).cloned().collect();
                let using = s.split("using").nth(1).map(|x| tikra_tokens(x.trim()));
                let target = if s.contains(" as ") { cs.get(1).cloned() } else { None };
                if let Some(var) = cs.first() {
                    evaluate.push((var.clone(), target, using));
                }
            }
            "parse" => {}
            _ => {}
        }
    }
    ForeachOptions { count, remember, evaluate }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// Errors that stop the whole figure: they guard against hostile input.
pub fn is_limit(e: &Error) -> bool {
    let s = e.to_string();
    s.contains("figure too large") || s.contains("scope/\\foreach nesting went past") || s.contains("picture options:") || s.contains("scope options:")
}

/// A macro without parameters holding text.
pub fn macro_from(s: &str) -> Meaning {
    macro_tokens(tex::string_tokens(s))
}

pub fn macro_tokens(v: Vec<Token>) -> Meaning {
    Meaning::Macro(Rc::new(tex::Macro { params: Vec::new(), body: v.into_iter().map(tex::Body::Tok).collect(), optional: None, protected: false }))
}

/// Text read as TeX reads it (control sequences, braces), for internal use.
pub fn tikra_tokens(text: &str) -> Vec<Token> {
    let mut t = Tex::new_lexer();
    t.push_text(text);
    let mut v = Vec::new();
    while let Some(x) = t.get_token() {
        v.push(x);
    }
    v
}

/// A number as pgfmath writes one: `2`, `0.5`, `-3.14159`.
pub fn pgf_number(v: f64) -> String {
    let r = (v * 100000.0).round() / 100000.0;
    if r.fract() == 0.0 && r.abs() < 1e15 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

fn font_size(name: &str) -> f64 {
    match name {
        "tiny" => 5.0,
        "scriptsize" => 7.0,
        "footnotesize" => 8.0,
        "small" => 9.0,
        "large" => 12.0,
        "Large" => 14.4,
        "LARGE" => 17.28,
        "huge" => 20.74,
        "Huge" => 24.88,
        _ => 10.0,
    }
}

/// `1/a` → [`1`, `a`]: an item split at its top-level slashes.
fn split_slash(item: &[Token]) -> Vec<Vec<Token>> {
    let mut parts = vec![Vec::new()];
    let mut depth = 0usize;
    for t in item {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char('/', _) if depth == 0 => {
                parts.push(Vec::new());
                continue;
            }
            _ => {}
        }
        if let Some(p) = parts.last_mut() {
            p.push(t.clone());
        }
    }
    parts.into_iter().map(|p| tex::strip_outer_braces(crate::keys::trim(&p).to_vec())).collect()
}

/// A `\foreach` list cut at its commas, the items as they are written.
fn foreach_split(list: &[Token]) -> Vec<Vec<Token>> {
    let mut raw: Vec<Vec<Token>> = Vec::new();
    let mut current: Vec<Token> = Vec::new();
    let mut depth = 0usize;
    // an item that starts with `(` runs to its `)` (`(0,0), (1,1)`), as pgffor reads it
    let mut parens = 0usize;
    for t in list {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char('(', _) if depth == 0 && (parens > 0 || current.iter().all(Token::is_space)) => parens += 1,
            Token::Char(')', _) if depth == 0 && parens > 0 => parens -= 1,
            Token::Char(',', _) if depth == 0 && parens == 0 => {
                raw.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(t.clone());
    }
    raw.push(current);
    raw
}

/// The items of a `\foreach` list, with `...` ranges expanded as pgffor does (numbers and letters).
pub fn foreach_items(list: &[Token]) -> Vec<Vec<Token>> {
    let raw: Vec<Vec<Token>> = foreach_split(list).into_iter().map(|p| tex::strip_outer_braces(crate::keys::trim(&p).to_vec())).filter(|p| !p.is_empty()).collect();
    let text = |v: &[Token]| tex::tokens_to_string(v).trim().to_string();
    let mut out: Vec<Vec<Token>> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if text(&raw[i]) == "..." && !out.is_empty() && i + 1 < raw.len() {
            let end_s = text(&raw[i + 1]);
            let last_s = text(&out[out.len() - 1]);
            let before = if out.len() >= 2 { Some(text(&out[out.len() - 2])) } else { None };
            // letters: a,...,e
            let letter = |s: &str| if s.chars().count() == 1 && s.chars().all(|c| c.is_ascii_alphabetic()) { s.chars().next() } else { None };
            if let (Some(a), Some(b)) = (letter(&last_s), letter(&end_s)) {
                let step: i32 = before.as_deref().and_then(letter).map_or(1, |p| a as i32 - p as i32).max(1);
                let mut c = a as i32 + step;
                while c <= b as i32 && out.len() < 100_000 {
                    out.push(tex::string_tokens(&char::from_u32(c as u32).unwrap_or('?').to_string()));
                    c += step;
                }
                i += 2;
                continue;
            }
            let (Ok(last), Ok(end)) = (last_s.parse::<f64>(), end_s.parse::<f64>()) else {
                i += 1;
                continue;
            };
            let step = before.and_then(|b| b.parse::<f64>().ok()).map_or(if end >= last { 1.0 } else { -1.0 }, |b| last - b);
            if step.abs() > 1e-12 {
                let mut k = 1.0;
                loop {
                    let v = last + step * k;
                    if (step > 0.0 && v > end + 1e-9) || (step < 0.0 && v < end - 1e-9) || out.len() > 100_000 {
                        break;
                    }
                    out.push(tex::string_tokens(&pgf_number(v)));
                    k += 1.0;
                }
            }
            i += 2;
            continue;
        }
        out.push(raw[i].clone());
        i += 1;
    }
    out
}

/// The text commands that change the style of their argument, set in the text around the pictures
/// as in a node's text.
const TEXT_STYLES: &[&str] = &["emph", "textit", "textbf", "textsl", "textsc", "texttt", "textsf", "textrm", "textup", "textmd", "textnormal"];

/// TeX's interword space and line spacing for Computer Modern at 10 pt, in cm.
const SPACE: f64 = 3.33333 * crate::PT;
const BASELINE_SKIP: f64 = 12.0 * crate::PT;
const LINE_SKIP: f64 = 1.0 * crate::PT;
/// LaTeX's `\topsep` and `\partopsep` at 10 pt (their natural sizes), in cm.
const TOPSEP: f64 = 8.0 * crate::PT;
const PARTOPSEP: f64 = 2.0 * crate::PT;

/// Lays the pictures out as TeX sets boxes in a paragraph: side by side on a baseline, a space
/// where the source had one, a new line at each paragraph; then one canvas with all of them.
/// `varwidth` (cm, of the `standalone` class): lines are broken at that width, and the page is as
/// wide as its widest line.
fn typeset(boxes: &[PictureBox], layout: &[Layout], words: &[(String, String)], margin: f64, varwidth: Option<f64>) -> Canvas {
    let mut out = Canvas { margin, ..Canvas::default() };
    // the words as TeX sets them, in the text font
    let mut warnings = Vec::new();
    let labels: Vec<crate::scene::Label> = words.iter().map(|(font, w)| crate::label::compose_label(w, font, None, 'l', crate::color::Color::BLACK, &mut warnings)).collect();
    // each `tabular` becomes a box of its own
    let (table_boxes, layout) = tables(boxes, layout, &labels);
    let boxes = &table_boxes[..];
    let layout = &layout[..];
    let width_of = |l: &Layout| match l {
        Layout::Picture(i) => boxes[*i].canvas.min.zip(boxes[*i].canvas.max).map_or(0.0, |(a, b)| b.x - a.x),
        Layout::Word(i) => labels[*i].width(),
        Layout::Skip(w) => *w,
        Layout::Space => SPACE,
        _ => 0.0,
    };
    // the lines: a paragraph ends one, so does `\\` (and `\\` then a paragraph's end leaves an
    // empty line); in `center` they are centered
    struct Line {
        items: Vec<Layout>,
        center: bool,
        empty_kept: bool,
        /// vertical glue above the line (`\addvspace`: the larger of two that meet)
        skip: f64,
    }
    let line = |center| Line { items: Vec::new(), center, empty_kept: false, skip: 0.0 };
    let mut lines = vec![line(false)];
    let mut center = false;
    let mut hsize: Option<f64> = None;
    let mut after_break = false;
    let mut vbox = false;
    // whether TeX is between paragraphs (vertical mode), and whether it was before the last
    // paragraph's end
    let (mut vmode, mut vmode_before_par) = (true, true);
    // `center` is a list: `\topsep` above and below it, `\partopsep` more if it begins a paragraph
    let mut topsep_add = 0.0;
    for l in layout {
        match l {
            Layout::Par => {
                // in `center`, `\\` ends the paragraph itself: the paragraph's end after it adds no line
                if after_break && !center {
                    lines.last_mut().expect("a line").empty_kept = true;
                }
                vmode_before_par = vmode;
                vmode = true;
                lines.push(line(center));
            }
            Layout::Break => {
                vmode = center;
                lines.push(line(center));
            }
            Layout::Center(c) => {
                center = *c;
                if center {
                    topsep_add = TOPSEP + if vmode_before_par { PARTOPSEP } else { 0.0 };
                }
                let last = lines.last_mut().expect("a line");
                last.center = center;
                last.skip = last.skip.max(topsep_add);
            }
            Layout::Vbox => vbox = true,
            other => {
                if let Layout::Width(w) = other {
                    hsize = Some(*w);
                }
                if matches!(other, Layout::Picture(_) | Layout::Word(_) | Layout::Skip(_)) {
                    vmode = false;
                }
                lines.last_mut().expect("a line").items.push(*other);
            }
        }
        after_break = matches!(l, Layout::Break) || (after_break && matches!(l, Layout::Center(_)));
    }
    // a line longer than the line's width (when known) is broken at its spaces
    if let Some(w) = hsize.or(varwidth) {
        let mut broken = Vec::new();
        for l in lines {
            // (`varwidth` alone: a line that fits is left as it is)
            if hsize.is_none() && l.items.iter().map(&width_of).sum::<f64>() <= w + 1e-9 {
                broken.push(l);
                continue;
            }
            let mut cur = Line { items: Vec::new(), center: l.center, empty_kept: l.empty_kept, skip: l.skip };
            let (mut x, mut pending) = (0.0, 0.0);
            let mut started = false;
            let mut chunk: Vec<Layout> = Vec::new();
            let mut chunk_w = 0.0;
            let flush = |cur: &mut Line, broken: &mut Vec<Line>, chunk: &mut Vec<Layout>, chunk_w: &mut f64, x: &mut f64, pending: &mut f64, started: &mut bool| {
                if chunk.is_empty() {
                    return;
                }
                let content = chunk.iter().any(|c| matches!(c, Layout::Picture(_) | Layout::Word(_)));
                if *started && content && *x + *pending + *chunk_w > w + 1e-9 {
                    let next = Line { items: Vec::new(), center: cur.center, empty_kept: false, skip: 0.0 };
                    broken.push(std::mem::replace(cur, next));
                    *x = 0.0;
                    *pending = 0.0;
                    // glue after a break is discarded (TeX keeps it only at a paragraph's start)
                    while let Some(&Layout::Skip(g)) = chunk.first() {
                        chunk.remove(0);
                        *chunk_w -= g;
                    }
                } else if *started {
                    cur.items.push(Layout::Space);
                    *x += *pending;
                }
                *pending = 0.0;
                cur.items.append(chunk);
                *x += *chunk_w;
                *chunk_w = 0.0;
                *started |= content;
            };
            for item in l.items {
                if matches!(item, Layout::Space) {
                    flush(&mut cur, &mut broken, &mut chunk, &mut chunk_w, &mut x, &mut pending, &mut started);
                    if started {
                        pending = SPACE;
                    }
                } else {
                    chunk_w += width_of(&item);
                    chunk.push(item);
                }
            }
            flush(&mut cur, &mut broken, &mut chunk, &mut chunk_w, &mut x, &mut pending, &mut started);
            broken.push(cur);
        }
        lines = broken;
    }
    let mut baseline_y = 0.0;
    let mut previous_depth: Option<f64> = None;
    let has_picture = layout.iter().any(|x| matches!(x, Layout::Picture(_)));
    let shown = |l: &Line| l.items.iter().any(|x| matches!(x, Layout::Picture(_))) || (has_picture && (l.empty_kept || l.items.iter().any(|x| matches!(x, Layout::Word(_)))));
    // the glue waiting above the next line (that of lines not shown too)
    let mut skip = 0.0f64;
    for line in &lines {
        skip = skip.max(line.skip);
        if !shown(line) {
            continue;
        }
        let (mut height, mut depth) = (0.0f64, 0.0f64);
        for l in &line.items {
            match l {
                Layout::Picture(i) => {
                    let b = &boxes[*i];
                    let (lo, hi) = (b.canvas.min.map_or(0.0, |m| m.y), b.canvas.max.map_or(0.0, |m| m.y));
                    height = height.max(hi - b.baseline);
                    depth = depth.max(b.baseline - lo);
                }
                Layout::Word(i) => {
                    height = height.max(labels[*i].height());
                    depth = depth.max(labels[*i].depth());
                }
                _ => {}
            }
        }
        if let Some(d) = previous_depth {
            let gap = BASELINE_SKIP - d - height;
            baseline_y -= skip + if gap >= 0.0 { BASELINE_SKIP } else { d + LINE_SKIP + height };
        }
        // (glue at the top of the box is dropped)
        skip = 0.0;
        // centered: what the line holds (from its first box) in the middle of its width
        let mut x = 0.0;
        if let (true, Some(w)) = (line.center, hsize) {
            let first = line.items.iter().position(|l| matches!(l, Layout::Picture(_) | Layout::Word(_) | Layout::Skip(_))).unwrap_or(line.items.len());
            let natural: f64 = line.items[first..].iter().map(&width_of).sum();
            x = ((w - natural) / 2.0).max(0.0);
        }
        let mut started = false;
        for l in &line.items {
            match l {
                Layout::Space if started => x += SPACE,
                // glue at the start of a paragraph stays (`\hskip 1cm` before a picture)
                Layout::Skip(w) => x += w,
                Layout::Width(w) => {
                    out.include(Point::new(0.0, baseline_y));
                    out.include(Point::new(*w, baseline_y));
                }
                Layout::Word(i) => {
                    let label = &labels[*i];
                    out.include(Point::new(x, baseline_y - label.depth()));
                    out.include(Point::new(x + label.width(), baseline_y + label.height()));
                    out.items.push(Item::Text { label: label.clone(), place: crate::scene::Transform::translation(x, baseline_y), opacity: 1.0 });
                    x += label.width();
                    started = true;
                }
                Layout::Picture(i) => {
                    let b = &boxes[*i];
                    let (Some(min), Some(max)) = (b.canvas.min, b.canvas.max) else {
                        // an empty picture is still where the line starts: the glue after it counts
                        if !started {
                            out.include(Point::new(x, baseline_y));
                        }
                        started = true;
                        continue;
                    };
                    let d = Point::new(x - min.x, baseline_y - b.baseline);
                    for item in &b.canvas.items {
                        let mut it = item.clone();
                        it.translate(d);
                        out.items.push(it);
                    }
                    out.named.extend(b.canvas.named.iter().map(|(n, p)| (n.clone(), p.plus(d))));
                    out.origin.get_or_insert(d);
                    // (what glue moved left of the line's start, `\hskip-1cm`, is off the page)
                    out.include(Point::new((min.x + d.x).max(0.0), min.y + d.y));
                    out.include(max.plus(d));
                    x += max.x - min.x;
                    started = true;
                }
                _ => {}
            }
        }
        previous_depth = Some(depth);
    }
    // glue at the end of a `\vbox` (that below a `center`) is part of its height; the page drops it
    if let (true, Some(d)) = (vbox && skip > 0.0, previous_depth) {
        out.include(Point::new(0.0, baseline_y - d - skip));
    }
    // a `\vbox` of a known `\hsize` is that wide: what sticks out of it is cut off with the page
    if let (Some(w), Some(lo), Some(hi)) = (hsize, out.min.as_mut(), out.max.as_mut()) {
        lo.x = 0.0;
        hi.x = w;
    }
    out
}

/// `\tabcolsep`, and the strut of a row (`\arraystretch` 1: 0.7 and 0.3 `\baselineskip`), in cm.
const TABCOLSEP: f64 = 6.0 * crate::PT;
const STRUT: (f64, f64) = (8.4 * crate::PT, 3.6 * crate::PT);
/// The math axis of Computer Modern at 10 pt, which a `tabular` is centered on.
const MATH_AXIS: f64 = 2.5 * crate::PT;

/// The pictures and words of each `tabular` set in its cells, as one box: its rows one under the
/// other (struts making them tall enough), its columns as wide as their widest cell with
/// `\tabcolsep` on both sides, the whole centered on the math axis. Returns the boxes (the
/// pictures', then the tables') and the layout with each table as one of them.
fn tables(boxes: &[PictureBox], layout: &[Layout], labels: &[crate::scene::Label]) -> (Vec<PictureBox>, Vec<Layout>) {
    let mut all: Vec<PictureBox> = boxes.to_vec();
    if !layout.iter().any(|l| matches!(l, Layout::TabStart(_))) {
        return (all, layout.to_vec());
    }
    // what a cell's item is: its width, and its height and depth around the baseline
    let measure = |all: &[PictureBox], l: &Layout| -> Option<(f64, f64, f64)> {
        match l {
            Layout::Picture(i) => {
                let b = &all[*i];
                let (lo, hi) = b.canvas.min.zip(b.canvas.max)?;
                Some((hi.x - lo.x, hi.y - b.baseline, b.baseline - lo.y))
            }
            Layout::Word(i) => Some((labels[*i].width(), labels[*i].height(), labels[*i].depth())),
            Layout::Skip(w) => Some((*w, 0.0, 0.0)),
            // the rest is a space: a cell holds pictures, words and glue only
            _ => Some((SPACE, 0.0, 0.0)),
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < layout.len() {
        let Layout::TabStart(code) = layout[i] else {
            out.push(layout[i]);
            i += 1;
            continue;
        };
        let ncols = (code & 0xff) as usize;
        let align = |k: usize| if k < ncols { (code >> (8 + 2 * k)) & 3 } else { 0 };
        // the rows and their cells, up to the table's end
        let mut rows: Vec<Vec<Vec<Layout>>> = vec![vec![Vec::new()]];
        i += 1;
        while i < layout.len() && !matches!(layout[i], Layout::TabEnd) {
            match layout[i] {
                Layout::Cell => rows.last_mut().expect("a row").push(Vec::new()),
                Layout::Row => rows.push(vec![Vec::new()]),
                Layout::Picture(_) | Layout::Word(_) | Layout::Skip(_) | Layout::Space => rows.last_mut().expect("a row").last_mut().expect("a cell").push(layout[i]),
                // the line's width stays known
                Layout::Width(w) if !out.contains(&Layout::Width(w)) => out.push(Layout::Width(w)),
                Layout::Vbox if !out.contains(&Layout::Vbox) => out.push(Layout::Vbox),
                _ => {}
            }
            i += 1;
        }
        i += 1;
        // the spaces between words stay, those at a cell's ends do not
        for cell in rows.iter_mut().flatten() {
            while cell.last() == Some(&Layout::Space) {
                cell.pop();
            }
            let first = cell.iter().position(|l| *l != Layout::Space).unwrap_or(cell.len());
            cell.drain(..first);
        }
        // `\\` before `\end{tabular}` ends the last row: nothing after it
        if rows.len() > 1 && rows.last().is_some_and(|r| r.iter().all(Vec::is_empty)) {
            rows.pop();
        }
        let size = |cell: &[Layout]| cell.iter().filter_map(|l| measure(&all, l)).fold((0.0f64, 0.0f64, 0.0f64), |(w, h, d), (a, b, c)| (w + a, h.max(b), d.max(c)));
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<f64> = (0..n).map(|k| rows.iter().filter_map(|r| r.get(k)).map(|c| size(c).0).fold(0.0, f64::max)).collect();
        let mut canvas = Canvas::default();
        let mut y = 0.0;
        for row in &rows {
            let (h, d) = row.iter().map(|c| size(c)).fold(STRUT, |(h, d), (_, a, b)| (h.max(a), d.max(b)));
            let base = y - h;
            let mut x = 0.0;
            for (k, w) in widths.iter().enumerate() {
                let cell = row.get(k).map_or(&[][..], Vec::as_slice);
                let room = w - size(cell).0;
                let mut cx = x
                    + TABCOLSEP
                    + match align(k) {
                        2 => room,
                        1 => room / 2.0,
                        _ => 0.0,
                    };
                for l in cell {
                    match l {
                        Layout::Picture(p) => {
                            let b = &all[*p];
                            let Some((lo, hi)) = b.canvas.min.zip(b.canvas.max) else { continue };
                            let shift = Point::new(cx - lo.x, base - b.baseline);
                            for item in &b.canvas.items {
                                let mut it = item.clone();
                                it.translate(shift);
                                canvas.items.push(it);
                            }
                            canvas.include(lo.plus(shift));
                            canvas.include(hi.plus(shift));
                            cx += hi.x - lo.x;
                        }
                        Layout::Word(wd) => {
                            let label = &labels[*wd];
                            canvas.items.push(Item::Text { label: label.clone(), place: crate::scene::Transform::translation(cx, base), opacity: 1.0 });
                            canvas.include(Point::new(cx, base - label.depth()));
                            canvas.include(Point::new(cx + label.width(), base + label.height()));
                            cx += label.width();
                        }
                        Layout::Skip(s) => cx += s,
                        // the rest is a space: a cell holds pictures, words and glue only
                        _ => cx += SPACE,
                    }
                }
                x += w + 2.0 * TABCOLSEP;
            }
            // the table's box: its columns' room and its rows' struts
            canvas.include(Point::new(0.0, y));
            canvas.include(Point::new(x, base - d));
            y = base - d;
        }
        // centered on the math axis
        let baseline = y / 2.0 - MATH_AXIS;
        out.push(Layout::Picture(all.len()));
        all.push(PictureBox { canvas, baseline });
    }
    (all, out)
}

/// Spans in characters of `code` as lines and columns, from 1, without the blanks at their end.
fn source_spans(code: &str, spans: &[(usize, usize)]) -> Vec<crate::canvas::SourceSpan> {
    let chars: Vec<char> = code.chars().collect();
    let mut starts = vec![0];
    starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
    let place = |i: usize| {
        let line = starts.partition_point(|&s| s <= i);
        (line, i - starts[line - 1] + 1)
    };
    spans
        .iter()
        .map(|&(first, end)| {
            let mut last = end.max(first + 1) - 1;
            while last > first && chars.get(last).is_none_or(|c| c.is_whitespace()) {
                last -= 1;
            }
            crate::canvas::SourceSpan { first: place(first), last: place(last) }
        })
        .collect()
}

/// Runs a whole document (or figure) and returns the canvas and the warnings.
pub fn run_document(code: &str) -> Result<(Canvas, Vec<String>), Error> {
    run_document_noting(code).map(|(canvas, warnings, _)| (canvas, warnings))
}

/// [`run_document`], and what the figure shows, for its description in words.
pub fn run_document_noting(code: &str) -> Result<(Canvas, Vec<String>, crate::describe::Facts), Error> {
    run_document_with(code, &[])
}

/// [`run_document_noting`] with libraries from outside defined.
pub fn run_document_with(code: &str, libraries: &[&'static crate::extension::Library]) -> Result<(Canvas, Vec<String>, crate::describe::Facts), Error> {
    run_document_mapped(code, libraries, Run::default())
}

/// How to run a document.
#[derive(Debug, Clone, Copy, Default)]
pub struct Run {
    /// Each statement's place in the code on the canvas.
    pub map: bool,
    /// The slide, from 1, for overlays.
    pub slide: usize,
    /// The animations library played, not shown still.
    pub play: bool,
    /// Each bar and mark of a plot an element of its own, with its values.
    pub points: bool,
}

/// [`run_document_with`], as `run` says.
pub fn run_document_mapped(code: &str, libraries: &[&'static crate::extension::Library], run: Run) -> Result<(Canvas, Vec<String>, crate::describe::Facts), Error> {
    let mut e = Engine::with_libraries(libraries);
    e.slide = run.slide.max(1);
    e.play = run.play;
    e.points = run.points;
    let map = run.map;
    crate::expr::reset_random();
    // tikra's margin around a figure, unless a standalone border says otherwise
    e.canvas.margin = 2.0 * crate::PT;
    e.tex.push_text(code);
    if map {
        e.spans = Some(Vec::new());
        e.tex.note_reading();
    }
    e.run()?;
    // anything left open is closed
    while e.picture_depth > 0 {
        e.end_picture();
    }
    while !e.scopes.is_empty() {
        e.end_scope();
    }
    let margin = e.canvas.margin;
    e.canvas = typeset(&e.boxes, &e.layout, &e.words, margin, e.varwidth);
    e.canvas.page_color = e.page_color;
    e.canvas.slides = e.slides;
    e.canvas.sources = source_spans(code, e.spans.as_deref().unwrap_or_default());
    let mut warnings = std::mem::take(&mut e.tex.warnings);
    warnings.dedup();
    let _ = Transform::default();
    Ok((e.canvas, warnings, std::mem::take(&mut e.facts)))
}
