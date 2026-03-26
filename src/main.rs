mod app;
#[cfg(test)]
mod app_tests;
mod credentials;
mod sshconfig;
mod state;
mod termio;
mod tmuxrun;
mod ui;

fn main() {
    if let Err(err) = app::run(std::env::args_os()) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
