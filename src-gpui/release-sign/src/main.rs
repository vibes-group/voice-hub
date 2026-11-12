use base64::Engine;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let path = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or("Pass the GPUI executable path")?;
    let encoded = std::env::var("TAURI_SIGNING_PRIVATE_KEY").map_err(|_| "Missing signing key")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| "Invalid signing key encoding")?;
    let key_box = minisign::SecretKeyBox::from_string(
        std::str::from_utf8(&bytes).map_err(|_| "Invalid signing key")?,
    )
    .map_err(|_| "Invalid signing key")?;
    let key = minisign::SecretKey::from_box(key_box, Some(String::new()))
        .map_err(|_| "Cannot unlock signing key")?;
    let file = std::fs::File::open(&path).map_err(|_| "Cannot read release executable")?;
    let comment = format!(
        "Voice Hub GPUI {}",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid executable filename")?
    );
    let signature = minisign::sign(
        None,
        &key,
        file,
        Some(&comment),
        Some("Voice Hub GPUI release"),
    )
    .map_err(|_| "Cannot sign release executable")?;
    // Verify with the committed updater key before publishing any signature.
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../src-tauri/tauri.conf.json"))
            .map_err(|_| "Cannot read updater key")?;
    let public = base64::engine::general_purpose::STANDARD
        .decode(
            config["plugins"]["updater"]["pubkey"]
                .as_str()
                .ok_or("Missing updater key")?,
        )
        .map_err(|_| "Invalid updater key")?;
    let public = minisign_verify::PublicKey::decode(
        std::str::from_utf8(&public).map_err(|_| "Invalid updater key")?,
    )
    .map_err(|_| "Invalid updater key")?;
    let signature_text = signature.to_string();
    let signature = minisign_verify::Signature::decode(&signature_text)
        .map_err(|_| "Invalid generated signature")?;
    let mut verify = public
        .verify_stream(&signature)
        .map_err(|_| "Signing key does not match updater key")?;
    use std::io::Read;
    let mut file = std::fs::File::open(&path).map_err(|_| "Cannot read release executable")?;
    let mut buffer = [0; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Cannot read release executable")?;
        if count == 0 {
            break;
        }
        verify.update(&buffer[..count]);
    }
    verify
        .finalize()
        .map_err(|_| "Signing key does not match updater key")?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature_text);
    std::fs::write(path.with_extension("exe.sig"), encoded)
        .map_err(|_| "Cannot write signature")?;
    println!("Signed and verified GPUI executable");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_are_compatible_with_native_updater_and_reject_tampering() {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let signature = minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            &b"release-executable"[..],
            Some("Voice Hub GPUI test"),
            None,
        )
        .unwrap();
        let public =
            minisign_verify::PublicKey::decode(&pair.pk.to_box().unwrap().to_string()).unwrap();
        let signature = minisign_verify::Signature::decode(&signature.to_string()).unwrap();
        public
            .verify(b"release-executable", &signature, false)
            .unwrap();
        assert!(
            public
                .verify(b"modified-executable", &signature, false)
                .is_err()
        );
        assert!(
            base64::engine::general_purpose::STANDARD
                .decode("invalid signature")
                .is_err()
        );
    }
}
