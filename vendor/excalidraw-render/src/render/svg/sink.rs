//! The writer the SVG plan targets twice: `SvgSink` is the byte-appending
//! surface `write` spells the document onto, `String` is the real artifact
//! buffer, and `CountingWriter` is the dry run `measure` uses so the exact
//! output size is known before the buffer is reserved.

pub(crate) trait SvgSink: std::fmt::Write {
    fn push_str(&mut self, value: &str);

    fn push(&mut self, value: char);
}

impl SvgSink for String {
    fn push_str(&mut self, value: &str) {
        String::push_str(self, value);
    }

    fn push(&mut self, value: char) {
        String::push(self, value);
    }
}

impl<T: SvgSink + ?Sized> SvgSink for &mut T {
    fn push_str(&mut self, value: &str) {
        T::push_str(self, value);
    }

    fn push(&mut self, value: char) {
        T::push(self, value);
    }
}

#[derive(Default)]
pub(super) struct CountingWriter {
    pub(super) len: usize,
    pub(super) overflowed: bool,
}

impl CountingWriter {
    fn add(&mut self, bytes: usize) {
        if let Some(len) = self.len.checked_add(bytes) {
            self.len = len;
        } else {
            self.overflowed = true;
        }
    }
}

impl std::fmt::Write for CountingWriter {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.add(value.len());
        Ok(())
    }
}

impl SvgSink for CountingWriter {
    fn push_str(&mut self, value: &str) {
        self.add(value.len());
    }

    fn push(&mut self, value: char) {
        self.add(value.len_utf8());
    }
}
