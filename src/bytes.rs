//! The byte-level encoding used inside the cache file: integers are
//! fixed-width little-endian, and a string is a `u32` length followed by
//! UTF-8.

pub fn put_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

pub fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Writes a count or length.
pub fn put_len(out: &mut Vec<u8>, length: usize) {
    put_u32(out, u32::try_from(length).expect("nothing yio stores reaches 4 GiB"));
}

pub fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_len(out, bytes.len());
    out.extend_from_slice(bytes);
}

pub fn put_str(out: &mut Vec<u8>, text: &str) {
    put_bytes(out, text.as_bytes());
}

pub fn put_option(out: &mut Vec<u8>, text: Option<&str>) {
    put_u8(out, u8::from(text.is_some()));
    if let Some(text) = text {
        put_str(out, text);
    }
}

/// The unread remainder of some encoded bytes. Every read is bounds-checked,
/// so a damaged file produces an error rather than a panic.
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let (taken, rest) = self.data.split_at_checked(length).ok_or("cache is truncated")?;
        self.data = rest;
        Ok(taken)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?.try_into().expect("took four bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?.try_into().expect("took eight bytes");
        Ok(u64::from_le_bytes(bytes))
    }

    /// Reads a length-prefixed run of bytes.
    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    pub fn str(&mut self) -> Result<&'a str, String> {
        std::str::from_utf8(self.bytes()?).map_err(|_| "cache has text that is not UTF-8".to_string())
    }

    pub fn option(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0 => Ok(None),
            _ => Ok(Some(self.str()?.to_string())),
        }
    }
}
