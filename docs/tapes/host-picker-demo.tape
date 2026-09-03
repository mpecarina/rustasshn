# host-picker-demo — fuzzy filtering, favorites, and multi-select over a synthetic SSH inventory.
#
# Render with the gate, not with bare vhs:
#   terminal-rec/scripts/render.sh docs/tapes/host-picker-demo.tape \
#     --expect "favorite added" --expect "Selected: 3"
#
# Geometry is pixels. Budget 150x28.

Output docs/media/host-picker-demo.mp4
Output .terminal-rec/host-picker-demo.txt

Require bash

Set Shell "bash"
Set FontSize 16
Set Width 1600
Set Height 538
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 55ms
Set Framerate 24
Set CursorBlink false
Set BorderRadius 8
Set WindowBar "Colorful"

# Keep the recording isolated from the operator's real SSH inventory and rustasshn state.
Env HOME ".terminal-rec/rustasshn-home"
Env XDG_CONFIG_HOME ".terminal-rec/rustasshn-config"
Env TMUX "terminal-rec-demo"

# Setup nobody should watch. An explicit PS1 keeps the prompt identical on every machine.
Hide
Type "PS1='❯ ' && bash docs/tapes/host-picker-demo.sh setup && clear"
Enter
Sleep 800ms
Show

Type "./bin/rustasshn"
Sleep 1200ms
Enter
Wait+Screen@20s /enter connect/
Sleep 1200ms

# Find the lab router, leave search mode, and favorite it.
Type@120ms "router"
Sleep 1200ms
Escape
Type "f"
Wait+Screen@20s /favorite added/
Sleep 1200ms
Type "F"
Sleep 1200ms
Type "F"

# Replace the query, then select every production host in one keystroke.
Type "/"
Backspace 6
Type@120ms "prod"
Sleep 1200ms
Ctrl+a
Wait+Screen@20s /Selected: 3/
Sleep 1200ms

Screenshot docs/media/host-picker-demo.png

Escape
Type "q"
Sleep 600ms

# Let the end state hold, then stop. Trailing Sleep is dead documentation.
Sleep 800ms
