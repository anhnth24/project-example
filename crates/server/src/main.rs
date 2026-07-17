fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("fileconv-server\n\nPhase F scaffold; HTTP routes are not available yet.");
        return;
    }
    match fileconv_server::config::ServerConfig::from_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            println!(
                "configuration valid: profile={:?}, bind={}",
                config.profile, config.bind_addr
            );
        }
        Ok(_) => println!("fileconv-server scaffold: no routes are enabled yet"),
        Err(error) => {
            eprintln!("invalid server configuration: {error}");
            std::process::exit(1);
        }
    }
}
