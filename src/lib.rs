mod analysis;
mod app;
mod cache;
mod cli;
mod git;
mod render;

pub fn run() -> i32 {
    match cli::parse(std::env::args_os()) {
        Ok(cli::Parsed::Command(command)) => render::run(command),
        Ok(cli::Parsed::Display(text)) => render::display(&text),
        Err(error) => render::finish(Err(error)),
    }
}
