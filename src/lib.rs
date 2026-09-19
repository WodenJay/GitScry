mod analysis;
mod app;
mod cache;
mod cli;
mod git;
mod render;

pub fn run() -> i32 {
    let result = cli::parse(std::env::args_os()).and_then(app::execute);
    render::finish(result)
}
