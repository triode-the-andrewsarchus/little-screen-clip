//! pgf's basic layer: `\pgfpathmoveto`, `\pgfusepath`, `\pgfpoint…`, `\pgfset…`, `\pgftransform…`,
//! `pgfscope`, `pgfpicture`, `\pgftext`, `\pgfnode`. Points are in the current coordinate system,
//! as pgf keeps them; the current transformation applies when a point goes into a path.

use crate::scene::{Point, Segment, Transform};
use crate::tex::{self, Meaning, Token};
use crate::{Error, PT};

use super::nodes::{NodeSpec, OnPath};
use super::shapes::{ShapePath, ShapeText, Usage};
use super::state::Mode;
use super::{tikra_tokens, Engine};

/// Ids of the basic layer's commands (`Meaning::Native`), after the drawing layer's.
pub const BASE: u16 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum B {
    Process,
    ExtractX,
    ExtractY,
    MoveTo,
    LineTo,
    CurveTo,
    QuadraticCurveTo,
    Close,
    Arc,
    ArcAxes,
    Ellipse,
    Circle,
    Rectangle,
    RectangleCorners,
    Grid,
    Parabola,
    Sine,
    Cosine,
    UsePath,
    UsePathQStroke,
    UsePathQFill,
    UsePathQFillStroke,
    UsePathQClip,
    LineWidth,
    Dash,
    StrokeColor,
    FillColor,
    SetColor,
    StrokeOpacity,
    FillOpacity,
    ButtCap,
    RoundCap,
    RectCap,
    MiterJoin,
    RoundJoin,
    BevelJoin,
    MiterLimit,
    EoRule,
    NonzeroRule,
    Arrows,
    ArrowsStart,
    ArrowsEnd,
    ShortenStart,
    ShortenEnd,
    CornersArced,
    Shift,
    XShift,
    YShift,
    Scale,
    XScale,
    YScale,
    XSlant,
    YSlant,
    Rotate,
    Cm,
    Reset,
    ResetNonTranslations,
    Scope,
    EndScope,
    Picture,
    EndPicture,
    Text,
    Node,
    MultipartNode,
    Coordinate,
    XVec,
    YVec,
    ZVec,
    /// `\pgfplothandlerlineto`… (the handler's name in [`PLOT_HANDLERS`])
    PlotHandler,
    PlotHandlerMark,
    PlotHandlerMarkListed,
    StreamStart,
    StreamPoint,
    StreamEnd,
    PlotTension,
    PlotHandlerRecord,
    PlotFunction,
    FirstLineTo,
    FirstMoveTo,
    UsePlotMark,
    PlotMarkSize,
    DeclarePlotMark,
    Triangle,
    ArrowFrame,
    LineAtTime,
    CurveAtTime,
    ArcAxesAtTime,
    Invert,
    Adjustments,
    LowLevel,
    LowLevelObj,
    LowLevelSync,
    NonlinearFlatness,
    QMoveTo,
    QLineTo,
    QCurveTo,
    QCircle,
    ArcTo,
    ArcToPrecomputed,
    DeclareFunctional,
    ShadeColorToRgb,
    GetPath,
    SetPath,
    IntersectionOfPaths,
    SetKeys,
    DecToBase,
    ToDigitList,
    InnerLineWidth,
    InnerStrokeColor,
    Baseline,
    BaselineLater,
    BaselineNow,
    Svg,
    DeclarePlotHandler,
    UsePlotHandler,
    SysAnimate,
    GetLastXY,
    InterruptPath,
    EndInterruptPath,
}

const COMMANDS: &[(&str, B)] = &[
    ("pgf@process", B::Process),
    ("pgfextractx", B::ExtractX),
    ("pgfextracty", B::ExtractY),
    ("pgfpathmoveto", B::MoveTo),
    ("pgfclosepath", B::Close),
    ("pgfpathlineto", B::LineTo),
    ("pgfpathcurveto", B::CurveTo),
    ("pgfpathquadraticcurveto", B::QuadraticCurveTo),
    ("pgfpathclose", B::Close),
    ("pgfpatharc", B::Arc),
    ("pgfpatharcaxes", B::ArcAxes),
    ("pgfpathellipse", B::Ellipse),
    ("pgfpathcircle", B::Circle),
    ("pgfpathrectangle", B::Rectangle),
    ("pgfpathrectanglecorners", B::RectangleCorners),
    ("pgfpathgrid", B::Grid),
    ("pgfpathparabola", B::Parabola),
    ("pgfpathsine", B::Sine),
    ("pgfpathcosine", B::Cosine),
    ("pgfusepath", B::UsePath),
    ("pgfinterruptpath", B::InterruptPath),
    ("endpgfinterruptpath", B::EndInterruptPath),
    ("pgfusepathqstroke", B::UsePathQStroke),
    ("pgfusepathqfill", B::UsePathQFill),
    ("pgfusepathqfillstroke", B::UsePathQFillStroke),
    ("pgfusepathqclip", B::UsePathQClip),
    ("pgfsetlinewidth", B::LineWidth),
    ("pgfsetdash", B::Dash),
    ("pgfsetstrokecolor", B::StrokeColor),
    ("pgfsetfillcolor", B::FillColor),
    ("pgfsetcolor", B::SetColor),
    ("pgfsetstrokeopacity", B::StrokeOpacity),
    ("pgfsetfillopacity", B::FillOpacity),
    ("pgfsetbuttcap", B::ButtCap),
    ("pgfsetroundcap", B::RoundCap),
    ("pgfsetrectcap", B::RectCap),
    ("pgfsetmiterjoin", B::MiterJoin),
    ("pgfsetroundjoin", B::RoundJoin),
    ("pgfsetbeveljoin", B::BevelJoin),
    ("pgfsetmiterlimit", B::MiterLimit),
    ("pgfseteorule", B::EoRule),
    ("pgfsetnonzerorule", B::NonzeroRule),
    ("pgfsetarrows", B::Arrows),
    ("pgfsetarrowsstart", B::ArrowsStart),
    ("pgfsetarrowsend", B::ArrowsEnd),
    ("pgfsetshortenstart", B::ShortenStart),
    ("pgfsetshortenend", B::ShortenEnd),
    ("pgfsetcornersarced", B::CornersArced),
    ("pgftransformshift", B::Shift),
    ("pgftransformxshift", B::XShift),
    ("pgftransformyshift", B::YShift),
    ("pgftransformscale", B::Scale),
    ("pgftransformxscale", B::XScale),
    ("pgftransformyscale", B::YScale),
    ("pgftransformxslant", B::XSlant),
    ("pgftransformyslant", B::YSlant),
    ("pgftransformrotate", B::Rotate),
    ("pgftransformcm", B::Cm),
    ("pgftransformreset", B::Reset),
    ("pgftransformresetnontranslations", B::ResetNonTranslations),
    ("pgfscope", B::Scope),
    ("endpgfscope", B::EndScope),
    ("pgfpicture", B::Picture),
    ("endpgfpicture", B::EndPicture),
    ("pgftext", B::Text),
    ("pgfnode", B::Node),
    ("pgfmultipartnode", B::MultipartNode),
    ("pgfcoordinate", B::Coordinate),
    ("pgfsetxvec", B::XVec),
    ("pgfsetyvec", B::YVec),
    ("pgfsetzvec", B::ZVec),
    ("pgfplothandlermark", B::PlotHandlerMark),
    ("pgfplothandlermarklisted", B::PlotHandlerMarkListed),
    ("pgfplotstreamstart", B::StreamStart),
    ("pgfplotstreampoint", B::StreamPoint),
    ("pgfplotstreamend", B::StreamEnd),
    ("pgfsetplottension", B::PlotTension),
    ("pgfplothandlerlineto", B::PlotHandler),
    ("pgfplothandlerpolygon", B::PlotHandler),
    ("pgfplothandlercurveto", B::PlotHandler),
    ("pgfplothandlerclosedcurve", B::PlotHandler),
    ("pgfplothandlerdiscard", B::PlotHandler),
    ("pgfplothandlerxcomb", B::PlotHandler),
    ("pgfplothandlerycomb", B::PlotHandler),
    ("pgfplothandlerpolarcomb", B::PlotHandler),
    ("pgfplothandlerconstantlineto", B::PlotHandler),
    ("pgfplothandlerconstantlinetomarkright", B::PlotHandler),
    ("pgfplothandlerconstantlinetomarkmid", B::PlotHandler),
    ("pgfplothandlerjumpmarkleft", B::PlotHandler),
    ("pgfplothandlerjumpmarkright", B::PlotHandler),
    ("pgfplothandlerjumpmarkmid", B::PlotHandler),
    ("pgfplothandlerxbar", B::PlotHandler),
    ("pgfplothandlerybar", B::PlotHandler),
    ("pgfplothandlerxbarinterval", B::PlotHandler),
    ("pgfplothandlerybarinterval", B::PlotHandler),
    ("pgfplothandlergaplineto", B::PlotHandler),
    ("pgfplothandlergapcycle", B::PlotHandler),
    ("pgfplothandlerrecord", B::PlotHandlerRecord),
    ("pgfplotfunction", B::PlotFunction),
    ("pgfsetlinetofirstplotpoint", B::FirstLineTo),
    ("pgfsetmovetofirstplotpoint", B::FirstMoveTo),
    ("pgfuseplotmark", B::UsePlotMark),
    ("pgfsetplotmarksize", B::PlotMarkSize),
    ("pgfdeclareplotmark", B::DeclarePlotMark),
    ("pgftransformtriangle", B::Triangle),
    ("pgftransformarrow", B::ArrowFrame),
    ("pgftransformlineattime", B::LineAtTime),
    ("pgftransformcurveattime", B::CurveAtTime),
    ("pgftransformarcaxesattime", B::ArcAxesAtTime),
    ("pgftransforminvert", B::Invert),
    ("pgftransformationadjustments", B::Adjustments),
    ("pgflowlevel", B::LowLevel),
    ("pgflowlevelobj", B::LowLevelObj),
    ("pgflowlevelsynccm", B::LowLevelSync),
    ("pgfsettransformnonlinearflatness", B::NonlinearFlatness),
    ("pgfpathqmoveto", B::QMoveTo),
    ("pgfpathqlineto", B::QLineTo),
    ("pgfpathqcurveto", B::QCurveTo),
    ("pgfpathqcircle", B::QCircle),
    ("pgfpatharcto", B::ArcTo),
    ("pgfpatharctoprecomputed", B::ArcToPrecomputed),
    ("pgfdeclarefunctionalshading", B::DeclareFunctional),
    ("pgfshadecolortorgb", B::ShadeColorToRgb),
    ("pgfgetpath", B::GetPath),
    ("pgfsetpath", B::SetPath),
    ("pgfintersectionofpaths", B::IntersectionOfPaths),
    ("setkeys", B::SetKeys),
    ("pgfmathdectobase", B::DecToBase),
    ("pgfmathtodigitlist", B::ToDigitList),
    ("pgfsetinnerlinewidth", B::InnerLineWidth),
    ("pgfsetinnerstrokecolor", B::InnerStrokeColor),
    ("pgfsetbaseline", B::Baseline),
    ("pgfsetbaselinepointlater", B::BaselineLater),
    ("pgfsetbaselinepointnow", B::BaselineNow),
    ("pgfpathsvg", B::Svg),
    ("pgfdeclareplothandler", B::DeclarePlotHandler),
    ("tikra@plothandler", B::UsePlotHandler),
    ("tikra@sysanimate", B::SysAnimate),
    ("pgfgetlastxy", B::GetLastXY),
];

/// The basic layer's definitions made in TeX: the switches of the `…attime` transformations, the
/// transformation adjustments before they are computed, and `pgflowlevelscope`. The animations of the
/// basic layer take their arguments and do nothing: a still picture shows what they animate in its
/// base state. Those of the system layer do too, unless a snapshot is set (`\pgfsysanimsnapshot`):
/// their keys make a timeline, which `\pgfsysanimate` turns into the attribute's value at the
/// snapshot time, given to the object the id refers to.
pub const DEFINITIONS: &str = r"
% pgf's scratch counts, which documents borrow
\newcount\c@pgf@counta \newcount\c@pgf@countb \newcount\c@pgf@countc \newcount\c@pgf@countd
\newif\ifpgfslopedattime \newif\ifpgfallowupsidedownattime \newif\ifpgfresetnontranslationattime
\def\pgfhorizontaltransformationadjustment{1}\def\pgfverticaltransformationadjustment{1}
\newenvironment{pgflowlevelscope}[1]{\pgfscope\pgflowlevel{#1}}{\endpgfscope}
\def\pgfidrefnextuse#1#2{\def#1{#2}}\def\pgfidrefprevuse#1#2{\def#1{#2}}
\def\pgfsetblendmode#1{\tikzset{blend mode={#1}}}
\def\tikzdeclarecoordinatesystem#1#2{\expandafter\def\csname tikra@cs@#1\endcsname##1{#2}}
\def\define@key#1#2{\expandafter\def\csname KV@#1@#2\endcsname##1}
\def\pgfmathsetbasenumberlength#1{\def\pgfmathbasenumberlength{#1}}\def\pgfmathbasenumberlength{1}
% pgfmath's commands of one function: \pgfmathresult is its value
\def\pgfmathsin#1{\pgfmathparse{sin(#1)}}
\def\pgfmathcos#1{\pgfmathparse{cos(#1)}}
\def\pgfmathtan#1{\pgfmathparse{tan(#1)}}
\def\pgfmathsqrt#1{\pgfmathparse{sqrt(#1)}}
\def\pgfmathabs#1{\pgfmathparse{abs(#1)}}
\def\pgfmathneg#1{\pgfmathparse{neg(#1)}}
\def\pgfmathround#1{\pgfmathparse{round(#1)}}
\def\pgfmathfloor#1{\pgfmathparse{floor(#1)}}
\def\pgfmathceil#1{\pgfmathparse{ceil(#1)}}
\def\pgfmathexp#1{\pgfmathparse{exp(#1)}}
\def\pgfmathln#1{\pgfmathparse{ln(#1)}}
\def\pgfmathdeg#1{\pgfmathparse{deg(#1)}}
\def\pgfmathrad#1{\pgfmathparse{rad(#1)}}
\def\pgfmathsign#1{\pgfmathparse{sign(#1)}}
\def\pgfmathint#1{\pgfmathparse{int(#1)}}
\def\pgfmathfrac#1{\pgfmathparse{frac(#1)}}
\def\pgfmathasin#1{\pgfmathparse{asin(#1)}}
\def\pgfmathacos#1{\pgfmathparse{acos(#1)}}
\def\pgfmathatan#1{\pgfmathparse{atan(#1)}}
\def\pgfmathadd#1#2{\pgfmathparse{(#1)+(#2)}}
\def\pgfmathsubtract#1#2{\pgfmathparse{(#1)-(#2)}}
\def\pgfmathmultiply#1#2{\pgfmathparse{(#1)*(#2)}}
\def\pgfmathdivide#1#2{\pgfmathparse{(#1)/(#2)}}
\def\pgfmathmax#1#2{\pgfmathparse{max(#1,#2)}}
\def\pgfmathmin#1#2{\pgfmathparse{min(#1,#2)}}
\def\pgfmathmod#1#2{\pgfmathparse{mod(#1,#2)}}
\def\pgfmathMod#1#2{\pgfmathparse{Mod(#1,#2)}}
\def\pgfmathveclen#1#2{\pgfmathparse{veclen(#1,#2)}}
\def\pgfmathpow#1#2{\pgfmathparse{pow(#1,#2)}}
\def\pgfmathatan2#1#2{\pgfmathparse{atan2(#1,#2)}}
\def\pgfmathreciprocal#1{\pgfmathparse{1/(#1)}}
\def\breakforeach{\gdef\tikra@foreach@break{1}}
\def\pgfanimateattribute#1#2{}\def\pgfsnapshot#1{}\def\pgfsnapshotafter#1{}
\def\pgfsysanimsnapshot#1{\tikzset{make snapshot of={#1}}}\def\pgfsysanimsnapshotafter#1{\tikzset{make snapshot after={#1}}}
\def\tikra@anim@spec{}\def\tikra@anim@whom{}\def\tikra@anim@time{0}
\def\tikra@anim@add#1{\edef\tikra@anim@spec{\tikra@anim@spec#1,}}
\def\tikra@anim@val#1{\tikra@anim@add{\tikra@anim@time={#1}}}
\def\pgfsysanimate#1{\tikra@sysanimate{#1}{\tikra@anim@whom}{\tikra@anim@spec}\def\tikra@anim@spec{}}
\def\pgfsysanimkeywhom#1#2{\edef\tikra@anim@whom{#1}}\def\pgfsysanimkeytime#1#2#3#4#5{\def\tikra@anim@time{#1}}
\def\pgfsysanimkeyfreezeatend{\tikra@anim@add{freeze}}\def\pgfsysanimkeysnapshotstart#1{\tikra@anim@add{begin snapshot=#1}}
\def\pgfsysanimvalscalar#1{\tikra@anim@val{#1}}\def\pgfsysanimvaldimension#1{\tikra@anim@val{#1}}
\def\pgfsysanimkeyrestartalways{}\def\pgfsysanimkeyrestartnever{}\def\pgfsysanimkeyrestartwhennotactive{}
\def\pgfsysanimkeyrepeat#1{}\def\pgfsysanimkeyrepeatindefinite{}\def\pgfsysanimkeyrepeatdur#1{}
\def\pgfsysanimkeyremoveatend{}
\def\pgfsysanimkeybase{\def\tikra@anim@time{base}}\def\pgfsysanimkeyoffset#1#2{}
\def\pgfsysanimkeysyncbegin#1#2#3#4{}\def\pgfsysanimkeysyncend#1#2#3#4{}
\def\pgfsysanimkeyevent#1#2#3#4#5{}\def\pgfsysanimkeyrepeatevent#1#2#3#4#5{}
\def\pgfsysanimkeyaccesskey#1#2#3{}\def\pgfsysanimkeyaccumulate{}\def\pgfsysanimkeynoaccumulate{}
\def\pgfsysanimkeyrotatealong{}\def\pgfsysanimkeynorotatealong{}
\def\pgfsysanimkeymovealong#1{}\def\pgfsysanimkeytipmarkers#1#2{}\def\pgfsysanimkeycanvastransform#1#2{}
\def\pgfsysanimvalcurrent{}\def\pgfsysanimvalnone{}\def\pgfsysanimvaltext#1{}
\def\pgfsysanimvalcolor#1{}\def\pgfsysanimvalcolorrgb#1#2#3{}
\def\pgfsysanimvalcolorcmyk#1#2#3#4{}\def\pgfsysanimvalcolorcmy#1#2#3{}\def\pgfsysanimvalcolorgray#1{}
\def\pgfsysanimvalpath#1{}\def\pgfsysanimvaltranslate#1#2{}\def\pgfsysanimvalscale#1#2{}
\def\pgfsysanimvalviewbox#1#2#3#4{}\def\pgfsysanimvaldash#1#2{}
";

/// The basic layer's plot handlers, by command, as TikZ's `plot` names them.
const PLOT_HANDLERS: &[(&str, &str)] = &[
    ("pgfplothandlerlineto", "sharp plot"),
    ("pgfplothandlerpolygon", "sharp cycle"),
    ("pgfplothandlercurveto", "smooth"),
    ("pgfplothandlerclosedcurve", "smooth cycle"),
    ("pgfplothandlerdiscard", "discard"),
    ("pgfplothandlerxcomb", "xcomb"),
    ("pgfplothandlerycomb", "ycomb"),
    ("pgfplothandlerpolarcomb", "polar comb"),
    ("pgfplothandlerconstantlineto", "const plot"),
    ("pgfplothandlerconstantlinetomarkright", "const plot mark right"),
    ("pgfplothandlerconstantlinetomarkmid", "const plot mark mid"),
    ("pgfplothandlerjumpmarkleft", "jump mark left"),
    ("pgfplothandlerjumpmarkright", "jump mark right"),
    ("pgfplothandlerjumpmarkmid", "jump mark mid"),
    ("pgfplothandlerxbar", "xbar"),
    ("pgfplothandlerybar", "ybar"),
    ("pgfplothandlerxbarinterval", "xbar interval"),
    ("pgfplothandlerybarinterval", "ybar interval"),
    ("pgfplothandlergaplineto", "gap lineto"),
    ("pgfplothandlergapcycle", "gap cycle"),
];

/// The names a point argument can start with.
const POINTS: &[&str] = &[
    "pgfpoint",
    "pgfqpoint",
    "pgfpointorigin",
    "pgfpointpolar",
    "pgfqpointpolar",
    "pgfpointxy",
    "pgfqpointxy",
    "pgfpointxyz",
    "pgfqpointxyz",
    "pgfpointpolarxy",
    "pgfpointcylindrical",
    "pgfpointspherical",
    "pgfpointadd",
    "pgfpointdiff",
    "pgfpointscale",
    "pgfqpointscale",
    "pgfpointlineattime",
    "pgfpointlineatdistance",
    "pgfpointcurveattime",
    "pgfpointnormalised",
    "pgfpointanchor",
    "pgfpointshapeborder",
    "pgfpointintersectionoflines",
    "pgfpointtransformed",
    "pgfpointarcaxesattime",
    "pgfpointintersectionsolution",
    "pgfplotspointaxisxy",
    "pgfplotspointaxisxyz",
    "pgfplotspointaxisdirectionxy",
    "pgfplotspointaxisdirectionxyz",
];

pub fn define(e: &mut Engine) {
    for (i, (name, _)) in COMMANDS.iter().enumerate() {
        e.tex.define(name, Meaning::Native(BASE + i as u16));
    }
    // points are read by the commands that take them; on their own they put the point in
    // `\pgf@x` and `\pgf@y`
    for (i, name) in POINTS.iter().enumerate() {
        e.tex.define(name, Meaning::Native(POINT_BASE + i as u16));
    }
}

/// Where the points' ids start, as statements.
const POINT_BASE: u16 = 700;

pub fn is_basic(id: u16) -> bool {
    (BASE..BASE + COMMANDS.len() as u16).contains(&id) || (POINT_BASE..POINT_BASE + POINTS.len() as u16).contains(&id)
}

/// The basic layer's path being built, in canvas coordinates.
#[derive(Debug, Clone, Default)]
pub struct PgfPath {
    pub segments: Vec<Segment>,
    pub last: Option<Point>,
    pub start: Option<Point>,
    /// built by the quick commands only (`\pgfpathqmoveto`…): it does not make the picture larger
    pub quick: bool,
}

impl Engine {
    /// A point argument (`\pgfpoint{1cm}{2cm}`, `\pgfpointpolar{30}{1cm}`, a macro that holds one), in
    /// the current coordinate system (untransformed).
    /// The point in `\pgf@x`, `\pgf@y`.
    pub fn pgf_xy(&self) -> Point {
        let get = |r: u32| self.tex.dimens.get(&r).copied().unwrap_or(0) as f64 / 65536.0 * PT;
        Point::new(get(4050), get(4051))
    }

    pub fn set_pgf_xy(&mut self, p: Point) {
        self.tex.dimens.insert(4050, (p.x / PT * 65536.0).round() as i64);
        self.tex.dimens.insert(4051, (p.y / PT * 65536.0).round() as i64);
    }

    pub fn point_arg(&mut self) -> Result<Point, Error> {
        let tokens = self.argument()?;
        self.point_of(tokens)
    }

    pub fn point_of(&mut self, tokens: Vec<Token>) -> Result<Point, Error> {
        let depth = self.tex.depth();
        self.tex.push_frame(tokens);
        let r = self.point_here(depth);
        self.tex.drain_to(depth);
        r
    }

    fn point_here(&mut self, depth: usize) -> Result<Point, Error> {
        // the first token, expanding macros until a point command appears
        let mut guard = 0;
        let name = loop {
            let Some(t) = self.tex.get_token_until(depth) else { return Err(Error::Figure("a point is missing".into())) };
            if t.is_space() {
                continue;
            }
            match &t {
                Token::Cs(n) if POINTS.contains(&&**n) => break n.to_string(),
                _ => {
                    let m = self.tex.meaning(&t);
                    if m.is_expandable() && guard < 1000 {
                        guard += 1;
                        self.tex.expand(&t, &m)?;
                        continue;
                    }
                    // pgfmath's computations before the point run first
                    if let Meaning::Native(id) = m {
                        if Engine::is_pgfmath(id) {
                            self.command(id, &t)?;
                            continue;
                        }
                    }
                    // code that leaves the point in `\pgf@x`, `\pgf@y` (a shape's saved anchor)
                    if matches!(m, Meaning::Prim(_) | Meaning::Dimen(_) | Meaning::Native(_)) {
                        let mut code = vec![t];
                        while let Some(x) = self.tex.get_token_until(depth) {
                            code.push(x);
                        }
                        code.push(Token::cs("relax"));
                        self.run_tokens(code)?;
                        return Ok(self.pgf_xy());
                    }
                    return Err(Error::Figure(format!("not a point: {}", tex::show_token(&t))));
                }
            }
        };
        let arg = |e: &mut Engine| e.tex.read_undelimited();
        let length = |e: &mut Engine, v: Vec<Token>| e.length_of(v);
        Ok(match name.as_str() {
            "pgfpointorigin" => Point::default(),
            "pgfpoint" | "pgfqpoint" => {
                let x = arg(self)?;
                let y = arg(self)?;
                Point::new(length(self, x)?, length(self, y)?)
            }
            "pgfpointpolar" | "pgfqpointpolar" => {
                let a = arg(self)?;
                let r = arg(self)?;
                let angle = self.number_of(a)?;
                let text = tex::tokens_to_string(&r);
                let (rx, ry) = match text.split_once(" and ") {
                    Some((x, y)) => (self.length(x)?, self.length(y)?),
                    None => {
                        let v = length(self, r)?;
                        (v, v)
                    }
                };
                let (s, c) = angle.to_radians().sin_cos();
                Point::new(rx * c, ry * s)
            }
            "pgfpointxy" | "pgfqpointxy" => {
                let x = arg(self)?;
                let y = arg(self)?;
                let (x, y) = (self.number_of(x)?, self.number_of(y)?);
                self.st.x.scale(x).plus(self.st.y.scale(y))
            }
            "pgfpointxyz" | "pgfqpointxyz" => {
                let x = arg(self)?;
                let y = arg(self)?;
                let z = arg(self)?;
                let (x, y, z) = (self.number_of(x)?, self.number_of(y)?, self.number_of(z)?);
                self.st.x.scale(x).plus(self.st.y.scale(y)).plus(self.st.z.scale(z))
            }
            // pgfplots: a point of the axis being drawn (`axis cs`), or a direction in it
            "pgfplotspointaxisxy" | "pgfplotspointaxisxyz" | "pgfplotspointaxisdirectionxy" | "pgfplotspointaxisdirectionxyz" => {
                let n = if name.ends_with("xyz") { 3 } else { 2 };
                let mut v = [0.0; 3];
                for x in v.iter_mut().take(n) {
                    let a = arg(self)?;
                    *x = self.number_of(a)?;
                }
                let Some(f) = self.frame.clone() else { return Err(Error::Figure(format!("\\{name} outside an axis"))) };
                let inverse = self.st.transform.inverse();
                if name.contains("direction") {
                    let d = (0..n).fold(Point::default(), |p, a| p.plus(f.e[a].scale(v[a])));
                    inverse.apply(d).minus(inverse.apply(Point::default()))
                } else {
                    let l = [f.logical(0, v[0]), f.logical(1, v[1]), if n == 3 { f.logical(2, v[2]) } else { f.min[2] }];
                    inverse.apply(f.at(l))
                }
            }
            "pgfpointpolarxy" => {
                let a = arg(self)?;
                let r = arg(self)?;
                let (angle, radius) = (self.number_of(a)?, self.number_of(r)?);
                let (s, c) = angle.to_radians().sin_cos();
                self.st.x.scale(radius * c).plus(self.st.y.scale(radius * s))
            }
            "pgfpointcylindrical" | "pgfpointspherical" => {
                // the 3d points: (angle, radius, z) and (longitude, latitude, radius)
                let (a, b, c) = (arg(self)?, arg(self)?, arg(self)?);
                let (a, b, c) = (self.number_of(a)?, self.number_of(b)?, self.number_of(c)?);
                if name == "pgfpointcylindrical" {
                    let (s, co) = a.to_radians().sin_cos();
                    self.st.x.scale(b * co).plus(self.st.y.scale(b * s)).plus(self.st.z.scale(c))
                } else {
                    let ((ls, lc), (ts, tc)) = (a.to_radians().sin_cos(), b.to_radians().sin_cos());
                    self.st.x.scale(ls).plus(self.st.y.scale(lc)).scale(tc).plus(self.st.z.scale(ts)).scale(c)
                }
            }
            "pgfpointadd" => {
                let p = arg(self)?;
                let q = arg(self)?;
                self.point_of(p)?.plus(self.point_of(q)?)
            }
            "pgfpointdiff" => {
                // the second minus the first
                let p = arg(self)?;
                let q = arg(self)?;
                self.point_of(q)?.minus(self.point_of(p)?)
            }
            "pgfpointscale" | "pgfqpointscale" => {
                let f = arg(self)?;
                let p = arg(self)?;
                let f = self.number_of(f)?;
                self.point_of(p)?.scale(f)
            }
            "pgfpointlineattime" => {
                let t = arg(self)?;
                let p = arg(self)?;
                let q = arg(self)?;
                let t = self.number_of(t)?;
                self.point_of(p)?.lerp(self.point_of(q)?, t)
            }
            "pgfpointlineatdistance" => {
                let d = arg(self)?;
                let p = arg(self)?;
                let q = arg(self)?;
                let d = length(self, d)?;
                let (p, q) = (self.point_of(p)?, self.point_of(q)?);
                let v = q.minus(p);
                p.plus(v.scale(d / v.norm().max(1e-12)))
            }
            "pgfpointcurveattime" => {
                let t = arg(self)?;
                let t = self.number_of(t)?;
                let mut pts = Vec::new();
                for _ in 0..4 {
                    let a = arg(self)?;
                    pts.push(self.point_of(a)?);
                }
                let u = 1.0 - t;
                pts[0].scale(u * u * u).plus(pts[1].scale(3.0 * u * u * t)).plus(pts[2].scale(3.0 * u * t * t)).plus(pts[3].scale(t * t * t))
            }
            "pgfpointnormalised" => {
                let p = arg(self)?;
                let p = self.point_of(p)?;
                // pgf normalises to a length of 1pt
                p.scale(PT / p.norm().max(1e-12))
            }
            "pgfpointanchor" | "pgfpointshapeborder" => {
                let name_cmd = name.clone();
                let name = arg(self)?;
                let second = arg(self)?;
                let name = self.tex.expand_to_string(name)?.trim().to_string();
                let canvas = {
                    let node = self.nodes.get(&name).cloned().ok_or_else(|| Error::Figure(format!("unknown node: {name}")))?;
                    if name.is_empty() {
                        Point::default()
                    } else if name_cmd == "pgfpointshapeborder" {
                        let p = self.point_of(second)?;
                        node.border_toward(self.st.transform.apply(p))
                    } else {
                        let a = self.tex.expand_to_string(second)?;
                        self.node_anchor(&node, a.trim()).ok_or_else(|| Error::Figure(format!("unknown anchor: {name}.{}", a.trim())))?
                    }
                };
                self.st.transform.inverse().apply(canvas)
            }
            "pgfpointintersectionoflines" => {
                let mut p = Vec::new();
                for _ in 0..4 {
                    let a = arg(self)?;
                    p.push(self.point_of(a)?);
                }
                let (r, s) = (p[1].minus(p[0]), p[3].minus(p[2]));
                let den = r.x * s.y - r.y * s.x;
                if den.abs() < 1e-12 {
                    return Err(Error::Figure("intersection of parallel lines".into()));
                }
                let t = ((p[2].x - p[0].x) * s.y - (p[2].y - p[0].y) * s.x) / den;
                p[0].plus(r.scale(t))
            }
            "pgfpointarcaxesattime" => self.arc_axes_at_time()?.0,
            "pgfpointintersectionsolution" => {
                // the solution (counted from 1) in the current coordinate system
                let k = arg(self)?;
                let k = self.number_of(k)?;
                let p = (k >= 1.0).then(|| self.pgf_solutions.get(k as usize - 1).copied()).flatten();
                let p = p.ok_or_else(|| Error::Figure(format!("no intersection {k}")))?;
                self.st.transform.inverse().apply(p)
            }
            // the last of `POINTS`: `\pgfpointtransformed`
            _ => {
                let p = arg(self)?;
                let p = self.point_of(p)?;
                self.st.transform.apply(p)
            }
        })
    }

    pub fn pgf_add(&mut self, s: Segment) {
        self.pgf_path.quick = false;
        let p = match s {
            Segment::MoveTo(p) | Segment::LineTo(p) | Segment::CurveTo(_, _, p) => Some(p),
            Segment::Close => self.pgf_path.start,
        };
        if let Segment::MoveTo(p) = s {
            self.pgf_path.start = Some(p);
        }
        if !matches!(s, Segment::MoveTo(_)) && self.pgf_path.segments.is_empty() {
            let at = self.pgf_path.last.unwrap_or_default();
            self.pgf_path.segments.push(Segment::MoveTo(at));
            self.pgf_path.start = Some(at);
        }
        self.pgf_path.segments.push(s);
        self.pgf_path.last = p;
    }

    fn last_local(&self) -> Point {
        self.st.transform.inverse().apply(self.pgf_path.last.unwrap_or_default())
    }

    /// Uses the basic layer's path with `actions` (`fill`, `stroke`, `draw`, `clip`, `discard`…).
    fn pgf_use(&mut self, actions: &str) -> Result<(), Error> {
        let segs = std::mem::take(&mut self.pgf_path.segments);
        let quick = std::mem::take(&mut self.pgf_path.quick);
        if let Some(p) = self.pgf_path.last {
            self.last_xy = p;
        }
        self.pgf_path = PgfPath::default();
        let frozen = self.canvas_frozen;
        self.canvas_frozen |= quick;
        self.begin_group();
        self.st.mode = Mode::default();
        for a in actions.split(',').map(str::trim) {
            match a {
                // after \pgfsetfillpattern, a fill is the pattern
                "fill" if self.st.fill_pattern => self.st.mode.pattern = true,
                "fill" => self.st.mode.fill = true,
                "stroke" | "draw" => self.st.mode.draw = true,
                "clip" => self.st.mode.clip = true,
                "use as bounding box" => self.st.mode.bounding_box = true,
                "discard" | "" => {}
                other => self.warn(format!("\\pgfusepath{{{other}}} not supported: ignored")),
            }
        }
        // in a declared shape's code: a path of the shape
        if let Some(layer) = self.shape_capture.as_ref().and_then(|c| c.layer) {
            let mut uses = Vec::new();
            if self.st.mode.fill {
                uses.push(Usage::Fill(self.st.fill_paint_color().xcolor()));
            }
            if self.st.mode.draw {
                uses.push(Usage::Stroke);
            }
            let capture = self.shape_capture.as_mut().expect("a shape's code: its layer was just read");
            capture.paths.extend(uses.into_iter().map(|usage| ShapePath { layer, segments: segs.clone(), usage }));
            self.end_group();
            self.canvas_frozen = frozen;
            return Ok(());
        }
        // the clip outlives this group: it belongs to the pgf scope around
        let r = self.use_path(segs, "\\pgfusepath");
        self.end_group();
        if quick {
            self.canvas_frozen = frozen;
        }
        r
    }

    /// `\pgfpicture` … `\endpgfpicture`, and the `pgfpicture` environment.
    pub fn basic_picture(&mut self, begin: bool) -> Result<(), Error> {
        if begin {
            self.begin_picture(None)
        } else {
            self.end_picture();
            Ok(())
        }
    }

    pub fn basic_command(&mut self, id: u16) -> Result<(), Error> {
        if id >= POINT_BASE {
            // a point on its own: into `\pgf@x`, `\pgf@y` (pt), in the current coordinate system
            let Some(name) = POINTS.get((id - POINT_BASE) as usize) else { return Ok(()) };
            self.tex.back(Token::cs(name));
            let p = self.point_here(0)?;
            self.tex.dimens.insert(4050, (p.x / crate::PT * 65536.0).round() as i64);
            self.tex.dimens.insert(4051, (p.y / crate::PT * 65536.0).round() as i64);
            return Ok(());
        }
        let Some(&(_, cmd)) = COMMANDS.get((id - BASE) as usize) else { return Ok(()) };
        let t = self.st.transform;
        match cmd {
            B::Process => {
                // the point's code run, its result left in `\pgf@x`, `\pgf@y`
                let p = self.point_arg()?;
                self.set_pgf_xy(p);
            }
            B::ExtractX | B::ExtractY => {
                // `\pgfextractx{\dimen}{point}`
                let mut target = self.argument()?;
                let p = self.point_arg()?;
                let v = if matches!(cmd, B::ExtractX) { p.x } else { p.y };
                target.extend(tikra_tokens(&format!("={}pt\\relax", v / PT)));
                self.run_tokens(target)?;
            }
            B::MoveTo => {
                let p = self.point_arg()?;
                self.pgf_add(Segment::MoveTo(t.apply(p)));
            }
            B::LineTo => {
                let p = self.point_arg()?;
                self.pgf_add(Segment::LineTo(t.apply(p)));
            }
            B::CurveTo => {
                let a = self.point_arg()?;
                let b = self.point_arg()?;
                let c = self.point_arg()?;
                self.pgf_add(Segment::CurveTo(t.apply(a), t.apply(b), t.apply(c)));
            }
            B::QuadraticCurveTo => {
                let q = self.point_arg()?;
                let p = self.point_arg()?;
                let a = self.last_local();
                self.pgf_add(Segment::CurveTo(t.apply(a.lerp(q, 2.0 / 3.0)), t.apply(p.lerp(q, 2.0 / 3.0)), t.apply(p)));
            }
            B::Close => self.pgf_add(Segment::Close),
            B::Arc | B::ArcAxes => {
                let s = self.argument()?;
                let en = self.argument()?;
                let (a0, a1) = (self.number_of(s)?, self.number_of(en)?);
                let center_from = self.last_local();
                let (segments, end) = if cmd == B::Arc {
                    let r = self.argument()?;
                    let text = tex::tokens_to_string(&r);
                    let (rx, ry) = match text.split_once(" and ") {
                        Some((x, y)) => (self.length(x)?, self.length(y)?),
                        None => {
                            let v = self.length_of(r)?;
                            (v, v)
                        }
                    };
                    let (s0, c0) = a0.to_radians().sin_cos();
                    let center = center_from.minus(Point::new(rx * c0, ry * s0));
                    let segs = super::path::arc_segments(t.apply(center), rx, ry, a0, a1, &t);
                    let (s1, c1) = a1.to_radians().sin_cos();
                    (segs, t.apply(center.plus(Point::new(rx * c1, ry * s1))))
                } else {
                    // the arc of the ellipse with these two axes
                    let ax = self.point_arg()?;
                    let ay = self.point_arg()?;
                    let axes = Transform { a: ax.x, b: ax.y, c: ay.x, d: ay.y, e: 0.0, f: 0.0 };
                    let (s0, c0) = a0.to_radians().sin_cos();
                    let center = center_from.minus(axes.apply(Point::new(c0, s0)));
                    let full = t.compose(&Transform::translation(center.x, center.y)).compose(&axes);
                    let segs = super::path::arc_segments(full.apply(Point::default()), 1.0, 1.0, a0, a1, &Transform { e: 0.0, f: 0.0, ..full });
                    let (s1, c1) = a1.to_radians().sin_cos();
                    (segs, full.apply(Point::new(c1, s1)))
                };
                for s in segments {
                    self.pgf_add(s);
                }
                self.pgf_path.last = Some(end);
            }
            B::Ellipse | B::Circle => {
                let c = self.point_arg()?;
                let (a, b) = if cmd == B::Circle {
                    let r = self.argument()?;
                    let r = self.length_of(r)?;
                    (Point::new(r, 0.0), Point::new(0.0, r))
                } else {
                    (self.point_arg()?, self.point_arg()?)
                };
                let axes = Transform { a: a.x, b: a.y, c: b.x, d: b.y, e: 0.0, f: 0.0 };
                let full = t.compose(&Transform::translation(c.x, c.y)).compose(&axes);
                self.pgf_add(Segment::MoveTo(full.apply(Point::new(1.0, 0.0))));
                for s in super::path::arc_segments(full.apply(Point::default()), 1.0, 1.0, 0.0, 360.0, &Transform { e: 0.0, f: 0.0, ..full }) {
                    self.pgf_add(s);
                }
                self.pgf_add(Segment::Close);
                self.pgf_path.last = Some(t.apply(c));
            }
            B::Rectangle | B::RectangleCorners => {
                let a = self.point_arg()?;
                let b = self.point_arg()?;
                let b = if cmd == B::Rectangle { a.plus(b) } else { b };
                self.pgf_add(Segment::MoveTo(t.apply(a)));
                self.pgf_add(Segment::LineTo(t.apply(Point::new(a.x, b.y))));
                self.pgf_add(Segment::LineTo(t.apply(b)));
                self.pgf_add(Segment::LineTo(t.apply(Point::new(b.x, a.y))));
                self.pgf_add(Segment::Close);
                self.pgf_path.last = Some(t.apply(a));
            }
            B::Grid => {
                // /pgf/stepx and /pgf/stepy are 1cm
                let mut step = (1.0, 1.0);
                if let Some(o) = self.optional()? {
                    for item in crate::keys::split_list(&o) {
                        let Some(v) = item.value else { continue };
                        match item.key.as_str() {
                            // a point: its x and y
                            "step" => {
                                let p = self.point_of(v)?;
                                step = (p.x.abs(), p.y.abs());
                            }
                            "stepx" => step.0 = self.length_of(v)?,
                            "stepy" => step.1 = self.length_of(v)?,
                            _ => {}
                        }
                    }
                }
                let ll = self.point_arg()?;
                let ur = self.point_arg()?;
                let (x0, x1) = (ll.x.min(ur.x), ll.x.max(ur.x));
                let (y0, y1) = (ll.y.min(ur.y), ll.y.max(ur.y));
                let lines = |a: f64, b: f64, s: f64| -> Vec<f64> {
                    if s <= 0.0 {
                        return Vec::new();
                    }
                    let first = (a / s).ceil();
                    let n = ((b / s).floor() - first + 1.0).clamp(0.0, 10_000.0) as usize;
                    (0..n).map(|k| (first + k as f64) * s).collect()
                };
                for y in lines(y0, y1, step.1) {
                    self.pgf_add(Segment::MoveTo(t.apply(Point::new(x0, y))));
                    self.pgf_add(Segment::LineTo(t.apply(Point::new(x1, y))));
                }
                for x in lines(x0, x1, step.0) {
                    self.pgf_add(Segment::MoveTo(t.apply(Point::new(x, y0))));
                    self.pgf_add(Segment::LineTo(t.apply(Point::new(x, y1))));
                }
            }
            B::Parabola => {
                let bend = self.point_arg()?;
                let end = self.point_arg()?;
                let mut from = self.last_local();
                if bend.norm() > 1e-12 {
                    let c = bend;
                    self.pgf_add(Segment::CurveTo(
                        t.apply(from.plus(Point::new(0.1125 * c.x, 0.225 * c.y))),
                        t.apply(from.plus(Point::new(0.5 * c.x, c.y))),
                        t.apply(from.plus(c)),
                    ));
                    from = from.plus(c);
                }
                if end.norm() > 1e-12 {
                    let c = end;
                    self.pgf_add(Segment::CurveTo(
                        t.apply(from.plus(Point::new(0.5 * c.x, 0.0))),
                        t.apply(from.plus(Point::new(0.8875 * c.x, 0.775 * c.y))),
                        t.apply(from.plus(c)),
                    ));
                }
            }
            B::Sine | B::Cosine => {
                let d = self.point_arg()?;
                let a = self.last_local();
                let (c1, c2) = if cmd == B::Sine {
                    (Point::new(0.3260 * d.x, 0.5120 * d.y), Point::new(0.6380 * d.x, d.y))
                } else {
                    (Point::new(0.3620 * d.x, 0.0), Point::new(0.6740 * d.x, 0.4880 * d.y))
                };
                self.pgf_add(Segment::CurveTo(t.apply(a.plus(c1)), t.apply(a.plus(c2)), t.apply(a.plus(d))));
            }
            B::InterruptPath => self.interrupt_path(),
            B::EndInterruptPath => self.end_interrupt_path(),
            B::UsePath => {
                let a = self.argument_string()?;
                self.pgf_use(&a)?;
            }
            B::UsePathQStroke => self.pgf_use("stroke")?,
            B::UsePathQFill => self.pgf_use("fill")?,
            B::UsePathQFillStroke => self.pgf_use("fill,stroke")?,
            B::UsePathQClip => self.pgf_use("clip")?,
            B::LineWidth => {
                let v = self.argument()?;
                let w = self.length_of(v)?;
                self.process_keys(tikra_tokens(&format!("line width={}pt", w / PT)), Some("/tikz"))?;
            }
            B::Dash => {
                // \pgfsetdash{{3pt}{2pt}}{0pt}
                let pattern = self.argument()?;
                let phase = self.argument()?;
                let parts = super::options::split_arguments(&pattern, 16);
                let mut v = Vec::new();
                for p in parts.into_iter().filter(|p| !crate::keys::trim(p).is_empty()) {
                    v.push(self.length_of(p)?);
                }
                let phase = self.length_of(phase).unwrap_or(0.0);
                self.st.dash = if v.is_empty() { None } else { Some((v, phase)) };
            }
            B::StrokeColor | B::FillColor | B::SetColor => {
                let c = self.argument_string()?;
                match self.color(&c) {
                    Some(c) => match cmd {
                        B::StrokeColor => self.st.draw_color = Some(c),
                        B::FillColor => {
                            self.st.fill_color = Some(c);
                            self.st.fill_pattern = false;
                        }
                        _ => {
                            self.st.fill_pattern = false;
                            self.st.color = c;
                            self.st.draw_color = None;
                            self.st.fill_color = None;
                            self.st.text_color = None;
                        }
                    },
                    None => self.warn(format!("unknown color '{c}'")),
                }
            }
            B::StrokeOpacity | B::FillOpacity => {
                let v = self.argument()?;
                let o = self.number_of(v)?;
                if cmd == B::StrokeOpacity {
                    self.st.draw_opacity = o;
                } else {
                    self.st.fill_opacity = o;
                }
            }
            B::ButtCap => self.st.cap = crate::canvas::Cap::Butt,
            B::RoundCap => self.st.cap = crate::canvas::Cap::Round,
            B::RectCap => self.st.cap = crate::canvas::Cap::Square,
            B::MiterJoin => self.st.join = crate::canvas::Join::Miter,
            B::RoundJoin => self.st.join = crate::canvas::Join::Round,
            B::BevelJoin => self.st.join = crate::canvas::Join::Bevel,
            B::MiterLimit => {
                let v = self.argument()?;
                self.st.miter_limit = self.number_of(v)?;
            }
            B::EoRule => self.st.even_odd = true,
            B::NonzeroRule => self.st.even_odd = false,
            B::Arrows | B::ArrowsStart | B::ArrowsEnd => {
                let s = self.argument_string()?;
                match cmd {
                    B::Arrows => {
                        if let Some((a, b)) = super::arrows::parse_arrows(&s, &self.st.tips) {
                            self.st.start_arrow = a;
                            self.st.end_arrow = b;
                        }
                    }
                    B::ArrowsStart => self.st.start_arrow = Some(s).filter(|x| !x.is_empty()),
                    _ => self.st.end_arrow = Some(s).filter(|x| !x.is_empty()),
                }
            }
            B::ShortenStart | B::ShortenEnd => {
                let v = self.argument()?;
                let d = self.length_of(v)?;
                if cmd == B::ShortenStart {
                    self.st.shorten_start = d;
                } else {
                    self.st.shorten_end = d;
                }
            }
            B::CornersArced => {
                let p = self.point_arg()?;
                // the corner's arc starts p.x before it and ends p.y after it
                (self.st.rounded, self.st.rounded_out) = (p.x, p.y);
            }
            B::Shift => {
                let p = self.point_arg()?;
                self.st.transform = t.compose(&Transform::translation(p.x, p.y));
            }
            B::XShift | B::YShift => {
                let v = self.argument()?;
                let d = self.length_of(v)?;
                let s = if cmd == B::XShift { Transform::translation(d, 0.0) } else { Transform::translation(0.0, d) };
                self.st.transform = t.compose(&s);
            }
            B::Scale | B::XScale | B::YScale => {
                let v = self.argument()?;
                let f = self.number_of(v)?;
                let s = match cmd {
                    B::Scale => Transform::scaling(f, f),
                    B::XScale => Transform::scaling(f, 1.0),
                    _ => Transform::scaling(1.0, f),
                };
                self.st.transform = t.compose(&s);
            }
            B::XSlant | B::YSlant => {
                let v = self.argument()?;
                let f = self.number_of(v)?;
                let s = if cmd == B::XSlant { Transform { c: f, ..Transform::default() } } else { Transform { b: f, ..Transform::default() } };
                self.st.transform = t.compose(&s);
            }
            B::Rotate => {
                let v = self.argument()?;
                let a = self.number_of(v)?;
                self.st.transform = t.compose(&Transform::rotation(a));
            }
            B::Cm => {
                let mut n = [0.0; 4];
                for k in &mut n {
                    let v = self.argument()?;
                    *k = self.number_of(v)?;
                }
                let p = self.point_arg()?;
                self.st.transform = t.compose(&Transform { a: n[0], b: n[1], c: n[2], d: n[3], e: p.x, f: p.y });
            }
            B::Reset => self.st.transform = Transform::default(),
            B::ResetNonTranslations => self.st.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, ..t },
            B::Scope => {
                self.begin_group();
                self.begin_scope();
            }
            B::EndScope => {
                self.end_scope();
                self.end_group();
            }
            B::Picture => self.basic_picture(true)?,
            B::EndPicture => self.basic_picture(false)?,
            B::Text => {
                // \pgftext[left,base,at=…,rotate=…]{text}
                let o = self.optional()?.unwrap_or_default();
                let text = self.argument()?;
                let mut anchor_x = "";
                let mut anchor_y = "";
                let mut at = Point::default();
                let mut rotate = 0.0;
                for item in crate::keys::split_list(&o) {
                    match (item.key.as_str(), item.value) {
                        ("left", None) => anchor_x = "west",
                        ("right", None) => anchor_x = "east",
                        ("top", None) => anchor_y = "north",
                        ("bottom", None) => anchor_y = "south",
                        ("base", None) => anchor_y = "base",
                        ("at", Some(v)) => at = self.point_of(v)?,
                        ("x", Some(v)) => at.x += self.length_of(v)?,
                        ("y", Some(v)) => at.y += self.length_of(v)?,
                        ("rotate", Some(v)) => rotate = self.number_of(v)?,
                        _ => {}
                    }
                }
                let anchor = match (anchor_y, anchor_x) {
                    ("", "") => "center".to_string(),
                    ("", x) => x.to_string(),
                    (y, "") => y.to_string(),
                    (y, x) => format!("{y} {x}"),
                };
                // under a nonlinear transformation, the text goes where its point goes
                let place = if self.st.nonlinear.is_empty() { t.apply(at) } else { self.nonlinear_point(t.apply(at))? };
                // a shading as the text: its box, placed by the anchor (its baseline is its bottom)
                let words: Vec<&Token> = text.iter().filter(|x| !x.is_space()).collect();
                if let [Token::Cs(cmd), ..] = words[..] {
                    if &**cmd == "pgfuseshading" {
                        let name = crate::tex::tokens_to_string(&text).replace("\\pgfuseshading", "").replace(['{', '}'], "");
                        if let Some((w, h)) = self.shading_half_size(name.trim()) {
                            let dx = if anchor.contains("west") {
                                w
                            } else if anchor.contains("east") {
                                -w
                            } else {
                                0.0
                            };
                            let dy = if anchor.contains("north") {
                                -h
                            } else if anchor.contains("south") || anchor.contains("base") {
                                h
                            } else {
                                0.0
                            };
                            let s = Transform::translation(place.x, place.y).compose(&Transform::rotation(rotate)).compose(&Transform::translation(dx, dy));
                            return self.draw_shading(name.trim(), &s);
                        }
                    }
                }
                // in a declared shape's code: a text of the shape
                if let Some(c) = &mut self.shape_capture {
                    use crate::extension::TextAnchor as A;
                    let anchor = match anchor.as_str() {
                        "west" => A::West,
                        "east" => A::East,
                        "north" => A::North,
                        "south" => A::South,
                        "base" => A::Base,
                        "north west" => A::NorthWest,
                        "north east" => A::NorthEast,
                        "south west" => A::SouthWest,
                        "south east" => A::SouthEast,
                        "base west" => A::BaseWest,
                        "base east" => A::BaseEast,
                        _ => A::Center,
                    };
                    let font = Some(self.st.font.clone());
                    c.texts.push(ShapeText { tex: tex::tokens_to_string(&text), at: place, color: None, anchor: Some(anchor), font, turn: rotate, upright: false });
                    return Ok(());
                }
                // the text is scaled and turned by the transformation in force, as pgf boxes it
                let linear = t.a != 1.0 || t.b != 0.0 || t.c != 0.0 || t.d != 1.0;
                let shape = if linear { ",transform shape" } else { "" };
                let options = tikra_tokens(&format!("shape=rectangle,inner sep=0pt,outer sep=0pt,anchor={anchor},rotate={rotate}{shape}"));
                // (the node moves its point by the nonlinear transformation itself)
                let at = t.apply(at);
                let items = self.node(NodeSpec { options, name: None, at: Some(at), text, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None })?;
                self.canvas.items.extend(items);
            }
            B::Node | B::MultipartNode => {
                // \pgfnode{shape}{anchor}{text}{name}{path usage}; \pgfmultipartnode has no text
                // (its parts are boxes, empty here)
                let shape = self.argument_string()?;
                let anchor = self.argument_string()?;
                let text = if matches!(cmd, B::Node) { self.argument()? } else { self.node_part_boxes() };
                let name = self.argument_string()?;
                let usage = self.argument()?;
                let usage = tex::tokens_to_string(&usage);
                // pgf's nodes are in the whole transformation, not only its shift
                let mut opts = format!("shape={shape},anchor={anchor},transform shape");
                if usage.contains("stroke") || usage.contains("draw") {
                    opts.push_str(",draw");
                }
                if usage.contains("fill") {
                    opts.push_str(",fill");
                }
                let place = t.apply(Point::default());
                let name = (!name.is_empty()).then_some(name);
                let items = self.node(NodeSpec {
                    options: tikra_tokens(&opts),
                    name,
                    at: Some(place),
                    text,
                    place: OnPath::Point(place),
                    point: place,
                    coordinate: false,
                    default_pos: None,
                })?;
                self.canvas.items.extend(items);
            }
            B::Coordinate => {
                let name = self.argument_string()?;
                let p = self.point_arg()?;
                let place = t.apply(p);
                let items = self.node(NodeSpec {
                    options: Vec::new(),
                    name: Some(name),
                    at: Some(place),
                    text: Vec::new(),
                    place: OnPath::Point(place),
                    point: place,
                    coordinate: true,
                    default_pos: None,
                })?;
                self.canvas.items.extend(items);
            }
            B::XVec | B::YVec | B::ZVec => {
                let p = self.point_arg()?;
                match cmd {
                    B::XVec => self.st.x = p,
                    B::YVec => self.st.y = p,
                    _ => self.st.z = p,
                }
            }
            B::PlotHandler => {
                let name = COMMANDS[(id - BASE) as usize].0;
                let handler = PLOT_HANDLERS.iter().find(|(c, _)| *c == name).map_or("sharp plot", |(_, h)| h);
                self.st.basic_plot = (handler.to_string(), Vec::new());
            }
            B::PlotHandlerMark => {
                let code = self.argument()?;
                self.st.basic_plot = ("mark".into(), code);
            }
            B::PlotHandlerMarkListed => {
                // `{code}{1,3}`: a mark on the listed points only (counted from 1)
                let code = self.argument()?;
                let list = self.argument_string()?;
                self.st.basic_plot = (format!("mark listed:{list}"), code);
            }
            B::PlotTension => {
                let v = self.argument()?;
                self.st.tension = self.number_of(v)?;
            }
            B::StreamStart => self.plot_stream.clear(),
            B::StreamPoint => {
                let p = self.point_arg()?;
                self.plot_stream.push(t.apply(p));
            }
            B::StreamEnd => self.plot_stream_end()?,
            B::PlotHandlerRecord => {
                let m = self.argument()?;
                self.st.basic_plot = ("record".into(), m);
            }
            B::FirstLineTo => self.st.plot_first_lineto = true,
            B::FirstMoveTo => self.st.plot_first_lineto = false,
            B::PlotFunction => {
                // \pgfplotfunction{\x}{list}{point}: a stream of the point for each item
                let var = self.argument()?;
                let list = self.argument()?;
                let point = self.argument()?;
                let list = self.tex.expand_fully(list)?;
                let name = var.into_iter().find(|t| matches!(t, Token::Cs(_)));
                self.plot_stream.clear();
                for item in super::foreach_items(&list) {
                    if let Some(n) = &name {
                        self.tex.set_meaning(n, super::macro_tokens(item), false);
                    }
                    let p = self.point_of(point.clone())?;
                    self.plot_stream.push(t.apply(p));
                }
                self.plot_stream_end()?;
            }
            B::UsePlotMark => {
                let name = self.argument_string()?;
                self.use_plot_mark(name.trim())?;
            }
            B::PlotMarkSize => {
                let v = self.argument()?;
                self.st.mark_size = self.length_of(v)?;
            }
            B::DeclarePlotMark => {
                let name = self.argument_string()?;
                let code = self.argument()?;
                self.plot_marks_declared.insert(name.trim().to_string(), code);
            }
            B::Triangle => {
                // the origin goes to the first point, (1pt,0pt) to the second, (0pt,1pt) to the third
                let o = self.point_arg()?;
                let x = self.point_arg()?;
                let y = self.point_arg()?;
                let (x, y) = (x.minus(o).scale(1.0 / PT), y.minus(o).scale(1.0 / PT));
                self.st.transform = t.compose(&Transform { a: x.x, b: x.y, c: y.x, d: y.y, e: o.x, f: o.y });
            }
            B::ArrowFrame => {
                // at the end point, the x axis pointing away from the start
                let from = self.point_arg()?;
                let to = self.point_arg()?;
                let d = to.minus(from);
                let d = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(1.0, 0.0) };
                self.st.transform = t.compose(&Transform { a: d.x, b: d.y, c: -d.y, d: d.x, e: to.x, f: to.y });
            }
            B::LineAtTime => {
                let time = self.argument()?;
                let time = self.number_of(time)?;
                let p = self.point_arg()?;
                let q = self.point_arg()?;
                self.transform_at_time(p.lerp(q, time), q.minus(p));
            }
            B::CurveAtTime => {
                let time = self.argument()?;
                let s = self.number_of(time)?;
                let mut c = [Point::default(); 4];
                for p in &mut c {
                    *p = self.point_arg()?;
                }
                let u = 1.0 - s;
                let at = c[0].scale(u * u * u).plus(c[1].scale(3.0 * u * u * s)).plus(c[2].scale(3.0 * u * s * s)).plus(c[3].scale(s * s * s));
                let tangent = c[1].minus(c[0]).scale(u * u).plus(c[2].minus(c[1]).scale(2.0 * u * s)).plus(c[3].minus(c[2]).scale(s * s));
                self.transform_at_time(at, tangent);
            }
            B::ArcAxesAtTime => {
                let (at, tangent) = self.arc_axes_at_time()?;
                self.transform_at_time(at, tangent);
            }
            B::Invert => self.st.transform = t.inverse(),
            B::Adjustments => {
                // how much a horizontal and a vertical length shrink to come out at their size
                let h = 1.0 / t.linear(Point::new(1.0, 0.0)).norm().max(1e-12);
                let v = 1.0 / t.linear(Point::new(0.0, 1.0)).norm().max(1e-12);
                for (name, value) in [("pgfhorizontaltransformationadjustment", h), ("pgfverticaltransformationadjustment", v)] {
                    self.tex.set_meaning(&Token::cs(name), super::macro_tokens(tikra_tokens(&format!("{value}"))), false);
                }
            }
            B::LowLevel => {
                let code = self.argument()?;
                self.low_level(code)?;
            }
            B::LowLevelObj => {
                let code = self.argument()?;
                let body = self.argument()?;
                self.begin_group();
                self.begin_scope();
                let r = self.low_level(code).and_then(|()| self.run_tokens(body));
                self.end_scope();
                self.end_group();
                r?;
            }
            B::LowLevelSync => {
                // the coordinate transformation moves to the canvas, line widths and all
                self.open_in_scope(crate::canvas::Item::Transform { transform: t });
                self.st.transform = Transform::default();
            }
            B::NonlinearFlatness => {
                let _ = self.argument()?;
            }
            B::QMoveTo | B::QLineTo | B::QCurveTo | B::QCircle => {
                // the quick commands take canvas lengths, untransformed; a path of them only does
                // not make the picture larger
                let quick = self.pgf_path.segments.is_empty() || self.pgf_path.quick;
                self.quick_path(cmd)?;
                self.pgf_path.quick = quick;
            }
            B::ArcTo => {
                // {x radius}{y radius}{rotation}{large arc}{counterclockwise}{target}, as SVG's arc
                let rx = self.argument()?;
                let ry = self.argument()?;
                let rot = self.argument()?;
                let large = self.argument()?;
                let ccw = self.argument()?;
                let (rx, ry, rot) = (self.length_of(rx)?.abs(), self.length_of(ry)?.abs(), self.number_of(rot)?);
                let (large, ccw) = (self.number_of(large)? != 0.0, self.number_of(ccw)? != 0.0);
                let to = self.point_arg()?;
                let from = self.last_local();
                for s in super::path::svg_arc_segments(from.scale(1.0 / PT), to.scale(1.0 / PT), rx / PT, ry / PT, rot, large, ccw, &t) {
                    self.pgf_add(s);
                }
                self.pgf_path.last = Some(t.apply(to));
            }
            B::DeclareFunctional => self.declare_functional_shading()?,
            // the picture's baseline: a height, a point's (evaluated now or when the picture ends)
            B::DeclarePlotHandler => {
                // \pgfdeclareplothandler{\macro}{#1…}{point=code}: \macro{args} makes it the
                // plot handler, the code run at each point
                let name = self.argument()?;
                let params = self.argument()?;
                let options = self.argument()?;
                let Some(Token::Cs(name)) = name.into_iter().find(|t| matches!(t, Token::Cs(_))) else { return Ok(()) };
                let n = params.iter().filter(|t| matches!(t, Token::Char(_, tex::Cat::Param))).count();
                let mut point = String::new();
                for item in crate::keys::split_list(&options) {
                    if item.key.trim() == "point" {
                        point = tex::tokens_to_string(&item.value.unwrap_or_default());
                    }
                }
                self.plot_handlers.insert(name.to_string(), point);
                let args: String = (1..=n).map(|k| format!("{{#{k}}}")).collect();
                let params: String = (1..=n).map(|k| format!("#{k}")).collect();
                self.run_tokens(tikra_tokens(&format!("\\def\\{name}{params}{{\\tikra@plothandler{{{name}}}{{{args}}}}}")))?;
            }
            B::UsePlotHandler => {
                let name = self.argument_string()?;
                let args = self.argument()?;
                self.st.basic_plot = (format!("custom:{name}"), args);
            }
            B::SysAnimate => {
                let attribute = self.argument_string()?;
                let whom = self.argument_string()?;
                // the timeline the keys made (a macro)
                let spec = self.argument_string()?;
                self.sys_animate(attribute.trim(), whom.trim(), tikra_tokens(&spec))?;
            }
            B::Svg => {
                let data = self.argument_string()?;
                for s in super::path::svg_path(self, &data, self.pgf_path.last) {
                    self.pgf_add(s);
                }
            }
            B::Baseline => {
                let v = self.argument_string()?;
                self.st.baseline = Some(v);
            }
            B::BaselineLater => {
                let p = self.argument()?;
                self.st.baseline = Some(format!("tikra@point:{}", tex::tokens_to_string(&p)));
            }
            B::BaselineNow => {
                let p = self.point_arg()?;
                self.st.baseline = Some(format!("{}pt", t.apply(p).y / PT));
            }
            B::InnerLineWidth => {
                let v = self.argument()?;
                self.st.inner_line.0 = self.length_of(v)?;
            }
            B::InnerStrokeColor => {
                let c = self.argument_string()?;
                match self.color(&c) {
                    Some(c) => self.st.inner_line.1 = Some(c),
                    None => self.warn(format!("unknown color '{c}'")),
                }
            }
            B::DecToBase => {
                // \pgfmathdectobase{\macro}{number}{base}: its digits, padded with zeros to
                // \pgfmathbasenumberlength
                let target = self.argument()?;
                let n = self.argument()?;
                let base = self.argument()?;
                let (n, base) = (self.number_of(n)? as i64, (self.number_of(base)? as u32).clamp(2, 36));
                let mut digits = Vec::new();
                let mut m = n.unsigned_abs();
                while m > 0 || digits.is_empty() {
                    digits.push(char::from_digit((m % u64::from(base)) as u32, base).unwrap_or('0'));
                    m /= u64::from(base);
                }
                let length = self.tex.expand_to_string(tikra_tokens("\\pgfmathbasenumberlength")).ok().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
                while digits.len() < length {
                    digits.push('0');
                }
                let s: String = digits.iter().rev().collect();
                if let Some(name) = target.iter().find(|t| matches!(t, Token::Cs(_))) {
                    self.tex.set_meaning(name, super::macro_from(&s), false);
                }
            }
            B::GetLastXY => {
                // \pgfgetlastxy{\x}{\y}: the last point of the path, on the canvas, in pt
                let p = self.pgf_path.last.unwrap_or(self.last_xy);
                for v in [p.x, p.y] {
                    let target = self.argument()?;
                    if let Some(name) = target.iter().find(|t| matches!(t, Token::Cs(_))) {
                        let sp = (v / crate::PT * 65536.0).round() as i64;
                        self.tex.set_meaning(name, super::macro_from(&crate::tex::dimen::show(sp)), false);
                    }
                }
            }
            B::ToDigitList => {
                // \pgfmathtodigitlist{\macro}{number}: its digits as a list
                let target = self.argument()?;
                let n = self.argument_string()?;
                let list = n.trim().chars().map(String::from).collect::<Vec<_>>().join(",");
                if let Some(name) = target.iter().find(|t| matches!(t, Token::Cs(_))) {
                    self.tex.set_meaning(name, super::macro_from(&list), false);
                }
            }
            B::SetKeys => {
                // keyval's \setkeys{family}{key=value,…}: the code \define@key gave each key
                let family = self.argument_string()?;
                let list = self.argument()?;
                for item in crate::keys::split_list(&list) {
                    let mut code = vec![Token::cs(&format!("KV@{family}@{}", item.key.trim()))];
                    code.push(Token::Char('{', tex::Cat::Begin));
                    code.extend(item.value.unwrap_or_default());
                    code.push(Token::Char('}', tex::Cat::End));
                    self.run_tokens(code)?;
                }
            }
            B::GetPath | B::SetPath => {
                // \pgfgetpath\macro keeps the path being built under the macro's name;
                // \pgfsetpath\macro makes it the path being built again
                let m = self.argument()?;
                let name = m.iter().find_map(|t| if let Token::Cs(n) = t { Some(n.to_string()) } else { None }).unwrap_or_default();
                if cmd == B::GetPath {
                    self.saved_paths.insert(format!("pgf@{name}"), self.pgf_path.segments.clone());
                } else if let Some(segs) = m
                    .iter()
                    .find(|t| matches!(t, Token::Cs(_)))
                    .and_then(|t| super::pgfplots::fillbetween::softpath_of(self, t))
                    .or_else(|| self.saved_paths.get(&format!("pgf@{name}")).cloned())
                {
                    self.pgf_path = PgfPath::default();
                    for s in segs {
                        self.pgf_add(s);
                    }
                }
            }
            B::IntersectionOfPaths => {
                // each code builds a path (what it draws is drawn); where the two paths cross
                let mut paths = Vec::new();
                for _ in 0..2 {
                    let code = self.argument()?;
                    self.begin_group();
                    self.begin_scope();
                    self.pgf_path = PgfPath::default();
                    let r = self.run_tokens(code);
                    paths.push(std::mem::take(&mut self.pgf_path.segments));
                    self.pgf_path = PgfPath::default();
                    self.end_scope();
                    self.end_group();
                    r?;
                }
                self.pgf_solutions = super::intersections::intersections(&paths[0], &paths[1]);
                // kept for `\pgfcomputeintersectionsegments`
                for (k, p) in paths.iter().enumerate() {
                    self.saved_paths.insert(format!("@intersection path {}", k + 1), p.clone());
                }
                let n = self.pgf_solutions.len();
                self.tex.set_meaning(&Token::cs("pgfintersectionsolutions"), super::macro_tokens(tikra_tokens(&n.to_string())), false);
            }
            B::ShadeColorToRgb => self.shade_color_to_rgb()?,
            B::ArcToPrecomputed => {
                // {center}{start angle}{end angle}{end point}{x radius}{y radius}{the radii's ratios}: the
                // arc of the axis-parallel ellipse
                let c = self.point_arg()?;
                let a0 = self.argument()?;
                let a1 = self.argument()?;
                let (a0, a1) = (self.number_of(a0)?, self.number_of(a1)?);
                let end = self.point_arg()?;
                let rx = self.argument()?;
                let ry = self.argument()?;
                let (rx, ry) = (self.length_of(rx)?, self.length_of(ry)?);
                let _ = self.argument()?;
                let _ = self.argument()?;
                for s in super::path::arc_segments(t.apply(c), rx, ry, a0, a1, &t) {
                    self.pgf_add(s);
                }
                self.pgf_path.last = Some(t.apply(end));
            }
        }
        Ok(())
    }
}

impl Engine {
    /// `\pgfpathqmoveto{x}{y}`, `\pgfpathqlineto`, `\pgfpathqcurveto` (six lengths) and
    /// `\pgfpathqcircle{radius}`: canvas lengths, untransformed.
    fn quick_path(&mut self, cmd: B) -> Result<(), Error> {
        match cmd {
            B::QMoveTo | B::QLineTo => {
                let x = self.argument()?;
                let y = self.argument()?;
                let p = Point::new(self.length_of(x)?, self.length_of(y)?);
                self.pgf_add(if cmd == B::QMoveTo { Segment::MoveTo(p) } else { Segment::LineTo(p) });
            }
            B::QCurveTo => {
                let mut v = [0.0; 6];
                for k in &mut v {
                    let a = self.argument()?;
                    *k = self.length_of(a)?;
                }
                self.pgf_add(Segment::CurveTo(Point::new(v[0], v[1]), Point::new(v[2], v[3]), Point::new(v[4], v[5])));
            }
            _ => {
                let r = self.argument()?;
                let r = self.length_of(r)?;
                self.pgf_add(Segment::MoveTo(Point::new(r, 0.0)));
                for s in super::path::arc_segments(Point::default(), r, r, 0.0, 360.0, &Transform::default()) {
                    self.pgf_add(s);
                }
                self.pgf_add(Segment::Close);
            }
        }
        Ok(())
    }

    /// `\pgfpointarcaxesattime{time}{center}{0° axis}{90° axis}{start}{end}`: the point of the arc at
    /// that time, and the direction the arc goes there.
    fn arc_axes_at_time(&mut self) -> Result<(Point, Point), Error> {
        let time = self.argument()?;
        let time = self.number_of(time)?;
        let c = self.point_arg()?;
        let x = self.point_arg()?;
        let y = self.point_arg()?;
        let a0 = self.argument()?;
        let a1 = self.argument()?;
        let (a0, a1) = (self.number_of(a0)?, self.number_of(a1)?);
        let (s, co) = (a0 + (a1 - a0) * time).to_radians().sin_cos();
        let tangent = y.scale(co).minus(x.scale(s));
        let tangent = if a0 > a1 { tangent.scale(-1.0) } else { tangent };
        Ok((c.plus(x.scale(co)).plus(y.scale(s)), tangent))
    }

    /// The coordinate system of a point on a line or curve: shifted there; with `\pgfslopedattimetrue`
    /// turned along `tangent` (kept upright unless `\pgfallowupsidedownattimetrue`); with
    /// `\pgfresetnontranslationattimetrue` without the rotations and scalings before.
    fn transform_at_time(&mut self, at: Point, tangent: Point) {
        let on = |e: &Engine, name: &str| e.tex.meaning_of(name) == Meaning::Prim(tex::Prim::Iftrue);
        let mut t = self.st.transform.compose(&Transform::translation(at.x, at.y));
        if on(self, "ifpgfresetnontranslationattime") {
            t = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, ..t };
        }
        if on(self, "ifpgfslopedattime") {
            let mut d = tangent;
            if !on(self, "ifpgfallowupsidedownattime") && d.x < 0.0 {
                d = d.scale(-1.0);
            }
            let d = if d.norm() > 1e-12 { d.scale(1.0 / d.norm()) } else { Point::new(1.0, 0.0) };
            t = t.compose(&Transform { a: d.x, b: d.y, c: -d.y, d: d.x, e: 0.0, f: 0.0 });
        }
        self.st.transform = t;
    }

    /// `\pgflowlevel{code}`: the transformation the code makes (from none) applies to the canvas, up
    /// to the end of the scope, line widths and text included.
    fn low_level(&mut self, code: Vec<Token>) -> Result<(), Error> {
        self.begin_group();
        self.st.transform = Transform::default();
        let r = self.run_tokens(code);
        let t = self.st.transform;
        self.end_group();
        r?;
        self.open_in_scope(crate::canvas::Item::Transform { transform: t });
        Ok(())
    }

    /// A multipart node's text from the boxes `\setbox\pgfnodepart<name>box=\hbox{…}` filled: the
    /// text part first, then the others with `\nodepart{name}`.
    fn node_part_boxes(&mut self) -> Vec<Token> {
        let boxes = std::mem::take(&mut self.tex.named_boxes);
        let mut parts: Vec<(String, Vec<Token>)> = boxes.iter().filter_map(|(n, b)| Some((n.strip_prefix("pgfnodepart")?.strip_suffix("box")?.to_string(), b.clone()))).collect();
        parts.sort_by_key(|(n, _)| n != "text");
        let mut text = Vec::new();
        for (name, body) in parts {
            if name != "text" {
                text.extend(tikra_tokens(&format!("\\nodepart{{{name}}}")));
            }
            text.extend(body);
        }
        self.tex.named_boxes = boxes;
        text
    }
}

impl Engine {
    /// `\pgfplotstreamend`: the points streamed go into the path as the plot handler makes it, or
    /// each gets the mark handler's code, shifted there.
    fn plot_stream_end(&mut self) -> Result<(), Error> {
        let points = std::mem::take(&mut self.plot_stream);
        let (handler, code) = self.st.basic_plot.clone();
        match handler.as_str() {
            "discard" => {}
            "record" => {
                // the stream as commands in the macro, its points in the coordinate system
                let inv = self.st.transform.inverse();
                let mut v = tikra_tokens("\\pgfplotstreamstart");
                for p in points {
                    let q = inv.apply(p);
                    v.extend(tikra_tokens(&format!("\\pgfplotstreampoint{{\\pgfqpoint{{{}pt}}{{{}pt}}}}", q.x / PT, q.y / PT)));
                }
                v.extend(tikra_tokens("\\pgfplotstreamend"));
                if let Some(m) = code.iter().find(|t| matches!(t, Token::Cs(_))) {
                    self.tex.set_meaning(m, super::macro_tokens(v), true);
                }
            }
            h if h.starts_with("custom:") => {
                // a declared handler: its code at each point, `##1` the point, `#k` its arguments
                let code = self.plot_handlers.get(&h["custom:".len()..]).cloned().unwrap_or_default();
                let inv = self.st.transform.inverse();
                let params = super::options::split_arguments(&self.st.basic_plot.1, 9);
                for p in points {
                    let q = inv.apply(p);
                    let mut text = code.replace("##1", &format!("\\pgfqpoint{{{}pt}}{{{}pt}}", q.x / PT, q.y / PT));
                    for (k, a) in params.iter().enumerate() {
                        text = text.replace(&format!("#{}", k + 1), &tex::tokens_to_string(a));
                    }
                    self.run_tokens(tikra_tokens(&text))?;
                }
            }
            h if h == "mark" || h.starts_with("mark listed:") => {
                let listed: Option<Vec<usize>> = h.strip_prefix("mark listed:").map(|l| l.split(',').filter_map(|i| i.trim().parse().ok()).collect());
                let t = self.st.transform;
                for (i, p) in points.into_iter().enumerate() {
                    if listed.as_ref().is_some_and(|l| !l.contains(&(i + 1))) {
                        continue;
                    }
                    self.begin_group();
                    self.begin_scope();
                    self.st.transform = Transform::translation(p.x, p.y).compose(&Transform { e: 0.0, f: 0.0, ..t });
                    let r = self.run_tokens(code.clone());
                    self.end_scope();
                    self.end_group();
                    r?;
                }
            }
            h => {
                let saved = std::mem::replace(&mut self.st.plot_handler, h.to_string());
                let r = super::path::plot_segments(self, &points);
                self.st.plot_handler = saved;
                let mut segs = r?;
                // \pgfsetlinetofirstplotpoint: the plot goes on from the path's last point
                if self.st.plot_first_lineto && self.pgf_path.last.is_some() {
                    if let Some(Segment::MoveTo(p)) = segs.first().copied() {
                        segs[0] = Segment::LineTo(p);
                    }
                }
                for s in segs {
                    self.pgf_add(s);
                }
            }
        }
        Ok(())
    }
}
