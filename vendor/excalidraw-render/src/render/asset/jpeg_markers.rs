//! The first image of a JPEG file as the pinned reference reads it: Blink's
//! `JPEGImageDecoder` over libjpeg-turbo. The reader mirrors libjpeg's marker
//! loop (`jdmarker.c`), its color-space choice (`jdapimin.c`), its ICC
//! assembly (`jdicc.c`) and Blink's EXIF header check, and produces the byte
//! stream the strict `zune-jpeg` decoder reads so that it decodes the same
//! image:
//!
//! - bytes after the first image's EOI are not part of it;
//! - non-marker bytes between segments are skipped, as `next_marker` does;
//! - RSTn and TEM outside a scan are parameterless markers libjpeg ignores;
//! - a scan referring to an undefined Huffman table 0 or 1 uses the standard
//!   tables of ITU T.81 K.3 (`jpeg_std_huff_table`), as for MJPEG;
//! - a progressive image whose scans refine every coefficient of every
//!   component to full precision draws completely without its EOI;
//! - the color space follows `default_decompress_parms`.
//!
//! The reference paints a file it cannot decode completely only partially
//! (the last rows of a baseline image missing its EOI, a truncated scan, an
//! incomplete progression, restart markers missing or out of sequence): those
//! remain the project's strict decode failure, the declared divergence (#85).

use std::borrow::Cow;
use std::ops::Range;

use super::ResolveError;

const INVALID: ResolveError = ResolveError::JpegDecodeFailed;
/// libjpeg's `MAX_COMPONENTS`.
const MAX_COMPONENTS: usize = 10;
/// An APP0 `AVI1` segment: zune-jpeg then fills every undefined Huffman
/// table 0 or 1 with the ITU T.81 K.3 tables, as libjpeg-turbo does.
const STANDARD_TABLES: &[u8] = b"\xff\xe0\x00\x07AVI1\0";
/// An Adobe APP14 segment whose transform selects YCCK.
const ADOBE_YCCK: &[u8] = b"\xff\xee\x00\x0eAdobe\x00\x64\x00\x00\x00\x00\x02";

/// libjpeg's `jpeg_color_space` for the first image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JpegModel {
    Gray,
    YCbCr,
    Rgb,
    Cmyk,
    Ycck,
    /// A component count libjpeg names no color space for.
    Unknown,
}

/// The first image of a JPEG file.
pub(super) struct JpegPrimary<'a> {
    /// The stream the decoder reads.
    pub(super) stream: Cow<'a, [u8]>,
    pub(super) model: JpegModel,
    /// The orientation of the first EXIF block, if it holds a valid one.
    pub(super) orientation: Option<u8>,
    /// The first image carries an MPF index or `hdrgm` gain-map metadata; the
    /// whole file is then judged as a gain-map container.
    pub(super) gain_map_container: bool,
    icc: IccParts,
    bytes: &'a [u8],
}

impl JpegPrimary<'_> {
    /// The embedded ICC profile as `jpeg_read_icc_profile` assembles it:
    /// none when the `ICC_PROFILE` segments are absent or their numbering is
    /// inconsistent, incomplete or duplicated, or they carry no data.
    pub(super) fn icc_profile(&self) -> Result<Option<Vec<u8>>, ResolveError> {
        let IccParts::Parts { count, parts } = &self.icc else {
            return Ok(None);
        };
        let parts = parts.get(1..=usize::from(*count)).ok_or(INVALID)?;
        let mut size = 0_usize;
        for part in parts {
            let Some(range) = part else {
                return Ok(None);
            };
            size = size.checked_add(range.len()).ok_or(ResolveError::Memory)?;
        }
        if size == 0 {
            return Ok(None);
        }
        let mut profile = Vec::new();
        profile
            .try_reserve_exact(size)
            .map_err(|_| ResolveError::Memory)?;
        for range in parts.iter().flatten() {
            profile.extend_from_slice(self.bytes.get(range.clone()).ok_or(INVALID)?);
        }
        Ok(Some(profile))
    }
}

#[derive(Default)]
enum IccParts {
    #[default]
    None,
    Invalid,
    Parts {
        count: u8,
        parts: Vec<Option<Range<usize>>>,
    },
}

/// One step of libjpeg's marker loop over the first image.
enum Event {
    /// A marker segment: `payload` excludes the two length bytes.
    Segment { marker: u8, payload: Range<usize> },
    /// Bytes or markers libjpeg skips.
    Skipped,
    /// The entropy-coded data of the scan just read, with its restart
    /// markers: their count, or `None` when one is out of sequence.
    Entropy {
        data: Range<usize>,
        restarts: Option<u64>,
    },
}

/// Reads the first image of `bytes`.
pub(super) fn read_primary(bytes: &[u8]) -> Result<JpegPrimary<'_>, ResolveError> {
    let mut reader = Reader {
        zune: Some(ZuneSpace::YCbCr),
        ..Reader::default()
    };
    let end = walk(bytes, |event| reader.visit(bytes, event))?;
    let frame = reader.frame.as_ref().ok_or(INVALID)?;
    if !reader.saw_scan {
        return Err(INVALID);
    }
    let complete = match end {
        End::Eoi(_) => true,
        // Only a complete progression survives a missing EOI: the reference
        // paints the last rows of a sequential image only after its EOI.
        End::Truncated => frame.progressive && reader.progression_complete(),
    };
    if !complete {
        return Err(INVALID);
    }
    let model = reader.libjpeg_model();
    let recolor = model != JpegModel::Unknown && reader.zune_model() != Some(model);
    let stream = match end {
        End::Eoi(eoi_end) if !reader.edited && !reader.needs_standard_tables && !recolor => {
            Cow::Borrowed(bytes.get(..eoi_end).ok_or(INVALID)?)
        }
        _ => Cow::Owned(reader.rewrite(bytes, model, recolor)?),
    };
    Ok(JpegPrimary {
        stream,
        model,
        orientation: reader.orientation.flatten(),
        gain_map_container: reader.mpf || reader.hdrgm,
        icc: reader.icc,
        bytes,
    })
}

enum End {
    /// The offset just past the first image's EOI.
    Eoi(usize),
    /// The file ended within the entropy-coded data of a scan.
    Truncated,
}

/// Runs libjpeg's marker loop over the first image of `bytes`, calling
/// `visit` for each step. The end of the file within a header is a failure.
fn walk(
    bytes: &[u8],
    mut visit: impl FnMut(Event) -> Result<(), ResolveError>,
) -> Result<End, ResolveError> {
    // first_marker: the file starts with SOI exactly.
    if bytes.get(..2) != Some(b"\xff\xd8") {
        return Err(INVALID);
    }
    let mut offset = 2_usize;
    // A marker the entropy decoder has already read: (marker, offset after).
    let mut pending: Option<(u8, usize)> = None;
    let mut saw_scan = false;
    loop {
        let (marker, after) = if let Some(pending) = pending.take() {
            pending
        } else {
            let (marker, after, skipped) = next_marker(bytes, offset)?;
            if skipped {
                visit(Event::Skipped)?;
            }
            (marker, after)
        };
        offset = after;
        match marker {
            0xd9 => {
                // EOI before any scan is libjpeg's JERR_NO_IMAGE.
                return if saw_scan {
                    Ok(End::Eoi(offset))
                } else {
                    Err(INVALID)
                };
            }
            // A second SOI is JERR_SOI_DUPLICATE.
            0xd8 => return Err(INVALID),
            // Parameterless markers read_markers ignores.
            0x01 | 0xd0..=0xd7 => visit(Event::Skipped)?,
            _ => {
                let length_end = offset.checked_add(2).ok_or(INVALID)?;
                let length = bytes.get(offset..length_end).ok_or(INVALID)?;
                let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
                if length < 2 {
                    return Err(INVALID);
                }
                let end = offset.checked_add(length).ok_or(INVALID)?;
                if end > bytes.len() {
                    return Err(INVALID);
                }
                visit(Event::Segment {
                    marker,
                    payload: length_end..end,
                })?;
                offset = end;
                if marker != 0xda {
                    continue;
                }
                saw_scan = true;
                let scan = entropy_data(bytes, offset);
                visit(Event::Entropy {
                    data: offset..scan.end,
                    restarts: scan.restarts,
                })?;
                match scan.next {
                    Some(next) => pending = Some(next),
                    None => return Ok(End::Truncated),
                }
            }
        }
    }
}

/// libjpeg's `next_marker`: skips non-0xFF bytes, fill bytes and stuffed
/// zeros up to the next marker. Returns the marker, the offset after it and
/// whether any byte other than fill bytes was skipped.
fn next_marker(bytes: &[u8], mut offset: usize) -> Result<(u8, usize, bool), ResolveError> {
    let mut skipped = false;
    loop {
        while *bytes.get(offset).ok_or(INVALID)? != 0xff {
            skipped = true;
            offset += 1;
        }
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes.get(offset).ok_or(INVALID)?;
        offset += 1;
        if marker != 0 {
            return Ok((marker, offset, skipped));
        }
        // FF/00 outside a scan is discarded as extraneous data.
        skipped = true;
    }
}

struct ScanData {
    /// The end of the entropy-coded data: the first fill byte of the marker
    /// that ends it, or the end of the file.
    end: usize,
    restarts: Option<u64>,
    /// The marker that ends the scan and the offset after it.
    next: Option<(u8, usize)>,
}

/// The entropy-coded data from `start`: stuffed zeros and RSTn belong to the
/// scan, as libjpeg's entropy decoder reads them.
fn entropy_data(bytes: &[u8], start: usize) -> ScanData {
    let mut offset = start;
    let mut restarts = Some(0_u64);
    loop {
        let Some(ff) = bytes
            .get(offset..)
            .and_then(|rest| rest.iter().position(|&b| b == 0xff))
            .map(|at| offset + at)
        else {
            return ScanData {
                end: bytes.len(),
                restarts,
                next: None,
            };
        };
        let mut at = ff + 1;
        while bytes.get(at) == Some(&0xff) {
            at += 1;
        }
        match bytes.get(at).copied() {
            None => {
                return ScanData {
                    end: ff,
                    restarts,
                    next: None,
                }
            }
            Some(0) => {}
            Some(marker @ 0xd0..=0xd7) => {
                restarts = restarts
                    .and_then(|count| (u64::from(marker - 0xd0) == count % 8).then_some(count + 1));
            }
            Some(marker) => {
                return ScanData {
                    end: ff,
                    restarts,
                    next: Some((marker, at + 1)),
                }
            }
        }
        offset = at + 1;
    }
}

#[derive(Clone, Copy, Default)]
struct Component {
    id: u8,
    h: u8,
    v: u8,
    quantization: u8,
}

struct Frame {
    progressive: bool,
    width: u16,
    height: u16,
    components: [Component; MAX_COMPONENTS],
    count: usize,
}

impl Frame {
    fn components(&self) -> &[Component] {
        &self.components[..self.count]
    }
}

/// What libjpeg learns from the first image's markers.
#[derive(Default)]
struct Reader {
    frame: Option<Frame>,
    saw_scan: bool,
    restart_interval: u16,
    /// Huffman tables defined so far: DC then AC, ids 0-3.
    tables: [[bool; 4]; 2],
    needs_standard_tables: bool,
    /// Quantization tables defined so far, ids 0-3.
    quantization: [bool; 4],
    /// Components whose quantization table a scan has latched.
    latched: [bool; MAX_COMPONENTS],
    /// libjpeg's `coef_bits` per component and coefficient: the successive
    /// approximation bit position known, or -1 before any scan.
    coef_bits: Option<Vec<[i8; 64]>>,
    /// The restart markers the scan just read must hold.
    expected_restarts: u64,
    saw_jfif: bool,
    adobe_transform: Option<u8>,
    /// zune-jpeg's reading of the same header: its color space, or `None`
    /// where it refuses an Adobe segment libjpeg accepts or ignores.
    zune: Option<ZuneSpace>,
    icc: IccParts,
    orientation: Option<Option<u8>>,
    mpf: bool,
    hdrgm: bool,
    /// Bytes or markers were skipped: the stream must be rewritten.
    edited: bool,
    /// The frame's three component ids are 'R', 'G', 'B'.
    rgb_ids: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ZuneSpace {
    Luma,
    YCbCr,
    Rgb,
    Cmyk,
    Ycck,
}

impl Reader {
    fn visit(&mut self, bytes: &[u8], event: Event) -> Result<(), ResolveError> {
        match event {
            Event::Skipped => self.edited = true,
            Event::Entropy { restarts, .. } => {
                // Missing, extra or out-of-sequence restart markers leave the
                // reference resynchronizing: it paints garbled rows.
                if restarts != Some(self.expected_restarts) {
                    return Err(INVALID);
                }
            }
            Event::Segment { marker, payload } => {
                let segment = bytes.get(payload.clone()).ok_or(INVALID)?;
                self.segment(marker, segment, payload.start)?;
            }
        }
        Ok(())
    }

    fn segment(&mut self, marker: u8, segment: &[u8], start: usize) -> Result<(), ResolveError> {
        match marker {
            0xc0..=0xc2 => self.start_of_frame(marker == 0xc2, segment)?,
            // Lossless, hierarchical and arithmetic frames are unsupported.
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => return Err(INVALID),
            0xc4 => self.huffman_tables(segment)?,
            0xdb => self.quantization_tables(segment)?,
            0xdd => {
                // get_dri: JERR_BAD_LENGTH unless exactly two bytes.
                let [high, low] = segment else {
                    return Err(INVALID);
                };
                self.restart_interval = u16::from_be_bytes([*high, *low]);
            }
            0xda => self.start_of_scan(segment)?,
            0xe0 if !self.saw_scan && segment.len() >= 14 && segment.starts_with(b"JFIF\0") => {
                self.saw_jfif = true;
            }
            0xe1 => {
                if segment
                    .windows(b"hdrgm:Version".len())
                    .any(|window| window == b"hdrgm:Version")
                {
                    self.hdrgm = true;
                }
                // Blink reads the first EXIF block: "Exif\0" and a fill byte.
                if !self.saw_scan
                    && self.orientation.is_none()
                    && segment.len() >= 6
                    && segment.starts_with(b"Exif\0")
                    && matches!(segment[5], 0x00 | 0xff)
                {
                    self.orientation = Some(super::jpeg::jpeg_tiff_orientation(&segment[6..]));
                }
            }
            0xe2 => {
                if segment.starts_with(b"MPF\0") {
                    self.mpf = true;
                }
                if !self.saw_scan && segment.len() >= 14 && segment.starts_with(b"ICC_PROFILE\0") {
                    self.icc_part(segment[12], segment[13], start + 14..start + segment.len());
                }
            }
            // zune-jpeg reads any segment starting "Adobe" as the Adobe
            // marker; libjpeg only one of at least twelve bytes.
            0xee if !self.saw_scan && segment.starts_with(b"Adobe") => {
                let transform = segment.get(11).copied();
                if let Some(transform) = transform {
                    self.adobe_transform = Some(transform);
                }
                self.zune = match transform {
                    Some(0) => self.zune.map(|_| ZuneSpace::Cmyk),
                    Some(1) => self.zune.map(|_| ZuneSpace::YCbCr),
                    Some(2) => self.zune.map(|_| ZuneSpace::Ycck),
                    _ => None,
                };
            }
            _ => {}
        }
        Ok(())
    }

    /// jdicc.c: the numbering must be consistent, within the count and
    /// unique; otherwise the profile is ignored.
    fn icc_part(&mut self, sequence: u8, count: u8, data: Range<usize>) {
        let valid = match &self.icc {
            IccParts::Invalid => false,
            IccParts::None => sequence != 0 && sequence <= count,
            IccParts::Parts {
                count: expected,
                parts,
            } => {
                *expected == count
                    && sequence != 0
                    && sequence <= count
                    && parts[usize::from(sequence)].is_none()
            }
        };
        if !valid {
            self.icc = IccParts::Invalid;
            return;
        }
        if matches!(self.icc, IccParts::None) {
            self.icc = IccParts::Parts {
                count,
                parts: vec![None; usize::from(count) + 1],
            };
        }
        if let IccParts::Parts { parts, .. } = &mut self.icc {
            parts[usize::from(sequence)] = Some(data);
        }
    }

    fn start_of_frame(&mut self, progressive: bool, segment: &[u8]) -> Result<(), ResolveError> {
        // JERR_SOF_DUPLICATE; the reference decodes eight-bit samples only.
        if self.frame.is_some() || segment.len() < 6 || segment[0] != 8 {
            return Err(INVALID);
        }
        let height = u16::from_be_bytes([segment[1], segment[2]]);
        let width = u16::from_be_bytes([segment[3], segment[4]]);
        let count = usize::from(segment[5]);
        // JERR_EMPTY_IMAGE, JERR_COMPONENT_COUNT and JERR_BAD_LENGTH.
        if width == 0
            || height == 0
            || count == 0
            || count > MAX_COMPONENTS
            || segment.len() != 6 + 3 * count
        {
            return Err(INVALID);
        }
        let mut components = [Component::default(); MAX_COMPONENTS];
        for (index, component) in components[..count].iter_mut().enumerate() {
            let at = 6 + 3 * index;
            let sampling = segment[at + 1];
            *component = Component {
                id: segment[at],
                h: sampling >> 4,
                v: sampling & 15,
                quantization: segment[at + 2],
            };
            // JERR_BAD_SAMPLING.
            if !(1..=4).contains(&component.h) || !(1..=4).contains(&component.v) {
                return Err(INVALID);
            }
        }
        self.rgb_ids = count == 3 && components[..3].iter().map(|c| c.id).eq(*b"RGB");
        // zune-jpeg's frame rules: one component is luma; four default to CMYK.
        self.zune = self.zune.map(|space| match (count, space) {
            (1, _) => ZuneSpace::Luma,
            (4, ZuneSpace::YCbCr) => ZuneSpace::Cmyk,
            (_, space) => space,
        });
        self.frame = Some(Frame {
            progressive,
            width,
            height,
            components,
            count,
        });
        Ok(())
    }

    fn huffman_tables(&mut self, mut segment: &[u8]) -> Result<(), ResolveError> {
        // get_dht: JERR_BAD_HUFF_TABLE, JERR_DHT_INDEX and JERR_BAD_LENGTH.
        while !segment.is_empty() {
            let (&index, rest) = segment.split_first().ok_or(INVALID)?;
            let counts = rest.get(..16).ok_or(INVALID)?;
            let values: usize = counts.iter().map(|&count| usize::from(count)).sum();
            let class = usize::from(index >> 4);
            let id = usize::from(index & 15);
            if values > 256 || class > 1 || id > 3 {
                return Err(INVALID);
            }
            segment = rest.get(16 + values..).ok_or(INVALID)?;
            self.tables[class][id] = true;
        }
        Ok(())
    }

    fn quantization_tables(&mut self, mut segment: &[u8]) -> Result<(), ResolveError> {
        // get_dqt: JERR_DQT_INDEX and JERR_BAD_LENGTH; any nonzero precision
        // reads sixteen-bit values.
        while let Some((&index, rest)) = segment.split_first() {
            let size = if index >> 4 == 0 { 64 } else { 128 };
            let id = usize::from(index & 15);
            segment = rest.get(size..).ok_or(INVALID)?;
            *self.quantization.get_mut(id).ok_or(INVALID)? = true;
        }
        Ok(())
    }

    fn start_of_scan(&mut self, segment: &[u8]) -> Result<(), ResolveError> {
        // JERR_SOF_BEFORE.
        let frame = self.frame.as_ref().ok_or(INVALID)?;
        let count = usize::from(*segment.first().ok_or(INVALID)?);
        // get_sos: JERR_BAD_LENGTH.
        if count == 0 || count > 4 || segment.len() != 4 + 2 * count {
            return Err(INVALID);
        }
        let spectral = &segment[1 + 2 * count..];
        let (start, end) = (spectral[0], spectral[1]);
        let (high, low) = (spectral[2] >> 4, spectral[2] & 15);
        let mut members = [0_usize; 4];
        for index in 0..count {
            let id = segment[1 + 2 * index];
            let tables = segment[2 + 2 * index];
            // JERR_BAD_COMPONENT_ID, including a component named twice.
            let member = frame
                .components()
                .iter()
                .position(|c| c.id == id)
                .ok_or(INVALID)?;
            if members[..index].contains(&member) {
                return Err(INVALID);
            }
            members[index] = member;
            // latch_quant_tables: JERR_NO_QUANT_TABLE when a component's first
            // scan starts without its table.
            if !self.latched[member] {
                let table = usize::from(frame.components[member].quantization);
                if !self.quantization.get(table).copied().ok_or(INVALID)? {
                    return Err(INVALID);
                }
                self.latched[member] = true;
            }
            let (dc, ac) = (usize::from(tables >> 4), usize::from(tables & 15));
            let needs_dc = !frame.progressive || (start == 0 && high == 0);
            let needs_ac = !frame.progressive || start > 0;
            for (class, id, needed) in [(0, dc, needs_dc), (1, ac, needs_ac)] {
                if !needed {
                    continue;
                }
                // jpeg_make_d_derived_tbl: an undefined table 0 or 1 is the
                // standard table; any other is JERR_NO_HUFF_TABLE.
                if !self.tables[class].get(id).copied().ok_or(INVALID)? {
                    if id > 1 {
                        return Err(INVALID);
                    }
                    self.needs_standard_tables = true;
                }
            }
        }
        let members = &members[..count];
        if frame.progressive {
            // jdphuff.c: JERR_BAD_PROGRESSION.
            if end > 63
                || start > end
                || (start == 0 && end != 0)
                || (start > 0 && count != 1)
                || (high != 0 && low + 1 != high)
                || low > 13
            {
                return Err(INVALID);
            }
            let coef_bits = self
                .coef_bits
                .get_or_insert_with(|| vec![[-1; 64]; frame.count]);
            for &member in members {
                for bits in &mut coef_bits[member][usize::from(start)..=usize::from(end)] {
                    *bits = low as i8;
                }
            }
        }
        self.expected_restarts = expected_restarts(frame, members, self.restart_interval);
        if !self.saw_scan {
            self.saw_scan = true;
            // zune-jpeg names components 'R', 'G', 'B' RGB, then resolves a
            // three-component CMYK tag to RGB and YCCK to YCbCr.
            self.zune = self.zune.and_then(|space| match (frame.count, space) {
                (3, _) if self.rgb_ids => Some(ZuneSpace::Rgb),
                (3, ZuneSpace::Cmyk) => Some(ZuneSpace::Rgb),
                (3, ZuneSpace::Ycck) => Some(ZuneSpace::YCbCr),
                (count, ZuneSpace::YCbCr | ZuneSpace::Rgb) if count < 3 => None,
                (count, ZuneSpace::Cmyk | ZuneSpace::Ycck) if count < 4 => None,
                (_, space) => Some(space),
            });
        }
        Ok(())
    }

    fn progression_complete(&self) -> bool {
        self.coef_bits
            .as_ref()
            .is_some_and(|bits| bits.iter().flatten().all(|&bits| bits == 0))
    }

    /// jdapimin.c `default_decompress_parms`: JFIF wins over the Adobe
    /// transform, which wins over the component ids.
    fn libjpeg_model(&self) -> JpegModel {
        let Some(frame) = &self.frame else {
            return JpegModel::Unknown;
        };
        match frame.count {
            1 => JpegModel::Gray,
            3 if self.saw_jfif => JpegModel::YCbCr,
            3 => match self.adobe_transform {
                Some(0) => JpegModel::Rgb,
                Some(_) => JpegModel::YCbCr,
                None if self.rgb_ids => JpegModel::Rgb,
                None => JpegModel::YCbCr,
            },
            4 => match self.adobe_transform {
                None | Some(0) => JpegModel::Cmyk,
                Some(_) => JpegModel::Ycck,
            },
            _ => JpegModel::Unknown,
        }
    }

    fn zune_model(&self) -> Option<JpegModel> {
        Some(match self.zune? {
            ZuneSpace::Luma => JpegModel::Gray,
            ZuneSpace::YCbCr => JpegModel::YCbCr,
            ZuneSpace::Rgb => JpegModel::Rgb,
            ZuneSpace::Cmyk => JpegModel::Cmyk,
            ZuneSpace::Ycck => JpegModel::Ycck,
        })
    }

    /// The first image rewritten so zune-jpeg reads what libjpeg reads.
    fn rewrite(
        &self,
        bytes: &[u8],
        model: JpegModel,
        recolor: bool,
    ) -> Result<Vec<u8>, ResolveError> {
        let mut out = Vec::new();
        out.try_reserve_exact(bytes.len().saturating_add(32))
            .map_err(|_| ResolveError::Memory)?;
        out.extend_from_slice(b"\xff\xd8");
        if self.needs_standard_tables {
            out.extend_from_slice(STANDARD_TABLES);
        }
        if recolor && model == JpegModel::Ycck {
            out.extend_from_slice(ADOBE_YCCK);
        }
        // Three components take the ids zune-jpeg reads as the model.
        let ids: Option<[u8; 3]> = match (recolor, model) {
            (true, JpegModel::Rgb) => Some(*b"RGB"),
            (true, JpegModel::YCbCr) => Some([1, 2, 3]),
            _ => None,
        };
        let frame_ids = self.frame.as_ref().map(|frame| {
            let mut ids = [0_u8; MAX_COMPONENTS];
            for (id, component) in ids.iter_mut().zip(frame.components()) {
                *id = component.id;
            }
            ids
        });
        let rename = |id: u8| -> u8 {
            match (ids, frame_ids) {
                (Some(ids), Some(frame_ids)) => frame_ids[..3]
                    .iter()
                    .position(|&frame_id| frame_id == id)
                    .map_or(id, |index| ids[index]),
                _ => id,
            }
        };
        let mut saw_scan = false;
        walk(bytes, |event| {
            match event {
                Event::Skipped => {}
                Event::Entropy { data, .. } => {
                    out.extend_from_slice(bytes.get(data).ok_or(INVALID)?);
                }
                Event::Segment { marker, payload } => {
                    let segment = bytes.get(payload.clone()).ok_or(INVALID)?;
                    if recolor && !saw_scan && marker == 0xee && segment.starts_with(b"Adobe") {
                        return Ok(());
                    }
                    out.extend_from_slice(&[0xff, marker]);
                    out.extend_from_slice(
                        bytes.get(payload.start - 2..payload.start).ok_or(INVALID)?,
                    );
                    let start = out.len();
                    out.extend_from_slice(segment);
                    if ids.is_some() {
                        let written = &mut out[start..];
                        if matches!(marker, 0xc0..=0xc2) {
                            for index in 0..usize::from(written[5]) {
                                written[6 + 3 * index] = rename(written[6 + 3 * index]);
                            }
                        } else if marker == 0xda {
                            for index in 0..usize::from(written[0]) {
                                written[1 + 2 * index] = rename(written[1 + 2 * index]);
                            }
                        }
                    }
                    if marker == 0xda {
                        saw_scan = true;
                    }
                }
            }
            Ok(())
        })?;
        out.extend_from_slice(b"\xff\xd9");
        Ok(out)
    }
}

/// The restart markers a scan of `members` holds: one before each restart
/// interval but the first (libjpeg `per_scan_setup` MCU counts).
fn expected_restarts(frame: &Frame, members: &[usize], interval: u16) -> u64 {
    if interval == 0 {
        return 0;
    }
    let components = frame.components();
    let h_max = u64::from(components.iter().map(|c| c.h).max().unwrap_or(1));
    let v_max = u64::from(components.iter().map(|c| c.v).max().unwrap_or(1));
    let (width, height) = (u64::from(frame.width), u64::from(frame.height));
    let mcus = if let [single] = members {
        let component = components[*single];
        (width * u64::from(component.h)).div_ceil(h_max * 8)
            * (height * u64::from(component.v)).div_ceil(v_max * 8)
    } else {
        width.div_ceil(h_max * 8) * height.div_ceil(v_max * 8)
    };
    mcus.div_ceil(u64::from(interval)).saturating_sub(1)
}
