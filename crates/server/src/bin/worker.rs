fn main() {
    if std::env::args().any(|argument| argument == "--help" || argument == "-h") {
        println!("fileconv-worker\n\nPhase F scaffold; no jobs are enabled yet.");
        return;
    }
    if let Err(error) = fileconv_server::validate_configuration() {
        eprintln!("invalid worker configuration: {error}");
        std::process::exit(1);
    }
    println!("fileconv-worker scaffold: no jobs are enabled yet");
}
