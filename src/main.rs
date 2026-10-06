fn main() {
    // Keep the debug build viable: clap's derived parser for this many subcommands
    // overflows the default 1 MiB main-thread stack before reaching `run`.
    let code = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(gitscry::run)
        .and_then(|handle| {
            handle
                .join()
                .map_err(|_| std::io::Error::other("main panicked"))
        })
        .expect("run gitscry on a worker thread");
    std::process::exit(code);
}
