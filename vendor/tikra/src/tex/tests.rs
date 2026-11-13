use super::*;

/// Runs `text` and returns what TeX would typeset: the characters left after expansion and assignments.
fn run(text: &str) -> String {
    let mut t = Tex::new();
    run_in(&mut t, text)
}

fn run_in(t: &mut Tex, text: &str) -> String {
    t.push_text(text);
    let mut out = String::new();
    loop {
        let x = match t.get_x_token() {
            Ok(Some(x)) => x,
            Ok(None) => break,
            Err(e) => return format!("error: {e}"),
        };
        match &x {
            Token::Char(c, Cat::Letter | Cat::Other | Cat::Space | Cat::Math | Cat::Sup | Cat::Sub | Cat::Align) => out.push(*c),
            _ => match t.execute_basic(x.clone()) {
                Ok(true) => {}
                Ok(false) => out.push_str(&show_token(&x)),
                Err(e) => return format!("error: {e}"),
            },
        }
    }
    out
}

#[test]
fn macros_with_parameters() {
    assert_eq!(run(r"\def\a#1#2{(#2,#1)}\a xy"), "(y,x)");
    assert_eq!(run(r"\def\a#1{[#1]}\a{hello world}"), "[hello world]");
    assert_eq!(run(r"\def\pair(#1,#2){#1+#2}\pair(3,4)"), "3+4");
    // braces around a whole delimited argument go; around a part they stay (and are a group when run)
    assert_eq!(run(r"\def\a#1.{<#1>}\a{x}.\a{y}z."), "<x><yz>");
    assert_eq!(run(r"\def\a#1.{\detokenize{#1}}\a{x}.\a{y}z."), "x{y}z");
    // a nested definition with ##
    assert_eq!(run(r"\def\outer#1{\def\inner##1{#1##1}}\outer a\inner b"), "ab");
}

#[test]
fn spaces_lines_and_comments() {
    assert_eq!(run("a  b"), "a b");
    assert_eq!(run("\\def\\x{X}\\x y"), "Xy");
    assert_eq!(run("a% comment\n   b"), "ab");
    assert_eq!(run("a\nb"), "a b");
    assert_eq!(run("a\n\nb"), "ab");
}

/// `^^` followed by two lowercase hexadecimal digits is the character of that code; followed by
/// anything else, the character 64 away.
#[test]
fn caret_notation() {
    assert_eq!(run("^^41^^62c"), "Abc");
    assert_eq!(run("^^41x"), "Ax");
    assert_eq!(run("^^5a^^4G"), "ZtG");
    assert_eq!(run(r"\def\a{^^7a}\a"), "z");
}

#[test]
fn groups_and_global() {
    assert_eq!(run(r"\def\a{1}{\def\a{2}\a}\a"), "21");
    assert_eq!(run(r"\def\a{1}{\gdef\a{2}}\a"), "2");
    assert_eq!(run(r"\def\a{1}{\global\def\a{2}\def\a{3}}\a"), "2");
    assert_eq!(run(r"\def\a{1}\begingroup\def\a{2}\endgroup\a"), "1");
    assert_eq!(run(r"\def\x{X}{\aftergroup\x}y"), "Xy");
}

/// `\global` before `\catcode` makes that one assignment global, and no other after it.
#[test]
fn global_catcode_is_one_assignment() {
    assert_eq!(run(r"{\global\catcode`\Q=11 \count1=7 }\the\count1"), "0");
    assert_eq!(run(r"\def\a{1}{\global\catcode`\Q=11 \def\a{2}}\a"), "1");
    assert_eq!(run(r"{\global\catcode`\Q=12 }\the\catcode`\Q"), "12");
}

#[test]
fn edef_let_and_expansion_control() {
    assert_eq!(run(r"\def\a{A}\edef\b{\a\a}\def\a{Z}\b"), "AA");
    assert_eq!(run(r"\def\a{A}\edef\b{\noexpand\a}\def\a{Z}\b"), "Z");
    assert_eq!(run(r"\def\a{A}\edef\b{\unexpanded{\a}}\def\a{Z}\b"), "Z");
    // a number at the end of an \edef's body ends there: the 2 after the definition is not part of it
    assert_eq!(run(r"\count1=5 \edef\e{\the\count1}2\e"), "25");
    assert_eq!(run(r"\def\a{A}\let\b\a\def\a{Z}\b\a"), "AZ");
    assert_eq!(run(r"\def\a#1{[#1]}\def\b{x}\expandafter\a\b"), "[x]");
    assert_eq!(run(r"\expandafter\def\csname my macro\endcsname{M}\csname my macro\endcsname"), "M");
    assert_eq!(run(r"\string\foo"), "\\foo");
    assert_eq!(run(r"\detokenize{\a b}"), "\\a b");
}

#[test]
fn registers_and_arithmetic() {
    assert_eq!(run(r"\count255=7 \advance\count255 by 3 \the\count255"), "10");
    assert_eq!(run(r"\newcount\n \n=4 \multiply\n by 5 \divide\n 3 \the\n"), "6");
    assert_eq!(run(r"\newdimen\d \d=1.5pt \advance\d by 2pt \the\d"), "3.5pt");
    assert_eq!(run(r"\newdimen\d \d=1cm \the\d"), "28.45274pt");
    assert_eq!(run(r"\newlength\l\setlength{\l}{2pt}\addtolength{\l}{1pt}\the\l"), "3.0pt");
    assert_eq!(run(r"\the\numexpr 2*(3+4)\relax"), "14");
    assert_eq!(run(r"\the\numexpr 7/2\relax"), "4");
    assert_eq!(run(r"\the\dimexpr 1pt*3\relax"), "3.0pt");
    assert_eq!(run(r"\number`a"), "97");
    assert_eq!(run(r"\romannumeral 14"), "xiv");
    assert_eq!(run(r"\newcounter{c}\setcounter{c}{3}\stepcounter{c}\arabic{c}"), "4");
}

/// The stretch and shrink of glue are read whole when a space comes before their number.
#[test]
fn glue_with_a_space_before_its_stretch() {
    assert_eq!(run(r"\skip3=2pt plus 1pt x"), "x");
    assert_eq!(run(r"\skip3=2pt plus 1pt minus 2pt x"), "x");
    assert_eq!(run(r"\skip3=2pt plus 1 fil x"), "x");
    assert_eq!(run(r"\skip3=2pt minus -1pt x\the\skip3"), "x2.0pt");
}

#[test]
fn conditionals() {
    assert_eq!(run(r"\ifnum 3>2 yes\else no\fi"), "yes");
    assert_eq!(run(r"\ifnum 3<2 yes\else no\fi"), "no");
    assert_eq!(run(r"\ifdim 1pt<2pt a\fi b"), "ab");
    assert_eq!(run(r"\def\a{x}\def\b{x}\ifx\a\b same\else different\fi"), "same");
    assert_eq!(run(r"\def\a{x}\def\b{y}\ifx\a\b same\else different\fi"), "different");
    assert_eq!(run(r"\ifcase 2 zero\or one\or two\else many\fi"), "two");
    assert_eq!(run(r"\ifcase 5 zero\or one\else many\fi"), "many");
    // the space after the second character is text
    assert_eq!(run(r"\if aa yes\fi\if ab no\fi"), " yes");
    assert_eq!(run(r"\ifodd 3 odd\fi"), "odd");
    assert_eq!(run(r"\iftrue\iffalse x\else y\fi\fi"), "y");
    assert_eq!(run(r"\newif\ifshow \showtrue \ifshow S\fi\showfalse\ifshow T\fi"), "S");
    assert_eq!(run(r"\ifdefined\nothing a\else b\fi\ifdefined\relax c\fi"), "bc");
    assert_eq!(run(r"\ifcsname relax\endcsname a\fi\ifcsname nothing\endcsname b\fi"), "a");
    assert_eq!(run(r"\unless\ifnum 1=2 not\fi"), "not");
    // the number of an \ifcase ends at an \or right after it
    assert_eq!(run(r"\def\m{02}\ifcase\m\or a\or b\or c\fi\ifcase1\or d\fi"), "bd");
    assert_eq!(run(r"\ifthenelse{\equal{a}{a}}{yes}{no}"), "yes");
    assert_eq!(run(r"\ifthenelse{3 > 5 \or 1=1}{yes}{no}"), "yes");
    assert_eq!(run(r"\ifthenelse{\not 3 > 5 \and \isodd{3}}{yes}{no}"), "yes");
}

#[test]
fn latex_commands() {
    assert_eq!(run(r"\newcommand{\pt}[2]{(#1|#2)}\pt{a}{b}"), "(a|b)");
    assert_eq!(run(r"\newcommand\opt[2][d]{#1-#2}\opt{x}\opt[y]{z}"), "d-xy-z");
    assert_eq!(run(r"\newcommand*{\x}{X}\renewcommand{\x}{Y}\providecommand{\x}{Z}\x"), "Y");
    assert_eq!(run(r"\makeatletter\def\a@b{AB}\a@b\makeatother"), "AB");
    assert_eq!(run(r"\makeatletter\@ifnextchar[{opt}{none}[x"), "opt[x");
    assert_eq!(run(r"\makeatletter\@ifnextchar[{opt}{none} x"), "nonex");
    let mut t = Tex::new();
    assert_eq!(run_in(&mut t, r"\newenvironment{box}[1]{<#1|}{|>}"), "");
    assert!(t.environments.contains_key("box"));
}

#[test]
fn loops_end() {
    assert_eq!(run(r"\newcount\i \i=0 \loop\ifnum\i<3 \advance\i 1 \the\i\repeat"), "123");
    assert_eq!(run(r"\newcounter{k}\whiledo{\value{k}<3}{\stepcounter{k}\arabic{k}}"), "123");
    // a conditional opened while the test of another is read: the outer's false branch skips its
    // \else, as TeX does
    assert_eq!(run(r"\def\t#1{\ifdim#1pt>-1pt\ifdim#1pt<1pt A\else B\fi\fi C}\t{-2}\t{0}\t{2}"), "CACBC");
    // ifthen compares as \ifnum does, so \numexpr is evaluated
    assert_eq!(run(r"\newcounter{n}\def\m{3}\whiledo{\numexpr\arabic{n}-1 < \m}{\stepcounter{n}\arabic{n}}"), "1234");
    assert_eq!(run(r"\ifthenelse{\numexpr 2*3\relax = 6}{y}{n}"), "y");
    // endless expansion stops with an error instead of hanging
    assert!(run(r"\def\a{\a}\a").starts_with("error"));
    assert!(run(r"\def\a{x\a}\a").starts_with("error"));
}

#[test]
fn document_commands() {
    // optional arguments in brackets, with their defaults, then mandatory ones
    let def = r"\NewDocumentCommand{\c}{O{} O{0.2cm} m m}{(#1|#2|#3|#4)}";
    assert_eq!(run(&format!(r"{def}\c[a][b]{{x}}{{y}}")), "(a|b|x|y)");
    assert_eq!(run(&format!(r"{def}\c[a] {{x}}y")), "(a|0.2cm|x|y)");
    assert_eq!(run(&format!(r"{def}\c{{x}}{{y}}")), "(|0.2cm|x|y)");
    // a missing `o` is no value; `s` is a star or not
    let def = r"\NewDocumentCommand\d{s o m}{\IfBooleanTF{#1}{*}{-}\IfNoValueTF{#2}{none}{[#2]}\IfValueT{#2}{!}#3}";
    assert_eq!(run(&format!(r"{def}\d*[q]{{z}}")), "*[q]!z");
    assert_eq!(run(&format!(r"{def}\d{{z}}")), "-nonez");
    // other delimiters, a required pair and a token flag
    let def = r"\DeclareDocumentCommand\e{D<>{1} t+ r() +m}{#1#2#3#4}\def\BooleanTrue{T}\def\BooleanFalse{F}";
    assert_eq!(run(&format!(r"{def}\e<2>+(3){{4}}")), "2T34");
    assert_eq!(run(&format!(r"{def}\e(3)4")), "1F34");
    // a missing value typesets as LaTeX's marker; Provide keeps a command, Renew replaces it
    assert_eq!(run(r"\NewDocumentCommand\f{o}{#1}\f"), "-NoValue-");
    assert_eq!(run(r"\def\g{old}\ProvideDocumentCommand\g{}{new}\g\RenewDocumentCommand\g{m}{<#1>}\g x"), "old<x>");
}

#[test]
fn appending_to_macros() {
    // \g@addto@macro: unexpanded, for the whole document, even from a group; `#` stays a token
    assert_eq!(run(r"\makeatletter\def\a{x}{\g@addto@macro\a{y\b}}\def\b{z}\a"), "xyz");
    assert_eq!(run(r"\makeatletter\def\a{x}\g@addto@macro\a{##}\detokenize\expandafter{\a}"), "x##");
    // etoolbox: \appto and \preto local, \gappto global, \eappto and \xappto expand; babel's \addto
    // is global; an undefined macro is defined; parameters are kept
    assert_eq!(run(r"\def\c{1}{\appto\c{2}\c}\c"), "121");
    assert_eq!(run(r"\def\c{1}{\gappto\c{2}}{\addto\c{3}}{\xappto\c{4}}\c"), "1234");
    assert_eq!(run(r"\def\c{1}\preto\c{0}{\gpreto\c{-}}\c"), "-01");
    assert_eq!(run(r"\def\d{A}\def\e{B}\eappto\d{\e}\def\e{C}\epreto\d{\e}\d"), "CAB");
    assert_eq!(run(r"\appto\new{z}\xpreto\new{y}\new"), "yz");
    assert_eq!(run(r"\def\c#1{a#1}\appto\c{b}\c x"), "axb");
}

#[test]
fn etoolbox_tests() {
    // nothing is expanded: a macro that is empty is not an empty text, a space is not empty but blank
    assert_eq!(run(r"\def\e{}\ifstrempty{}{T}{F}\ifstrempty{ }{T}{F}\ifstrempty{\e}{T}{F}\ifstrempty{x}{T}{F}"), "TFFF");
    assert_eq!(run(r"\def\e{}\ifblank{}{T}{F}\ifblank{ }{T}{F}\ifblank{\e}{T}{F}\ifblank{x}{T}{F}"), "TTFF");
    // \ifdefempty: a macro without parameters whose body is empty
    assert_eq!(run(r"\def\e{}\def\s{ }\def\p#1{}\ifdefempty{\e}{T}{F}\ifdefempty\s{T}{F}\ifdefempty{\p}{T}{F}\ifdefempty{\relax}{T}{F}"), "TFFF");
}

/// Commands whose kind is in their name keep it under another name given by `\let`.
#[test]
fn commands_of_a_kind_under_let() {
    assert_eq!(run(r"\let\x\ifstrempty \x{}{T}{F}\let\y\ifblank \y{ }{T}{F}"), "TT");
    assert_eq!(run(r"\let\x\IfStrEq \x{a}{a}{T}{F}\let\y\StrLen \y{abc}"), "T3");
    assert_eq!(run(r"\def\m{a}\let\x\preto \x\m{b}\m"), "ba");
    assert_eq!(run(r"\let\x\NewDocumentCommand \x\d{o}{\IfNoValueTF{#1}{-}{#1}}\d\d[z]"), "-z");
    assert_eq!(run(r"\let\x\IfBooleanTF \x{\BooleanTrue}{T}{F}"), "T");
}

/// etoolbox's `\ifstrequal` compares its two texts as written: a macro is not its expansion.
#[test]
fn ifstrequal_expands_nothing() {
    assert_eq!(run(r"\def\a{A}\ifstrequal{\a}{A}{T}{F}\ifstrequal{\a}{\a}{T}{F}\ifstrequal{ab}{ab}{T}{F}\ifstrequal{a}{b}{T}{F}"), "FTTF");
}

#[test]
fn etoolbox_toggles() {
    // false when made; set locally, or globally with \global; \providetoggle keeps one that exists
    let t = r"\newtoggle{a}\iftoggle{a}{T}{F}\toggletrue{a}\iftoggle{a}{T}{F}\nottoggle{a}{T}{F}";
    assert_eq!(run(t), "FTF");
    assert_eq!(run(r"\newtoggle{a}\toggletrue{a}{\togglefalse{a}\iftoggle{a}{T}{F}}\iftoggle{a}{T}{F}"), "FT");
    assert_eq!(run(r"\newtoggle{a}{\global\toggletrue{a}}\iftoggle{a}{T}{F}\settoggle{a}{false}\iftoggle{a}{T}{F}"), "TF");
    assert_eq!(run(r"\newtoggle{a}\toggletrue{a}\providetoggle{a}\providetoggle{b}\iftoggle{a}{T}{F}\iftoggle{b}{T}{F}"), "TF");
}

#[test]
fn counters_print_as_the_counter() {
    assert_eq!(run(r"\newcounter{Sum}\setcounter{Sum}{5}\addtocounter{Sum}{2}\theSum"), "7");
}

/// `\the\value{c}` is the counter's value, as `\the` of its register.
#[test]
fn the_value_of_a_counter() {
    assert_eq!(run(r"\newcounter{c}\setcounter{c}{13}\the\value{c}"), "13");
    assert_eq!(run(r"\newcounter{c}\setcounter{c}{2}\the\value{c}x"), "2x");
    assert_eq!(run(r"\newcounter{c}\setcounter{c}{4}\edef\v{\the\value{c}}\v"), "4");
}

#[test]
fn latex_page_lengths() {
    // article at 10pt on letter paper; glue keeps its natural size
    assert_eq!(run(r"\the\paperwidth/\the\paperheight/\the\textheight/\the\oddsidemargin/\the\parskip"), "614.295pt/794.96999pt/550.0pt/62.0pt/0.0pt");
    assert_eq!(run(r"\setlength\parskip{2pt plus 1pt}\setlength{\headheight}{14pt}\the\parskip/\the\headheight/\the\captionmargin"), "2.0pt/14.0pt/0.0pt");
    // the paper from the class or geometry's options; landscape turns it
    let mut t = Tex::new();
    t.paper_options("11pt, a4paper");
    assert_eq!(run_in(&mut t, r"\the\paperwidth/\the\paperheight"), "597.50787pt/845.04684pt");
    t.paper_options("landscape");
    assert_eq!(run_in(&mut t, r"\the\paperwidth/\the\paperheight"), "845.04684pt/597.50787pt");
    t.paper_options("paper = a5paper, landscape");
    assert_eq!(run_in(&mut t, r"\the\paperwidth/\the\paperheight"), "597.50787pt/421.10078pt");
    t.paper_options("draft");
    assert_eq!(run_in(&mut t, r"\the\paperwidth"), "597.50787pt");
}

#[test]
fn xstring_parts() {
    let x = |s: &str| run(&format!(r"\def\s{{a,b,c}}{s}"));
    // lengths: a group is one character, spaces count, the text is expanded
    assert_eq!(x(r"\StrLen{abc}[\r]\r/\StrLen{a b}[\r]\r/\StrLen{\s}[\r]\r/\StrLen{a{bc}d}[\r]\r/\StrLen{}"), "3/3/5/3/0");
    assert_eq!(x(r"\StrLeft{abcdef}{2}[\r]\r/\StrLeft{abc}{5}/\StrLeft{abc}{-1}/\StrRight{abcdef}{2}/\StrRight{abc}{5}"), "ab/abc//ef/abc");
    assert_eq!(x(r"\StrMid{abcdef}{2}{4}/\StrMid{abcdef}{0}{9}/\StrMid{abcdef}{4}{2}/\StrChar{abc}{2}/\StrChar{abc}{5}"), "bcd/abcdef//b/");
    assert_eq!(x(r"\def\n{2}\StrLeft{abcd}{\n}/\StrLeft{abcd}{\numexpr1+2}"), "ab/abc");
    // around the n-th occurrence; none found: nothing (\StrCut: all on the left)
    assert_eq!(x(r"\StrBefore{\s}{,}/\StrBefore[2]{a,b,c}{,}/\StrBefore[3]{a,b,c}{,}/\StrBefore[0]{a,b,c}{,}/\StrBefore{abc}{x}"), "a/a,b///");
    assert_eq!(x(r"\StrBehind{a,b,c}{,}/\StrBehind[2]{a,b,c}{,}/\StrBehind{abc}{x}"), "b,c/c/");
    assert_eq!(x(r"\StrCut{a,b,c}{,}\l\r[\l][\r]\StrCut[2]{\s}{,}{\l}{\r}[\l][\r]\StrCut[0]{a,b,c}{,}\l\r[\l][\r]\StrCut{abc}{x}\l\r[\l][\r]"), "[a][b,c][a,b][c][a][b,c][abc][]");
    // the n1-th A and the n2-th B, both counted from the start
    assert_eq!(x(r"\StrBetween{a(b)c(d)e}{(}{)}/\StrBetween[2,2]{a(b)c(d)e}{(}{)}/\StrBetween[2,3]{,a,b,}{,}{,}/\StrBetween[1,\numexpr1+1]{,a,b,}{,}{,}"), "b/d/b/a");
    assert_eq!(x(r"\StrBetween[3,1]{,a,b,}{,}{,}/\StrBetween{abc}{x}{y}"), "/");
    // the result goes into a macro locally
    assert_eq!(x(r"\def\r{0}{\StrLeft{abc}{1}[\r]}\r"), "0");
}

#[test]
fn xstring_substitutions_and_tests() {
    let x = |s: &str| run(&format!(r"\def\s{{a,b,c}}{s}"));
    assert_eq!(x(r"\StrSubstitute{\s}{,}{}/\StrSubstitute[1]{a,b,c}{,}{--}/\StrSubstitute{aaa}{aa}{b}/\StrSubstitute{abc}{}{x}"), "abc/a--b,c/ba/abc");
    assert_eq!(x(r"\StrDel{a,b,c}{,}[\r]\r/\StrDel[1]{a,b,c}{,}/\StrCount{a,b,c}{,}/\StrCount{aaa}{aa}"), "abc/ab,c/2/1");
    assert_eq!(x(r"\IfSubStr{abc}{b}{T}{F}\IfSubStr{abc}{x}{T}{F}\IfSubStr[2]{\s}{,}{T}{F}\IfSubStr[3]{a,b,c}{,}{T}{F}"), "TFTF");
    assert_eq!(x(r"\def\w{xyz}\IfStrEq{\w}{xyz}{T}{F}\IfStrEq{a}{b}{T}{F}\IfStrEq{1}{01}{T}{F}"), "TFF");
    assert_eq!(x(r"\IfEq{1}{01}{T}{F}\IfEq{2.0}{2}{T}{F}\IfEq{a}{a}{T}{F}\IfEq{a}{b}{T}{F}\IfEq{1}{2}{T}{F}"), "TTTFF");
    assert_eq!(x(r"\IfBeginWith{abc}{ab}{T}{F}\IfBeginWith{abc}{b}{T}{F}\IfEndWith{abc}{bc}{T}{F}\IfEndWith{abc}{b}{T}{F}"), "TFTF");
}
