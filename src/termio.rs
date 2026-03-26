use anyhow::Result;

#[cfg(unix)]
pub fn sanitize_stdin_before_exec() -> Result<()> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    use nix::unistd::{isatty, read};
    use std::time::{Duration, Instant};

    let fd = 0;
    if !isatty(fd).unwrap_or(false) {
        return Ok(());
    }

    let orig = fcntl(fd, FcntlArg::F_GETFL)?;
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
    Ok(())
}

#[cfg(not(unix))]
pub fn sanitize_stdin_before_exec() -> Result<()> {
    Ok(())
}
