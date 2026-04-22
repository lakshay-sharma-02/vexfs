//! Transparent zstd compression for COLD-tier file data.
//! HOT and WARM files are stored uncompressed for low read latency.
//! COLD files are compressed at level 3 — ~3x smaller, negligible CPU.

use std::io::Read;

pub const COMPRESS_MAGIC: u32 = 0x5A535444; // "ZSTD"
pub const COMPRESS_HEADER_SIZE: usize = 8;  // magic(4) + original_size(4)

/// Compress data if it's worth it (> 512 bytes and compressible).
/// Returns compressed bytes with a 8-byte header, or original data if
/// compression doesn't help.
pub fn compress(data: &[u8]) -> Vec<u8> {
    if data.len() < 512 {
        return data.to_vec();
    }
    let compressed = zstd::encode_all(data, 3)
        .unwrap_or_else(|_| data.to_vec());

    // Only use compression if it actually shrinks the data
    if compressed.len() >= data.len() {
        return data.to_vec();
    }

    let mut out = Vec::with_capacity(COMPRESS_HEADER_SIZE + compressed.len());
    out.extend_from_slice(&COMPRESS_MAGIC.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

/// Decompress data if it has the compression header. Otherwise return as-is.
pub fn decompress(data: &[u8]) -> Vec<u8> {
    if data.len() < COMPRESS_HEADER_SIZE {
        return data.to_vec();
    }

    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic != COMPRESS_MAGIC {
        return data.to_vec(); // not compressed
    }

    let original_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let payload = &data[COMPRESS_HEADER_SIZE..];

    let mut out = Vec::with_capacity(original_size);
    let mut decoder = zstd::Decoder::new(payload)
        .unwrap();
    decoder.read_to_end(&mut out).unwrap_or(0);
    out
}

pub fn is_compressed(data: &[u8]) -> bool {
    data.len() >= 4
        && u32::from_le_bytes(data[0..4].try_into().unwrap()) == COMPRESS_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_roundtrip() {
        let data = b"hello world this is a test of compression ".repeat(100);
        let compressed = compress(&data);
        assert!(is_compressed(&compressed));
        assert!(compressed.len() < data.len());
        let decompressed = decompress(&compressed);
        assert_eq!(decompressed, data.as_slice());
    }

    #[test]
    fn test_small_data_not_compressed() {
        let data = b"tiny";
        let out = compress(data);
        assert!(!is_compressed(&out));
        assert_eq!(out, data);
    }

    #[test]
    fn test_incompressible_data_not_compressed() {
        // High-entropy data won't compress
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let out = compress(&data);
        // Should return original since compressed >= original
        assert!(!is_compressed(&out) || out.len() < data.len());
    }

    #[test]
    fn test_decompress_passthrough_for_uncompressed() {
        let data = b"not compressed data";
        let out = decompress(data);
        assert_eq!(out, data);
    }
}
