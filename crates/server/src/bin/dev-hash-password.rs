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

#[cfg(test)]
mod tests {
    use super::password_from_inputs;

    #[test]
    fn stdin_mode_accepts_one_lf_or_crlf_terminated_password() {
        assert_eq!(
            password_from_inputs(&["--stdin"], b"correct horse\n").unwrap(),
            "correct horse"
        );
        assert_eq!(
            password_from_inputs(&["--stdin"], b"correct horse\r\n").unwrap(),
            "correct horse"
        );
    }

    #[test]
    fn stdin_mode_rejects_empty_multiline_or_unterminated_input() {
        for invalid in [
            b"\n".as_slice(),
            b"\r\n".as_slice(),
            b"".as_slice(),
            b"one line".as_slice(),
            b"one\nsecond\n".as_slice(),
            b"one\ntrailing".as_slice(),
        ] {
            assert!(password_from_inputs(&["--stdin"], invalid).is_err());
        }
    }

    #[test]
    fn legacy_exactly_one_argv_password_remains_supported() {
        assert_eq!(
            password_from_inputs(&["legacy-password"], b"ignored").unwrap(),
            "legacy-password"
        );
    }

    #[test]
    fn invalid_arity_and_stdin_extra_args_are_rejected() {
        assert!(password_from_inputs(&[], b"").is_err());
        assert!(password_from_inputs(&["first", "second"], b"").is_err());
        assert!(password_from_inputs(&["--stdin", "extra"], b"secret\n").is_err());
        assert!(password_from_inputs(&[""], b"").is_err());
    }
}
