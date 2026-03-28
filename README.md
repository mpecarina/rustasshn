# rustasshn

tmux host picker for SSH based on your `~/.ssh/config`.

## Install (TPM)

Add this to your `~/.tmux.conf`:

```tmux
set -g @plugin 'mpecarina/rustasshn'

# optional
set -g @rustasshn_key 's'
set -g @rustasshn_launch_mode 'popup'
set -g @rustasshn_enter_mode 'p'

run '~/.tmux/plugins/tpm/tpm'
```

Then in tmux: `prefix + I` to install.

## Install (Manual)

1) Clone somewhere:

```sh
git clone https://github.com/mpecarina/rustasshn.git
```

2) Build the binary and place it at `bin/rustasshn`:

```sh
cd rustasshn
cargo build --release
mkdir -p bin
cp target/release/rustasshn bin/rustasshn
```

3) Source the plugin file from `~/.tmux.conf`:

```tmux
run-shell '/absolute/path/to/rustasshn/rustasshn.tmux'

# tell the wrapper where the binary is
set -g @rustasshn_bin '/absolute/path/to/rustasshn/bin/rustasshn'
```

Reload tmux config.

## Usage

Default key binding is `s` (configure with `@rustasshn_key`).

Inside the picker there are two input modes:

- `search mode` (text input focused): typing filters the host list
- `command mode` (list focused): single-key actions run (split/window/origin/etc)

If you start in `search` mode (the default), you must press `Esc` to leave search
mode before single-key actions like `v`, `s`, `w`, `t`, `o`, `a`, `c`, `d` will
execute. The exception is `Enter` when `@rustasshn_implicit_select` is enabled.

Inside the picker:

- `Enter` uses `@rustasshn_enter_mode`
- `Esc` leaves search mode (or quits if already in command mode)
- `p` connects in the current pane (the pane running the picker)
- `w` opens in a new tmux window
- `v` opens in a vertical split
- `s` opens in a horizontal split
- `t` opens selected hosts in a tiled layout
- `o` opens in the origin pane (the pane that spawned the popup), if available

## Options

All options are tmux global options.

```tmux
# key used to open the picker
set -g @rustasshn_key 's'

# path to the rustasshn binary
set -g @rustasshn_bin '~/.../bin/rustasshn'

# where the picker UI is shown: popup | pane | window
set -g @rustasshn_launch_mode 'popup'


# picker start mode (passed as --mode); use 'normal' to start not-searching
set -g @rustasshn_mode 'search'


# implicit select behavior (passed as --implicit-select=false when off)
set -g @rustasshn_implicit_select 'true'


# what Enter does in the picker: p|pane, w|window, v|split-v, s|split-h, o|origin
set -g @rustasshn_enter_mode 'p'
```

### Search Mode vs Normal Mode

`@rustasshn_mode` controls what the picker focuses on when it opens:

- `search`: the `/` prompt is focused; keystrokes are treated as search input
- `normal`: the list is focused; keystrokes are treated as commands/navigation

Notes:

- In `search` mode, press `Esc` to switch to command mode.
- `Enter` behavior depends on `@rustasshn_implicit_select`:
  - when `true`: `Enter` exits search and immediately runs the configured `@rustasshn_enter_mode`
  - when `false`: `Enter` only exits search; press `Enter` again in command mode to run

## Popup + Origin Mode

If you launch the picker as a tmux popup, the default `p`/`pane` enter mode will
connect inside the popup pane.

To pick hosts in a popup but connect in the pane that spawned the popup:

```tmux
set -g @rustasshn_launch_mode "popup"
set -g @rustasshn_enter_mode 'o'
```

`origin` falls back to the current behavior when the origin pane is not known
(for example when not launched via popup).

## Example Config (Matches My Setup)

### tmux (`~/.tmux.conf`)

This matches the defaults I run (popup UI, start in search, and `Enter` opens in
the origin pane that spawned the popup):

```tmux
set -g @plugin 'mpecarina/rustasshn'

set -g @rustasshn_launch_mode "popup"
set -g @rustasshn_enter_mode 'o'
set -g @rustasshn_mode 'search'
set -g @rustasshn_key 's'
```

### zsh (`~/.zshrc`)

I use `rustasshn` as the default `ssh`/`scp` so stored credentials can enable an
SSH_ASKPASS layer automatically:

```zsh
# Make sure the rustasshn binary is on PATH (example path if installed via TPM)
export PATH="$PATH:$HOME/.tmux/plugins/rustasshn/bin"

# Wrap ssh/scp so rustasshn can enable askpass when a stored password exists
alias ssh='rustasshn ssh'
alias scp='rustasshn scp'
```

Optional: a fuzzy menu bound to Alt+s that uses `rustasshn list` + `rustasshn connect`:

```zsh
rustasshn-menu() {
  local sel
  sel="$(fzf --prompt='ssh ' < <(rustasshn list))" || return
  [[ -z "$sel" ]] && return

  # Use connect (not ssh) so the same askpass + stdin sanitization path is used.
  </dev/tty >/dev/tty 2>/dev/tty rustasshn connect "$sel"
}

tssm-run() {
  zle -I
  rustasshn-menu
}
zle -N tssm-run
bindkey '^[s' tssm-run   # Alt+s

# Optional convenience alias/function
s() {
  rustasshn-menu
}
```

### Credential Storage (Askpass)

- In the picker UI: press `c` to store a credential, `d` to delete.
- From the CLI: `rustasshn cred set --host <alias> --user <user> --kind password`
