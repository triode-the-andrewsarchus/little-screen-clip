use super::{Compression, Error};
use std::{
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};

const BUFFER_BYTES: usize = 32 * 1024;
const MAX_ZSTD_WINDOW: u32 = 27;

type Gzip<R> = BufReader<flate2::bufread::MultiGzDecoder<R>>;
type Zstd<R> = BufReader<zstd::stream::read::Decoder<'static, R>>;
type Lz4<R> = BufReader<lz4_flex::frame::FrameDecoder<R>>;

enum Stream<R: BufRead> {
    Plain(R),
    Gzip(Gzip<R>),
    Zstd(Zstd<R>),
    Lz4(Lz4<R>),
}

impl<R: BufRead> Stream<R> {
    fn new(input: R, compression: Compression) -> io::Result<Self> {
        match compression {
            Compression::Plain => Ok(Self::Plain(input)),
            Compression::Gzip => Ok(Self::Gzip(BufReader::with_capacity(
                BUFFER_BYTES,
                flate2::bufread::MultiGzDecoder::new(input),
            ))),
            Compression::Zstd => {
                let mut decoder = zstd::stream::read::Decoder::with_buffer(input)?;
                decoder.window_log_max(MAX_ZSTD_WINDOW)?;
                Ok(Self::Zstd(BufReader::with_capacity(BUFFER_BYTES, decoder)))
            }
            Compression::Lz4 => Ok(Self::Lz4(BufReader::with_capacity(
                BUFFER_BYTES,
                lz4_flex::frame::FrameDecoder::new(input),
            ))),
        }
    }

    fn buffered(&mut self) -> &mut dyn BufRead {
        match self {
            Self::Plain(reader) => reader,
            Self::Gzip(reader) => reader,
            Self::Zstd(reader) => reader,
            Self::Lz4(reader) => reader,
        }
    }

    fn into_input(self) -> R {
        match self {
            Self::Plain(input) => input,
            Self::Gzip(input) => input.into_inner().into_inner(),
            Self::Zstd(input) => input.into_inner().finish(),
            Self::Lz4(input) => input.into_inner().into_inner(),
        }
    }
}

pub(super) struct Source<R: BufRead> {
    stream: Option<Stream<R>>,
    pub compression: Compression,
    pub position: u64,
    length: Option<u64>,
    pub limit: Option<u64>,
}

impl<R: BufRead + Seek> Source<R> {
    pub fn open(mut input: R) -> Result<Self, Error> {
        let length = input.seek(SeekFrom::End(0))?;
        input.seek(SeekFrom::Start(0))?;
        let sig = if let Some(sig) = input.fill_buf()?.first_chunk::<4>() {
            *sig
        } else {
            let mut sig = [0; 4];
            input.read_exact(&mut sig).map_err(short_read)?;
            input.seek(SeekFrom::Start(0))?;
            sig
        };
        let compression = match sig {
            [0x1f, 0x8b, ..] => Compression::Gzip,
            [0x28, 0xb5, 0x2f, 0xfd] => Compression::Zstd,
            [0x04, 0x22, 0x4d, 0x18] => Compression::Lz4,
            _ => Compression::Plain,
        };
        let length = (compression == Compression::Plain).then_some(length);
        Ok(Self {
            stream: Some(Stream::new(input, compression)?),
            compression,
            position: 0,
            length,
            limit: None,
        })
    }

    pub fn read<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut bytes = [0; N];
        self.fill(&mut bytes)?;
        Ok(bytes)
    }

    pub fn read_optional<const N: usize>(&mut self) -> Result<Option<[u8; N]>, Error> {
        if self.limit.is_some_and(|limit| self.position >= limit) {
            return Err(Error::PrefixLimit);
        }
        if self.stream()?.buffered().fill_buf()?.is_empty() {
            return Ok(None);
        }
        self.read().map(Some)
    }

    pub fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        let end = self.checked_end(bytes.len() as u64)?;
        self.stream()?
            .buffered()
            .read_exact(bytes)
            .map_err(short_read)?;
        self.position = end;
        Ok(())
    }

    pub fn skip(&mut self, bytes: u64, cancel: &AtomicBool) -> Result<(), Error> {
        let end = self.checked_end(bytes)?;
        if bytes == 0 {
            return Ok(());
        }
        if let Stream::Plain(input) = self.stream()? {
            let buffered = input.fill_buf()?.len();
            if bytes <= buffered as u64 {
                input.consume(bytes as usize);
            } else {
                input.seek(SeekFrom::Start(end))?;
            }
            self.position = end;
            return Ok(());
        }
        while self.position < end {
            cancelled(cancel)?;
            let remaining = end - self.position;
            let input = self.stream()?.buffered();
            let buffer = input.fill_buf()?;
            if buffer.is_empty() {
                return Err(Error::Truncated);
            }
            let amount = remaining.min(buffer.len() as u64) as usize;
            input.consume(amount);
            self.position += amount as u64;
        }
        Ok(())
    }

    pub fn seek(&mut self, offset: u64, cancel: &AtomicBool) -> Result<(), Error> {
        cancelled(cancel)?;
        if let Stream::Plain(input) = self.stream()? {
            input.seek(SeekFrom::Start(offset))?;
            self.position = offset;
            return Ok(());
        }
        if offset < self.position {
            let mut input = self.stream.take().ok_or(Error::Truncated)?.into_input();
            input.seek(SeekFrom::Start(0))?;
            self.stream = Some(Stream::new(input, self.compression)?);
            self.position = 0;
        }
        self.skip(offset - self.position, cancel)
    }

    fn checked_end(&self, amount: u64) -> Result<u64, Error> {
        let end = self.position.checked_add(amount).ok_or(Error::Truncated)?;
        if self.limit.is_some_and(|limit| end > limit) {
            return Err(Error::PrefixLimit);
        }
        if self.length.is_some_and(|length| end > length) {
            return Err(Error::Truncated);
        }
        Ok(end)
    }

    fn stream(&mut self) -> Result<&mut Stream<R>, Error> {
        self.stream.as_mut().ok_or(Error::Truncated)
    }
}

pub(super) fn short_read(error: io::Error) -> Error {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        Error::Truncated
    } else {
        Error::Io(error)
    }
}

pub(super) fn cancelled(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
