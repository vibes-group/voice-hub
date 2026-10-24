//! Port of Go's http.DetectContentType (WHATWG MIME sniffing subset), so an
//! upload gets exactly the MIME type the Go server assigned.

pub const SNIFF_LEN: usize = 512;

enum Sig {
    Html(&'static [u8]),
    Masked { mask: &'static [u8], pat: &'static [u8], skip_ws: bool, ct: &'static str },
    Exact(&'static [u8], &'static str),
    Mp4,
    Text,
}

const fn masked(mask: &'static [u8], pat: &'static [u8], ct: &'static str) -> Sig {
    Sig::Masked { mask, pat, skip_ws: false, ct }
}

static SIGNATURES: &[Sig] = &[
    Sig::Html(b"<!DOCTYPE HTML"),
    Sig::Html(b"<HTML"),
    Sig::Html(b"<HEAD"),
    Sig::Html(b"<SCRIPT"),
    Sig::Html(b"<IFRAME"),
    Sig::Html(b"<H1"),
    Sig::Html(b"<DIV"),
    Sig::Html(b"<FONT"),
    Sig::Html(b"<TABLE"),
    Sig::Html(b"<A"),
    Sig::Html(b"<STYLE"),
    Sig::Html(b"<TITLE"),
    Sig::Html(b"<B"),
    Sig::Html(b"<BODY"),
    Sig::Html(b"<BR"),
    Sig::Html(b"<P"),
    Sig::Html(b"<!--"),
    Sig::Masked { mask: b"\xFF\xFF\xFF\xFF\xFF", pat: b"<?xml", skip_ws: true, ct: "text/xml; charset=utf-8" },
    Sig::Exact(b"%PDF-", "application/pdf"),
    Sig::Exact(b"%!PS-Adobe-", "application/postscript"),
    masked(b"\xFF\xFF\x00\x00", b"\xFE\xFF\x00\x00", "text/plain; charset=utf-16be"),
    masked(b"\xFF\xFF\x00\x00", b"\xFF\xFE\x00\x00", "text/plain; charset=utf-16le"),
    masked(b"\xFF\xFF\xFF\x00", b"\xEF\xBB\xBF\x00", "text/plain; charset=utf-8"),
    Sig::Exact(b"\x00\x00\x01\x00", "image/x-icon"),
    Sig::Exact(b"\x00\x00\x02\x00", "image/x-icon"),
    Sig::Exact(b"BM", "image/bmp"),
    Sig::Exact(b"GIF87a", "image/gif"),
    Sig::Exact(b"GIF89a", "image/gif"),
    masked(b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF\xFF\xFF", b"RIFF\x00\x00\x00\x00WEBPVP", "image/webp"),
    Sig::Exact(b"\x89PNG\x0D\x0A\x1A\x0A", "image/png"),
    Sig::Exact(b"\xFF\xD8\xFF", "image/jpeg"),
    masked(b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF", b"FORM\x00\x00\x00\x00AIFF", "audio/aiff"),
    masked(b"\xFF\xFF\xFF", b"ID3", "audio/mpeg"),
    masked(b"\xFF\xFF\xFF\xFF\xFF", b"OggS\x00", "application/ogg"),
    masked(b"\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF", b"MThd\x00\x00\x00\x06", "audio/midi"),
    masked(b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF", b"RIFF\x00\x00\x00\x00AVI ", "video/avi"),
    masked(b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF", b"RIFF\x00\x00\x00\x00WAVE", "audio/wave"),
    Sig::Mp4,
    Sig::Exact(b"\x1A\x45\xDF\xA3", "video/webm"),
    masked(
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xFF\xFF",
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00LP",
        "application/vnd.ms-fontobject",
    ),
    Sig::Exact(b"\x00\x01\x00\x00", "font/ttf"),
    Sig::Exact(b"OTTO", "font/otf"),
    Sig::Exact(b"ttcf", "font/collection"),
    Sig::Exact(b"wOFF", "font/woff"),
    Sig::Exact(b"wOF2", "font/woff2"),
    Sig::Exact(b"\x1F\x8B\x08", "application/x-gzip"),
    Sig::Exact(b"PK\x03\x04", "application/zip"),
    Sig::Exact(b"Rar!\x1A\x07\x00", "application/x-rar-compressed"),
    Sig::Exact(b"Rar!\x1A\x07\x01\x00", "application/x-rar-compressed"),
    Sig::Exact(b"\x00\x61\x73\x6D", "application/wasm"),
    Sig::Text,
];

pub fn detect_content_type(data: &[u8]) -> &'static str {
    let data = &data[..data.len().min(SNIFF_LEN)];
    let ws = data.iter().position(|b| !matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')).unwrap_or(data.len());
    SIGNATURES.iter().find_map(|s| s.matches(data, ws)).unwrap_or("application/octet-stream")
}

impl Sig {
    fn matches(&self, data: &[u8], ws: usize) -> Option<&'static str> {
        match *self {
            Sig::Html(h) => {
                let d = &data[ws..];
                if d.len() < h.len() + 1 {
                    return None;
                }
                for (i, &b) in h.iter().enumerate() {
                    let db = if b.is_ascii_uppercase() { d[i] & 0xDF } else { d[i] };
                    if b != db {
                        return None;
                    }
                }
                matches!(d[h.len()], b' ' | b'>').then_some("text/html; charset=utf-8")
            }
            Sig::Masked { mask, pat, skip_ws, ct } => {
                let d = if skip_ws { &data[ws..] } else { data };
                if d.len() < pat.len() {
                    return None;
                }
                pat.iter().enumerate().all(|(i, &p)| d[i] & mask[i] == p).then_some(ct)
            }
            Sig::Exact(sig, ct) => data.starts_with(sig).then_some(ct),
            Sig::Mp4 => {
                if data.len() < 12 {
                    return None;
                }
                let box_size = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
                if data.len() < box_size || !box_size.is_multiple_of(4) || &data[4..8] != b"ftyp" {
                    return None;
                }
                (8..box_size).step_by(4).filter(|&st| st != 12).any(|st| &data[st..st + 3] == b"mp4").then_some("video/mp4")
            }
            Sig::Text => data[ws..]
                .iter()
                .all(|&b| !(b <= 0x08 || b == 0x0B || (0x0E..=0x1A).contains(&b) || (0x1C..=0x1F).contains(&b)))
                .then_some("text/plain; charset=utf-8"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::detect_content_type as d;

    #[test]
    fn known_types() {
        assert_eq!(d(b""), "text/plain; charset=utf-8");
        assert_eq!(d(b"\x89PNG\x0D\x0A\x1A\x0Arest"), "image/png");
        assert_eq!(d(b"\xFF\xD8\xFF\xE0"), "image/jpeg");
        assert_eq!(d(b"  <html><body>"), "text/html; charset=utf-8");
        assert_eq!(d(b"hello world"), "text/plain; charset=utf-8");
        assert_eq!(d(b"\x00\x01\x02\x03\x04"), "application/octet-stream");
        assert_eq!(d(b"RIFF\x01\x02\x03\x04WEBPVP8 "), "image/webp");
        assert_eq!(d(b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom"), "video/mp4");
        assert_eq!(d(b"%PDF-1.7"), "application/pdf");
    }
}
