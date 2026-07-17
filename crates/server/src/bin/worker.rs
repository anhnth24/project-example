fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("fileconv-worker\n\nPhase F scaffold; no jobs are enabled yet.");
        return;
    }
    match fileconv_server::config::ServerConfig::from_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            println!(
                "configuration valid: profile={:?}, bind={}",
                config.profile, config.bind_addr
            );
        }
        Ok(_) => println!("fileconv-worker scaffold: no jobs are enabled yet"),
        Err(error) => {
            eprintln!("invalid worker configuration: {error}");
            std::process::exit(1);
        }
    }
}
