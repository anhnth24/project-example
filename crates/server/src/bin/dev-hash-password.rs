//! Dev-only helper: print an Argon2id PHC hash for bootstrap scripts.
//! Uses the same defaults as `AuthConfig` / `Argon2Config::defaults()`.

use fileconv_server::auth::password;
use fileconv_server::config::Argon2Config;

fn main() {
    let password = std::env::args()
        .nth(1)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            eprintln!("usage: dev-hash-password <password>");
            std::process::exit(2);
        });
    let hash =
        password::hash_password(&password, &Argon2Config::defaults()).unwrap_or_else(|error| {
            eprintln!("dev-hash-password: {error}");
            std::process::exit(1);
        });
    print!("{}", hash.expose());
}
