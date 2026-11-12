//! An image's width and height from its file's header, without decoding
//! it: for thumbnails, which hold back big pictures while others take the
//! memory (see icons.rs). PNG, JPEG, GIF, WebP, BMP, TIFF, JPEG XL, and
//! HEIC/AVIF; None for anything else or a header that doesn't make sense.

use std::io::{Read, Seek, SeekFrom};

/// How much of a file is read at most to find its size (JPEG's frame
/// header can come after a big EXIF block with its own thumbnail).
const MAX_SCAN: u64 = 1 << 20;

pub fn dimensions(path: &std::path::Path) -> Option<(u32, u32)> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 64];
    let n = read_up_to(&mut f, &mut head)?;
    let h = &head[..n];
    let size = if h.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(h)
    } else if h.starts_with(&[0xff, 0xd8]) {
        jpeg(&mut f)
    } else if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") {
        Some((le16(h, 6)?, le16(h, 8)?))
    } else if h.starts_with(b"BM") {
        bmp(h)
    } else if h.starts_with(b"RIFF") && h.get(8..12) == Some(b"WEBP") {
        webp(h)
    } else if h.starts_with(b"II*\0") || h.starts_with(b"MM\0*") {
        tiff(&mut f, h[0] == b'I')
    } else if h.starts_with(&[0xff, 0x0a]) {
        jxl_codestream(&h[2..])
    } else if h.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
        jxl_container(&mut f)
    } else if h.get(4..8) == Some(b"ftyp") {
        isobmff(&mut f)
    } else {
        None
    };
    size.filter(|&(w, h)| w > 0 && h > 0)
}

fn read_up_to(f: &mut std::fs::File, buf: &mut [u8]) -> Option<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]).ok()? {
            0 => break,
            k => n += k,
        }
    }
    Some(n)
}

fn read_at(f: &mut std::fs::File, pos: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(pos)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as u32)
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?) as u32)
}

fn le32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn png(h: &[u8]) -> Option<(u32, u32)> {
    (h.get(12..16)? == b"IHDR").then_some(())?;
    Some((be32(h, 16)?, be32(h, 20)?))
}

/// The first frame header (SOFn) after the markers before it.
fn jpeg(f: &mut std::fs::File) -> Option<(u32, u32)> {
    let mut pos = 2u64;
    while pos < MAX_SCAN {
        let m = read_at(f, pos, 2)?;
        if m[0] != 0xff {
            return None;
        }
        // Fill bytes, and markers without a length.
        if m[1] == 0xff || m[1] == 0x01 || (0xd0..=0xd7).contains(&m[1]) {
            pos += if m[1] == 0xff { 1 } else { 2 };
            continue;
        }
        let seg = read_at(f, pos + 2, 7)?;
        let len = be16(&seg, 0)? as u64;
        if matches!(m[1], 0xc0..=0xcf) && !matches!(m[1], 0xc4 | 0xc8 | 0xcc) {
            return Some((be16(&seg, 5)?, be16(&seg, 3)?));
        }
        if len < 2 {
            return None;
        }
        pos += 2 + len;
    }
    None
}

fn bmp(h: &[u8]) -> Option<(u32, u32)> {
    if le32(h, 14)? == 12 {
        Some((le16(h, 18)?, le16(h, 20)?))
    } else {
        let w = i32::from_le_bytes(h.get(18..22)?.try_into().ok()?);
        let hh = i32::from_le_bytes(h.get(22..26)?.try_into().ok()?);
        Some((w.unsigned_abs(), hh.unsigned_abs()))
    }
}

fn webp(h: &[u8]) -> Option<(u32, u32)> {
    let u24 = |i: usize| -> Option<u32> { Some(h.get(i)?.to_owned() as u32 | (*h.get(i + 1)? as u32) << 8 | (*h.get(i + 2)? as u32) << 16) };
    match h.get(12..16)? {
        b"VP8X" => Some((u24(24)? + 1, u24(27)? + 1)),
        b"VP8L" => {
            (*h.get(20)? == 0x2f).then_some(())?;
            let b = le32(h, 21)?;
            Some(((b & 0x3fff) + 1, ((b >> 14) & 0x3fff) + 1))
        }
        b"VP8 " => {
            (h.get(23..26)? == [0x9d, 0x01, 0x2a]).then_some(())?;
            Some((le16(h, 26)? & 0x3fff, le16(h, 28)? & 0x3fff))
        }
        _ => None,
    }
}

/// ImageWidth (256) and ImageLength (257) in the first directory.
fn tiff(f: &mut std::fs::File, little: bool) -> Option<(u32, u32)> {
    let r16 = |b: &[u8], i| if little { le16(b, i) } else { be16(b, i) };
    let r32 = |b: &[u8], i| if little { le32(b, i) } else { be32(b, i) };
    let ifd = r32(&read_at(f, 4, 4)?, 0)? as u64;
    let count = r16(&read_at(f, ifd, 2)?, 0)? as usize;
    let entries = read_at(f, ifd + 2, count.min(512) * 12)?;
    let (mut w, mut h) = (None, None);
    for e in entries.chunks_exact(12) {
        let value = match r16(e, 2)? {
            3 => r16(e, 8)?,
            4 => r32(e, 8)?,
            _ => continue,
        };
        match r16(e, 0)? {
            256 => w = Some(value),
            257 => h = Some(value),
            _ => {}
        }
    }
    Some((w?, h?))
}

/// JPEG XL's bits, least significant first.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for i in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            v |= (((byte >> (self.pos % 8)) & 1) as u32) << i;
            self.pos += 1;
        }
        Some(v)
    }

    /// A dimension: one of four bit widths, plus one.
    fn dim(&mut self) -> Option<u32> {
        let bits = [9, 13, 18, 30][self.read(2)? as usize];
        Some(self.read(bits)? + 1)
    }
}

/// The SizeHeader right after the codestream's signature.
fn jxl_codestream(b: &[u8]) -> Option<(u32, u32)> {
    let mut r = Bits { data: b, pos: 0 };
    let div8 = r.read(1)? == 1;
    let height = if div8 { (r.read(5)? + 1) * 8 } else { r.dim()? };
    let ratio = r.read(3)?;
    let width = match ratio {
        0 if div8 => (r.read(5)? + 1) * 8,
        0 => r.dim()?,
        _ => {
            let (num, den) = [(1, 1), (12, 10), (4, 3), (3, 2), (16, 9), (5, 4), (2, 1)][ratio as usize - 1];
            (height as u64 * num / den) as u32
        }
    };
    Some((width, height))
}

/// The codestream in a JPEG XL container: in a jxlc box, or the first jxlp.
fn jxl_container(f: &mut std::fs::File) -> Option<(u32, u32)> {
    let mut pos = 0u64;
    while pos < MAX_SCAN {
        let (kind, start, end) = box_at(f, pos)?;
        match &kind {
            b"jxlc" => return jxl_codestream(read_at(f, start, 16)?.strip_prefix(&[0xff, 0x0a])?),
            b"jxlp" => return jxl_codestream(read_at(f, start + 4, 16)?.strip_prefix(&[0xff, 0x0a])?),
            _ => pos = end,
        }
    }
    None
}

/// The box at `pos` in an ISO base media file: its type, where its content
/// starts and where it ends.
fn box_at(f: &mut std::fs::File, pos: u64) -> Option<([u8; 4], u64, u64)> {
    let h = read_at(f, pos, 8)?;
    let kind: [u8; 4] = h[4..8].try_into().ok()?;
    let (size, header) = match be32(&h, 0)? {
        1 => (u64::from_be_bytes(read_at(f, pos + 8, 8)?.try_into().ok()?), 16),
        0 => (f.metadata().ok()?.len().checked_sub(pos)?, 8),
        n => (n as u64, 8),
    };
    (size >= header).then_some(())?;
    Some((kind, pos + header, pos + size))
}

/// HEIC and AVIF: the biggest image size (ispe) among the item properties
/// (meta → iprp → ipco); a picture made of tiles has its whole size there.
/// It's the coded size, which can be a row or column more than shown.
fn isobmff(f: &mut std::fs::File) -> Option<(u32, u32)> {
    let find = |f: &mut std::fs::File, mut pos: u64, end: u64, want: &[u8; 4]| -> Option<(u64, u64)> {
        while pos < end.min(MAX_SCAN) {
            let (kind, start, next) = box_at(f, pos)?;
            if &kind == want {
                return Some((start, next));
            }
            pos = next;
        }
        None
    };
    let len = f.metadata().ok()?.len();
    let (meta, meta_end) = find(f, 0, len, b"meta")?;
    // meta is a full box: version and flags first.
    let (iprp, iprp_end) = find(f, meta + 4, meta_end, b"iprp")?;
    let (mut pos, ipco_end) = find(f, iprp, iprp_end, b"ipco")?;
    let mut best: Option<(u32, u32)> = None;
    while pos < ipco_end {
        let (kind, start, next) = box_at(f, pos)?;
        if &kind == b"ispe" {
            let b = read_at(f, start + 4, 8)?;
            let s = (be32(&b, 0)?, be32(&b, 4)?);
            if best.is_none_or(|(w, h)| s.0 as u64 * s.1 as u64 > w as u64 * h as u64) {
                best = Some(s);
            }
        }
        pos = next;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size_of(t: &crate::testutil::TestDir, name: &str, data: &[u8]) -> Option<(u32, u32)> {
        let p = t.0.join(name);
        std::fs::write(&p, data).unwrap();
        dimensions(&p)
    }

    #[test]
    fn common_formats() {
        let t = crate::testutil::TestDir::new("imagesize");
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(640u32.to_be_bytes());
        png.extend(480u32.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        assert_eq!(size_of(&t, "a.png", &png), Some((640, 480)));

        // An APP1 segment, then the frame header (SOF2, progressive).
        let mut jpg = vec![0xff, 0xd8, 0xff, 0xe1, 0x00, 0x06, 1, 2, 3, 4];
        jpg.extend([0xff, 0xff, 0xc2, 0x00, 0x11, 8]);
        jpg.extend(3000u16.to_be_bytes());
        jpg.extend(4000u16.to_be_bytes());
        jpg.extend([3; 12]);
        assert_eq!(size_of(&t, "a.jpg", &jpg), Some((4000, 3000)));

        let mut gif = b"GIF89a".to_vec();
        gif.extend(300u16.to_le_bytes());
        gif.extend(200u16.to_le_bytes());
        gif.extend([0; 8]);
        assert_eq!(size_of(&t, "a.gif", &gif), Some((300, 200)));

        let mut bmp = b"BM".to_vec();
        bmp.extend([0; 12]);
        bmp.extend(40u32.to_le_bytes());
        bmp.extend(1024i32.to_le_bytes());
        bmp.extend((-768i32).to_le_bytes());
        bmp.extend([0; 8]);
        assert_eq!(size_of(&t, "a.bmp", &bmp), Some((1024, 768)));

        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend(&1919u32.to_le_bytes()[..3]);
        webp.extend(&1079u32.to_le_bytes()[..3]);
        assert_eq!(size_of(&t, "a.webp", &webp), Some((1920, 1080)));

        // Little-endian TIFF: one directory with width (SHORT), length (LONG).
        let mut tif = b"II*\0".to_vec();
        tif.extend(8u32.to_le_bytes());
        tif.extend(2u16.to_le_bytes());
        tif.extend([0x00, 0x01, 3, 0, 1, 0, 0, 0]);
        tif.extend(2500u16.to_le_bytes());
        tif.extend([0, 0]);
        tif.extend([0x01, 0x01, 4, 0, 1, 0, 0, 0]);
        tif.extend(1500u32.to_le_bytes());
        assert_eq!(size_of(&t, "a.tif", &tif), Some((2500, 1500)));

        assert_eq!(size_of(&t, "a.txt", b"just some text, not a picture at all"), None);
        assert_eq!(size_of(&t, "short.png", b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(size_of(&t, "bad.jpg", &[0xff, 0xd8, 0x12, 0x34]), None);
    }

    /// A JPEG XL SizeHeader, written as the format puts it.
    fn jxl_size_header(w: u32, h: u32) -> Vec<u8> {
        let mut bits: Vec<u8> = vec![];
        let mut put = |v: u32, n: usize| (0..n).for_each(|i| bits.push(((v >> i) & 1) as u8));
        put(0, 1); // not a multiple of 8
        put(2, 2); // 18 bits
        put(h - 1, 18);
        put(0, 3); // no ratio: the width follows
        put(2, 2);
        put(w - 1, 18);
        let mut out = vec![0xff, 0x0a];
        out.extend(bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, bit)| b | bit << i)));
        out.extend([0; 8]);
        out
    }

    #[test]
    fn jpeg_xl_and_heif() {
        let t = crate::testutil::TestDir::new("imagesize-jxl");
        let cs = jxl_size_header(4096, 2304);
        assert_eq!(size_of(&t, "a.jxl", &cs), Some((4096, 2304)));
        // In a container: signature, ftyp, then the codestream box.
        let mut c = b"\0\0\0\x0cJXL \r\n\x87\n\0\0\0\x14ftypjxl \0\0\0\0jxl ".to_vec();
        c.extend(((cs.len() + 8) as u32).to_be_bytes());
        c.extend(b"jxlc");
        c.extend(&cs);
        assert_eq!(size_of(&t, "b.jxl", &c), Some((4096, 2304)));
        // A 16:9 ratio, height a multiple of 8: 8 × 32 = 256 high.
        let mut bits = vec![1u8];
        bits.extend((0..5).map(|i| ((31 >> i) & 1) as u8));
        bits.extend([1, 0, 1]); // ratio 5: 16:9
        let mut r = vec![0xff, 0x0a];
        r.extend(bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, bit)| b | bit << i)));
        r.extend([0; 4]);
        assert_eq!(size_of(&t, "c.jxl", &r), Some((455, 256)));

        // HEIF: ftyp, then meta (full box) → iprp → ipco with two ispe.
        let bx = |kind: &[u8], body: &[u8]| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(body);
            b
        };
        let ispe = |w: u32, h: u32| {
            let mut body = vec![0, 0, 0, 0];
            body.extend(w.to_be_bytes());
            body.extend(h.to_be_bytes());
            bx(b"ispe", &body)
        };
        let ipco = bx(b"ipco", &[ispe(512, 512), ispe(4032, 3024)].concat());
        let meta = bx(b"meta", &[vec![0, 0, 0, 0], bx(b"hdlr", &[0; 24]), bx(b"iprp", &ipco)].concat());
        let heic = [bx(b"ftyp", b"heic\0\0\0\0mif1heic"), meta].concat();
        assert_eq!(size_of(&t, "a.heic", &heic), Some((4032, 3024)));
    }
}
