use crate::{error::Result, ir::Page};

pub(crate) struct ConvertOptions {
    pub max_input_bytes: u64,
    pub max_zip_entry_bytes: u64,
    pub max_xml_events: usize,
    pub max_pages: usize,
    pub embed_drawio_source: bool,
}

pub(crate) trait PageConsumer {
    fn consume(&mut self, page: Page) -> Result<()>;
}
