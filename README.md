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

Inside the picker:

- `Enter` uses `@rustasshn_enter_mode`
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
