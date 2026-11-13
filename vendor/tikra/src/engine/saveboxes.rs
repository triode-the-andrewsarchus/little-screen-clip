//! LaTeX's boxes: `\newsavebox{\name}`, `\savebox{\name}{…}` (or `\sbox`, or the `lrbox`
//! environment) typeset what they hold once, as a line of its own; `\usebox{\name}` puts it
//! where it is used: among the pictures of the document, or as the text of a node.

use crate::canvas::Canvas;
use crate::scene::Point;
use crate::tex::{self, Cat, Token};
use crate::Error;

use super::{Engine, Layout, PictureBox};

impl Engine {
    /// `\newsavebox{\name}`: a box register, empty until something is saved in it.
    pub fn new_save_box(&mut self) -> Result<(), Error> {
        let arg = self.argument()?;
        let Some(name) = box_name(&arg) else {
            self.warn("\\newsavebox: no box name".to_string());
            return Ok(());
        };
        let n = self.saved_boxes.len() as i64;
        self.tex.set_meaning(&Token::cs(&name), tex::Meaning::CharDef(n), true);
        self.saved_boxes.insert(name, PictureBox { canvas: Canvas::default(), baseline: 0.0 });
        Ok(())
    }

    /// `\savebox{\name}[width][position]{…}`, `\sbox{\name}{…}`: what the text typesets, kept.
    pub fn save_box(&mut self, savebox: bool) -> Result<(), Error> {
        let name = self.argument()?;
        if savebox && self.optional()?.is_some() {
            // the box of a given width: kept at its natural width
            self.warn("\\savebox: the width is ignored".to_string());
            let _ = self.optional()?;
        }
        let content = self.argument()?;
        self.keep_box(&name, content)
    }

    /// `\begin{lrbox}{\name}…\end{lrbox}`: what the body typesets, kept.
    pub fn lrbox(&mut self) -> Result<(), Error> {
        let name = self.argument()?;
        let mut body = Vec::new();
        while let Some(t) = self.tex.get_token() {
            if t.is_cs("end") {
                let arg = self.tex.read_undelimited()?;
                if tex::tokens_to_string(&arg).trim() == "lrbox" {
                    break;
                }
                body.push(t);
                body.push(Token::Char('{', Cat::Begin));
                body.extend(arg);
                body.push(Token::Char('}', Cat::End));
            } else {
                body.push(t);
            }
        }
        self.keep_box(&name, body)
    }

    /// `\usebox{\name}`: the box, set where the text is (in a picture, only a node's text shows it).
    pub fn use_box(&mut self) -> Result<(), Error> {
        let name = self.argument()?;
        let Some(b) = self.saved_box(&name) else { return Ok(()) };
        if self.picture_depth == 0 {
            self.layout.push(Layout::Picture(self.boxes.len()));
            self.boxes.push(b);
        }
        Ok(())
    }

    /// `\PreviewEnvironment{tikzpicture}` of the active preview package: each picture is a page of
    /// its own, as it is, whatever box it is in.
    pub fn preview_environment(&mut self) -> Result<(), Error> {
        if self.argument_string()? == "tikzpicture" {
            let m = self.tex.meaning_of("tikra@unboxed");
            for name in ["scalebox", "resizebox", "fbox"] {
                self.tex.set_meaning(&Token::cs(name), m.clone(), false);
            }
        }
        Ok(())
    }

    /// `\scalebox{factor}[factor]{…}`, `\resizebox*{width}{height}{…}` or `\fbox{…}` whose
    /// pictures the preview package takes out (`\PreviewEnvironment{tikzpicture}`): what is in it,
    /// as it is.
    pub fn unboxed(&mut self, t: &Token) -> Result<(), Error> {
        if t.is_cs("scalebox") {
            let _ = self.argument()?;
            let _ = self.optional()?;
        } else if t.is_cs("resizebox") {
            if let Some(t) = self.tex.next_nonspace().filter(|t| !t.is_char('*')) {
                self.tex.back(t);
            }
            let _ = self.argument()?;
            let _ = self.argument()?;
        }
        let content = self.argument()?;
        self.begin_group();
        let r = self.run_tokens(content);
        self.end_group();
        r
    }

    /// A node's text that is all `\usebox{\name}`: the box.
    pub(super) fn text_box(&mut self, text: &str) -> Option<PictureBox> {
        let rest = text.trim().strip_prefix("\\usebox")?.trim();
        let name = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(rest).trim();
        let cs = name.strip_prefix('\\').filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphabetic() || c == '@'))?;
        self.saved_box(&[Token::cs(cs)])
    }

    /// The box the name (a control sequence) holds, or none with a warning.
    fn saved_box(&mut self, name: &[Token]) -> Option<PictureBox> {
        let found = box_name(name);
        match found.as_ref().and_then(|n| self.saved_boxes.get(n)) {
            Some(b) => Some(b.clone()),
            None => {
                self.warn(format!("\\usebox: no box '\\{}'", found.unwrap_or_default()));
                None
            }
        }
    }

    /// What `content` typesets, kept as the box `name`: a lone picture as it is (with what it
    /// draws outside its box, `overlay`), anything else as one line from its baseline.
    fn keep_box(&mut self, name: &[Token], content: Vec<Token>) -> Result<(), Error> {
        let Some(n) = box_name(name) else {
            self.warn("\\savebox: no box name".to_string());
            return Ok(());
        };
        let (mut boxes, layout, words) = self.run_apart(content)?;
        let lone = layout.iter().filter(|l| !matches!(l, Layout::Space)).eq([&Layout::Picture(0)]) && boxes.len() == 1;
        let b = match boxes.pop() {
            Some(mut b) if lone => {
                // a picture of no size is a box of none at its origin
                if b.canvas.min.is_none() {
                    (b.canvas.min, b.canvas.max) = (Some(Point::new(0.0, 0.0)), Some(Point::new(0.0, 0.0)));
                }
                b
            }
            last => {
                boxes.extend(last);
                PictureBox { canvas: super::typeset(&boxes, &layout, &words, 0.0, None), baseline: 0.0 }
            }
        };
        self.saved_boxes.insert(n, b);
        Ok(())
    }
}

/// The name of a box: the control sequence in the argument.
fn box_name(arg: &[Token]) -> Option<String> {
    arg.iter().find_map(|t| if let Token::Cs(n) = t { Some(n.to_string()) } else { None })
}
