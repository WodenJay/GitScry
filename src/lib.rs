mod analysis;
mod app;
mod cache;
mod cli;
mod git;
mod github;
mod render;
#[doc(hidden)]
pub mod runtime;
mod semantic;

pub fn run() -> i32 {
    match cli::parse(std::env::args_os()) {
        Ok(cli::Parsed::Command(command)) => {
            let invocation = command
                .usage_name()
                .and_then(cache::usage::Invocation::begin);
            let exit_code = render::run(*command);
            if let Some(invocation) = invocation {
                invocation.finish();
            }
            exit_code
        }
        Ok(cli::Parsed::Display(text)) => render::display(&text),
        Err(error) => render::finish(Err(error)),
    }
}
