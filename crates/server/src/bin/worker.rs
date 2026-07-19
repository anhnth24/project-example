fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "fileconv-worker\n\nPhase 1B worker runtime; job handlers are enabled by later issues."
        );
        return;
    }
    match fileconv_server::config::ServerConfig::from_worker_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            match fileconv_server::state::RuntimeState::from_config(config) {
                Ok(state) => println!(
                    "configuration valid: profile={:?}, bind={}",
                    state.config().profile(),
                    state.config().bind_addr()
                ),
                Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
            }
        }
        Ok(config) => match fileconv_server::state::RuntimeState::from_config(config) {
            Ok(_) => println!("fileconv-worker: no runnable job handlers are enabled yet"),
            Err(error) => exit_with_error(format!("invalid worker configuration: {error}")),
        },
        Err(error) => {
            exit_with_error(format!("invalid worker configuration: {error}"));
        }
    }
}

fn exit_with_error(error: String) -> ! {
    eprintln!("fileconv-worker: {error}");
    std::process::exit(1);
}
