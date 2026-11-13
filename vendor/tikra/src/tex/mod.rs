//! A small TeX: what TikZ code needs from TeX itself, so figures read as TeX reads them.
//!
//! Tokens with category codes, read lazily from the text so `\makeatletter` applies to what follows;
//! macros with parameters (delimited ones too), `\def`, `\edef`, `\let`, `\newcommand` with an optional
//! argument, `\NewDocumentCommand` and its argument types, `\newenvironment`; groups, with local and
//! `\global` assignments; count and dimen registers; conditionals; `\expandafter`, `\csname`,
//! `\string`, `\the`, `\number`, `\noexpand` and the e-TeX extensions pgf relies on (`\numexpr`,
//! `\dimexpr`, `\unexpanded`, `\detokenize`, `\ifdefined`, `\ifcsname`, `\unless`, `\protected`).
//!
//! What is drawn is not here: commands such as `\draw` are [`Meaning::Native`], run by the TikZ layer
//! with full access to the token stream, the way `tikz.code.tex` reads its input.

pub mod dimen;
mod xstring;

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;

use crate::Error;

/// Category codes, as in TeX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cat {
    Escape,
    Begin,
    End,
    Math,
    Align,
    EndLine,
    Param,
    Sup,
    Sub,
    Ignored,
    Space,
    Letter,
    Other,
    Active,
    Comment,
    Invalid,
}

impl Cat {
    pub fn from_code(n: i64) -> Option<Cat> {
        use Cat::*;
        Some(match n {
            0 => Escape,
            1 => Begin,
            2 => End,
            3 => Math,
            4 => Align,
            5 => EndLine,
            6 => Param,
            7 => Sup,
            8 => Sub,
            9 => Ignored,
            10 => Space,
            11 => Letter,
            12 => Other,
            13 => Active,
            14 => Comment,
            15 => Invalid,
            _ => return None,
        })
    }

    pub fn code(self) -> i64 {
        self as i64
    }
}

/// A token: a control sequence, or a character with its category.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    Cs(Rc<str>),
    Char(char, Cat),
}

impl Token {
    pub fn cs(name: &str) -> Token {
        Token::Cs(Rc::from(name))
    }
    pub fn other(c: char) -> Token {
        Token::Char(c, Cat::Other)
    }
    pub fn is_char(&self, c: char) -> bool {
        matches!(self, Token::Char(x, cat) if *x == c && !matches!(cat, Cat::Active))
    }
    pub fn is_cs(&self, name: &str) -> bool {
        matches!(self, Token::Cs(n) if &**n == name)
    }
    pub fn is_space(&self) -> bool {
        matches!(self, Token::Char(_, Cat::Space))
    }
}

/// A parameter text item: a token to match, or `#n`.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Tok(Token),
    Arg(u8),
}

/// A body item: a token, or `#n` to replace.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Tok(Token),
    Arg(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Macro {
    pub params: Vec<Param>,
    pub body: Vec<Body>,
    /// `\newcommand\x[2][default]{…}`: the first argument is optional, in brackets.
    pub optional: Option<Vec<Token>>,
    pub protected: bool,
}

/// TeX primitives (and the few LaTeX and plain commands better done natively).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    // expandable
    Expandafter,
    Noexpand,
    Csname,
    Endcsname,
    Stringify,
    Number,
    Romannumeral,
    The,
    ShowMeaning,
    Unexpanded,
    Detokenize,
    If,
    Ifcat,
    Ifx,
    Ifnum,
    Ifdim,
    Ifodd,
    Ifcase,
    Iftrue,
    Iffalse,
    Ifdefined,
    Ifcsname,
    Ifvmode,
    Ifhmode,
    Ifmmode,
    Ifinner,
    Else,
    Or,
    Fi,
    Unless,
    Jobname,
    Pdfstrcmp,
    // unexpandable
    Relax,
    Def,
    Gdef,
    Edef,
    Xdef,
    Let,
    Futurelet,
    Global,
    Long,
    Outer,
    Protected,
    Begingroup,
    Endgroup,
    Count,
    Dimen,
    Skip,
    Toks,
    Countdef,
    Dimendef,
    Skipdef,
    Toksdef,
    Chardef,
    Mathchardef,
    Advance,
    Multiply,
    Divide,
    Catcode,
    Numexpr,
    Dimexpr,
    Glueexpr,
    Afterassignment,
    Aftergroup,
    Ignorespaces,
    Par,
    Message,
    Immediate,
    Write,
    Typeout,
    Errmessage,
    Showtokens,
    Newcount,
    Newdimen,
    Newskip,
    Newlength,
    Newtoks,
    Newif,
    Setlength,
    Addtolength,
    Setcounter,
    Addtocounter,
    Newcounter,
    Stepcounter,
    Value,
    Arabic,
    Newcommand,
    Renewcommand,
    Providecommand,
    Declarerobustcommand,
    /// `\NewDocumentCommand` and its kin (the kind by its name, which `\let` keeps)
    Newdocumentcommand(&'static str),
    /// what a document command expands to first: reads its arguments by its specification
    Documentarguments,
    /// `\IfNoValueTF`, `\IfBooleanTF` and their kin (the kind by its name)
    Documentif(&'static str),
    Newenvironment,
    Renewenvironment,
    Makeatletter,
    Makeatother,
    Ifnextchar,
    Firstoftwo,
    Secondoftwo,
    Gobble,
    Gobbletwo,
    Firstofone,
    Ifthenelse,
    Whiledo,
    Ifstrequal,
    /// etoolbox's `\ifstrempty`, `\ifblank`, `\ifdefempty` (the kind by its name)
    Ifstrempty(&'static str),
    Ifboolean,
    Newboolean,
    /// `\g@addto@macro`, etoolbox's `\appto` and its kin, babel's `\addto` (the kind by its name)
    Addtomacro(&'static str),
    /// the xstring package's commands (by its name)
    Xstring(&'static str),
    Setboolean,
    Wd,
    Ht,
    Dp,
    Setbox,
    Hbox,
    Uppercase,
    Lowercase,
    Show,
    Showthe,
}

/// What a control sequence (or active character) means.
#[derive(Debug, Clone, PartialEq)]
pub enum Meaning {
    Macro(Rc<Macro>),
    /// `\let\x=a`: the character itself.
    Char(char, Cat),
    Prim(Prim),
    Count(u32),
    Dimen(u32),
    Skip(u32),
    Toks(u32),
    CharDef(i64),
    /// A command of the drawing layer, by number.
    Native(u16),
    /// `\let\x\undefined`
    Undefined,
}

impl Meaning {
    pub fn is_expandable(&self) -> bool {
        match self {
            Meaning::Macro(_) => true,
            Meaning::Prim(p) => p.is_expandable(),
            _ => false,
        }
    }
}

impl Prim {
    pub fn is_expandable(self) -> bool {
        use Prim::*;
        matches!(
            self,
            Expandafter
                | Noexpand
                | Csname
                | Stringify
                | Number
                | Romannumeral
                | The
                | ShowMeaning
                | Unexpanded
                | Detokenize
                | If
                | Ifcat
                | Ifx
                | Ifnum
                | Ifdim
                | Ifodd
                | Ifcase
                | Iftrue
                | Iffalse
                | Ifdefined
                | Ifcsname
                | Ifvmode
                | Ifhmode
                | Ifmmode
                | Ifinner
                | Else
                | Or
                | Fi
                | Unless
                | Jobname
                | Pdfstrcmp
                | Ifnextchar
                | Firstoftwo
                | Secondoftwo
                | Gobble
                | Gobbletwo
                | Firstofone
                | Value
                | Arabic
                | Ifthenelse
                | Whiledo
                | Ifstrequal
                | Ifstrempty(_)
                | Ifboolean
                | Documentarguments
                | Documentif(_)
        )
    }

    fn is_conditional(self) -> bool {
        use Prim::*;
        matches!(self, If | Ifcat | Ifx | Ifnum | Ifdim | Ifodd | Ifcase | Iftrue | Iffalse | Ifdefined | Ifcsname | Ifvmode | Ifhmode | Ifmmode | Ifinner)
    }
}

/// What has been read of the main text since a statement started, as offsets in characters.
#[derive(Debug, Clone, Default)]
struct Reading {
    first: Option<usize>,
    end: usize,
    /// The last tokens read from the text, and where.
    recent: Vec<(Token, usize, usize)>,
    /// Those put back to look ahead: a statement that peeks at what follows did not read it.
    backed: Vec<(Token, usize, usize)>,
}

impl Reading {
    /// `t` comes from the text, from `start` to just before `end`.
    fn read(&mut self, t: Token, start: usize, end: usize) {
        self.first.get_or_insert(start);
        self.end = end;
        if self.recent.len() >= 64 {
            self.recent.drain(..32);
        }
        self.recent.push((t, start, end));
    }

    /// `t` is put back: if it is the last one read, it has not been read yet.
    fn back(&mut self, t: &Token) {
        if self.recent.last().is_some_and(|r| r.0 == *t) {
            let Some(r) = self.recent.pop() else { return };
            self.end = self.recent.last().map_or(r.1, |p| p.2);
            if self.first == Some(r.1) {
                self.first = None;
            }
            self.backed.push(r);
        }
    }

    /// `t` comes from a token list: if it is one put back, it is read now.
    fn again(&mut self, t: &Token) {
        if self.backed.last().is_some_and(|r| r.0 == *t) {
            if let Some((t, start, end)) = self.backed.pop() {
                self.read(t, start, end);
            }
        }
    }
}

/// Where tokens come from: a token list, or text read with the current category codes.
#[derive(Clone)]
enum Frame {
    Tokens(VecDeque<Token>),
    Text { chars: Rc<Vec<char>>, pos: usize, skip_blanks: bool },
}

/// One value saved on entering a group, put back when it ends.
#[derive(Clone)]
enum Saved {
    Meaning(Rc<str>, Option<Meaning>),
    Active(char, Option<Meaning>),
    Count(u32, Option<i64>),
    Dimen(u32, Option<i64>),
    Toks(u32, Option<Rc<Vec<Token>>>),
    Catcode(char, Option<Cat>),
}

/// Cap on expansions per figure: `\def\a{\a}\a` stops with an error instead of hanging.
pub const MAX_EXPANSIONS: usize = 2_000_000;
/// Cap on tokens in one list (a macro doubling itself on each step stops here).
pub const MAX_TOKENS: usize = 1_000_000;

/// A fast hash for the tables TeX looks into at every token (names, registers, catcodes): FxHash,
/// as rustc uses. Names come from the document, so crafted ones can collide; that only slows the
/// lookups, and the engine's caps on statements and expansions bound the work.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(*c)).wrapping_mul(K);
        }
        for b in rest {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*b)).wrapping_mul(K);
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(v)).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type Map<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

/// The state of TeX: input, meanings, registers, groups.
#[derive(Clone)]
pub struct Tex {
    input: Vec<Frame>,
    /// The frames below this many are out of reach: code run on its own (a key's code, an
    /// `\edef`'s body) ends where its tokens end, and reads nothing that comes after it.
    floor: usize,
    /// How far the main text has been read, for the source map. `None` when nobody asked.
    reading: Option<Reading>,
    meanings: Map<Rc<str>, Meaning>,
    active: Map<char, Meaning>,
    pub counts: Map<u32, i64>,
    /// in scaled points (65536 sp = 1 pt)
    pub dimens: Map<u32, i64>,
    pub toks: Map<u32, Rc<Vec<Token>>>,
    catcodes: Map<char, Cat>,
    save: Vec<Vec<Saved>>,
    /// a `\global` prefix is pending
    global: bool,
    /// open conditionals whose true branch is being read (an `\else` there skips to `\fi`)
    conds: usize,
    next_register: u32,
    pub expansions: usize,
    /// expansions since the drawing layer last called [`Tex::new_statement`]
    pub statement_expansions: usize,
    after_group: Vec<Vec<Token>>,
    after_assignment: Option<Token>,
    pub warnings: Vec<String>,
    /// `\begin{name}` defined by `\newenvironment`: (begin code with its parameters, end code)
    pub environments: HashMap<String, (Rc<Macro>, Vec<Token>)>,
    /// the current environment names, innermost last, for `\end`
    pub env_stack: Vec<String>,
    /// `\setbox\name=\hbox{…}` for a box named by a control sequence (`\pgfnodepartlowerbox`):
    /// what was put in it, in order
    pub named_boxes: Vec<(String, Vec<Token>)>,
    /// the width, height and depth of box registers that have them (sp), by number: what
    /// `\wd`, `\ht`, `\dp` read (0pt otherwise)
    pub box_dims: Map<i64, [i64; 3]>,
}

impl Default for Tex {
    fn default() -> Self {
        Tex::new()
    }
}

/// Registers reserved before `\newcount` hands out new ones (pgf uses low numbers as scratch).
const FIRST_FREE_REGISTER: u32 = 256;

/// The register of the first of [`LATEX_LENGTHS`] (the others follow it).
const FIRST_LATEX_LENGTH: u32 = 180;

/// LaTeX's lengths and their values in `article` at 10pt on letter paper, as TeX shows them;
/// `\paperwidth` and `\paperheight` first.
const LATEX_LENGTHS: &[(&str, &str)] = &[
    ("paperwidth", "614.295pt"),
    ("paperheight", "794.96999pt"),
    ("textheight", "550pt"),
    ("vsize", "550pt"),
    ("oddsidemargin", "62pt"),
    ("evensidemargin", "62pt"),
    ("topmargin", "16pt"),
    ("headheight", "12pt"),
    ("headsep", "25pt"),
    ("footskip", "30pt"),
    ("marginparwidth", "65pt"),
    ("marginparsep", "11pt"),
    ("marginparpush", "5pt"),
    ("hoffset", "0pt"),
    ("voffset", "0pt"),
    ("maxdepth", "5pt"),
    ("topskip", "10pt"),
    ("columnsep", "10pt"),
    ("columnseprule", "0pt"),
    ("parskip", "0pt"),
    ("tabcolsep", "6pt"),
    ("arraycolsep", "5pt"),
    ("arrayrulewidth", "0.4pt"),
    ("doublerulesep", "2pt"),
    ("footnotesep", "6.65pt"),
    ("jot", "3pt"),
    ("lineskip", "1pt"),
    ("lineskiplimit", "0pt"),
    ("emergencystretch", "0pt"),
    ("labelsep", "5pt"),
    ("labelwidth", "20.00003pt"),
    ("leftmargin", "25.00003pt"),
    ("rightmargin", "0pt"),
    ("itemindent", "0pt"),
    ("listparindent", "0pt"),
    ("itemsep", "4pt"),
    ("parsep", "4pt"),
    ("topsep", "8pt"),
    ("partopsep", "2pt"),
    ("smallskipamount", "3pt"),
    ("medskipamount", "6pt"),
    ("bigskipamount", "12pt"),
    ("floatsep", "12pt"),
    ("textfloatsep", "20pt"),
    ("intextsep", "12pt"),
    ("abovedisplayskip", "10pt"),
    ("belowdisplayskip", "10pt"),
    ("captionmargin", "0pt"),
];

/// Those of [`LATEX_LENGTHS`] that are glue (`\skip` registers).
const SKIPS: &[&str] = &[
    "topskip",
    "parskip",
    "itemsep",
    "parsep",
    "topsep",
    "partopsep",
    "smallskipamount",
    "medskipamount",
    "bigskipamount",
    "floatsep",
    "textfloatsep",
    "intextsep",
    "abovedisplayskip",
    "belowdisplayskip",
];

/// LaTeX's paper sizes: name, width, height.
const PAPERS: &[(&str, &str, &str)] = &[
    ("letterpaper", "614.295pt", "794.96999pt"),
    ("a4paper", "597.50787pt", "845.04684pt"),
    ("a5paper", "421.10078pt", "597.50787pt"),
    ("b5paper", "500.7685pt", "711.3189pt"),
    ("legalpaper", "614.295pt", "1011.78pt"),
    ("executivepaper", "523.95749pt", "758.83499pt"),
];

impl Tex {
    pub fn new() -> Tex {
        let mut t = Tex {
            input: Vec::new(),
            floor: 0,
            reading: None,
            meanings: Map::default(),
            active: Map::default(),
            counts: Map::default(),
            dimens: Map::default(),
            toks: Map::default(),
            catcodes: Map::default(),
            save: Vec::new(),
            global: false,
            conds: 0,
            next_register: FIRST_FREE_REGISTER,
            expansions: 0,
            statement_expansions: 0,
            after_group: Vec::new(),
            after_assignment: None,
            warnings: Vec::new(),
            environments: HashMap::new(),
            env_stack: Vec::new(),
            named_boxes: Vec::new(),
            box_dims: Map::default(),
        };
        t.define_primitives();
        t
    }

    fn define_primitives(&mut self) {
        use Prim::*;
        let list: &[(&str, Prim)] = &[
            ("expandafter", Expandafter),
            ("noexpand", Noexpand),
            ("csname", Csname),
            ("endcsname", Endcsname),
            ("string", Stringify),
            ("number", Number),
            ("romannumeral", Romannumeral),
            ("the", The),
            ("meaning", ShowMeaning),
            ("unexpanded", Unexpanded),
            ("detokenize", Detokenize),
            ("if", If),
            ("ifcat", Ifcat),
            ("ifx", Ifx),
            ("ifnum", Ifnum),
            ("ifdim", Ifdim),
            ("ifodd", Ifodd),
            ("ifcase", Ifcase),
            ("iftrue", Iftrue),
            ("iffalse", Iffalse),
            ("ifdefined", Ifdefined),
            ("ifcsname", Ifcsname),
            ("ifvmode", Ifvmode),
            ("ifhmode", Ifhmode),
            ("ifmmode", Ifmmode),
            ("ifinner", Ifinner),
            ("else", Else),
            ("or", Or),
            ("fi", Fi),
            ("unless", Unless),
            ("jobname", Jobname),
            ("pdfstrcmp", Pdfstrcmp),
            ("strcmp", Pdfstrcmp),
            ("relax", Relax),
            ("def", Def),
            ("gdef", Gdef),
            ("edef", Edef),
            ("xdef", Xdef),
            ("let", Let),
            ("futurelet", Futurelet),
            ("global", Global),
            ("long", Long),
            ("outer", Outer),
            ("protected", Protected),
            ("begingroup", Begingroup),
            ("endgroup", Endgroup),
            ("count", Count),
            ("dimen", Dimen),
            ("skip", Skip),
            ("toks", Toks),
            ("countdef", Countdef),
            ("dimendef", Dimendef),
            ("skipdef", Skipdef),
            ("toksdef", Toksdef),
            ("chardef", Chardef),
            ("mathchardef", Mathchardef),
            ("advance", Advance),
            ("multiply", Multiply),
            ("divide", Divide),
            ("catcode", Catcode),
            ("numexpr", Numexpr),
            ("dimexpr", Dimexpr),
            ("glueexpr", Glueexpr),
            ("afterassignment", Afterassignment),
            ("aftergroup", Aftergroup),
            ("ignorespaces", Ignorespaces),
            ("par", Par),
            ("message", Message),
            ("immediate", Immediate),
            ("write", Write),
            ("typeout", Typeout),
            ("errmessage", Errmessage),
            ("PackageWarning", Typeout),
            ("showtokens", Showtokens),
            ("newcount", Newcount),
            // a box register: a number, as `\setbox` reads it
            ("newbox", Newcount),
            ("newdimen", Newdimen),
            ("newskip", Newskip),
            ("newlength", Newlength),
            ("newtoks", Newtoks),
            ("newif", Newif),
            ("setlength", Setlength),
            ("addtolength", Addtolength),
            ("setcounter", Setcounter),
            ("addtocounter", Addtocounter),
            ("newcounter", Newcounter),
            ("stepcounter", Stepcounter),
            ("refstepcounter", Stepcounter),
            ("value", Value),
            ("arabic", Arabic),
            ("newcommand", Newcommand),
            ("renewcommand", Renewcommand),
            ("providecommand", Providecommand),
            ("DeclareRobustCommand", Declarerobustcommand),
            ("NewDocumentCommand", Newdocumentcommand("NewDocumentCommand")),
            ("RenewDocumentCommand", Newdocumentcommand("RenewDocumentCommand")),
            ("ProvideDocumentCommand", Newdocumentcommand("ProvideDocumentCommand")),
            ("DeclareDocumentCommand", Newdocumentcommand("DeclareDocumentCommand")),
            ("NewExpandableDocumentCommand", Newdocumentcommand("NewExpandableDocumentCommand")),
            ("RenewExpandableDocumentCommand", Newdocumentcommand("RenewExpandableDocumentCommand")),
            ("ProvideExpandableDocumentCommand", Newdocumentcommand("ProvideExpandableDocumentCommand")),
            ("DeclareExpandableDocumentCommand", Newdocumentcommand("DeclareExpandableDocumentCommand")),
            ("tikra@documentarguments", Documentarguments),
            ("IfNoValueTF", Documentif("IfNoValueTF")),
            ("IfNoValueT", Documentif("IfNoValueT")),
            ("IfNoValueF", Documentif("IfNoValueF")),
            ("IfValueTF", Documentif("IfValueTF")),
            ("IfValueT", Documentif("IfValueT")),
            ("IfValueF", Documentif("IfValueF")),
            ("IfBooleanTF", Documentif("IfBooleanTF")),
            ("IfBooleanT", Documentif("IfBooleanT")),
            ("IfBooleanF", Documentif("IfBooleanF")),
            ("newenvironment", Newenvironment),
            ("renewenvironment", Renewenvironment),
            ("makeatletter", Makeatletter),
            ("makeatother", Makeatother),
            ("@ifnextchar", Ifnextchar),
            ("pgfutil@ifnextchar", Ifnextchar),
            ("@firstoftwo", Firstoftwo),
            ("pgfutil@firstoftwo", Firstoftwo),
            ("@secondoftwo", Secondoftwo),
            ("pgfutil@secondoftwo", Secondoftwo),
            ("@gobble", Gobble),
            ("pgfutil@gobble", Gobble),
            ("@gobbletwo", Gobbletwo),
            ("@firstofone", Firstofone),
            ("pgfutil@firstofone", Firstofone),
            ("ifthenelse", Ifthenelse),
            ("whiledo", Whiledo),
            ("ifstrequal", Ifstrequal),
            ("ifstrempty", Ifstrempty("ifstrempty")),
            ("ifblank", Ifstrempty("ifblank")),
            ("ifdefempty", Ifstrempty("ifdefempty")),
            ("g@addto@macro", Addtomacro("g@addto@macro")),
            ("addto", Addtomacro("addto")),
            ("appto", Addtomacro("appto")),
            ("gappto", Addtomacro("gappto")),
            ("eappto", Addtomacro("eappto")),
            ("xappto", Addtomacro("xappto")),
            ("preto", Addtomacro("preto")),
            ("gpreto", Addtomacro("gpreto")),
            ("epreto", Addtomacro("epreto")),
            ("xpreto", Addtomacro("xpreto")),
            ("boolean", Ifboolean),
            ("newboolean", Newboolean),
            ("setboolean", Setboolean),
            ("wd", Wd),
            ("ht", Ht),
            ("dp", Dp),
            ("setbox", Setbox),
            ("hbox", Hbox),
            ("uppercase", Uppercase),
            ("lowercase", Lowercase),
            ("show", Show),
            ("showthe", Showthe),
        ];
        for (name, p) in list {
            self.meanings.insert(Rc::from(*name), Meaning::Prim(*p));
        }
        for name in xstring::COMMANDS {
            self.meanings.insert(Rc::from(*name), Meaning::Prim(Xstring(name)));
        }
        // the mark \noexpand and \unexpanded put before a token; on its own it does nothing
        self.meanings.insert(Rc::from("tikra@noexpand"), Meaning::Prim(Relax));
        // a few plain and LaTeX macros, defined the way they are there
        // the value of a missing optional argument of a document command, and of an `s` one
        self.define_text(r"\def\tikra@novalue{-NoValue-}\def\BooleanTrue{\BooleanTrue}\def\BooleanFalse{\BooleanFalse}");
        self.define_text(r"\def\space{ }\def\empty{}\def\@empty{}\def\pgfutil@empty{}\def\@nil{\@nil}\def\pgfutil@nil{\pgfutil@nil}\def\pgfeov{\pgfeov}");
        self.define_text(r"\def\@ifundefined#1{\ifcsname#1\endcsname\expandafter\ifx\csname#1\endcsname\relax\expandafter\expandafter\expandafter\@firstoftwo\else\expandafter\expandafter\expandafter\@secondoftwo\fi\else\expandafter\@firstoftwo\fi}");
        self.define_text(r"\let\pgfutil@ifundefined\@ifundefined\def\@namedef#1{\expandafter\def\csname#1\endcsname}\def\@nameuse#1{\csname#1\endcsname}");
        self.define_text(r"\def\loop#1\repeat{\def\body{#1}\iterate}\def\iterate{\body\let\next\iterate\else\let\next\relax\fi\next}\let\repeat\fi");
        // pgfmath's loop: as \loop, with \pgfmathcounter counting the rounds from 1
        self.define_text(
            r"\def\pgfmathloop#1\repeatpgfmathloop{\def\pgfmathcounter{1}\def\tikra@mathloop@body{#1}\tikra@mathloop}\let\repeatpgfmathloop\fi
              \def\tikra@mathloop{\tikra@mathloop@body\edef\pgfmathcounter{\the\numexpr\pgfmathcounter+1\relax}\let\next\tikra@mathloop\else\let\next\relax\fi\next}",
        );
        self.define_text(
            r"\def\obeyspaces{}\def\protect{}\def\nobreak{}\def\noindent{}\def\indent{}\def\leavevmode{}\def\unskip{}\def\strut{}\def\quad{\ }\def\qquad{\ \ }\def\enspace{\ }",
        );
        // the preview package's setup, as in `\PreviewEnvironment{tikzpicture}\setlength\PreviewBorder{5pt}`:
        // the figure is already all that is drawn
        self.define_text(r"\def\PreviewEnvironment#1{}\def\PreviewMacro#1{}");
        // amsmath's `\DeclareMathOperator{\p}{p}`: `\p` is the operator's name in upright letters
        // (starred, with limits above and below)
        self.define_text(
            r"\def\DeclareMathOperator{\@ifnextchar*{\tikra@operator@star}{\tikra@operator}}\def\tikra@operator#1#2{\def#1{\operatorname{#2}}}\def\tikra@operator@star*#1#2{\def#1{\operatorname*{#2}}}",
        );
        // etoolbox's toggles: each one a macro that takes the first or the second of two branches
        self.define_text(
            r"\def\newtoggle#1{\expandafter\let\csname tikra@toggle@#1\endcsname\@secondoftwo}\def\providetoggle#1{\ifcsname tikra@toggle@#1\endcsname\else\newtoggle{#1}\fi}\def\toggletrue#1{\expandafter\let\csname tikra@toggle@#1\endcsname\@firstoftwo}\let\togglefalse\newtoggle\def\settoggle#1#2{\csname toggle#2\endcsname{#1}}\def\iftoggle#1{\csname tikra@toggle@#1\endcsname}\def\nottoggle#1#2#3{\iftoggle{#1}{#3}{#2}}",
        );
        for i in 0..10u32 {
            self.dimens.insert(i, 0);
            self.counts.insert(i, 0);
        }
        self.meanings.insert(Rc::from("z@"), Meaning::Dimen(250));
        self.meanings.insert(Rc::from("p@"), Meaning::Dimen(251));
        self.dimens.insert(251, 65536);
        self.meanings.insert(Rc::from("pgfutil@tempdima"), Meaning::Dimen(252));
        self.meanings.insert(Rc::from("pgfutil@tempdimb"), Meaning::Dimen(253));
        self.meanings.insert(Rc::from("pgfutil@tempcnta"), Meaning::Count(252));
        self.meanings.insert(Rc::from("pgfutil@tempcntb"), Meaning::Count(253));
        // pgf's scratch counters, which code of a figure's own uses too
        for (i, c) in ["a", "b", "c", "d"].iter().enumerate() {
            self.meanings.insert(Rc::from(format!("c@pgf@count{c}")), Meaning::Count(244 + i as u32));
        }
        self.meanings.insert(Rc::from("m@ne"), Meaning::CharDef(-1));
        self.meanings.insert(Rc::from("z@"), Meaning::Dimen(250));
        self.meanings.insert(Rc::from("@ne"), Meaning::CharDef(1));
        self.meanings.insert(Rc::from("tw@"), Meaning::CharDef(2));
        self.meanings.insert(Rc::from("thr@@"), Meaning::CharDef(3));
        self.meanings.insert(Rc::from("@m"), Meaning::CharDef(1000));
        self.meanings.insert(Rc::from("@M"), Meaning::CharDef(10000));
        self.meanings.insert(Rc::from("active"), Meaning::CharDef(13));
        self.meanings.insert(Rc::from("textwidth"), Meaning::Dimen(254));
        self.dimens.insert(254, 345 * 65536);
        self.meanings.insert(Rc::from("linewidth"), Meaning::Dimen(255));
        self.dimens.insert(255, 345 * 65536);
        self.meanings.insert(Rc::from("columnwidth"), Meaning::Dimen(255));
        self.meanings.insert(Rc::from("hsize"), Meaning::Dimen(255));
        self.meanings.insert(Rc::from("baselineskip"), Meaning::Dimen(249));
        self.dimens.insert(249, 12 * 65536);
        self.meanings.insert(Rc::from("parindent"), Meaning::Dimen(248));
        self.meanings.insert(Rc::from("fboxsep"), Meaning::Dimen(247));
        self.dimens.insert(247, 3 * 65536);
        self.meanings.insert(Rc::from("fboxrule"), Meaning::Dimen(246));
        self.dimens.insert(246, 26214);
        self.meanings.insert(Rc::from("unitlength"), Meaning::Dimen(245));
        self.dimens.insert(245, 65536);
        self.meanings.insert(Rc::from("PreviewBorder"), Meaning::Dimen(244));
        // LaTeX's page layout and list lengths, as `article` sets them at 10pt on letter paper (glue
        // at its natural size), and the caption package's \captionmargin
        for (i, (name, value)) in LATEX_LENGTHS.iter().enumerate() {
            let r = FIRST_LATEX_LENGTH + i as u32;
            let skip = SKIPS.contains(name);
            self.meanings.insert(Rc::from(*name), if skip { Meaning::Skip(r) } else { Meaning::Dimen(r) });
            let v = self.dimen_of(string_tokens(value)).expect("a length");
            self.dimens.insert(r, v);
        }
    }

    /// `\paperwidth` and `\paperheight` from the paper options of `\documentclass` or the geometry
    /// package (`a4paper`, `paper=a5paper`, `landscape`); none of them leaves them as they are.
    pub fn paper_options(&mut self, options: &str) {
        let mut landscape = false;
        let mut size = None;
        for o in options.split(',').map(str::trim) {
            let o = o.strip_prefix("paper").and_then(|p| p.trim_start().strip_prefix('=')).map_or(o, str::trim);
            match PAPERS.iter().find(|(n, ..)| *n == o) {
                Some(&(_, w, h)) => size = Some((w, h)),
                None => landscape |= o == "landscape",
            }
        }
        if size.is_none() && !landscape {
            return;
        }
        let (w, h) = match size {
            Some((w, h)) => (self.dimen_of(string_tokens(w)).expect("a length"), self.dimen_of(string_tokens(h)).expect("a length")),
            None => (self.dimens[&FIRST_LATEX_LENGTH], self.dimens[&(FIRST_LATEX_LENGTH + 1)]),
        };
        let (w, h) = if landscape { (w.max(h), w.min(h)) } else { (w, h) };
        self.set_dimen(FIRST_LATEX_LENGTH, w, true);
        self.set_dimen(FIRST_LATEX_LENGTH + 1, h, true);
    }

    /// A Tex that only reads tokens, with `@` a letter (for text written by tikra itself).
    pub fn new_lexer() -> Tex {
        let mut t = Tex {
            input: Vec::new(),
            floor: 0,
            reading: None,
            meanings: Map::default(),
            active: Map::default(),
            counts: Map::default(),
            dimens: Map::default(),
            toks: Map::default(),
            catcodes: Map::default(),
            save: Vec::new(),
            global: false,
            conds: 0,
            next_register: FIRST_FREE_REGISTER,
            expansions: 0,
            statement_expansions: 0,
            after_group: Vec::new(),
            after_assignment: None,
            warnings: Vec::new(),
            environments: HashMap::new(),
            env_stack: Vec::new(),
            named_boxes: Vec::new(),
            box_dims: Map::default(),
        };
        t.catcodes.insert('@', Cat::Letter);
        t
    }

    /// Defines macros from TeX source (at load time; errors are bugs).
    pub fn define_text(&mut self, text: &str) {
        let saved_catcode = self.catcodes.get(&'@').copied();
        self.catcodes.insert('@', Cat::Letter);
        let depth = self.input.len();
        self.push_text(text);
        while self.input.len() > depth {
            match self.get_token() {
                Some(t) => {
                    let _ = self.execute_basic(t);
                }
                None => break,
            }
        }
        match saved_catcode {
            Some(c) => self.catcodes.insert('@', c),
            None => self.catcodes.remove(&'@'),
        };
    }

    // ---------------------------------------------------------------- category codes and input

    pub fn catcode(&self, c: char) -> Cat {
        if let Some(cat) = self.catcodes.get(&c) {
            return *cat;
        }
        match c {
            '\\' => Cat::Escape,
            '{' => Cat::Begin,
            '}' => Cat::End,
            '$' => Cat::Math,
            '&' => Cat::Align,
            '\r' | '\n' => Cat::EndLine,
            '#' => Cat::Param,
            '^' => Cat::Sup,
            '_' => Cat::Sub,
            '\0' => Cat::Ignored,
            ' ' | '\t' => Cat::Space,
            '~' => Cat::Active,
            '%' => Cat::Comment,
            '\u{7f}' => Cat::Invalid,
            c if c.is_alphabetic() => Cat::Letter,
            _ => Cat::Other,
        }
    }

    pub fn set_catcode(&mut self, c: char, cat: Cat) {
        let old = self.catcodes.get(&c).copied();
        // a \global prefix is for this assignment only
        if !std::mem::take(&mut self.global) {
            if let Some(level) = self.save.last_mut() {
                level.push(Saved::Catcode(c, old));
            }
        }
        self.catcodes.insert(c, cat);
    }

    pub fn push_text(&mut self, text: &str) {
        self.input.push(Frame::Text { chars: Rc::new(text.chars().collect()), pos: 0, skip_blanks: false });
    }

    /// Puts tokens back at the front of the input.
    pub fn push_tokens(&mut self, tokens: impl IntoIterator<Item = Token>) {
        let list: VecDeque<Token> = tokens.into_iter().collect();
        if list.is_empty() {
            return;
        }
        if let Some(Frame::Tokens(front)) = self.input.last_mut() {
            // join with the frame on top, so deep chains of back-pushed tokens do not pile up frames
            for t in list.into_iter().rev() {
                front.push_front(t);
            }
            return;
        }
        self.input.push(Frame::Tokens(list));
    }

    /// Pushes tokens as a frame of their own (never joined to the one below), for bounded reading.
    pub fn push_frame(&mut self, tokens: Vec<Token>) {
        self.input.push(Frame::Tokens(tokens.into_iter().collect()));
    }

    /// Reads a dimension from `tokens` alone (`\setlength{\l}{2pt}`): what is left of them is dropped.
    pub fn dimen_of(&mut self, tokens: Vec<Token>) -> Result<i64, Error> {
        let depth = self.input.len();
        let mut v = tokens;
        v.push(Token::cs("relax"));
        self.push_frame(v);
        let r = self.scan_dimen().and_then(|d| self.skip_glue_parts().map(|_| d));
        self.drain_to(depth);
        r
    }

    /// Reads an integer from `tokens` alone.
    pub fn int_of(&mut self, tokens: Vec<Token>) -> Result<i64, Error> {
        let depth = self.input.len();
        let mut v = tokens;
        v.push(Token::cs("relax"));
        self.push_frame(v);
        let r = self.scan_int();
        self.drain_to(depth);
        r
    }

    /// Reads `\numexpr …` (or `\dimexpr …` if `dim`, in sp) from the start of `tokens`, the command
    /// itself already taken: its value and the tokens after it.
    pub fn expr_of(&mut self, tokens: Vec<Token>, dim: bool) -> Result<(i64, Vec<Token>), Error> {
        let depth = self.input.len();
        let end = Token::cs("tikra@endexpr");
        let mut v = tokens;
        v.push(end.clone());
        self.push_frame(v);
        let r = if dim { self.scan_dimexpr() } else { self.scan_numexpr() };
        let mut rest = Vec::new();
        while let Some(t) = self.get_token_until(depth) {
            if t == end {
                break;
            }
            rest.push(t);
        }
        self.drain_to(depth);
        Ok((r?, rest))
    }

    pub fn back(&mut self, t: Token) {
        if let Some(r) = &mut self.reading {
            r.back(&t);
        }
        self.push_tokens([t]);
    }

    /// Starts noting how far the main text has been read: the text on top of the input is it.
    pub fn note_reading(&mut self) {
        self.reading = Some(Reading::default());
    }

    /// The characters of the main text read since the last [`Tex::end_statement`], from the first
    /// one to just after the last.
    pub fn statement_span(&self) -> Option<(usize, usize)> {
        let r = self.reading.as_ref()?;
        Some((r.first.unwrap_or(r.end), r.end))
    }

    /// A statement is over. Inside a macro's expansion the next one is still in the same call.
    pub fn end_statement(&mut self) {
        let Some(r) = &mut self.reading else { return };
        let waiting: Option<usize> = self.input.iter().skip(1).map(|f| if let Frame::Tokens(l) = f { Some(l.len()) } else { None }).sum();
        if waiting == Some(r.backed.len()) {
            r.first = None;
        }
    }

    /// The number of input frames (for reading only what was pushed after this point).
    pub fn depth(&self) -> usize {
        self.input.len()
    }

    /// Like [`Tex::get_x_token`], but only from frames above `depth`.
    pub fn get_x_token_until(&mut self, depth: usize) -> Result<Option<Token>, Error> {
        loop {
            let Some(t) = self.get_token_until(depth) else { return Ok(None) };
            let m = self.meaning(&t);
            if m.is_expandable() {
                self.expand(&t, &m)?;
                continue;
            }
            return Ok(Some(t));
        }
    }

    /// Drops all the input left (`\end{document}`: TeX reads no further).
    pub fn clear_input(&mut self) {
        self.input.clear();
    }

    /// Whether input is left (at any depth).
    pub fn has_input(&mut self) -> bool {
        if let Some(t) = self.get_token() {
            self.back(t);
            true
        } else {
            false
        }
    }

    /// The next token, unexpanded, or None at the end of the input.
    pub fn get_token(&mut self) -> Option<Token> {
        self.get_token_until(self.floor)
    }

    /// Reads nothing below `depth` from now on (until [`Tex::unseal`] with what this returns):
    /// the frames above it are code that ends where its tokens end.
    pub fn seal(&mut self, depth: usize) -> usize {
        let floor = depth.max(self.floor);
        std::mem::replace(&mut self.floor, floor)
    }

    /// Ends what [`Tex::seal`] began.
    pub fn unseal(&mut self, floor: usize) {
        self.floor = floor;
    }

    /// A token from frames above `depth` only.
    pub fn get_token_until(&mut self, depth: usize) -> Option<Token> {
        while self.input.len() > depth {
            let from_list = match self.input.last_mut()? {
                Frame::Tokens(list) => Some(list.pop_front()),
                Frame::Text { .. } => None,
            };
            let listed = from_list.is_some();
            let t = match from_list {
                Some(t) => t,
                None => self.read_text_token(),
            };
            match t {
                Some(t) => {
                    if let (Some(r), true) = (&mut self.reading, listed) {
                        r.again(&t);
                    }
                    return Some(t);
                }
                None => {
                    self.input.pop();
                }
            }
        }
        None
    }

    /// Reads one token from the text frame on top, applying TeX's rules for spaces, lines and comments.
    fn read_text_token(&mut self) -> Option<Token> {
        let Some(Frame::Text { chars, pos, skip_blanks }) = self.input.last() else { return None };
        let (chars, mut pos, mut skip) = (chars.clone(), *pos, *skip_blanks);
        let mut start;
        let result = loop {
            start = pos;
            let Some(&c) = chars.get(pos) else { break None };
            let next = chars.get(pos + 1).copied();
            let cat = self.catcode(c);
            match cat {
                Cat::Escape => {
                    pos += 1;
                    let Some(&first) = chars.get(pos) else {
                        skip = false;
                        break Some(Token::cs(""));
                    };
                    if self.catcode(first) == Cat::Letter {
                        let start = pos;
                        while pos < chars.len() && self.catcode(chars[pos]) == Cat::Letter {
                            pos += 1;
                        }
                        let name: String = chars[start..pos].iter().collect();
                        skip = true;
                        break Some(Token::Cs(Rc::from(name.as_str())));
                    }
                    pos += 1;
                    // after a one-character name, spaces are skipped only for `\ `
                    skip = self.catcode(first) == Cat::Space;
                    break Some(Token::Cs(Rc::from(first.to_string().as_str())));
                }
                Cat::Space => {
                    pos += 1;
                    if skip {
                        continue;
                    }
                    skip = true;
                    break Some(Token::Char(' ', Cat::Space));
                }
                Cat::EndLine => {
                    pos += 1;
                    if c == '\r' && next == Some('\n') {
                        pos += 1;
                    }
                    // an empty line is a paragraph break; another end of line is a space
                    let mut k = pos;
                    while k < chars.len() && matches!(chars[k], ' ' | '\t') {
                        k += 1;
                    }
                    let was_skipping = skip;
                    skip = true;
                    if matches!(chars.get(k), Some('\n') | Some('\r')) {
                        pos = k + 1;
                        break Some(Token::cs("par"));
                    }
                    if was_skipping {
                        continue;
                    }
                    break Some(Token::Char(' ', Cat::Space));
                }
                Cat::Comment => {
                    while pos < chars.len() && chars[pos] != '\n' {
                        pos += 1;
                    }
                    // the end of line goes too, and the next line's leading spaces
                    pos = (pos + 1).min(chars.len());
                    while pos < chars.len() && matches!(chars[pos], ' ' | '\t') {
                        pos += 1;
                    }
                    skip = false;
                    continue;
                }
                Cat::Ignored | Cat::Invalid => {
                    pos += 1;
                    continue;
                }
                Cat::Sup if next == Some(c) && pos + 2 < chars.len() => {
                    let x = chars[pos + 2];
                    let hex = |c: char| matches!(c, '0'..='9' | 'a'..='f');
                    let ch = match chars.get(pos + 3) {
                        // ^^41: the character of that code, in two lowercase hexadecimal digits
                        Some(&y) if hex(x) && hex(y) => {
                            pos += 4;
                            char::from_u32(u32::from_str_radix(&format!("{x}{y}"), 16).unwrap_or(0)).unwrap_or(x)
                        }
                        // ^^M, ^^?: the character 64 away
                        _ => {
                            pos += 3;
                            let code = x as u32;
                            char::from_u32(if code < 64 { code + 64 } else { code - 64 }).unwrap_or(x)
                        }
                    };
                    skip = false;
                    let cat = self.catcode(ch);
                    break Some(Token::Char(ch, cat));
                }
                _ => {
                    pos += 1;
                    skip = false;
                    break Some(Token::Char(c, cat));
                }
            }
        };
        if let Some(Frame::Text { pos: p, skip_blanks, .. }) = self.input.last_mut() {
            *p = pos;
            *skip_blanks = skip;
        }
        if self.input.len() == 1 {
            if let (Some(r), Some(t)) = (&mut self.reading, &result) {
                r.read(t.clone(), start, pos);
            }
        }
        result
    }

    // ---------------------------------------------------------------- meanings and groups

    pub fn meaning(&self, t: &Token) -> Meaning {
        match t {
            Token::Cs(n) => self.meanings.get(n).cloned().unwrap_or(Meaning::Undefined),
            Token::Char(c, Cat::Active) => self.active.get(c).cloned().unwrap_or(Meaning::Undefined),
            Token::Char(c, cat) => Meaning::Char(*c, *cat),
        }
    }

    pub fn meaning_of(&self, name: &str) -> Meaning {
        self.meanings.get(name).cloned().unwrap_or(Meaning::Undefined)
    }

    pub fn is_defined(&self, name: &str) -> bool {
        !matches!(self.meanings.get(name), None | Some(Meaning::Undefined))
    }

    /// Assigns a meaning, locally unless `\global` was given (or `global` is true).
    pub fn set_meaning(&mut self, t: &Token, m: Meaning, global: bool) {
        let global = global || std::mem::take(&mut self.global);
        match t {
            Token::Cs(n) => {
                if global {
                    self.forget_saved(|s| matches!(s, Saved::Meaning(k, _) if k == n));
                } else if let Some(level) = self.save.last_mut() {
                    level.push(Saved::Meaning(n.clone(), self.meanings.get(n).cloned()));
                }
                self.meanings.insert(n.clone(), m);
            }
            Token::Char(c, _) => {
                if global {
                    self.forget_saved(|s| matches!(s, Saved::Active(k, _) if k == c));
                } else if let Some(level) = self.save.last_mut() {
                    level.push(Saved::Active(*c, self.active.get(c).cloned()));
                }
                self.active.insert(*c, m);
            }
        }
        self.run_after_assignment();
    }

    pub fn define(&mut self, name: &str, m: Meaning) {
        self.set_meaning(&Token::cs(name), m, false);
    }

    /// A macro without parameters, from tokens.
    pub fn define_macro(&mut self, name: &str, body: Vec<Token>, global: bool) {
        let m = Macro { params: Vec::new(), body: body.into_iter().map(Body::Tok).collect(), optional: None, protected: false };
        self.set_meaning(&Token::cs(name), Meaning::Macro(Rc::new(m)), global);
    }

    /// A macro without parameters whose body is this text (read as letters and others).
    pub fn define_string(&mut self, name: &str, value: &str, global: bool) {
        self.define_macro(name, string_tokens(value), global);
    }

    fn forget_saved(&mut self, matches: impl Fn(&Saved) -> bool) {
        for level in &mut self.save {
            level.retain(|s| !matches(s));
        }
    }

    pub fn begin_group(&mut self) {
        self.save.push(Vec::new());
        self.after_group.push(Vec::new());
    }

    /// Ends a group: restores what it changed locally and runs its `\aftergroup` tokens.
    pub fn end_group(&mut self) -> bool {
        let Some(level) = self.save.pop() else { return false };
        for s in level.into_iter().rev() {
            match s {
                Saved::Meaning(n, old) => restore(&mut self.meanings, n, old),
                Saved::Active(c, old) => restore(&mut self.active, c, old),
                Saved::Count(n, old) => restore(&mut self.counts, n, old),
                Saved::Dimen(n, old) => restore(&mut self.dimens, n, old),
                Saved::Toks(n, old) => restore(&mut self.toks, n, old),
                Saved::Catcode(c, old) => restore(&mut self.catcodes, c, old),
            }
        }
        if let Some(after) = self.after_group.pop() {
            self.push_tokens(after);
        }
        true
    }

    pub fn group_depth(&self) -> usize {
        self.save.len()
    }

    pub fn set_count(&mut self, n: u32, v: i64, global: bool) {
        let global = global || std::mem::take(&mut self.global);
        if global {
            self.forget_saved(|s| matches!(s, Saved::Count(k, _) if *k == n));
        } else if let Some(level) = self.save.last_mut() {
            level.push(Saved::Count(n, self.counts.get(&n).copied()));
        }
        self.counts.insert(n, v.clamp(-2_147_483_647, 2_147_483_647));
        self.run_after_assignment();
    }

    pub fn set_dimen(&mut self, n: u32, v: i64, global: bool) {
        let global = global || std::mem::take(&mut self.global);
        if global {
            self.forget_saved(|s| matches!(s, Saved::Dimen(k, _) if *k == n));
        } else if let Some(level) = self.save.last_mut() {
            level.push(Saved::Dimen(n, self.dimens.get(&n).copied()));
        }
        self.dimens.insert(n, v.clamp(-dimen::MAX_DIMEN, dimen::MAX_DIMEN));
        self.run_after_assignment();
    }

    pub fn set_toks(&mut self, n: u32, v: Vec<Token>, global: bool) {
        let global = global || std::mem::take(&mut self.global);
        if global {
            self.forget_saved(|s| matches!(s, Saved::Toks(k, _) if *k == n));
        } else if let Some(level) = self.save.last_mut() {
            level.push(Saved::Toks(n, self.toks.get(&n).cloned()));
        }
        self.toks.insert(n, Rc::new(v));
        self.run_after_assignment();
    }

    fn run_after_assignment(&mut self) {
        if let Some(t) = self.after_assignment.take() {
            self.back(t);
        }
    }

    pub fn new_register(&mut self) -> u32 {
        self.next_register += 1;
        self.next_register
    }

    pub fn warn(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.warnings.contains(&s) && self.warnings.len() < 1000 {
            self.warnings.push(s);
        }
    }

    // ---------------------------------------------------------------- expansion

    fn count_expansion(&mut self) -> Result<(), Error> {
        self.expansions += 1;
        self.statement_expansions += 1;
        if self.expansions > 10 * MAX_EXPANSIONS {
            return Err(Error::Figure(format!("figure too large: more than {} macro expansions", 10 * MAX_EXPANSIONS)));
        }
        if self.statement_expansions > MAX_EXPANSIONS {
            return Err(Error::Figure(format!("macro expansion did not end (more than {MAX_EXPANSIONS} steps)")));
        }
        Ok(())
    }

    /// A new statement starts: its expansions are counted afresh (the figure's total still is).
    pub fn new_statement(&mut self) {
        self.statement_expansions = 0;
    }

    /// The next token after expanding macros and expandable primitives.
    pub fn get_x_token(&mut self) -> Result<Option<Token>, Error> {
        loop {
            let Some(t) = self.get_token() else { return Ok(None) };
            let m = self.meaning(&t);
            if m.is_expandable() {
                self.expand(&t, &m)?;
                continue;
            }
            return Ok(Some(t));
        }
    }

    /// Expands `t` one level, pushing the result back on the input.
    pub fn expand(&mut self, t: &Token, m: &Meaning) -> Result<(), Error> {
        self.count_expansion()?;
        match m {
            Meaning::Macro(mac) => {
                let args = self.read_arguments(t, mac)?;
                let body = substitute(&mac.body, &args);
                self.push_tokens(body);
                Ok(())
            }
            Meaning::Prim(p) => self.expand_prim(*p, t),
            _ => Ok(()),
        }
    }

    /// Reads a macro's arguments from the input, matching its parameter text.
    fn read_arguments(&mut self, name: &Token, m: &Macro) -> Result<Vec<Vec<Token>>, Error> {
        let mut args: Vec<Vec<Token>> = Vec::new();
        if let Some(default) = &m.optional {
            // LaTeX: an optional first argument in brackets
            let t = self.next_nonspace();
            match t {
                Some(t) if t.is_char('[') => args.push(self.read_until_char(']', true)?),
                Some(t) => {
                    self.back(t);
                    args.push(default.clone());
                }
                None => args.push(default.clone()),
            }
        }
        let mut i = 0;
        let p = &m.params;
        while i < p.len() {
            match &p[i] {
                Param::Tok(t) => {
                    let got = self.get_token().ok_or_else(|| Error::Figure(format!("use of {} does not match its definition", show_token(name))))?;
                    if &got != t {
                        return Err(Error::Figure(format!("use of {} does not match its definition", show_token(name))));
                    }
                    i += 1;
                }
                Param::Arg(_) => {
                    // delimited by the tokens up to the next parameter (or the end)
                    let mut delim = Vec::new();
                    let mut k = i + 1;
                    while k < p.len() {
                        match &p[k] {
                            Param::Tok(t) => delim.push(t.clone()),
                            Param::Arg(_) => break,
                        }
                        k += 1;
                    }
                    if delim.is_empty() {
                        args.push(self.read_undelimited()?);
                    } else {
                        args.push(self.read_delimited(&delim, name)?);
                    }
                    i = k;
                }
            }
        }
        Ok(args)
    }

    /// An undelimited argument: one token, or a braced group without its braces (spaces before are skipped).
    pub fn read_undelimited(&mut self) -> Result<Vec<Token>, Error> {
        let t = self.next_nonspace_par().ok_or_else(|| Error::Figure("a command is missing its argument".into()))?;
        if matches!(t, Token::Char(_, Cat::Begin)) {
            return self.read_group_body();
        }
        Ok(vec![t])
    }

    fn next_nonspace_par(&mut self) -> Option<Token> {
        loop {
            let t = self.get_token()?;
            if !t.is_space() && !t.is_cs("par") {
                return Some(t);
            }
        }
    }

    pub fn next_nonspace(&mut self) -> Option<Token> {
        loop {
            let t = self.get_token()?;
            if !t.is_space() {
                return Some(t);
            }
        }
    }

    /// The rest of a group whose `{` was just read, up to its matching `}` (not included).
    pub fn read_group_body(&mut self) -> Result<Vec<Token>, Error> {
        let mut out = Vec::new();
        let mut depth = 1usize;
        loop {
            let t = self.get_token().ok_or_else(|| Error::Figure("unmatched brace: a group '{' never closes".into()))?;
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                _ => {}
            }
            out.push(t);
            if out.len() > MAX_TOKENS {
                return Err(Error::Figure("argument too long".into()));
            }
        }
    }

    /// Tokens up to `delim` at group level 0; one pair of braces around the whole argument is removed, as TeX does.
    fn read_delimited(&mut self, delim: &[Token], name: &Token) -> Result<Vec<Token>, Error> {
        let mut out: Vec<Token> = Vec::new();
        let mut depth = 0usize;
        loop {
            let t = self.get_token().ok_or_else(|| Error::Figure(format!("{} ends before its delimiter", show_token(name))))?;
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                _ => {}
            }
            out.push(t);
            if depth == 0 && out.ends_with(delim) {
                out.truncate(out.len() - delim.len());
                return Ok(strip_outer_braces(out));
            }
            if out.len() > MAX_TOKENS {
                return Err(Error::Figure(format!("{} ends before its delimiter", show_token(name))));
            }
        }
    }

    /// Tokens up to the character `c` at brace level 0 (for `[…]`), the `c` consumed.
    pub fn read_until_char(&mut self, c: char, strip: bool) -> Result<Vec<Token>, Error> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        loop {
            let t = self.get_token().ok_or_else(|| Error::Figure(if c == ']' { "unmatched bracket: ']' missing".into() } else { format!("'{c}' missing") }))?;
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
                ref x if depth == 0 && x.is_char(c) => return Ok(if strip { strip_outer_braces(out) } else { out }),
                _ => {}
            }
            out.push(t);
            if out.len() > MAX_TOKENS {
                return Err(Error::Figure(if c == ']' { "unmatched bracket: ']' missing".into() } else { format!("'{c}' missing") }));
            }
        }
    }

    fn expand_prim(&mut self, p: Prim, t: &Token) -> Result<(), Error> {
        use Prim::*;
        match p {
            Expandafter => {
                let a = self.get_token().ok_or_else(|| Error::Figure("\\expandafter at the end".into()))?;
                if let Some(b) = self.get_token() {
                    let m = self.meaning(&b);
                    if m.is_expandable() {
                        self.expand(&b, &m)?;
                    } else {
                        self.back(b);
                    }
                }
                self.back(a);
            }
            Noexpand => {
                // the next token, marked so it is not expanded this time: \relax-like when it was expandable
                if let Some(n) = self.get_token() {
                    if self.meaning(&n).is_expandable() {
                        self.push_tokens([Token::cs("tikra@noexpand"), n]);
                    } else {
                        self.back(n);
                    }
                }
            }
            Csname => {
                let mut name = String::new();
                loop {
                    let x = self.get_x_token()?.ok_or_else(|| Error::Figure("\\csname without \\endcsname".into()))?;
                    match x {
                        Token::Cs(ref n) if &**n == "endcsname" => break,
                        Token::Cs(ref n) => name.push_str(&format!("\\{n}")),
                        Token::Char(c, _) => name.push(c),
                    }
                    if name.len() > 10_000 {
                        return Err(Error::Figure("\\csname too long".into()));
                    }
                }
                let tok = Token::Cs(Rc::from(name.as_str()));
                if matches!(self.meaning(&tok), Meaning::Undefined) {
                    // \csname makes an undefined name mean \relax
                    self.meanings.insert(Rc::from(name.as_str()), Meaning::Prim(Relax));
                }
                self.back(tok);
            }
            Stringify => {
                let x = self.get_token().unwrap_or(Token::other(' '));
                self.push_tokens(string_tokens(&show_token(&x)));
            }
            Number => {
                let n = self.scan_int()?;
                self.push_tokens(string_tokens(&n.to_string()));
            }
            Romannumeral => {
                let n = self.scan_int()?;
                self.push_tokens(string_tokens(&roman(n)));
            }
            The => {
                let v = self.the()?;
                self.push_tokens(v);
            }
            ShowMeaning => {
                let x = self.get_token().unwrap_or(Token::other(' '));
                let m = self.meaning(&x);
                self.push_tokens(string_tokens(&describe(&m)));
            }
            Unexpanded => {
                let body = self.read_general_text()?;
                // marked as not to expand in \edef
                let mut out = Vec::with_capacity(body.len() * 2);
                for b in body {
                    out.push(Token::cs("tikra@noexpand"));
                    out.push(b);
                }
                self.push_tokens(out);
            }
            Detokenize => {
                let body = self.read_general_text()?;
                self.push_tokens(string_tokens(&tokens_to_string(&body)));
            }
            Jobname => self.push_tokens(string_tokens("figure")),
            Pdfstrcmp => {
                let a = self.read_expanded_general_text()?;
                let b = self.read_expanded_general_text()?;
                let (a, b) = (tokens_to_string(&a), tokens_to_string(&b));
                let r = match a.cmp(&b) {
                    std::cmp::Ordering::Less => "-1",
                    std::cmp::Ordering::Equal => "0",
                    std::cmp::Ordering::Greater => "1",
                };
                self.push_tokens(string_tokens(r));
            }
            Else | Or => {
                // reached the end of a taken branch: skip to the matching \fi
                if !self.close_cond() {
                    self.warn(format!("extra {}", show_token(t)));
                    return Ok(());
                }
                self.skip_to_fi()?;
            }
            Fi => {
                if !self.close_cond() {
                    self.warn("extra \\fi");
                }
            }
            Unless => {
                let n = self.get_token().ok_or_else(|| Error::Figure("\\unless at the end".into()))?;
                let Meaning::Prim(q) = self.meaning(&n) else { return Err(Error::Figure("\\unless must be followed by a conditional".into())) };
                if !q.is_conditional() || q == Ifcase {
                    return Err(Error::Figure("\\unless must be followed by a conditional".into()));
                }
                let open = self.conds;
                let b = self.evaluate_condition(q)?;
                self.conditional(!b, self.conds.saturating_sub(open))?;
            }
            Ifcase => {
                let n = self.scan_int()?;
                self.conds += 1;
                // skip to the n-th \or (or \else)
                let mut k = 0;
                while k < n {
                    match self.skip_branch()? {
                        BranchEnd::Or => k += 1,
                        BranchEnd::Else => return Ok(()),
                        BranchEnd::Fi => {
                            self.close_cond();
                            return Ok(());
                        }
                    }
                }
            }
            q if q.is_conditional() => {
                let open = self.conds;
                let b = self.evaluate_condition(q)?;
                self.conditional(b, self.conds.saturating_sub(open))?;
            }
            Ifnextchar => {
                // \@ifnextchar c{yes}{no}: looks past spaces
                let c = self.get_token().ok_or_else(|| Error::Figure("\\@ifnextchar without a character".into()))?;
                let yes = self.read_undelimited()?;
                let no = self.read_undelimited()?;
                let next = self.next_nonspace();
                let take = next.as_ref() == Some(&c) || matches!((&next, &c), (Some(Token::Char(a, _)), Token::Char(b, _)) if a == b);
                if let Some(n) = next {
                    self.back(n);
                }
                self.push_tokens(if take { yes } else { no });
            }
            Firstoftwo => {
                let a = self.read_undelimited()?;
                let _ = self.read_undelimited()?;
                self.push_tokens(a);
            }
            Secondoftwo => {
                let _ = self.read_undelimited()?;
                let b = self.read_undelimited()?;
                self.push_tokens(b);
            }
            Gobble => {
                let _ = self.read_undelimited()?;
            }
            Gobbletwo => {
                let _ = self.read_undelimited()?;
                let _ = self.read_undelimited()?;
            }
            Firstofone => {
                let a = self.read_undelimited()?;
                self.push_tokens(a);
            }
            Value => {
                let name = tokens_to_string(&self.read_undelimited()?);
                let v = self.counter(&name);
                self.push_tokens(string_tokens(&v.to_string()));
            }
            Arabic => {
                let name = tokens_to_string(&self.read_undelimited()?);
                let v = self.counter(&name);
                self.push_tokens(string_tokens(&v.to_string()));
            }
            Ifthenelse => {
                let test = self.read_undelimited()?;
                let yes = self.read_undelimited()?;
                let no = self.read_undelimited()?;
                let b = self.ifthen_test(test)?;
                self.push_tokens(if b { yes } else { no });
            }
            Whiledo => {
                let test = self.read_undelimited()?;
                let body = self.read_undelimited()?;
                if self.ifthen_test(test.clone())? {
                    // the body, then the loop again
                    let mut again = body.clone();
                    again.push(Token::cs("whiledo"));
                    again.push(Token::Char('{', Cat::Begin));
                    again.extend(test);
                    again.push(Token::Char('}', Cat::End));
                    again.push(Token::Char('{', Cat::Begin));
                    again.extend(body);
                    again.push(Token::Char('}', Cat::End));
                    self.push_tokens(again);
                }
            }
            Ifstrequal => {
                // etoolbox: the two texts compared as written, nothing expanded
                let a = tokens_to_string(&self.read_undelimited()?);
                let b = tokens_to_string(&self.read_undelimited()?);
                let yes = self.read_undelimited()?;
                let no = self.read_undelimited()?;
                self.push_tokens(if a == b { yes } else { no });
            }
            Ifstrempty(name) => {
                // etoolbox, nothing expanded: \ifstrempty{text} (no tokens at all), \ifblank{text}
                // (spaces only), \ifdefempty{\m} (a macro without parameters and with an empty body)
                let arg = self.read_undelimited()?;
                let test = match name {
                    "ifstrempty" => arg.is_empty(),
                    "ifblank" => arg.iter().all(Token::is_space),
                    _ => arg.first().is_some_and(|m| matches!(self.meaning(m), Meaning::Macro(m) if m.params.is_empty() && m.body.is_empty())),
                };
                let yes = self.read_undelimited()?;
                let no = self.read_undelimited()?;
                self.push_tokens(if test { yes } else { no });
            }
            Ifboolean => {
                let name = tokens_to_string(&self.read_undelimited()?);
                self.push_tokens([Token::Cs(Rc::from(format!("if{}", name.trim()).as_str()))]);
            }
            Documentarguments => self.document_arguments()?,
            Documentif(name) => {
                let arg: Vec<Token> = self.read_undelimited()?.into_iter().filter(|x| !x.is_space()).collect();
                let test = if name.starts_with("IfBoolean") {
                    arg.len() == 1 && arg[0].is_cs("BooleanTrue")
                } else {
                    let missing = arg.len() == 1 && arg[0].is_cs("tikra@novalue");
                    missing == name.starts_with("IfNoValue")
                };
                let (yes, no) = if name.ends_with("TF") {
                    (self.read_undelimited()?, self.read_undelimited()?)
                } else if name.ends_with('T') {
                    (self.read_undelimited()?, Vec::new())
                } else {
                    (Vec::new(), self.read_undelimited()?)
                };
                self.push_tokens(if test { yes } else { no });
            }
            _ => {}
        }
        Ok(())
    }

    fn close_cond(&mut self) -> bool {
        if self.conds == 0 {
            return false;
        }
        self.conds -= 1;
        true
    }

    fn counter(&self, name: &str) -> i64 {
        match self.meaning_of(&format!("c@{}", name.trim())) {
            Meaning::Count(r) => self.counts.get(&r).copied().unwrap_or(0),
            _ => 0,
        }
    }

    /// `ifthen` tests: `\equal{a}{b}`, `\isodd{n}`, `a < b`, `\boolean{x}`, `\not`, `\and`, `\or`.
    fn ifthen_test(&mut self, test: Vec<Token>) -> Result<bool, Error> {
        let mut t: Vec<Token> = Vec::new();
        // expand fully, keeping \equal & co. as markers
        let depth = self.input.len();
        self.push_frame(test);
        while self.input.len() > depth {
            let Some(x) = self.get_token_until(depth) else { break };
            if x.is_cs("equal")
                || x.is_cs("isodd")
                || x.is_cs("not")
                || x.is_cs("NOT")
                || x.is_cs("and")
                || x.is_cs("AND")
                || x.is_cs("or")
                || x.is_cs("OR")
                || x.is_cs("lengthtest")
                || x.is_cs("boolean")
            {
                t.push(x);
                continue;
            }
            let m = self.meaning(&x);
            if m.is_expandable() {
                self.expand(&x, &m)?;
            } else {
                t.push(x);
            }
        }
        ifthen_eval(self, &t)
    }

    fn skip_to_fi(&mut self) -> Result<(), Error> {
        let mut depth = 0usize;
        loop {
            let t = self.get_token().ok_or_else(|| Error::Figure("conditional without \\fi".into()))?;
            match self.meaning(&t) {
                Meaning::Prim(p) if p.is_conditional() => depth += 1,
                Meaning::Prim(Prim::Fi) => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    }

    /// Skips a false branch: up to the matching `\else`, `\or` or `\fi`.
    fn skip_branch(&mut self) -> Result<BranchEnd, Error> {
        let mut depth = 0usize;
        loop {
            let t = self.get_token().ok_or_else(|| Error::Figure("conditional without \\fi".into()))?;
            match self.meaning(&t) {
                Meaning::Prim(p) if p.is_conditional() => depth += 1,
                Meaning::Prim(Prim::Fi) => {
                    if depth == 0 {
                        return Ok(BranchEnd::Fi);
                    }
                    depth -= 1;
                }
                Meaning::Prim(Prim::Else) if depth == 0 => return Ok(BranchEnd::Else),
                Meaning::Prim(Prim::Or) if depth == 0 => return Ok(BranchEnd::Or),
                _ => {}
            }
        }
    }

    /// Takes the true branch (reading on), or skips to `\else`/`\fi`. `inner` conditionals were
    /// opened while the test was read (`\ifdim#1pt>0pt\ifdim…`, the second expanded in looking for
    /// a space after the first's `pt`) and are still open: as TeX does, a false branch is skipped
    /// past their `\else`, and each `\fi` ends one of them before one ends this conditional.
    fn conditional(&mut self, b: bool, mut inner: usize) -> Result<(), Error> {
        self.conds += 1;
        if b {
            return Ok(());
        }
        loop {
            match self.skip_branch()? {
                BranchEnd::Else | BranchEnd::Or if inner > 0 => {}
                BranchEnd::Fi if inner > 0 => {
                    inner -= 1;
                    self.close_cond();
                }
                BranchEnd::Else => return Ok(()),
                BranchEnd::Fi => {
                    self.close_cond();
                    return Ok(());
                }
                BranchEnd::Or => {
                    self.close_cond();
                    return Err(Error::Figure("\\or outside \\ifcase".into()));
                }
            }
        }
    }

    fn evaluate_condition(&mut self, p: Prim) -> Result<bool, Error> {
        use Prim::*;
        Ok(match p {
            Iftrue => true,
            Ifhmode => true,
            Ifnum => {
                let a = self.scan_int()?;
                let rel = self.scan_relation()?;
                let b = self.scan_int()?;
                compare(a, rel, b)
            }
            Ifdim => {
                let a = self.scan_dimen()?;
                let rel = self.scan_relation()?;
                let b = self.scan_dimen()?;
                compare(a, rel, b)
            }
            Ifodd => self.scan_int()? % 2 != 0,
            Ifx => {
                let a = self.get_token().ok_or_else(|| Error::Figure("\\ifx at the end".into()))?;
                let b = self.get_token().ok_or_else(|| Error::Figure("\\ifx at the end".into()))?;
                let (ma, mb) = (self.meaning(&a), self.meaning(&b));
                match (&ma, &mb) {
                    (Meaning::Macro(x), Meaning::Macro(y)) => x.params == y.params && x.body == y.body && x.optional == y.optional,
                    _ => ma == mb,
                }
            }
            If | Ifcat => {
                let a = self.x_token_for_if()?;
                let b = self.x_token_for_if()?;
                let code = |t: &Token| -> (Option<char>, Option<Cat>) {
                    match t {
                        Token::Char(c, cat) => (Some(*c), Some(*cat)),
                        _ => (None, None),
                    }
                };
                let (ca, ka) = code(&a);
                let (cb, kb) = code(&b);
                if p == If {
                    ca == cb
                } else {
                    ka == kb
                }
            }
            Ifdefined => {
                let a = self.get_token().ok_or_else(|| Error::Figure("\\ifdefined at the end".into()))?;
                !matches!(self.meaning(&a), Meaning::Undefined)
            }
            Ifcsname => {
                let mut name = String::new();
                loop {
                    let x = self.get_x_token()?.ok_or_else(|| Error::Figure("\\ifcsname without \\endcsname".into()))?;
                    match x {
                        Token::Cs(ref n) if &**n == "endcsname" => break,
                        Token::Cs(ref n) => name.push_str(&format!("\\{n}")),
                        Token::Char(c, _) => name.push(c),
                    }
                }
                self.is_defined(&name)
            }
            // \iffalse, and the modes a figure's text is never in: \ifvmode, \ifmmode, \ifinner
            // (\ifcase is read where it is met)
            _ => false,
        })
    }

    /// For `\if`: an expanded token; a control sequence that is not a character counts as character 256.
    fn x_token_for_if(&mut self) -> Result<Token, Error> {
        let t = self.get_x_token()?.ok_or_else(|| Error::Figure("\\if at the end".into()))?;
        if t.is_cs("tikra@noexpand") {
            return Ok(self.get_token().unwrap_or(t));
        }
        Ok(match self.meaning(&t) {
            Meaning::Char(c, cat) => Token::Char(c, cat),
            _ => t,
        })
    }

    fn scan_relation(&mut self) -> Result<char, Error> {
        let t = self.next_x_nonspace()?.ok_or_else(|| Error::Figure("missing = < or > in a comparison".into()))?;
        match t {
            Token::Char(c @ ('<' | '=' | '>'), _) => Ok(c),
            _ => Err(Error::Figure("missing = < or > in a comparison".into())),
        }
    }

    pub fn next_x_nonspace(&mut self) -> Result<Option<Token>, Error> {
        loop {
            let t = self.get_x_token()?;
            match t {
                Some(t) if t.is_space() => continue,
                other => return Ok(other),
            }
        }
    }

    /// `\the`: the value of a register or internal quantity, as tokens.
    fn the(&mut self) -> Result<Vec<Token>, Error> {
        // expanded up to the first unexpandable token, or LaTeX's \value{c}, which is a counter's
        // register there (expanding it would give its digits)
        let (t, m) = loop {
            let t = self.get_token().ok_or_else(|| Error::Figure("\\the at the end".into()))?;
            let m = self.meaning(&t);
            if m.is_expandable() && m != Meaning::Prim(Prim::Value) {
                self.expand(&t, &m)?;
                continue;
            }
            break (t, m);
        };
        Ok(match m {
            Meaning::Count(r) => string_tokens(&self.counts.get(&r).copied().unwrap_or(0).to_string()),
            Meaning::Dimen(r) | Meaning::Skip(r) => string_tokens(&dimen::show(self.dimens.get(&r).copied().unwrap_or(0))),
            Meaning::Toks(r) => self.toks.get(&r).map(|v| (**v).clone()).unwrap_or_default(),
            Meaning::CharDef(v) => string_tokens(&v.to_string()),
            Meaning::Prim(Prim::Count) => {
                let r = self.scan_register_number()?;
                string_tokens(&self.counts.get(&r).copied().unwrap_or(0).to_string())
            }
            Meaning::Prim(Prim::Dimen | Prim::Skip) => {
                let r = self.scan_register_number()?;
                string_tokens(&dimen::show(self.dimens.get(&r).copied().unwrap_or(0)))
            }
            Meaning::Prim(Prim::Toks) => {
                let r = self.scan_register_number()?;
                self.toks.get(&r).map(|v| (**v).clone()).unwrap_or_default()
            }
            Meaning::Prim(Prim::Value) => {
                let name = tokens_to_string(&self.read_undelimited()?);
                string_tokens(&self.counter(&name).to_string())
            }
            Meaning::Prim(Prim::Numexpr) => string_tokens(&self.scan_numexpr()?.to_string()),
            Meaning::Prim(Prim::Dimexpr | Prim::Glueexpr) => string_tokens(&dimen::show(self.scan_dimexpr()?)),
            Meaning::Prim(Prim::Catcode) => {
                let c = self.scan_int()?;
                let ch = char::from_u32(c.max(0) as u32).unwrap_or(' ');
                string_tokens(&self.catcode(ch).code().to_string())
            }
            Meaning::Prim(p @ (Prim::Wd | Prim::Ht | Prim::Dp)) => {
                let n = self.scan_int()?;
                string_tokens(&dimen::show(self.box_dimen(p, n)))
            }
            _ => {
                self.warn(format!("\\the{} is not supported", show_token(&t)));
                Vec::new()
            }
        })
    }

    /// `{…}` after optional spaces and `\relax` (for `\unexpanded`, `\detokenize`), unexpanded.
    pub fn read_general_text(&mut self) -> Result<Vec<Token>, Error> {
        loop {
            let t = self.get_x_token()?.ok_or_else(|| Error::Figure("missing '{'".into()))?;
            if t.is_space() || t.is_cs("relax") {
                continue;
            }
            if matches!(t, Token::Char(_, Cat::Begin)) {
                return self.read_group_body();
            }
            self.back(t);
            return Err(Error::Figure("missing '{'".into()));
        }
    }

    fn read_expanded_general_text(&mut self) -> Result<Vec<Token>, Error> {
        let body = self.read_general_text()?;
        self.expand_fully(body)
    }

    /// Expands tokens as `\edef` does: everything expandable, except what `\noexpand`/`\unexpanded` protect,
    /// protected macros, and `\the` of a token register (inserted as is).
    pub fn expand_fully(&mut self, tokens: Vec<Token>) -> Result<Vec<Token>, Error> {
        let depth = self.input.len();
        // a frame of its own, so we never read past our tokens
        self.input.push(Frame::Tokens(tokens.into_iter().collect()));
        let r = self.expand_frame(depth);
        if r.is_err() {
            // what was left of the expansion goes, so the error does not come back
            self.drain_to(depth);
        }
        r
    }

    fn expand_frame(&mut self, depth: usize) -> Result<Vec<Token>, Error> {
        self.expand_frame_until(depth, &[]).map(|(out, _)| out)
    }

    /// Starts expanding `tokens` piece by piece, with `expand_more`: the depth of the input that
    /// they end at.
    pub fn expand_start(&mut self, tokens: Vec<Token>) -> usize {
        let depth = self.input.len();
        self.input.push(Frame::Tokens(tokens.into_iter().collect()));
        depth
    }

    /// Expands what `expand_start` began, to its end or to the first unexpandable control sequence
    /// named in `stops` or register (whose assignment it begins): what it expanded to, and that
    /// control sequence, after which the input is what follows it (for the caller to read its
    /// arguments and run it, as TeX would while typesetting, and then to call again).
    pub fn expand_more(&mut self, depth: usize, stops: &[&str]) -> Result<(Vec<Token>, Option<Token>), Error> {
        let r = self.expand_frame_until(depth, stops);
        if r.is_err() {
            self.drain_to(depth);
        }
        r
    }

    fn expand_frame_until(&mut self, depth: usize, stops: &[&str]) -> Result<(Vec<Token>, Option<Token>), Error> {
        let floor = self.seal(depth);
        let r = self.expand_sealed(depth, stops);
        self.unseal(floor);
        r
    }

    fn expand_sealed(&mut self, depth: usize, stops: &[&str]) -> Result<(Vec<Token>, Option<Token>), Error> {
        let mut out = Vec::new();
        while self.input.len() > depth {
            let Some(t) = self.get_token_until(depth) else { break };
            if let Token::Cs(name) = &t {
                if stops.contains(&name.as_ref()) && !self.meaning(&t).is_expandable() {
                    return Ok((out, Some(t)));
                }
                if !stops.is_empty() && matches!(self.meaning(&t), Meaning::Count(_) | Meaning::Dimen(_) | Meaning::Skip(_) | Meaning::Toks(_)) {
                    return Ok((out, Some(t)));
                }
            }
            if t.is_cs("tikra@noexpand") {
                if let Some(n) = self.get_token_until(depth) {
                    out.push(n);
                }
                continue;
            }
            let m = self.meaning(&t);
            match &m {
                Meaning::Macro(mac) if mac.protected => out.push(t),
                Meaning::Prim(Prim::The) => {
                    self.count_expansion()?;
                    let v = self.the()?;
                    out.extend(v);
                }
                m if m.is_expandable() => self.expand(&t, m)?,
                _ => out.push(t),
            }
            if out.len() > MAX_TOKENS {
                return Err(Error::Figure("expansion too long".into()));
            }
        }
        Ok((out, None))
    }

    /// Expands tokens fully and returns them as text (control sequences written `\name `).
    pub fn expand_to_string(&mut self, tokens: Vec<Token>) -> Result<String, Error> {
        let t = self.expand_fully(tokens)?;
        Ok(tokens_to_string(&t))
    }

    // ---------------------------------------------------------------- scanning numbers and lengths

    fn scan_register_number(&mut self) -> Result<u32, Error> {
        let n = self.scan_int()?;
        if !(0..=65535).contains(&n) {
            return Err(Error::Figure(format!("bad register number {n}")));
        }
        Ok(n as u32)
    }

    /// An integer, as TeX reads one: signs, digits, `"hex`, `'octal`, `` `c ``, or an internal integer.
    pub fn scan_int(&mut self) -> Result<i64, Error> {
        let mut sign = 1i64;
        loop {
            let t = self.next_x_nonspace()?.ok_or_else(|| Error::Figure("missing number".into()))?;
            if t.is_char('-') {
                sign = -sign;
                continue;
            }
            if t.is_char('+') {
                continue;
            }
            if let Some(v) = self.internal_int(&t)? {
                return Ok(sign * v);
            }
            if let Token::Char(c, _) = t {
                if c == '`' {
                    let x = self.get_token().ok_or_else(|| Error::Figure("missing character after `".into()))?;
                    let v = match &x {
                        Token::Char(c, _) => *c as i64,
                        Token::Cs(n) => n.chars().next().map_or(0, |c| c as i64),
                    };
                    self.skip_one_optional_space()?;
                    return Ok(sign * v);
                }
                let radix = match c {
                    '"' => 16,
                    '\'' => 8,
                    _ => 10,
                };
                let mut digits = String::new();
                if radix == 10 {
                    if !c.is_ascii_digit() {
                        self.back(t);
                        return Err(Error::Figure("missing number".into()));
                    }
                    digits.push(c);
                }
                while let Some(x) = self.get_token() {
                    // `\ifcase2\or …`: an `\or`, `\else` or `\fi` ends the number, as TeX does it
                    // when it meets one while the condition is still incomplete
                    let m = self.meaning(&x);
                    if matches!(m, Meaning::Prim(Prim::Or | Prim::Else | Prim::Fi)) {
                        self.back(x);
                        break;
                    }
                    if m.is_expandable() {
                        self.expand(&x, &m)?;
                        continue;
                    }
                    match x {
                        Token::Char(d, Cat::Other | Cat::Letter) if d.is_digit(radix) && (radix == 16 && d.is_ascii_uppercase() || d.is_ascii_digit()) => digits.push(d),
                        Token::Char(_, Cat::Space) => break,
                        other => {
                            self.back(other);
                            break;
                        }
                    }
                }
                let v = i64::from_str_radix(&digits, radix).unwrap_or(2_147_483_647).min(2_147_483_647);
                return Ok(sign * v);
            }
            self.back(t);
            return Err(Error::Figure("missing number".into()));
        }
    }

    fn skip_one_optional_space(&mut self) -> Result<(), Error> {
        if let Some(t) = self.get_x_token()? {
            if !t.is_space() {
                self.back(t);
            }
        }
        Ok(())
    }

    /// The value of an internal integer quantity, if `t` is one.
    fn internal_int(&mut self, t: &Token) -> Result<Option<i64>, Error> {
        Ok(match self.meaning(t) {
            Meaning::Count(r) => Some(self.counts.get(&r).copied().unwrap_or(0)),
            Meaning::CharDef(v) => Some(v),
            Meaning::Dimen(r) | Meaning::Skip(r) => Some(self.dimens.get(&r).copied().unwrap_or(0)),
            Meaning::Prim(Prim::Count) => {
                let r = self.scan_register_number()?;
                Some(self.counts.get(&r).copied().unwrap_or(0))
            }
            Meaning::Prim(Prim::Dimen | Prim::Skip) => {
                let r = self.scan_register_number()?;
                Some(self.dimens.get(&r).copied().unwrap_or(0))
            }
            Meaning::Prim(Prim::Numexpr) => Some(self.scan_numexpr()?),
            Meaning::Prim(Prim::Dimexpr | Prim::Glueexpr) => Some(self.scan_dimexpr()?),
            Meaning::Prim(Prim::Catcode) => {
                let c = self.scan_int()?;
                Some(self.catcode(char::from_u32(c.max(0) as u32).unwrap_or(' ')).code())
            }
            Meaning::Prim(p @ (Prim::Wd | Prim::Ht | Prim::Dp)) => {
                let n = self.scan_int()?;
                Some(self.box_dimen(p, n))
            }
            _ => None,
        })
    }

    /// `\wd`, `\ht` or `\dp` of box register `n` (sp).
    fn box_dimen(&self, p: Prim, n: i64) -> i64 {
        let k = match p {
            Prim::Wd => 0,
            Prim::Ht => 1,
            _ => 2,
        };
        self.box_dims.get(&n).map_or(0, |d| d[k])
    }

    fn internal_dimen(&mut self, t: &Token) -> Result<Option<i64>, Error> {
        Ok(match self.meaning(t) {
            Meaning::Dimen(r) | Meaning::Skip(r) => Some(self.dimens.get(&r).copied().unwrap_or(0)),
            Meaning::Prim(Prim::Dimen | Prim::Skip) => {
                let r = self.scan_register_number()?;
                Some(self.dimens.get(&r).copied().unwrap_or(0))
            }
            Meaning::Prim(Prim::Dimexpr | Prim::Glueexpr) => Some(self.scan_dimexpr()?),
            Meaning::Prim(p @ (Prim::Wd | Prim::Ht | Prim::Dp)) => {
                let n = self.scan_int()?;
                Some(self.box_dimen(p, n))
            }
            _ => None,
        })
    }

    /// A dimension, as TeX reads one: a factor (possibly with decimals) and a unit, or an internal dimension.
    pub fn scan_dimen(&mut self) -> Result<i64, Error> {
        let mut sign = 1i64;
        loop {
            let t = self.next_x_nonspace()?.ok_or_else(|| Error::Figure("missing dimension".into()))?;
            if t.is_char('-') {
                sign = -sign;
                continue;
            }
            if t.is_char('+') {
                continue;
            }
            if let Some(v) = self.internal_dimen(&t)? {
                return Ok(sign * v);
            }
            // a factor: an internal integer, or digits with a decimal point or comma
            let (integer, digits): (i64, Vec<u8>) = if let Some(v) = self.internal_int(&t)? {
                (v, Vec::new())
            } else {
                let mut int = String::new();
                let mut frac = Vec::new();
                let mut point = false;
                let mut x = Some(t);
                while let Some(tok) = x.take() {
                    match tok {
                        Token::Char(d, _) if d.is_ascii_digit() => {
                            if point {
                                frac.push(d as u8 - b'0');
                            } else {
                                int.push(d);
                            }
                            x = self.get_x_token()?;
                        }
                        Token::Char('.' | ',', _) if !point => {
                            point = true;
                            x = self.get_x_token()?;
                        }
                        other => self.back(other),
                    }
                }
                if int.is_empty() && !point {
                    return Err(Error::Figure("missing dimension".into()));
                }
                (int.parse().unwrap_or(0).min(2_147_483_647), frac)
            };
            // the unit: an internal dimension multiplies, or a keyword
            let u = self.next_x_nonspace()?;
            if let Some(u) = u {
                if let Some(v) = self.internal_dimen(&u)? {
                    let (frac, _) = dimen::xn_over_d(v, dimen::round_decimals(&digits), 65536);
                    return Ok(sign * (dimen::mult(integer, v) + frac));
                }
                self.back(u);
            }
            self.scan_keyword("true")?;
            for (name, _) in dimen::UNITS {
                if self.scan_keyword(name)? {
                    self.skip_one_optional_space()?;
                    let v = dimen::exact(integer, &digits, name).unwrap_or(0);
                    return Ok(sign * v.clamp(-dimen::MAX_DIMEN, dimen::MAX_DIMEN));
                }
            }
            return Err(Error::Figure("illegal unit of measure (pt inserted)".into()));
        }
    }

    /// Reads `keyword` (letters, any case) if it comes next; otherwise leaves the input as it was.
    pub fn scan_keyword(&mut self, keyword: &str) -> Result<bool, Error> {
        let mut read = Vec::new();
        let mut first = true;
        for k in keyword.chars() {
            let t = if first { self.next_x_nonspace()? } else { self.get_x_token()? };
            first = false;
            match t {
                Some(Token::Char(c, _)) if c.eq_ignore_ascii_case(&k) => read.push(Token::Char(c, Cat::Letter)),
                Some(other) => {
                    self.back(other);
                    self.push_tokens(read);
                    return Ok(false);
                }
                None => {
                    self.push_tokens(read);
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// `plus 1fil minus 2pt` after a skip value: read and ignored (no stretching in figures).
    fn skip_glue_parts(&mut self) -> Result<(), Error> {
        for k in ["plus", "minus"] {
            if self.scan_keyword(k)? {
                let _ = self.scan_dimen_or_fil();
            }
        }
        Ok(())
    }

    fn scan_dimen_or_fil(&mut self) -> Result<(), Error> {
        let mut sign_and_number = Vec::new();
        loop {
            match self.get_x_token()? {
                // spaces stay spaces, which the number skips
                Some(t @ Token::Char(d, _)) if d.is_ascii_digit() || matches!(d, '.' | ',' | '-' | '+' | ' ') => sign_and_number.push(t),
                Some(other) => {
                    self.back(other);
                    break;
                }
                None => break,
            }
        }
        if self.scan_keyword("fil")? {
            while self.scan_keyword("l")? {}
            return Ok(());
        }
        self.push_tokens(sign_and_number);
        self.scan_dimen().map(|_| ())
    }

    fn scan_optional_equals(&mut self) -> Result<(), Error> {
        if let Some(t) = self.next_x_nonspace()? {
            if !t.is_char('=') {
                self.back(t);
            }
        }
        Ok(())
    }

    /// `\numexpr …` up to `\relax` or a token that cannot continue the expression.
    fn scan_numexpr(&mut self) -> Result<i64, Error> {
        let v = self.numexpr_sum(false)?;
        // an ending \relax is consumed
        if let Some(t) = self.next_x_nonspace()? {
            if !t.is_cs("relax") {
                self.back(t);
            }
        }
        Ok(v)
    }

    fn numexpr_sum(&mut self, dim: bool) -> Result<i64, Error> {
        let mut v = self.numexpr_term(dim)?;
        loop {
            let Some(t) = self.next_x_nonspace()? else { return Ok(v) };
            if t.is_char('+') {
                v += self.numexpr_term(dim)?;
            } else if t.is_char('-') {
                v -= self.numexpr_term(dim)?;
            } else {
                self.back(t);
                return Ok(v);
            }
        }
    }

    fn numexpr_term(&mut self, dim: bool) -> Result<i64, Error> {
        let mut v = self.numexpr_factor(dim)?;
        loop {
            let Some(t) = self.next_x_nonspace()? else { return Ok(v) };
            if t.is_char('*') {
                let f = self.numexpr_factor(false)?;
                v = v.saturating_mul(f);
            } else if t.is_char('/') {
                let f = self.numexpr_factor(false)?;
                if f == 0 {
                    return Err(Error::Figure("division by 0 in \\numexpr".into()));
                }
                // e-TeX rounds the quotient
                v = ((v as f64) / (f as f64)).round() as i64;
            } else {
                self.back(t);
                return Ok(v);
            }
        }
    }

    fn numexpr_factor(&mut self, dim: bool) -> Result<i64, Error> {
        let t = self.next_x_nonspace()?.ok_or_else(|| Error::Figure("missing number in \\numexpr".into()))?;
        if t.is_char('(') {
            let v = self.numexpr_sum(dim)?;
            if let Some(c) = self.next_x_nonspace()? {
                if !c.is_char(')') {
                    self.back(c);
                }
            }
            return Ok(v);
        }
        if t.is_char('-') {
            return Ok(-self.numexpr_factor(dim)?);
        }
        self.back(t);
        if dim {
            self.scan_dimen()
        } else {
            self.scan_int()
        }
    }

    fn scan_dimexpr(&mut self) -> Result<i64, Error> {
        let v = self.numexpr_sum(true)?;
        if let Some(t) = self.next_x_nonspace()? {
            if !t.is_cs("relax") {
                self.back(t);
            }
        }
        Ok(v)
    }

    // ---------------------------------------------------------------- the commands of TeX itself

    /// Runs a primitive or a character that the drawing layer does not handle. Returns false if `t` is not one.
    pub fn execute_basic(&mut self, t: Token) -> Result<bool, Error> {
        let m = self.meaning(&t);
        if m.is_expandable() {
            self.expand(&t, &m)?;
            return Ok(true);
        }
        match (&t, &m) {
            (Token::Char(_, Cat::Begin), _) => {
                self.begin_group();
                Ok(true)
            }
            (Token::Char(_, Cat::End), _) => {
                if !self.end_group() {
                    self.warn("extra '}'");
                }
                Ok(true)
            }
            (_, Meaning::Prim(p)) => self.execute_prim(*p, &t),
            (_, Meaning::Count(r)) => {
                let r = *r;
                self.scan_optional_equals()?;
                let v = self.scan_int()?;
                self.set_count(r, v, false);
                Ok(true)
            }
            (_, Meaning::Dimen(r) | Meaning::Skip(r)) => {
                let (r, glue) = (*r, matches!(m, Meaning::Skip(_)));
                self.scan_optional_equals()?;
                // a length named where a value is (`.6\linewidth]` in the options of an environment tikra
                // does not know) reads as an assignment with nothing to assign: what follows is kept
                let v = match self.scan_dimen() {
                    Err(Error::Figure(e)) if e == "missing dimension" => {
                        self.warn(format!("{}: no length to assign: ignored", show_token(&t)));
                        return Ok(true);
                    }
                    v => v?,
                };
                if glue {
                    self.skip_glue_parts()?;
                }
                self.set_dimen(r, v, false);
                Ok(true)
            }
            (_, Meaning::Toks(r)) => {
                let r = *r;
                self.scan_optional_equals()?;
                let v = self.read_general_text()?;
                self.set_toks(r, v, false);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn execute_prim(&mut self, p: Prim, t: &Token) -> Result<bool, Error> {
        use Prim::*;
        match p {
            Relax | Par | Endcsname | Outer | Long | Immediate | Ignorespaces => {
                if p == Ignorespaces {
                    while let Some(x) = self.get_x_token()? {
                        if !x.is_space() {
                            self.back(x);
                            break;
                        }
                    }
                }
            }
            Global => {
                self.global = true;
                // the next command is the assignment; leave the flag for it
            }
            Protected => {
                let next = self.get_token().ok_or_else(|| Error::Figure("\\protected at the end".into()))?;
                match self.meaning(&next) {
                    Meaning::Prim(Def | Gdef | Edef | Xdef) => {
                        let Meaning::Prim(q) = self.meaning(&next) else { unreachable!() };
                        self.definition(q, true)?;
                    }
                    Meaning::Prim(Global | Long) => {
                        if self.meaning(&next) == Meaning::Prim(Global) {
                            self.global = true;
                        }
                        self.back(Token::cs("protected"));
                    }
                    _ => self.back(next),
                }
            }
            Def | Gdef | Edef | Xdef => self.definition(p, false)?,
            Let => {
                let name = self.get_token().ok_or_else(|| Error::Figure("\\let at the end".into()))?;
                // \let\a= b: one optional equals sign and one optional space
                let mut v = self.get_token().ok_or_else(|| Error::Figure("\\let at the end".into()))?;
                while v.is_space() {
                    v = self.get_token().ok_or_else(|| Error::Figure("\\let at the end".into()))?;
                }
                if v.is_char('=') {
                    v = self.get_token().ok_or_else(|| Error::Figure("\\let at the end".into()))?;
                    if v.is_space() {
                        v = self.get_token().ok_or_else(|| Error::Figure("\\let at the end".into()))?;
                    }
                }
                let m = if v.is_cs("tikra@noexpand") { self.get_token().map(|x| self.meaning(&x)).unwrap_or(Meaning::Undefined) } else { self.meaning(&v) };
                self.set_meaning(&name, m, false);
            }
            Futurelet => {
                let name = self.get_token().ok_or_else(|| Error::Figure("\\futurelet at the end".into()))?;
                let a = self.get_token().ok_or_else(|| Error::Figure("\\futurelet at the end".into()))?;
                let b = self.get_token().ok_or_else(|| Error::Figure("\\futurelet at the end".into()))?;
                let m = self.meaning(&b);
                self.set_meaning(&name, m, false);
                self.push_tokens([a, b]);
            }
            Begingroup => self.begin_group(),
            Endgroup => {
                if !self.end_group() {
                    self.warn("extra \\endgroup");
                }
            }
            Count => {
                let r = self.scan_register_number()?;
                self.scan_optional_equals()?;
                let v = self.scan_int()?;
                self.set_count(r, v, false);
            }
            Dimen | Skip => {
                let r = self.scan_register_number()?;
                self.scan_optional_equals()?;
                let v = self.scan_dimen()?;
                if p == Skip {
                    self.skip_glue_parts()?;
                }
                self.set_dimen(r, v, false);
            }
            Toks => {
                let r = self.scan_register_number()?;
                self.scan_optional_equals()?;
                let v = self.read_general_text()?;
                self.set_toks(r, v, false);
            }
            Countdef | Dimendef | Skipdef | Toksdef | Chardef | Mathchardef => {
                let name = self.get_token().ok_or_else(|| Error::Figure("missing name".into()))?;
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                let m = match p {
                    Countdef => Meaning::Count(n.clamp(0, 65535) as u32),
                    Dimendef => Meaning::Dimen(n.clamp(0, 65535) as u32),
                    Skipdef => Meaning::Skip(n.clamp(0, 65535) as u32),
                    Toksdef => Meaning::Toks(n.clamp(0, 65535) as u32),
                    _ => Meaning::CharDef(n),
                };
                self.set_meaning(&name, m, false);
            }
            Newcount | Newdimen | Newskip | Newlength | Newtoks | Newcounter => {
                let name = if p == Newcounter {
                    let n = tokens_to_string(&self.read_undelimited()?);
                    // \newcounter{name}[parent]
                    if let Some(x) = self.next_nonspace() {
                        if x.is_char('[') {
                            self.read_until_char(']', true)?;
                        } else {
                            self.back(x);
                        }
                    }
                    // \the<name> prints the counter's value
                    let the = vec![Token::cs("arabic"), Token::Char('{', Cat::Begin)];
                    let the = the.into_iter().chain(string_tokens(n.trim())).chain([Token::Char('}', Cat::End)]).collect();
                    self.define_macro(&format!("the{}", n.trim()), the, true);
                    Token::Cs(Rc::from(format!("c@{}", n.trim()).as_str()))
                } else {
                    let t = self.read_undelimited()?;
                    t.into_iter().next().ok_or_else(|| Error::Figure("missing name".into()))?
                };
                let r = self.new_register();
                let m = match p {
                    Newcount | Newcounter => Meaning::Count(r),
                    Newdimen | Newlength => Meaning::Dimen(r),
                    Newskip => Meaning::Skip(r),
                    _ => Meaning::Toks(r),
                };
                self.global = false;
                self.set_meaning(&name, m, true);
                match p {
                    Newcount | Newcounter => {
                        self.counts.insert(r, 0);
                    }
                    Newtoks => {
                        self.toks.insert(r, Rc::new(Vec::new()));
                    }
                    _ => {
                        self.dimens.insert(r, 0);
                    }
                }
            }
            Newif => {
                // \newif\iffoo defines \foofalse, \footrue and \iffoo
                let t = self.get_token().ok_or_else(|| Error::Figure("\\newif at the end".into()))?;
                let Token::Cs(name) = &t else { return Ok(true) };
                let base = name.strip_prefix("if").unwrap_or(name).to_string();
                self.global = false;
                self.set_meaning(&t, Meaning::Prim(Iffalse), true);
                let mk = |v: bool| -> Meaning {
                    let body = vec![Body::Tok(Token::cs("let")), Body::Tok(t.clone()), Body::Tok(Token::cs(if v { "iftrue" } else { "iffalse" }))];
                    Meaning::Macro(Rc::new(Macro { params: Vec::new(), body, optional: None, protected: false }))
                };
                self.set_meaning(&Token::Cs(Rc::from(format!("{base}true").as_str())), mk(true), true);
                self.set_meaning(&Token::Cs(Rc::from(format!("{base}false").as_str())), mk(false), true);
            }
            Newboolean => {
                let n = tokens_to_string(&self.read_undelimited()?);
                self.push_tokens([Token::cs("newif"), Token::Cs(Rc::from(format!("if{}", n.trim()).as_str()))]);
            }
            Setboolean => {
                let n = tokens_to_string(&self.read_undelimited()?);
                let v = tokens_to_string(&self.read_undelimited()?);
                self.push_tokens([Token::Cs(Rc::from(format!("{}{}", n.trim(), v.trim().to_lowercase()).as_str()))]);
            }
            Xstring(name) => self.xstring(name)?,
            Addtomacro(name) => {
                // \g@addto@macro\m{tokens}: the tokens put at the end of \m's body, unexpanded, for
                // the whole document; etoolbox's \appto (\preto at the start), with \g… global and
                // \e…, \x… (global) the tokens expanded; babel's \addto is global. An undefined \m
                // is defined.
                let target = self.read_undelimited()?;
                let tokens = self.read_undelimited()?;
                let Some(m) = target.into_iter().find(|x| !x.is_space()) else { return Ok(true) };
                let tokens = if name.starts_with(['e', 'x']) { self.expand_fully(tokens)? } else { tokens };
                let global = !matches!(name, "appto" | "preto" | "eappto" | "epreto");
                let (params, old) = match self.meaning(&m) {
                    Meaning::Macro(old) => (old.params.clone(), old.body.clone()),
                    _ => (Vec::new(), Vec::new()),
                };
                let new = tokens.into_iter().map(Body::Tok);
                let body = if name.ends_with("preto") { new.chain(old).collect() } else { old.into_iter().chain(new).collect() };
                let def = Macro { params, body, optional: None, protected: false };
                self.set_meaning(&m, Meaning::Macro(Rc::new(def)), global);
            }
            Setlength | Addtolength => {
                let target = self.read_undelimited()?;
                let value = self.read_undelimited()?;
                let Some(name) = target.first() else { return Ok(true) };
                let r = match self.meaning(name) {
                    Meaning::Dimen(r) | Meaning::Skip(r) => r,
                    _ => {
                        self.warn(format!("\\setlength: {} is not a length", show_token(name)));
                        return Ok(true);
                    }
                };
                let v = self.dimen_of(value)?;
                let v = if p == Addtolength { self.dimens.get(&r).copied().unwrap_or(0) + v } else { v };
                self.set_dimen(r, v, false);
            }
            Setcounter | Addtocounter | Stepcounter => {
                let name = tokens_to_string(&self.read_undelimited()?);
                let v = if p == Stepcounter {
                    1
                } else {
                    let value = self.read_undelimited()?;
                    self.int_of(value)?
                };
                if let Meaning::Count(r) = self.meaning_of(&format!("c@{}", name.trim())) {
                    let v = if p == Setcounter { v } else { self.counts.get(&r).copied().unwrap_or(0) + v };
                    self.set_count(r, v, true);
                }
            }
            Advance | Multiply | Divide => {
                let target = self.get_x_token()?.ok_or_else(|| Error::Figure("missing register".into()))?;
                let (is_count, r) = match self.meaning(&target) {
                    Meaning::Count(r) => (true, r),
                    Meaning::Dimen(r) | Meaning::Skip(r) => (false, r),
                    Meaning::Prim(Count) => (true, self.scan_register_number()?),
                    Meaning::Prim(Dimen | Skip) => (false, self.scan_register_number()?),
                    _ => return Err(Error::Figure(format!("you can't use {} after \\advance", show_token(&target)))),
                };
                self.scan_keyword("by")?;
                let old = if is_count { self.counts.get(&r).copied().unwrap_or(0) } else { self.dimens.get(&r).copied().unwrap_or(0) };
                let v = match p {
                    Advance => old + if is_count { self.scan_int()? } else { self.scan_dimen()? },
                    Multiply => old.saturating_mul(self.scan_int()?),
                    _ => {
                        let d = self.scan_int()?;
                        if d == 0 {
                            return Err(Error::Figure("arithmetic overflow (division by 0)".into()));
                        }
                        old / d
                    }
                };
                if is_count {
                    self.set_count(r, v, false);
                } else {
                    self.set_dimen(r, v, false);
                }
            }
            Catcode => {
                let c = self.scan_int()?;
                self.scan_optional_equals()?;
                let v = self.scan_int()?;
                if let (Some(ch), Some(cat)) = (char::from_u32(c.max(0) as u32), Cat::from_code(v)) {
                    self.set_catcode(ch, cat);
                }
            }
            Makeatletter => self.set_catcode('@', Cat::Letter),
            Makeatother => self.set_catcode('@', Cat::Other),
            Afterassignment => {
                self.after_assignment = self.get_token();
            }
            Aftergroup => {
                if let Some(x) = self.get_token() {
                    if let Some(level) = self.after_group.last_mut() {
                        level.push(x);
                    }
                }
            }
            Message | Typeout | Errmessage | Showtokens | Write => {
                if p == Write {
                    let _ = self.scan_int();
                }
                let _ = self.read_undelimited()?;
            }
            Show | Showthe => {
                let _ = self.get_token();
            }
            Numexpr | Dimexpr | Glueexpr => {
                // outside \the or a comparison: read and dropped
                let _ = if p == Numexpr { self.scan_numexpr()? } else { self.scan_dimexpr()? };
            }
            Newcommand | Renewcommand | Providecommand | Declarerobustcommand => self.newcommand(p)?,
            Newdocumentcommand(kind) => self.new_document_command(t, kind)?,
            Newenvironment | Renewenvironment => self.newenvironment()?,
            Uppercase | Lowercase => {
                let body = self.read_general_text()?;
                let out = body
                    .into_iter()
                    .map(|x| match x {
                        Token::Char(c, cat) => Token::Char(if p == Uppercase { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }, cat),
                        other => other,
                    })
                    .collect::<Vec<_>>();
                self.push_tokens(out);
            }
            Setbox => {
                // a box named by an undefined control sequence (pgf's node part boxes) is kept by name
                let named = match self.next_nonspace() {
                    Some(Token::Cs(n)) if matches!(self.meaning(&Token::Cs(n.clone())), Meaning::Undefined) => Some(n.to_string()),
                    Some(other) => {
                        self.back(other);
                        let _ = self.scan_int()?;
                        None
                    }
                    None => None,
                };
                self.scan_optional_equals()?;
                // \setbox0=\hbox{…}: the box is measured as empty
                if let Some(x) = self.next_x_nonspace()? {
                    if x.is_cs("hbox") || x.is_cs("vbox") {
                        let body = self.read_general_text()?;
                        if let Some(n) = named {
                            self.named_boxes.retain(|(m, _)| *m != n);
                            self.named_boxes.push((n, body));
                        }
                    } else if x.is_cs("box") || x.is_cs("copy") {
                        // another box register's content: the register read (by name or number)
                        match self.next_nonspace() {
                            Some(Token::Cs(_)) | None => {}
                            Some(other) => {
                                self.back(other);
                                let _ = self.scan_int()?;
                            }
                        }
                    } else {
                        self.back(x);
                    }
                }
            }
            Hbox => {
                let _ = self.scan_keyword("to").and_then(|to| if to { self.scan_dimen().map(|_| ()) } else { Ok(()) });
                let body = self.read_general_text()?;
                self.push_tokens(body);
            }
            // \wd, \ht, \dp: the last ones (an expandable primitive is expanded before it gets here)
            _ => {
                let n = self.scan_int()?;
                self.scan_optional_equals()?;
                let v = self.scan_dimen()?;
                let k = match p {
                    Wd => 0,
                    Ht => 1,
                    _ => 2,
                };
                self.box_dims.entry(n).or_default()[k] = v;
            }
        }
        let _ = t;
        Ok(true)
    }

    /// Reads and drops what is left above `depth` (after reading a value from a pushed list).
    pub fn drain_to(&mut self, depth: usize) {
        while self.input.len() > depth {
            if self.get_token_until(depth).is_none() {
                break;
            }
        }
    }

    /// `\def\name<parameter text>{body}`, and `\edef` (body expanded at once).
    fn definition(&mut self, p: Prim, protected: bool) -> Result<(), Error> {
        let global = std::mem::take(&mut self.global) || matches!(p, Prim::Gdef | Prim::Xdef);
        let name = self.get_token().ok_or_else(|| Error::Figure("\\def at the end".into()))?;
        let mut params = Vec::new();
        let mut n = 0u8;
        loop {
            let t = self.get_token().ok_or_else(|| Error::Figure(format!("definition of {} without a body", show_token(&name))))?;
            match t {
                Token::Char(_, Cat::Begin) => break,
                Token::Char(_, Cat::Param) => {
                    let d = self.get_token().ok_or_else(|| Error::Figure("# at the end of a parameter text".into()))?;
                    match d {
                        Token::Char(c, _) if c.is_ascii_digit() => {
                            n += 1;
                            params.push(Param::Arg(c as u8 - b'0'));
                        }
                        // #{ : the last parameter is delimited by the opening brace, which stays
                        Token::Char(_, Cat::Begin) => {
                            params.push(Param::Tok(Token::Char('{', Cat::Begin)));
                            break;
                        }
                        other => params.push(Param::Tok(other)),
                    }
                }
                other => params.push(Param::Tok(other)),
            }
        }
        let raw = self.read_group_body()?;
        let raw = if matches!(p, Prim::Edef | Prim::Xdef) { self.expand_fully(raw)? } else { raw };
        let body = parse_body(raw);
        let _ = n;
        let m = Macro { params, body, optional: None, protected };
        self.global = false;
        self.set_meaning(&name, Meaning::Macro(Rc::new(m)), global);
        Ok(())
    }

    /// `\newcommand{\name}[n][default]{body}` (also `\newcommand*`, `\renewcommand`, `\providecommand`).
    fn newcommand(&mut self, p: Prim) -> Result<(), Error> {
        let mut t = self.next_nonspace().ok_or_else(|| Error::Figure("\\newcommand at the end".into()))?;
        if t.is_char('*') {
            t = self.next_nonspace().ok_or_else(|| Error::Figure("\\newcommand at the end".into()))?;
        }
        let name = if matches!(t, Token::Char(_, Cat::Begin)) {
            let body = self.read_group_body()?;
            body.into_iter().find(|x| !x.is_space()).ok_or_else(|| Error::Figure("\\newcommand without a name".into()))?
        } else {
            t
        };
        let mut n = 0u8;
        let mut optional = None;
        if let Some(x) = self.next_nonspace() {
            if x.is_char('[') {
                let k = self.read_until_char(']', true)?;
                n = tokens_to_string(&k).trim().parse().unwrap_or(0);
                if let Some(y) = self.next_nonspace() {
                    if y.is_char('[') {
                        optional = Some(self.read_until_char(']', false)?);
                    } else {
                        self.back(y);
                    }
                }
            } else {
                self.back(x);
            }
        }
        let body = self.read_undelimited()?;
        if p == Prim::Providecommand && !matches!(self.meaning(&name), Meaning::Undefined) {
            return Ok(());
        }
        let undelimited = if optional.is_some() { n.saturating_sub(1) } else { n };
        let first = if optional.is_some() { 2 } else { 1 };
        let params = (0..undelimited).map(|k| Param::Arg(first + k)).collect();
        let m = Macro { params, body: parse_body(body), optional, protected: false };
        self.set_meaning(&name, Meaning::Macro(Rc::new(m)), false);
        Ok(())
    }

    /// `\NewDocumentCommand\name{spec}{body}` (xparse, in the LaTeX kernel): `\name` reads its
    /// arguments by `spec`, then runs `body` with them as `#1`, `#2`…. The command is
    /// `\tikra@documentarguments{spec}` and a macro of undelimited parameters for the body.
    fn new_document_command(&mut self, t: &Token, kind: &str) -> Result<(), Error> {
        let first = self.next_nonspace().ok_or_else(|| Error::Figure(format!("{} at the end", show_token(t))))?;
        let name = if matches!(first, Token::Char(_, Cat::Begin)) {
            let body = self.read_group_body()?;
            body.into_iter().find(|x| !x.is_space()).ok_or_else(|| Error::Figure(format!("{} without a name", show_token(t))))?
        } else {
            first
        };
        let spec = self.read_undelimited()?;
        let body = self.read_undelimited()?;
        let defined = !matches!(self.meaning(&name), Meaning::Undefined);
        if kind.starts_with("Provide") && defined {
            return Ok(());
        }
        let n = parse_arg_spec(&spec, &mut self.warnings).len().min(9) as u8;
        let inner = Token::Cs(Rc::from(format!("tikra@document@{}", show_token(&name).trim_start_matches('\\')).as_str()));
        let params = (1..=n).map(Param::Arg).collect();
        self.set_meaning(&inner, Meaning::Macro(Rc::new(Macro { params, body: parse_body(body), optional: None, protected: false })), false);
        let mut call = vec![Token::cs("tikra@documentarguments"), Token::Char('{', Cat::Begin)];
        call.extend(spec);
        call.extend([Token::Char('}', Cat::End), inner]);
        let m = Macro { params: Vec::new(), body: call.into_iter().map(Body::Tok).collect(), optional: None, protected: !kind.contains("Expandable") };
        self.set_meaning(&name, Meaning::Macro(Rc::new(m)), false);
        Ok(())
    }

    /// `\tikra@documentarguments{spec}\inner`: reads a document command's arguments from the
    /// input and expands `\inner`'s body with them.
    fn document_arguments(&mut self) -> Result<(), Error> {
        let spec = self.read_undelimited()?;
        let inner = self.get_token().ok_or_else(|| Error::Figure("a document command without its body".into()))?;
        let Meaning::Macro(m) = self.meaning(&inner) else { return Ok(()) };
        let mut args = Vec::new();
        let novalue = || vec![Token::cs("tikra@novalue")];
        for a in parse_arg_spec(&spec, &mut Vec::new()) {
            let v = match a {
                ArgSpec::Mandatory => self.read_undelimited()?,
                ArgSpec::Optional(open, close, default) => match self.next_nonspace() {
                    Some(x) if x.is_char(open) => self.read_until_char(close, true)?,
                    other => {
                        if let Some(x) = other {
                            self.back(x);
                        }
                        default.unwrap_or_else(novalue)
                    }
                },
                ArgSpec::Required(open, close, default) => match self.next_nonspace() {
                    Some(x) if x.is_char(open) => self.read_until_char(close, true)?,
                    other => {
                        if let Some(x) = other {
                            self.back(x);
                        }
                        self.warnings.push(format!("{} is missing its argument in '{open}{close}'", show_token(&inner).replace("tikra@document@", "")));
                        default.unwrap_or_else(novalue)
                    }
                },
                ArgSpec::Flag(c) => match self.next_nonspace() {
                    Some(x) if x.is_char(c) => vec![Token::cs("BooleanTrue")],
                    other => {
                        if let Some(x) = other {
                            self.back(x);
                        }
                        vec![Token::cs("BooleanFalse")]
                    }
                },
            };
            args.push(v);
        }
        args.truncate(9);
        let body = substitute(&m.body, &args);
        self.push_tokens(body);
        Ok(())
    }

    /// `\newenvironment{name}[n][default]{begin}{end}`.
    fn newenvironment(&mut self) -> Result<(), Error> {
        let mut t = self.next_nonspace().ok_or_else(|| Error::Figure("\\newenvironment at the end".into()))?;
        if t.is_char('*') {
            t = self.next_nonspace().ok_or_else(|| Error::Figure("\\newenvironment at the end".into()))?;
        }
        self.back(t);
        let name = tokens_to_string(&self.read_undelimited()?).trim().to_string();
        let mut n = 0u8;
        let mut optional = None;
        if let Some(x) = self.next_nonspace() {
            if x.is_char('[') {
                n = tokens_to_string(&self.read_until_char(']', true)?).trim().parse().unwrap_or(0);
                if let Some(y) = self.next_nonspace() {
                    if y.is_char('[') {
                        optional = Some(self.read_until_char(']', false)?);
                    } else {
                        self.back(y);
                    }
                }
            } else {
                self.back(x);
            }
        }
        let begin = self.read_undelimited()?;
        let end = self.read_undelimited()?;
        let undelimited = if optional.is_some() { n.saturating_sub(1) } else { n };
        let first = if optional.is_some() { 2 } else { 1 };
        let params = (0..undelimited).map(|k| Param::Arg(first + k)).collect();
        let m = Macro { params, body: parse_body(begin), optional, protected: false };
        self.environments.insert(name, (Rc::new(m), end));
        Ok(())
    }

    /// Expands a `\newenvironment` begin code with its arguments read from the input.
    pub fn environment_begin(&mut self, name: &str) -> Result<Option<Vec<Token>>, Error> {
        let Some((m, end)) = self.environments.get(name).cloned() else { return Ok(None) };
        let args = self.read_arguments(&Token::cs(name), &m)?;
        let _ = end;
        Ok(Some(substitute(&m.body, &args)))
    }
}

enum BranchEnd {
    Else,
    Or,
    Fi,
}

fn restore<K: std::hash::Hash + Eq, V, S: std::hash::BuildHasher>(map: &mut HashMap<K, V, S>, k: K, old: Option<V>) {
    match old {
        Some(v) => {
            map.insert(k, v);
        }
        None => {
            map.remove(&k);
        }
    }
}

fn compare(a: i64, rel: char, b: i64) -> bool {
    match rel {
        '<' => a < b,
        '>' => a > b,
        _ => a == b,
    }
}

/// `#1` → argument, `##` → `#`.
/// One argument of a document command's specification.
#[derive(Debug, Clone, PartialEq)]
enum ArgSpec {
    /// `m`
    Mandatory,
    /// `o`, `O{default}`, `d<>`, `D<>{default}`: between the two characters if they are there
    Optional(char, char, Option<Vec<Token>>),
    /// `r<>`, `R<>{default}`
    Required(char, char, Option<Vec<Token>>),
    /// `s` (a star), `t+`: whether the character is there
    Flag(char),
}

/// Reads a document command's argument specification (`O{} O{0.2cm} m m m`); the prefixes `+`,
/// `!` and `>{…}` change nothing here, and unknown types are read as `m` with a warning.
fn parse_arg_spec(spec: &[Token], warnings: &mut Vec<String>) -> Vec<ArgSpec> {
    let mut out = Vec::new();
    let mut i = 0;
    // the next token that is not a space
    let next = |i: &mut usize| -> Option<Token> {
        while spec.get(*i).is_some_and(|t| t.is_space()) {
            *i += 1;
        }
        let t = spec.get(*i).cloned();
        *i += 1;
        t
    };
    let char_of = |t: Option<Token>| match t {
        Some(Token::Char(c, _)) => Some(c),
        _ => None,
    };
    // a `{…}` group, without its braces; nothing if there is none
    let group = |i: &mut usize| -> Vec<Token> {
        let mut j = *i;
        if !matches!(next(&mut j), Some(Token::Char(_, Cat::Begin))) {
            return Vec::new();
        }
        let mut depth = 1;
        let mut inside = Vec::new();
        while let Some(t) = spec.get(j) {
            j += 1;
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            inside.push(t.clone());
        }
        *i = j;
        inside
    };
    while i < spec.len() {
        let Some(c) = char_of(next(&mut i)) else { continue };
        match c {
            '+' | '!' => {}
            '>' => {
                group(&mut i);
            }
            'm' => out.push(ArgSpec::Mandatory),
            'o' => out.push(ArgSpec::Optional('[', ']', None)),
            'O' => out.push(ArgSpec::Optional('[', ']', Some(group(&mut i)))),
            's' => out.push(ArgSpec::Flag('*')),
            't' => out.push(ArgSpec::Flag(char_of(next(&mut i)).unwrap_or('*'))),
            'd' | 'D' | 'r' | 'R' => {
                let a = char_of(next(&mut i)).unwrap_or('[');
                let b = char_of(next(&mut i)).unwrap_or(']');
                let default = c.is_uppercase().then(|| group(&mut i));
                out.push(if c.eq_ignore_ascii_case(&'d') { ArgSpec::Optional(a, b, default) } else { ArgSpec::Required(a, b, default) });
            }
            other => {
                warnings.push(format!("document command argument type '{other}' not supported: read as 'm'"));
                out.push(ArgSpec::Mandatory);
            }
        }
    }
    out
}

pub fn parse_body(raw: Vec<Token>) -> Vec<Body> {
    let mut out = Vec::with_capacity(raw.len());
    let mut it = raw.into_iter().peekable();
    while let Some(t) = it.next() {
        if let Token::Char(c, Cat::Param) = t {
            match it.peek() {
                Some(Token::Char(d, _)) if d.is_ascii_digit() => {
                    let n = *d as u8 - b'0';
                    it.next();
                    out.push(Body::Arg(n));
                }
                Some(Token::Char(_, Cat::Param)) => {
                    // ## stays a parameter character (for definitions inside definitions)
                    let p = it.next().unwrap();
                    out.push(Body::Tok(p));
                }
                _ => out.push(Body::Tok(Token::Char(c, Cat::Param))),
            }
        } else {
            out.push(Body::Tok(t));
        }
    }
    out
}

pub fn substitute(body: &[Body], args: &[Vec<Token>]) -> Vec<Token> {
    let mut out = Vec::with_capacity(body.len());
    for b in body {
        match b {
            Body::Tok(t) => out.push(t.clone()),
            Body::Arg(n) => {
                if let Some(a) = args.get((*n as usize).wrapping_sub(1)) {
                    out.extend(a.iter().cloned());
                }
            }
        }
    }
    out
}

/// `{x}` → `x` when the braces enclose the whole list.
pub fn strip_outer_braces(v: Vec<Token>) -> Vec<Token> {
    if v.len() >= 2 && matches!(v[0], Token::Char(_, Cat::Begin)) && matches!(v[v.len() - 1], Token::Char(_, Cat::End)) {
        let mut depth = 0i32;
        for (i, t) in v.iter().enumerate() {
            match t {
                Token::Char(_, Cat::Begin) => depth += 1,
                Token::Char(_, Cat::End) => {
                    depth -= 1;
                    if depth == 0 && i != v.len() - 1 {
                        return v;
                    }
                }
                _ => {}
            }
        }
        return v[1..v.len() - 1].to_vec();
    }
    v
}

/// Text as character tokens (spaces as spaces, everything else "other"), as `\string` makes them.
pub fn string_tokens(s: &str) -> Vec<Token> {
    s.chars().map(|c| if c == ' ' { Token::Char(' ', Cat::Space) } else { Token::Char(c, Cat::Other) }).collect()
}

/// Tokens as text: control sequences as `\name` followed by a space when the name is a word.
pub fn tokens_to_string(v: &[Token]) -> String {
    let mut s = String::new();
    for (i, t) in v.iter().enumerate() {
        match t {
            Token::Cs(n) => {
                s.push('\\');
                s.push_str(n);
                let word = n.chars().next().is_some_and(|c| c.is_alphabetic() || c == '@');
                // a space only where a letter follows, so `\x+1` stays `\x+1`
                if word && matches!(v.get(i + 1), Some(Token::Char(c, _)) if c.is_alphabetic()) {
                    s.push(' ');
                }
            }
            Token::Char(c, _) => s.push(*c),
        }
    }
    s
}

/// Like [`tokens_to_string`], for text read again as TeX: a control word before a space (`\o` from
/// an active character's definition) gets `{}`, so the space stays.
pub fn tokens_to_text(v: &[Token]) -> String {
    let mut s = String::new();
    for (i, t) in v.iter().enumerate() {
        let word = matches!(t, Token::Cs(n) if n.chars().next().is_some_and(|c| c.is_alphabetic() || c == '@'));
        if word && matches!(v.get(i + 1), Some(Token::Char(_, Cat::Space))) {
            s.push_str(&tokens_to_string(std::slice::from_ref(t)));
            s.push_str("{}");
        } else {
            s.push_str(&tokens_to_string(&v[i..=i]));
            if word && matches!(v.get(i + 1), Some(Token::Char(c, _)) if c.is_alphabetic()) {
                s.push(' ');
            }
        }
    }
    s
}

pub fn show_token(t: &Token) -> String {
    match t {
        Token::Cs(n) => format!("\\{n}"),
        Token::Char(c, _) => c.to_string(),
    }
}

fn describe(m: &Meaning) -> String {
    match m {
        Meaning::Macro(mac) => {
            let params: String = mac
                .params
                .iter()
                .map(|p| match p {
                    Param::Tok(t) => show_token(t),
                    Param::Arg(n) => format!("#{n}"),
                })
                .collect();
            let body: String = mac
                .body
                .iter()
                .map(|b| match b {
                    Body::Tok(t) => show_token(t),
                    Body::Arg(n) => format!("#{n}"),
                })
                .collect();
            format!("macro:{params}->{body}")
        }
        Meaning::Char(c, _) => format!("the character {c}"),
        Meaning::Undefined => "undefined".into(),
        Meaning::Prim(Prim::Newdocumentcommand(n) | Prim::Documentif(n) | Prim::Ifstrempty(n) | Prim::Addtomacro(n) | Prim::Xstring(n)) => format!("\\{n}"),
        Meaning::Prim(p) => format!("\\{}", format!("{p:?}").to_lowercase()),
        Meaning::Count(r) => format!("\\count{r}"),
        Meaning::Dimen(r) => format!("\\dimen{r}"),
        Meaning::Skip(r) => format!("\\skip{r}"),
        Meaning::Toks(r) => format!("\\toks{r}"),
        Meaning::CharDef(v) => format!("\\char\"{v:X}"),
        Meaning::Native(_) => "\\relax".into(),
    }
}

fn roman(n: i64) -> String {
    if n <= 0 {
        return String::new();
    }
    let mut n = n.min(100_000);
    let mut s = String::new();
    for (v, r) in [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// The `ifthen` package's tests over expanded tokens.
fn ifthen_eval(tex: &mut Tex, t: &[Token]) -> Result<bool, Error> {
    // split by \or (lowest), then \and, with \not and parentheses \( \)
    fn split<'a>(t: &'a [Token], names: &[&str]) -> Vec<&'a [Token]> {
        let mut parts = Vec::new();
        let mut start = 0;
        for (i, x) in t.iter().enumerate() {
            if names.iter().any(|n| x.is_cs(n)) {
                parts.push(&t[start..i]);
                start = i + 1;
            }
        }
        parts.push(&t[start..]);
        parts
    }
    let ors = split(t, &["or", "OR"]);
    if ors.len() > 1 {
        for p in ors {
            if ifthen_eval(tex, p)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let ands = split(t, &["and", "AND"]);
    if ands.len() > 1 {
        for p in ands {
            if !ifthen_eval(tex, p)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let t: Vec<Token> = t.iter().filter(|x| !x.is_space()).cloned().collect();
    if let Some(first) = t.first() {
        if first.is_cs("not") || first.is_cs("NOT") {
            return Ok(!ifthen_eval(tex, &t[1..])?);
        }
        if first.is_cs("equal") {
            // \equal{a}{b}: the two groups that follow
            let rest = &t[1..];
            let (a, after) = take_group(rest);
            let (b, _) = take_group(after);
            return Ok(tokens_to_string(&a).trim() == tokens_to_string(&b).trim());
        }
        if first.is_cs("isodd") {
            let (a, _) = take_group(&t[1..]);
            let v: i64 = tokens_to_string(&a).trim().parse().unwrap_or(0);
            return Ok(v % 2 != 0);
        }
        if first.is_cs("boolean") {
            let (a, _) = take_group(&t[1..]);
            let name = format!("if{}", tokens_to_string(&a).trim());
            return Ok(tex.meaning_of(&name) == Meaning::Prim(Prim::Iftrue));
        }
        if first.is_cs("lengthtest") {
            let (a, _) = take_group(&t[1..]);
            let s = tokens_to_string(&a);
            let Some(i) = s.find(['<', '=', '>']) else { return Ok(false) };
            let rel = s[i..].chars().next().unwrap_or('=');
            let x = crate::expr::eval_length(s[..i].trim(), &crate::expr::Variables::new()).map(|v| v.0).unwrap_or(0.0);
            let y = crate::expr::eval_length(s[i + 1..].trim(), &crate::expr::Variables::new()).map(|v| v.0).unwrap_or(0.0);
            return Ok(match rel {
                '<' => x < y,
                '>' => x > y,
                _ => (x - y).abs() < 1e-9,
            });
        }
    }
    // a numeric comparison: a < b, a = b, a > b, read as \ifnum reads it (`\numexpr 3-1\relax` included)
    let s = tokens_to_string(&t).replace("\\numexpr", " ").replace("\\relax", " ");
    let Some(i) = s.find(['<', '=', '>']) else { return Ok(false) };
    let rel = s[i..].chars().next().unwrap_or('=');
    let a: f64 = crate::expr::eval(s[..i].trim(), &crate::expr::Variables::new()).unwrap_or(0.0);
    let b: f64 = crate::expr::eval(s[i + 1..].trim(), &crate::expr::Variables::new()).unwrap_or(0.0);
    Ok(match rel {
        '<' => a < b,
        '>' => a > b,
        _ => a == b,
    })
}

/// The tokens of a `{…}` group at the start of `t` (or the first token), and what follows; `t` is
/// a test without its spaces.
fn take_group(t: &[Token]) -> (Vec<Token>, &[Token]) {
    let Some(first) = t.first() else { return (Vec::new(), t) };
    if !matches!(first, Token::Char(_, Cat::Begin)) {
        return (vec![first.clone()], &t[1..]);
    }
    let mut depth = 0;
    let end = t.iter().position(|x| {
        match x {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth -= 1,
            _ => {}
        }
        depth == 0
    });
    end.map_or((t[1..].to_vec(), &[]), |k| (t[1..k].to_vec(), &t[k + 1..]))
}

#[cfg(test)]
mod tests;
