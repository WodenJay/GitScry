fn main() {
    // clap's derived augment_subcommands builds the whole command tree in one
    // stack frame (~1 MB in debug builds on Windows); raise the main-thread
    // reserve so startup does not overflow the default 1 MB stack.
    println!("cargo:rustc-link-arg-bins=/STACK:8388608");
}
