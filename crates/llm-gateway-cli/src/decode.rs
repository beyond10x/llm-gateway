//! The gateway's [`BodyDecoder`]: gzip and deflate (flate2), br (brotli-decompressor) and zstd
//! (ruzstd), so that a passthrough route reads the usage and the error text of a compressed
//! answer from a decoded copy while the client receives the answer's bytes unchanged.

use std::io::Read;

use llm_gateway::{BodyDecoder, Decoded};

/// Decodes the content codings a passthrough answer arrives in.
#[derive(Debug, Default, Clone, Copy)]
pub struct Decoder;

impl BodyDecoder for Decoder {
    fn decode(&self, coding: &str, encoded: &[u8], limit: usize) -> Option<Decoded> {
        let coding = coding.trim().to_ascii_lowercase();
        let reader: Box<dyn Read + '_> = match coding.as_str() {
            "gzip" | "x-gzip" => Box::new(flate2::read::MultiGzDecoder::new(encoded)),
            "deflate" => Box::new(flate2::read::ZlibDecoder::new(encoded)),
            "br" => Box::new(brotli_decompressor::Decompressor::new(encoded, 4096)),
            "zstd" => Box::new(ruzstd::decoding::StreamingDecoder::new(encoded).ok()?),
            _ => return None,
        };
        Some(read_within(reader, limit))
    }
}

/// Reads `reader` to its end or to `limit` bytes; complete only when it ended cleanly first.
fn read_within(reader: impl Read, limit: usize) -> Decoded {
    let mut bytes = Vec::new();
    let bound = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let ended = reader.take(bound).read_to_end(&mut bytes).is_ok();
    let complete = ended && bytes.len() <= limit;
    bytes.truncate(limit);
    Decoded { bytes, complete }
}

#[cfg(test)]
mod tests {
    use super::Decoder;
    use llm_gateway::BodyDecoder;
    use std::io::Write;

    #[test]
    fn gzip_is_decoded_and_a_truncated_body_is_incomplete() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"event: message_start\n").unwrap();
        let gzipped = encoder.finish().unwrap();
        let decoded = Decoder.decode("gzip", &gzipped, 1024).unwrap();
        assert!(decoded.complete);
        assert_eq!(decoded.bytes, b"event: message_start\n");
        let cut = Decoder
            .decode("gzip", &gzipped[..gzipped.len() - 4], 1024)
            .unwrap();
        assert!(!cut.complete);
        let short = Decoder.decode("gzip", &gzipped, 5).unwrap();
        assert!(!short.complete);
        assert_eq!(short.bytes, b"event");
        assert!(Decoder.decode("compress", &gzipped, 1024).is_none());
    }
}
