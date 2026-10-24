//! Prints the argon2id hash for APP_ADMIN_PASSWORD_HASH. The password comes
//! from stdin so it stays out of argv and shell history:
//!
//!     read -rs P && printf %s "$P" | cargo run -q --release --bin hashpass; unset P

use std::io::{IsTerminal, Read};

/// The hash is brute-forceable offline by anyone who can read the host's
/// .env, so a short password is only as safe as the host.
const MIN_RECOMMENDED_LEN: usize = 16;

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn main() {
    if std::io::stdin().is_terminal() {
        fail("pipe the password on stdin: read -rs P && printf %s \"$P\" | cargo run -q --release --bin hashpass; unset P");
    }
    let mut buf = Vec::new();
    if let Err(e) = std::io::stdin().take(4096).read_to_end(&mut buf) {
        fail(&format!("read stdin: {e}"));
    }
    let plain = String::from_utf8_lossy(&buf);
    let plain = plain.trim_end_matches(['\r', '\n']);
    if plain.is_empty() {
        fail("empty password");
    }
    if plain.len() < MIN_RECOMMENDED_LEN {
        eprintln!("warning: password is shorter than {MIN_RECOMMENDED_LEN} characters");
    }
    println!("{}", voice_hub::auth::password::hash(plain));
}
