//! Lettore bufferizzato per i parser di formato: accesso a byte e interi
//! a offset assoluti, con una finestra in cache.

use crate::source::BlockSource;

const WINDOW: usize = 256 * 1024;

pub struct ByteReader<'a> {
    src: &'a dyn BlockSource,
    buf: Vec<u8>,
    start: u64,
    len: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(src: &'a dyn BlockSource) -> Self {
        Self {
            src,
            buf: vec![0; WINDOW],
            start: 0,
            len: 0,
        }
    }

    pub fn size(&self) -> u64 {
        self.src.size()
    }

    fn fill(&mut self, pos: u64) -> bool {
        let start = pos - pos % 4096;
        let mut filled = 0;
        while filled < self.buf.len() {
            match self
                .src
                .read_at(start + filled as u64, &mut self.buf[filled..])
            {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => break,
            }
        }
        self.start = start;
        self.len = filled;
        pos < start + filled as u64
    }

    #[inline]
    pub fn byte(&mut self, pos: u64) -> Option<u8> {
        if pos >= self.start && pos < self.start + self.len as u64 {
            return Some(self.buf[(pos - self.start) as usize]);
        }
        if !self.fill(pos) {
            return None;
        }
        Some(self.buf[(pos - self.start) as usize])
    }

    pub fn read(&mut self, pos: u64, out: &mut [u8]) -> Option<()> {
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.byte(pos + i as u64)?;
        }
        Some(())
    }

    pub fn array<const N: usize>(&mut self, pos: u64) -> Option<[u8; N]> {
        let mut a = [0u8; N];
        self.read(pos, &mut a)?;
        Some(a)
    }

    pub fn u16_be(&mut self, pos: u64) -> Option<u16> {
        self.array::<2>(pos).map(u16::from_be_bytes)
    }
    pub fn u32_be(&mut self, pos: u64) -> Option<u32> {
        self.array::<4>(pos).map(u32::from_be_bytes)
    }
    pub fn u64_be(&mut self, pos: u64) -> Option<u64> {
        self.array::<8>(pos).map(u64::from_be_bytes)
    }
    pub fn u16_le(&mut self, pos: u64) -> Option<u16> {
        self.array::<2>(pos).map(u16::from_le_bytes)
    }
    pub fn u32_le(&mut self, pos: u64) -> Option<u32> {
        self.array::<4>(pos).map(u32::from_le_bytes)
    }

    /// Cerca `needle` nell'intervallo `[from, limit)`. Ritorna l'offset assoluto.
    pub fn find(&mut self, from: u64, limit: u64, needle: &[u8]) -> Option<u64> {
        let limit = limit.min(self.size());
        let mut pos = from;
        while pos + needle.len() as u64 <= limit {
            if !(pos >= self.start && pos < self.start + self.len as u64) && !self.fill(pos) {
                return None;
            }
            let rel = (pos - self.start) as usize;
            let end = ((limit - self.start) as usize).min(self.len);
            let hay = &self.buf[rel..end];
            if let Some(i) = hay.windows(needle.len()).position(|w| w == needle) {
                return Some(pos + i as u64);
            }
            let window_end = self.start + self.len as u64;
            if window_end >= limit || self.len == 0 {
                return None;
            }
            // Sovrapposizione per non perdere un match a cavallo delle finestre.
            pos = (window_end + 1)
                .saturating_sub(needle.len() as u64)
                .max(pos + 1);
            if !self.fill(pos) {
                return None;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    #[test]
    fn find_across_window_boundary() {
        let mut data = vec![0u8; WINDOW * 2];
        let at = WINDOW - 2;
        data[at..at + 4].copy_from_slice(b"%EOF");
        let src = MemorySource::new(data, "mem");
        let mut r = ByteReader::new(&src);
        assert_eq!(r.find(0, u64::MAX, b"%EOF"), Some(at as u64));
        assert_eq!(r.find(at as u64 + 1, u64::MAX, b"%EOF"), None);
    }

    #[test]
    fn integers() {
        let src = MemorySource::new(vec![0x12, 0x34, 0x56, 0x78], "mem");
        let mut r = ByteReader::new(&src);
        assert_eq!(r.u16_be(0), Some(0x1234));
        assert_eq!(r.u32_le(0), Some(0x7856_3412));
        assert_eq!(r.u32_be(1), None);
    }
}
