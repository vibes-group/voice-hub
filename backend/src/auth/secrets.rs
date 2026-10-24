use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::util::{hex, random_bytes};

/// Reads `n` bytes from dir/name; if missing or short, generates and persists
/// them. Bootstraps session/TURN secrets on first run without operator effort.
pub fn load_or_create(dir: &Path, name: &str, n: usize) -> Result<Vec<u8>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    let path = dir.join(name);
    match std::fs::read(&path) {
        Ok(data) if data.len() >= n => return Ok(data[..n].to_vec()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read {name}: {e}")),
    }
    let mut secret = vec![0u8; n];
    getrandom::fill(&mut secret).map_err(|e| format!("rand: {e}"))?;
    atomic_write(&path, &secret, 0o600).map_err(|e| format!("write {name}: {e}"))?;
    Ok(secret)
}

pub fn atomic_write(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!("{base}.tmp-{}", hex(&random_bytes::<6>())));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
