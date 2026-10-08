fn main() {
    // Clap's derived parser needs more stack in debug builds, so run it on a worker.
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
