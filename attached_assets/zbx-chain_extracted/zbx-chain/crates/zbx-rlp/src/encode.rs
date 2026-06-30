//! RLP encoding — stream-based builder.

/// Anything that can be RLP-encoded.
pub trait Encodable {
    fn encode_into(&self, s: &mut RlpStream);
}

/// Builder for RLP-encoded byte strings.
pub struct RlpStream {
    buf: Vec<u8>,
    /// Stack of in-progress list lengths (byte offsets into buf).
    list_stack: Vec<usize>,
}

impl RlpStream {
    /// Create an empty stream.
    pub fn new() -> Self {
        Self { buf: Vec::new(), list_stack: Vec::new() }
    }

    /// Create a stream pre-configured to emit exactly `n` list items.
    pub fn new_list(n: usize) -> Self {
        let mut s = Self::new();
        s.begin_list(n);
        s
    }

    /// Begin an RLP list with `n` items.
    pub fn begin_list(&mut self, _n: usize) -> &mut Self {
        // Mark position; we'll patch the length header on finish.
        self.list_stack.push(self.buf.len());
        // Reserve placeholder for length prefix (max 9 bytes).
        self.buf.extend_from_slice(&[0u8; 9]);
        self
    }

    /// Append a byte-string item.
    pub fn append(&mut self, data: &[u8]) -> &mut Self {
        self.encode_string(data);
        self
    }

    /// Append a pre-encoded RLP item (raw bytes, no extra wrapping).
    pub fn append_raw(&mut self, data: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(data);
        self
    }

    fn encode_string(&mut self, data: &[u8]) {
        match data.len() {
            0 => self.buf.push(0x80),
            1 if data[0] < 0x80 => self.buf.push(data[0]),
            n if n <= 55 => {
                self.buf.push(0x80 + n as u8);
                self.buf.extend_from_slice(data);
            }
            n => {
                let len_bytes = Self::minimal_bytes(n as u64);
                self.buf.push(0xb7 + len_bytes.len() as u8);
                self.buf.extend_from_slice(&len_bytes);
                self.buf.extend_from_slice(data);
            }
        }
    }

    fn minimal_bytes(n: u64) -> Vec<u8> {
        let bytes = n.to_be_bytes();
        let leading = bytes.iter().take_while(|&&b| b == 0).count();
        bytes[leading..].to_vec()
    }

    /// Finish the most recent list and patch its length header.
    pub fn finalize_unbounded_list(&mut self) {
        let start = self.list_stack.pop().expect("no open list");
        // Content starts after the 9-byte placeholder.
        let content_start = start + 9;
        let content_len = self.buf.len() - content_start;

        // Build the proper prefix.
        let prefix = if content_len <= 55 {
            let mut p = Vec::new();
            p.push(0xc0 + content_len as u8);
            p
        } else {
            let len_bytes = Self::minimal_bytes(content_len as u64);
            let mut p = Vec::new();
            p.push(0xf7 + len_bytes.len() as u8);
            p.extend_from_slice(&len_bytes);
            p
        };

        // Patch: replace 9-byte placeholder with real prefix, shift content.
        let placeholder_len = 9;
        let shift = prefix.len() as isize - placeholder_len as isize;
        if shift == 0 {
            self.buf[start..start + prefix.len()].copy_from_slice(&prefix);
        } else {
            // Rebuild from start.
            let content: Vec<u8> = self.buf[content_start..].to_vec();
            self.buf.truncate(start);
            self.buf.extend_from_slice(&prefix);
            self.buf.extend_from_slice(&content);
        }
    }

    /// Consume the stream and return encoded bytes.
    pub fn out(mut self) -> Vec<u8> {
        while !self.list_stack.is_empty() {
            self.finalize_unbounded_list();
        }
        self.buf
    }
}

impl Default for RlpStream {
    fn default() -> Self {
        Self::new()
    }
}

// Blanket impls for common types.

impl Encodable for Vec<u8> {
    fn encode_into(&self, s: &mut RlpStream) { s.append(self.as_slice()); }
}

impl Encodable for &[u8] {
    fn encode_into(&self, s: &mut RlpStream) { s.append(self); }
}

impl Encodable for u8  { fn encode_into(&self, s: &mut RlpStream) { s.encode_string(&[*self]); } }
impl Encodable for u64 {
    fn encode_into(&self, s: &mut RlpStream) {
        if *self == 0 {
            s.buf.push(0x80);
        } else {
            let bytes = RlpStream::minimal_bytes(*self);
            s.encode_string(&bytes);
        }
    }
}
impl Encodable for u128 {
    fn encode_into(&self, s: &mut RlpStream) {
        if *self == 0 {
            s.buf.push(0x80);
        } else {
            let b = self.to_be_bytes();
            let skip = b.iter().take_while(|&&x| x == 0).count();
            s.encode_string(&b[skip..]);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_empty_bytes() {
        let mut s = RlpStream::new();
        s.append(&[]);
        let out = s.out();
        assert_eq!(out, vec![0x80]);
    }

    #[test]
    fn encode_single_byte_under_128() {
        let mut s = RlpStream::new();
        s.append(&[0x42]);
        let out = s.out();
        assert_eq!(out, vec![0x42]);
    }

    #[test]
    fn encode_short_string() {
        let mut s = RlpStream::new();
        s.append(b"hello");
        let out = s.out();
        assert_eq!(out[0], 0x80 + 5);
        assert_eq!(&out[1..], b"hello");
    }

    #[test]
    fn encode_empty_list() {
        let mut s = RlpStream::new_list(0);
        let out = s.out();
        assert_eq!(out, vec![0xc0]);
    }

    #[test]
    fn encode_list_two_elements() {
        let mut s = RlpStream::new_list(2);
        s.append(&[0x01]);
        s.append(&[0x02]);
        let out = s.out();
        // 0xc2 = list-header for 2-byte payload, then 0x01, 0x02
        assert_eq!(out[0], 0xc0 + 2);
        assert_eq!(out[1], 0x01);
        assert_eq!(out[2], 0x02);
    }

    #[test]
    fn encode_append_raw() {
        let mut s = RlpStream::new();
        s.append_raw(&[0x01, 0x02]);
        let out = s.out();
        assert_eq!(out, vec![0x01, 0x02]);
    }
}
