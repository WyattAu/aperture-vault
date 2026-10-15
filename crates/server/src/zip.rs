//! A minimal ZIP writer: STORE method only.
//!
//! Photos and videos are already compressed; deflate on JPEG bytes spends
//! CPU to save nothing. STORE keeps the writer ~a hundred lines, and every
//! unzipper on earth reads it. This module exists for the download-archive
//! endpoint; if it ever grows features it should leave for its own estate
//! crate, not bloat the server.

use crc32fast::Hasher as Crc32;

/// An in-memory ZIP archive under construction. The whole archive is
/// buffered: the download endpoint caps the selection so this stays inside
/// a photo-sized budget instead of a video-sized one.
#[derive(Default)]
pub struct ZipArchive {
    entries: Vec<ZipEntry>,
    bytes: Vec<u8>,
}

struct ZipEntry {
    name: String,
    crc32: u32,
    size: u32,
    offset: u32,
}

impl ZipArchive {
    /// Start an empty archive.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one file. Names are stored as given — the caller supplies the
    /// display name (the original file name, or the address when none).
    pub fn add(&mut self, name: &str, data: &[u8]) {
        let offset = self.bytes.len().try_into().unwrap_or(u32::MAX);
        let mut crc = Crc32::new();
        crc.update(data);
        let crc32 = crc.finalize();
        let size = data.len().try_into().unwrap_or(u32::MAX);

        self.bytes
            .extend_from_slice(&0x04_03_4b_50u32.to_le_bytes());
        self.bytes.extend_from_slice(&20u16.to_le_bytes()); // version needed
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // flags
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // method: STORE
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // mod time
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // mod date
        self.bytes.extend_from_slice(&crc32.to_le_bytes());
        self.bytes.extend_from_slice(&size.to_le_bytes()); // compressed
        self.bytes.extend_from_slice(&size.to_le_bytes()); // uncompressed
        self.bytes
            .extend_from_slice(&(name.len() as u16).to_le_bytes());
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // extra len
        self.bytes.extend_from_slice(name.as_bytes());
        self.bytes.extend_from_slice(data);

        self.entries.push(ZipEntry {
            name: name.to_string(),
            crc32,
            size,
            offset,
        });
    }

    /// Bytes written so far, before the central directory.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been added yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Finish: write the central directory and end record.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        let central_start = self.bytes.len() as u32;
        for entry in &self.entries {
            self.bytes
                .extend_from_slice(&0x02_01_4b_50u32.to_le_bytes());
            self.bytes.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.bytes.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // flags
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // method
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // mod time
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // mod date
            self.bytes.extend_from_slice(&entry.crc32.to_le_bytes());
            self.bytes.extend_from_slice(&entry.size.to_le_bytes());
            self.bytes.extend_from_slice(&entry.size.to_le_bytes());
            self.bytes
                .extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // comment
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number
            self.bytes.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            self.bytes.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            self.bytes.extend_from_slice(&entry.offset.to_le_bytes());
            self.bytes.extend_from_slice(entry.name.as_bytes());
        }
        let central_size = self.bytes.len() as u32 - central_start;
        let count = self.entries.len() as u16;
        self.bytes
            .extend_from_slice(&0x06_05_4b_50u32.to_le_bytes());
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // central disk
        self.bytes.extend_from_slice(&count.to_le_bytes()); // entries this disk
        self.bytes.extend_from_slice(&count.to_le_bytes()); // entries total
        self.bytes.extend_from_slice(&central_size.to_le_bytes());
        self.bytes.extend_from_slice(&central_start.to_le_bytes());
        self.bytes.extend_from_slice(&0u16.to_le_bytes()); // comment len
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::ZipArchive;

    // Fixtures are allowed to assert: an `expect` here is the test stating
    // what it assumes.

    #[allow(clippy::expect_used)]
    #[test]
    fn archive_is_readable_by_a_real_unzipper() {
        let mut zip = ZipArchive::new();
        zip.add("one.txt", b"the first file");
        zip.add("two.txt", b"second file with a little more text");
        let bytes = zip.finish();

        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("opens");
        assert_eq!(reader.len(), 2);
        use std::io::Read as _;
        let mut one = Vec::new();
        reader
            .by_name("one.txt")
            .expect("entry")
            .read_to_end(&mut one)
            .expect("reads");
        assert_eq!(one, b"the first file");
        let mut two = Vec::new();
        reader
            .by_name("two.txt")
            .expect("entry")
            .read_to_end(&mut two)
            .expect("reads");
        assert_eq!(two, b"second file with a little more text");
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn binary_payloads_round_trip() {
        let mut zip = ZipArchive::new();
        let payload: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        zip.add("payload.bin", &payload);
        let bytes = zip.finish();
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("opens");
        use std::io::Read as _;
        let mut got = Vec::new();
        reader
            .by_name("payload.bin")
            .expect("entry")
            .read_to_end(&mut got)
            .expect("reads");
        assert_eq!(got, payload, "70 KiB binary payload survives");
    }
}
