
use bytes::{Buf, BytesMut};
use tokio_util::codec::Decoder;

const MAX_LINE_LENGTH: usize = 8192;

#[derive(Default)]
pub struct BytesLinesCodec;

impl Decoder for BytesLinesCodec {
    type Item = Vec<u8>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(newline_pos) = src.iter().position(|&b| b == b'\n') else {
            if src.len() > MAX_LINE_LENGTH {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line exceeded {MAX_LINE_LENGTH} bytes with no newline"),
                ));
            }
            return Ok(None);
        };
        if newline_pos > MAX_LINE_LENGTH {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("line exceeded {MAX_LINE_LENGTH} bytes"),
            ));
        }
        let mut line = src.split_to(newline_pos);
        src.advance(1); // consume the '\n' itself, not part of the line
        if line.last() == Some(&b'\r') {
            line.truncate(line.len() - 1);
        }
        Ok(Some(line.to_vec()))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(line) => Ok(Some(line)),
            None if src.is_empty() => Ok(None),
            None => {
                let mut line = src.split_to(src.len());
                if line.last() == Some(&b'\r') {
                    line.truncate(line.len() - 1);
                }
                Ok(Some(line.to_vec()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_returns_none_with_no_newline_yet() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::from(&b"partial"[..]);
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        // Nothing consumed -- still waiting for more bytes.
        assert_eq!(&buf[..], b"partial");
    }

    #[test]
    fn decode_splits_on_bare_newline() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::from(&b"hello\nworld"[..]);
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(b"hello".to_vec()));
        assert_eq!(&buf[..], b"world");
    }

    #[test]
    fn decode_strips_a_trailing_cr() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::from(&b"hello\r\nworld"[..]);
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(b"hello".to_vec()));
        assert_eq!(&buf[..], b"world");
    }

    #[test]
    fn decode_never_fails_on_non_utf8_bytes() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::from(&b"PRIVMSG #chan :\xff\xfe not utf8\n"[..]);
        let line = codec.decode(&mut buf).unwrap().expect("a complete line was present");
        assert_eq!(line, b"PRIVMSG #chan :\xff\xfe not utf8");
    }

    #[test]
    fn decode_eof_returns_a_final_line_with_no_trailing_newline() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::from(&b"no newline at all"[..]);
        assert_eq!(codec.decode_eof(&mut buf).unwrap(), Some(b"no newline at all".to_vec()));
    }

    #[test]
    fn decode_eof_returns_none_when_nothing_is_buffered() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::new();
        assert_eq!(codec.decode_eof(&mut buf).unwrap(), None);
    }

    #[test]
    fn decode_errors_once_an_unterminated_line_exceeds_the_length_cap() {
        let mut codec = BytesLinesCodec;
        // Exactly at the cap, no newline yet: still just waiting.
        let mut buf = BytesMut::from(&vec![b'a'; MAX_LINE_LENGTH][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
        // One more byte over the cap, still no newline: must error, not
        // buffer forever.
        buf.extend_from_slice(b"a");
        assert!(codec.decode(&mut buf).is_err());
    }

    /// Same guard, but for a line that DOES eventually get a `\n` --
    /// too long is too long either way, newline or not.
    #[test]
    fn decode_errors_on_an_oversized_line_even_with_a_trailing_newline() {
        let mut codec = BytesLinesCodec;
        let mut buf = BytesMut::from(&vec![b'a'; MAX_LINE_LENGTH + 1][..]);
        buf.extend_from_slice(b"\n");
        assert!(codec.decode(&mut buf).is_err());
    }
}
