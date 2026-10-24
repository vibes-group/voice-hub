//! POST /api/upload?room= and GET /api/file/{id}?room= for transient chat
//! attachments.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use http::{StatusCode, header};
use http_body_util::BodyExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use super::handlers::go_quote;
use super::server::App;
use super::sniff::{SNIFF_LEN, detect_content_type};
use super::{Req, Resp, error, json, not_found, path_escape, percent_decode, query, set_header};
use crate::log;

#[derive(Serialize)]
struct UploadResponse<'a> {
    #[serde(rename = "uploadId")]
    upload_id: &'a str,
}

enum Fail {
    BadRequest,
    Io(String),
}

/// Streams the body into a temp file and registers it. Unknown rooms are
/// rejected so a stray upload can't occupy the budget.
pub async fn upload(app: &App, req: Req) -> Resp {
    let (parts, body) = req.into_parts();
    let q = query(&parts.uri);
    let room = q.get("room").map(String::as_str).unwrap_or("");
    if room.is_empty() || !app.rooms.contains_key(room) {
        return not_found();
    }
    // Same allowance as http.MaxBytesReader(max+1).
    let limit = app.files.max_upload_bytes() + 1;
    let ct = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let media = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let too_large = Arc::new(AtomicBool::new(false));
    let raw = limited(body.into_data_stream(), limit, too_large.clone());

    let temp = app.files.temp_path();
    let result = if media.starts_with("multipart/") {
        receive_multipart(&ct, raw, &temp).await
    } else {
        let name =
            filename_from_content_disposition(parts.headers.get(header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).unwrap_or(""));
        copy_to(&temp, raw).await.map(|(size, head)| (name, size, head))
    };
    let (name, size, head) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return match e {
                _ if too_large.load(Ordering::Acquire) => error(StatusCode::PAYLOAD_TOO_LARGE, "file too large"),
                Fail::BadRequest => error(StatusCode::BAD_REQUEST, "bad request"),
                Fail::Io(msg) => {
                    log!("upload: copy: {msg}");
                    error(StatusCode::INTERNAL_SERVER_ERROR, "upload failed")
                }
            };
        }
    };

    let mime = detect_content_type(&head);
    match app.files.store(room, &sanitize_name(&name), mime, size, temp.clone()) {
        Ok(entry) => json(StatusCode::CREATED, &UploadResponse { upload_id: &entry.upload_id }),
        Err(_) => {
            let _ = tokio::fs::remove_file(&temp).await;
            error(StatusCode::INSUFFICIENT_STORAGE, "server storage full")
        }
    }
}

/// The first part carrying a filename is the upload.
async fn receive_multipart<S>(ct: &str, raw: S, temp: &std::path::Path) -> Result<(String, u64, Vec<u8>), Fail>
where
    S: Stream<Item = std::io::Result<Bytes>> + Send + 'static,
{
    let boundary = multer::parse_boundary(ct).map_err(|_| Fail::BadRequest)?;
    let mut mp = multer::Multipart::new(raw, boundary);
    while let Some(field) = mp.next_field().await.map_err(|_| Fail::BadRequest)? {
        if let Some(name) = field.file_name().filter(|n| !n.is_empty()).map(str::to_string) {
            return copy_to(temp, field).await.map(|(size, head)| (name, size, head));
        }
    }
    Err(Fail::BadRequest)
}

/// Caps the byte stream at `limit`, flagging `too_large` when it trips.
fn limited<S, E>(s: S, limit: u64, too_large: Arc<AtomicBool>) -> impl Stream<Item = std::io::Result<Bytes>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display,
{
    let mut seen: u64 = 0;
    s.map(move |chunk| {
        let chunk = chunk.map_err(|e| std::io::Error::other(e.to_string()))?;
        seen += chunk.len() as u64;
        if seen > limit {
            too_large.store(true, Ordering::Release);
            return Err(std::io::Error::other("request body too large"));
        }
        Ok(chunk)
    })
}

async fn copy_to<S, E>(path: &std::path::Path, mut s: S) -> Result<(u64, Vec<u8>), Fail>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut f = open_temp(path).await.map_err(|e| Fail::Io(format!("create temp: {e}")))?;
    let mut head = Vec::with_capacity(SNIFF_LEN);
    let mut size = 0u64;
    while let Some(chunk) = s.next().await {
        let chunk = chunk.map_err(|e| Fail::Io(e.to_string()))?;
        if head.len() < SNIFF_LEN {
            let take = (SNIFF_LEN - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        size += chunk.len() as u64;
        f.write_all(&chunk).await.map_err(|e| Fail::Io(e.to_string()))?;
    }
    f.flush().await.map_err(|e| Fail::Io(format!("close temp: {e}")))?;
    Ok((size, head))
}

async fn open_temp(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).await
}

/// Serves the file (images inline, else attachment) and extends its TTL.
pub async fn download(app: &App, req: Req, id: &str) -> Resp {
    let room = query(req.uri()).get("room").cloned().unwrap_or_default();
    // Room scoping: the id alone shouldn't grant cross-room access.
    let Some(entry) = app.files.get(id).filter(|e| e.room_id == room) else {
        return not_found();
    };
    if !entry.temp_path.exists() {
        return not_found(); // raced with eviction
    }
    let mut r = super::serve_fs(tower_http::services::ServeFile::new(&entry.temp_path), req).await;
    if r.status() != StatusCode::NOT_FOUND {
        let disposition = if entry.mime.starts_with("image/") { "inline" } else { "attachment" };
        set_header(&mut r, "content-type", &entry.mime);
        set_header(&mut r, "content-disposition", &content_disposition(disposition, &entry.name));
        app.files.touch(id);
    }
    r
}

/// Strips any path component so a name can't escape or mislead; empty → "file".
pub fn sanitize_name(name: &str) -> String {
    let name = name.trim();
    let name = match name.rfind(['/', '\\']) {
        Some(i) => &name[i + 1..],
        None => name,
    };
    if name.is_empty() { "file".into() } else { name.to_string() }
}

/// ASCII-safe filename plus the RFC 5987 UTF-8 form.
pub fn content_disposition(disposition: &str, filename: &str) -> String {
    if filename.is_empty() {
        return disposition.to_string();
    }
    let ascii: String =
        filename.chars().map(|c| if (c as u32) < 0x20 || (c as u32) > 0x7e || c == '"' || c == '\\' { '_' } else { c }).collect();
    format!("{disposition}; filename={}; filename*=UTF-8''{}", go_quote(&ascii), path_escape(filename))
}

/// `filename` from Content-Disposition, decoding the RFC 5987/2231 extended
/// form (`filename*=UTF-8''...`) like mime.ParseMediaType. Malformed → "".
pub fn filename_from_content_disposition(header: &str) -> String {
    parse_params(header).and_then(|mut p| p.remove("filename")).unwrap_or_default()
}

fn parse_params(header: &str) -> Option<HashMap<String, String>> {
    let mut parts = split_params(header)?.into_iter();
    let media = parts.next()?;
    if media.trim().is_empty() {
        return None;
    }
    let mut plain = HashMap::new();
    let mut ext = HashMap::new();
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (k, v) = p.split_once('=')?;
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        let v =
            if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) { unescape_quoted(inner) } else { v.to_string() };
        if let Some(base) = k.strip_suffix('*') {
            ext.insert(base.to_string(), decode_ext(&v)?);
        } else {
            plain.insert(k, v);
        }
    }
    plain.extend(ext);
    Some(plain)
}

fn split_params(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => {
                cur.push(c);
                escaped = true;
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if quoted {
        return None;
    }
    out.push(cur);
    Some(out)
}

fn unescape_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut esc = false;
    for c in s.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// `charset'lang'percent-encoded` (RFC 2231). Only UTF-8 and US-ASCII.
fn decode_ext(v: &str) -> Option<String> {
    let mut it = v.splitn(3, '\'');
    let charset = it.next()?.to_ascii_lowercase();
    let _lang = it.next()?;
    let enc = it.next()?;
    let bytes = percent_decode(enc)?;
    match charset.as_str() {
        "utf-8" | "us-ascii" | "" => String::from_utf8(bytes).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("a\\b.txt"), "b.txt");
        assert_eq!(sanitize_name("  "), "file");
        assert_eq!(filename_from_content_disposition("attachment; filename*=UTF-8''%D1%84%D0%B0%D0%B9%D0%BB.txt"), "файл.txt");
        assert_eq!(filename_from_content_disposition("attachment; filename=\"a b.txt\""), "a b.txt");
        assert_eq!(filename_from_content_disposition("attachment; filename=a.txt; filename*=UTF-8''b.txt"), "b.txt");
        assert_eq!(filename_from_content_disposition(""), "");
        assert_eq!(
            content_disposition("inline", "фото.png"),
            "inline; filename=\"____.png\"; filename*=UTF-8''%D1%84%D0%BE%D1%82%D0%BE.png"
        );
    }
}
