use anyhow::Result;

/// Escape sequences that undo terminal state a remote session may have left
/// behind when it died: the alternate screen, mouse and focus reporting,
/// bracketed paste, application keypad, alternate charsets, a clamped scroll
/// region, stale SGR, hidden cursor.
///
/// The leading CAN (0x18) aborts any escape sequence the remote truncated
/// mid-flight, so the reset itself is not swallowed by a half-parsed CSI.
/// Deliberately not RIS (`ESC c`) — that clears scrollback on many terminals.
///
/// Ordering matters twice: CAN first, so nothing else is eaten by a pending
/// sequence, and the alternate-screen exit early, so everything after it applies
/// to the screen the user is actually left looking at.
const TTY_RESTORE: &str = concat!(
    "\x18",        // CAN: abort a partially received escape sequence
    "\x1b[?1049l", // leave the alternate screen (a killed remote vim/htop/less)
    "\x1b[?1047l", // ditto, pre-1049 terminals; a no-op once back on normal
    "\x0f",        // SI: shift GL back to G0 (undoes a bare SO, 0x0e)
    "\x1b(B",      // G0 = ASCII, undoing a line-drawing charset
    "\x1b)B",      // G1 = ASCII
    "\x1b[?1000l", // mouse: normal tracking off
    "\x1b[?1002l", // mouse: button-event tracking off
    "\x1b[?1003l", // mouse: any-event tracking off
    "\x1b[?1005l", // mouse: UTF-8 extended reports off
    "\x1b[?1006l", // mouse: SGR extended reports off
    "\x1b[?1015l", // mouse: urxvt extended reports off
    "\x1b[?1004l", // focus reporting off (else focus changes type ESC[I/ESC[O)
    "\x1b[?2004l", // bracketed paste off
    "\x1b[?2026l", // synchronized output off, else rendering can stay frozen
    "\x1b[?1l",    // normal (not application) cursor keys
    "\x1b>",       // numeric (not application) keypad
    "\x1b[?7h",    // autowrap on
    "\x1b[r",      // full-screen scroll region
    "\x1b[m",      // SGR reset
    "\x1b[?25h",   // cursor visible
);

#[cfg(unix)]
pub fn sanitize_stdin_before_exec() -> Result<()> {
    drain_tty_input();
    Ok(())
}

/// Undo terminal damage left by a child that owned the tty (an ssh session that
/// dropped mid-sequence, a remote app killed with modes still set).
///
/// Drains first, so bytes queued during teardown — deferred DA1/DSR replies,
/// mouse reports — are consumed here instead of being read as keystrokes by the
/// next process in the pane.
#[cfg(unix)]
pub fn restore_after_child() -> Result<()> {
    drain_tty_input();
    restore_sane_termios();
    write_tty_restore();
    Ok(())
}

#[cfg(unix)]
fn drain_tty_input() {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    use nix::unistd::{isatty, read};
    use std::time::{Duration, Instant};

    let fd = 0;
    if !isatty(fd).unwrap_or(false) {
        return;
    }

    let orig = match fcntl(fd, FcntlArg::F_GETFL) {
        Ok(v) => v,
        Err(_) => return,
    };
    let orig_flags = OFlag::from_bits_truncate(orig);
    let mut flags = orig_flags;
    flags.insert(OFlag::O_NONBLOCK);
    let _ = fcntl(fd, FcntlArg::F_SETFL(flags));

    let start = Instant::now();
    let mut last_read = start;
    let max_total = Duration::from_millis(500);
    let quiet_for = Duration::from_millis(50);
    let sleep_step = Duration::from_millis(10);
    let mut buf = [0u8; 4096];
    loop {
        match read(fd, &mut buf) {
            Ok(0) => break,
            Ok(_n) => {
                last_read = Instant::now();
                continue;
            }
            Err(e) => {
                if e == nix::errno::Errno::EAGAIN || e == nix::errno::Errno::EWOULDBLOCK {
                    if last_read.elapsed() >= quiet_for {
                        break;
                    }
                    if start.elapsed() >= max_total {
                        break;
                    }
                    std::thread::sleep(sleep_step);
                    continue;
                }
                break;
            }
        }
    }

    let _ = fcntl(fd, FcntlArg::F_SETFL(orig_flags));
}

/// Re-enable the line-discipline flags an interactive shell needs. Only ORs
/// bits back in — a child killed before it could restore termios leaves the tty
/// raw and echoless, but anything the user deliberately set stays set.
#[cfg(unix)]
fn restore_sane_termios() {
    use nix::sys::termios::{InputFlags, LocalFlags, OutputFlags, SetArg, tcgetattr, tcsetattr};
    use nix::unistd::isatty;

    if !isatty(0).unwrap_or(false) {
        return;
    }
    let stdin = std::io::stdin();
    let Ok(mut t) = tcgetattr(&stdin) else {
        return;
    };
    t.input_flags |= InputFlags::BRKINT | InputFlags::ICRNL | InputFlags::IXON;
    t.output_flags |= OutputFlags::OPOST | OutputFlags::ONLCR;
    t.local_flags |= LocalFlags::ECHO
        | LocalFlags::ECHOE
        | LocalFlags::ECHOK
        | LocalFlags::ICANON
        | LocalFlags::IEXTEN
        | LocalFlags::ISIG;
    let _ = tcsetattr(&stdin, SetArg::TCSANOW, &t);
}

/// Write the reset to the terminal, preferring stdout but falling back to
/// /dev/tty so a redirected stdout does not swallow it (or get polluted).
#[cfg(unix)]
fn write_tty_restore() {
    use nix::unistd::isatty;
    use std::io::Write;

    if isatty(1).unwrap_or(false) {
        let mut out = std::io::stdout();
        let _ = out.write_all(TTY_RESTORE.as_bytes());
        let _ = out.flush();
        return;
    }
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(TTY_RESTORE.as_bytes());
        let _ = tty.flush();
    }
}

#[cfg(not(unix))]
pub fn sanitize_stdin_before_exec() -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
pub fn restore_after_child() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TTY_RESTORE;

    #[test]
    fn test_restore_aborts_pending_sequence_first() {
        assert!(TTY_RESTORE.starts_with('\u{18}'));
    }

    #[test]
    fn test_restore_covers_modes_a_remote_can_leave_set() {
        for seq in [
            "\x1b[?1049l",
            "\x1b[?1047l",
            "\x0f",
            "\x1b(B",
            "\x1b)B",
            "\x1b[?1000l",
            "\x1b[?1002l",
            "\x1b[?1003l",
            "\x1b[?1005l",
            "\x1b[?1006l",
            "\x1b[?1015l",
            "\x1b[?1004l",
            "\x1b[?2004l",
            "\x1b[?2026l",
            "\x1b[?1l",
            "\x1b>",
            "\x1b[?7h",
            "\x1b[r",
            "\x1b[m",
            "\x1b[?25h",
        ] {
            assert!(TTY_RESTORE.contains(seq), "missing {seq:?}");
        }
    }

    /// A remote full-screen app killed with the connection is the most common
    /// way to end up staring at a stranded alternate screen, so the exit has to
    /// come before the state we want applied to the normal screen.
    #[test]
    fn test_restore_leaves_alternate_screen_early() {
        let alt = TTY_RESTORE.find("\x1b[?1049l").expect("no alt-screen exit");
        let sgr = TTY_RESTORE.find("\x1b[m").expect("no SGR reset");
        let cursor = TTY_RESTORE.find("\x1b[?25h").expect("no cursor show");
        assert!(alt < sgr && alt < cursor);
        // CAN still has to be the very first byte.
        assert!(TTY_RESTORE.starts_with('\u{18}'));
        assert!(alt < TTY_RESTORE.find("\x1b[?1047l").unwrap());
    }

    #[test]
    fn test_restore_does_not_clear_scrollback() {
        assert!(!TTY_RESTORE.contains("\x1bc"));
    }
}
