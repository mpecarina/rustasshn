# rustasshn

tmux host picker for SSH based on your `~/.ssh/config`.

## Demo

https://github.com/user-attachments/assets/5afb5516-a6af-4c9a-b319-1b9e3e6deecb

## Tmux

Press `prefix + s`.

https://github.com/user-attachments/assets/efc9ac6d-da01-422c-85fa-9f76f9bd3c01

## Install (TPM)

Add this to your `~/.tmux.conf`:

```tmux
set -g @plugin 'mpecarina/rustasshn'

# optional
set -g @rustasshn_key 's'
set -g @rustasshn_launch_mode 'popup'
set -g @rustasshn_enter_mode 'o'

run '~/.tmux/plugins/tpm/tpm'
```

Then in tmux: `prefix + I` to install. A Rust toolchain with `cargo` is required
for the first build.

`bin/rustasshn` is a tracked wrapper that builds `target/release/rustasshn` when
the binary is missing, the git commit changed, or local Rust sources are newer.

## Install (Manual)

1) Clone somewhere:

```sh
git clone https://github.com/mpecarina/rustasshn.git
```

2) Build the binary through the tracked wrapper:

```sh
cd rustasshn
./bin/rustasshn --version
```

The wrapper remains in `bin/`; the compiled binary and commit stamp are kept
under `target/release/`.

3) Source the plugin file from `~/.tmux.conf`:

```tmux
run-shell '/absolute/path/to/rustasshn/rustasshn.tmux'

# tell the plugin where the tracked wrapper is
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


# what Enter does in the picker: o|origin, w|window, v|split-v, s|split-h
# every mode opens the session in a real tmux pane; default is 'o'
set -g @rustasshn_enter_mode 'o'
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

## Requires tmux

The picker only runs inside tmux, and every connect mode opens the session in a
real tmux pane. That is deliberate: a pane is what makes session logging work,
and it keeps a remote that dies mid-escape-sequence from damaging your terminal
directly. To connect without tmux, use the CLI instead: `rustasshn connect
<alias>`, or `rustasshn ssh ...` / `rustasshn scp ...`.

## Popup + Origin Mode

`origin` is the default: you pick a host in a popup and the session opens in the
pane that spawned the popup, so it lands where you were working.

```tmux
set -g @rustasshn_launch_mode "popup"
set -g @rustasshn_enter_mode 'o'
```

Because this respawns that pane, it kills whatever was running there. If you
would rather leave your current pane alone, use one of:

```tmux
set -g @rustasshn_enter_mode 'w'   # new window
set -g @rustasshn_enter_mode 'v'   # vertical split
set -g @rustasshn_enter_mode 's'   # horizontal split
```

`origin` falls back to the current pane when the origin pane is not known
(for example when not launched via popup).

## Example Config

### tmux (`~/.tmux.conf`)

Popup UI starts in search mode, and `Enter` opens in the origin pane:

```tmux
set -g @plugin 'mpecarina/rustasshn'

set -g @rustasshn_launch_mode "popup"
set -g @rustasshn_enter_mode 'o'
set -g @rustasshn_mode 'search'
set -g @rustasshn_key 's'
```

### zsh (`~/.zshrc`)

Wrap `ssh` and `scp` so stored credentials can enable an SSH_ASKPASS layer:

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

Credential storage backend:

- macOS: stored in Keychain.
- Linux: stored in the Freedesktop Secret Service keyring (GNOME Keyring / KWallet).
