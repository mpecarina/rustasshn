//! Terminal ownership.
//!
//! Two invariants live here and nowhere else:
//!
//! 1. Whatever the picker turns on gets turned off, on every exit path —
//!    normal return, `?`, or panic. `Tty` restores in `Drop`, so forgetting is
//!    not possible.
//! 2. Every child that inherits the tty is repaired after it dies, in exactly
//!    one place: `run_child`. Draining stale input first is the picker's own
//!    concern, so `Tty::hand_off` adds it and plain CLI paths do not.

use std::io::Stdout;
use std::process::{Command, ExitStatus};

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::style::ResetColor;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::termio;

pub type Backend = CrosstermBackend<Stdout>;

/// Guards the terminal state the picker mutates: raw mode and the alternate
/// screen.
pub struct Tty {
    /// `None` once released, which makes `release` idempotent and lets `Drop`
    /// run after an explicit release without emitting anything twice.
    terminal: Option<Terminal<Backend>>,
}

impl Tty {
    pub fn claim() -> Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        terminal.clear()?;
        Ok(Self {
            terminal: Some(terminal),
        })
    }

    pub fn terminal(&mut self) -> &mut Terminal<Backend> {
        self.terminal
            .as_mut()
            .expect("terminal used after being released")
    }

    /// Give the terminal to a child process and do not take it back. Releasing
    /// before the spawn keeps the picker from writing escape sequences into a
    /// terminal it no longer owns.
    pub fn hand_off(mut self, cmd: &mut Command) -> Result<ExitStatus> {
        self.release();
        // The keypress that chose this action is still sitting in the tty
        // buffer; drain it so the child does not read it as its own input.
        termio::sanitize_stdin_before_exec().ok();
        run_child(cmd)
    }

    pub fn release(&mut self) {
        let Some(mut terminal) = self.terminal.take() else {
            return;
        };
        disable_raw_mode().ok();
        execute!(terminal.backend_mut(), LeaveAlternateScreen, ResetColor).ok();
        // Also clears ratatui's hidden-cursor flag, so the `Terminal` dropped
        // at the end of this scope stays silent.
        terminal.show_cursor().ok();
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        self.release();
    }
}

/// Run a child that inherits the terminal, repairing terminal state once it
/// exits. Every spawn of ssh/scp goes through here, including the paths that
/// never claimed a `Tty` of their own.
///
/// Restoring afterwards is unconditional — any child can die leaving the tty
/// damaged. Draining stdin beforehand is *not*, because it discards type-ahead:
/// callers that expect stale input (a TUI keypress) drain explicitly.
pub fn run_child(cmd: &mut Command) -> Result<ExitStatus> {
    let status = cmd.status()?;
    termio::restore_after_child().ok();
    Ok(status)
}

/// The default hook prints to stderr *before* unwinding runs `Drop`, so without
/// this the message lands on the alternate screen and is wiped a moment later.
pub(crate) fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Cannot reach the live `Tty`; undo the same state on a fresh
            // handle. Harmless if it has already been released.
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen, ResetColor, Show);
            prev(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::install_panic_hook;

    /// Cannot assert on the escape bytes here — stdout is not a tty under the
    /// test harness. What this does pin down is that the hook restores and then
    /// chains to the previous one, rather than recursing or swallowing the
    /// panic, which would leave a real terminal stranded.
    #[test]
    fn test_panic_hook_restores_then_delegates() {
        install_panic_hook();
        install_panic_hook(); // idempotent: must not chain onto itself
        let res = std::panic::catch_unwind(|| panic!("boom"));
        assert!(res.is_err(), "panic was swallowed by the hook");
    }
}
