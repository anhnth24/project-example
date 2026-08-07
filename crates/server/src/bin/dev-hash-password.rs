//! Dev-only helper: print an Argon2id PHC hash for bootstrap scripts.
//! Uses the same defaults as `AuthConfig` / `Argon2Config::defaults()`.

use std::io::Read;

use fileconv_server::auth::password;
use fileconv_server::config::Argon2Config;

const USAGE: &str = "usage: dev-hash-password <password> | dev-hash-password --stdin";
const MAX_STDIN_BYTES: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputError {
    Usage,
    InvalidStdin,
}

fn password_from_inputs(args: &[&str], stdin: &[u8]) -> Result<String, InputError> {
    match args {
        ["--stdin"] => parse_stdin_password(stdin),
        [password] if !password.is_empty() => Ok((*password).to_owned()),
        _ => Err(InputError::Usage),
    }
}

fn parse_stdin_password(input: &[u8]) -> Result<String, InputError> {
    let terminated = input.strip_suffix(b"\n").ok_or(InputError::InvalidStdin)?;
    let line = terminated.strip_suffix(b"\r").unwrap_or(terminated);
    if line.is_empty() || line.contains(&b'\n') || line.contains(&b'\r') {
        return Err(InputError::InvalidStdin);
    }
    String::from_utf8(line.to_vec()).map_err(|_| InputError::InvalidStdin)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdin = if args.as_slice() == ["--stdin"] {
        let mut input = Vec::new();
        if std::io::stdin()
            .take((MAX_STDIN_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .is_err()
            || input.len() > MAX_STDIN_BYTES
        {
            eprintln!("dev-hash-password: invalid stdin password input");
            std::process::exit(2);
        }
        input
    } else {
        Vec::new()
    };
    let borrowed_args: Vec<&str> = args.iter().map(String::as_str).collect();
    let password = password_from_inputs(&borrowed_args, &stdin).unwrap_or_else(|error| {
        match error {
            InputError::Usage => eprintln!("{USAGE}"),
            InputError::InvalidStdin => {
                eprintln!("dev-hash-password: invalid stdin password input");
            }
        }
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
