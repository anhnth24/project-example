fn main() {
    if std::env::args().any(|argument| argument == "--help" || argument == "-h") {
        println!("fileconv-server\n\nPhase F scaffold; HTTP routes are not available yet.");
        return;
    }
    if let Err(error) = fileconv_server::validate_configuration() {
        eprintln!("invalid server configuration: {error}");
        std::process::exit(1);
    }
    println!("fileconv-server scaffold: no routes are enabled yet");
}
