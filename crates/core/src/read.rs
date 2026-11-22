//! Bounds-checked little/big endian views over byte slices.
//!
//! Every accessor returns `None` instead of panicking so a truncated or
//! malformed stream can never abort a scan.

/// Little-endian view over a borrowed slice.
pub struct Le<'a>(pub &'a [u8]);

impl<'a> Le<'a> {
    #[inline]
    pub fn u8(&self, off: usize) -> Option<u8> {
        self.0.get(off).copied()
    }

    #[inline]
    pub fn u16(&self, off: usize) -> Option<u16> {
        let s = self.0.get(off..off + 2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }

    #[inline]
    pub fn u32(&self, off: usize) -> Option<u32> {
        let s = self.0.get(off..off + 4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    #[inline]
    pub fn i32(&self, off: usize) -> Option<i32> {
        self.u32(off).map(|v| v as i32)
    }

    #[inline]
    pub fn u64(&self, off: usize) -> Option<u64> {
        let s = self.0.get(off..off + 8)?;
        Some(u64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }

    #[inline]
    pub fn f32(&self, off: usize) -> Option<f32> {
        self.u32(off).map(f32::from_bits)
    }

    #[inline]
    pub fn bytes(&self, off: usize, len: usize) -> Option<&'a [u8]> {
        self.0.get(off..off.checked_add(len)?)
    }
}

/// Big-endian view, used by the PSIN tagged-section chain.
pub struct Be<'a>(pub &'a [u8]);

impl<'a> Be<'a> {
    #[inline]
    pub fn u32(&self, off: usize) -> Option<u32> {
        let s = self.0.get(off..off + 4)?;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
}

/// Compare a slice prefix with a magic without allocation.
#[inline]
pub fn magic_is(data: &[u8], magic: &[u8]) -> bool {
    data.len() >= magic.len() && &data[..magic.len()] == magic
}

/// `zlib` stream head test used by every carver in this crate.
///
/// Accepts the four CMF/FLG combinations produced by every encoder shipped with
/// GTA VI, and rejects the two reserved FLG encodings. The 32-bit remainder test
/// (`(cmf<<8|flg) % 31 == 0`) is implied by the accepted FLG bytes, so it is not
/// repeated.
#[inline]
pub fn is_zlib_head(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0x78 && matches!(b[1], 0x01 | 0x5e | 0x9c | 0xda) && (b[2] & 0x06) != 0x06
}
