use super::{
    BufReader, BufWriter, CHECKSUM_BYTES, Digest, Error, File, Path, Read, Result, Sha256, Write,
};

pub(super) struct IndexWriter {
    writer: BufWriter<File>,
    digest: Sha256,
    written: u64,
    limit: u64,
}

impl IndexWriter {
    pub(super) fn new(file: File, limit: u64) -> Self {
        Self {
            writer: BufWriter::new(file),
            digest: Sha256::new(),
            written: 0,
            limit,
        }
    }

    pub(super) fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.written = self
            .written
            .checked_add(length)
            .ok_or_else(|| Error::index("<writer>", "serialized byte count overflow"))?;
        if self.written.saturating_add(CHECKSUM_BYTES) > self.limit {
            return Err(Error::index(
                "<writer>",
                format!("serialized index exceeds {} bytes", self.limit),
            ));
        }
        self.writer
            .write_all(bytes)
            .map_err(|source| Error::io("<writer>", source))?;
        self.digest.update(bytes);
        Ok(())
    }

    pub(super) fn u8(&mut self, value: u8) -> Result<()> {
        self.bytes(&[value])
    }

    pub(super) fn u16(&mut self, value: u16) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    pub(super) fn u32(&mut self, value: u32) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    pub(super) fn u64(&mut self, value: u64) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    pub(super) fn length_prefixed(&mut self, bytes: &[u8], max: usize) -> Result<()> {
        if bytes.len() > max {
            return Err(Error::index(
                "<writer>",
                format!("path length {} exceeds {max}", bytes.len()),
            ));
        }
        self.u32(
            u32::try_from(bytes.len())
                .map_err(|_| Error::index("<writer>", "path length exceeds u32"))?,
        )?;
        self.bytes(bytes)
    }

    pub(super) fn finish(mut self, path: &Path) -> Result<()> {
        let checksum = self.digest.finalize();
        self.writer
            .write_all(&checksum)
            .and_then(|()| self.writer.flush())
            .map_err(|source| Error::io(path, source))?;
        self.writer
            .get_ref()
            .sync_all()
            .map_err(|source| Error::io(path, source))
    }
}

pub(super) struct IndexReader {
    reader: BufReader<File>,
    digest: Sha256,
    read: u64,
    limit: u64,
}

impl IndexReader {
    pub(super) fn new(file: File, limit: u64) -> Self {
        Self {
            reader: BufReader::new(file),
            digest: Sha256::new(),
            read: 0,
            limit,
        }
    }

    pub(super) fn bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        let length_u64 = u64::try_from(length).unwrap_or(u64::MAX);
        self.read = self
            .read
            .checked_add(length_u64)
            .ok_or_else(|| Error::index("<reader>", "serialized byte count overflow"))?;
        if self.read.saturating_add(CHECKSUM_BYTES) > self.limit {
            return Err(Error::index(
                "<reader>",
                format!("serialized index exceeds {} bytes", self.limit),
            ));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|error| Error::index("<reader>", format!("allocation failed: {error}")))?;
        bytes.resize(length, 0);
        self.reader
            .read_exact(&mut bytes)
            .map_err(|source| Error::io("<reader>", source))?;
        self.digest.update(&bytes);
        Ok(bytes)
    }

    pub(super) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.bytes(N)?
            .try_into()
            .map_err(|_| Error::index("<reader>", "fixed-width field length mismatch"))
    }

    pub(super) fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub(super) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(super) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub(super) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub(super) fn length_prefixed(&mut self, max: usize) -> Result<Vec<u8>> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| Error::index("<reader>", "path length does not fit usize"))?;
        if length > max {
            return Err(Error::index(
                "<reader>",
                format!("path length {length} exceeds {max}"),
            ));
        }
        self.bytes(length)
    }

    pub(super) fn finish(mut self) -> Result<ChecksumResult> {
        let expected = {
            let mut bytes = [0_u8; 32];
            self.reader
                .read_exact(&mut bytes)
                .map_err(|source| Error::io("<reader>", source))?;
            bytes
        };
        let mut trailing = [0_u8; 1];
        let trailing_bytes = self
            .reader
            .read(&mut trailing)
            .map_err(|source| Error::io("<reader>", source))?;
        if trailing_bytes != 0 {
            return Err(Error::index("<reader>", "unexpected trailing bytes"));
        }
        let actual: [u8; 32] = self.digest.finalize().into();
        Ok(ChecksumResult {
            valid: actual == expected,
        })
    }
}

pub(super) struct ChecksumResult {
    pub(super) valid: bool,
}
