# rustasshn-tmux-demo — open the host picker through its real tmux popup binding.
#
# Render with:
#   terminal-rec/scripts/render.sh docs/tapes/rustasshn-tmux-demo.tape \
#     --expect "lab-routeros" --expect "enter connect" --expect "rustasshn"

Output docs/media/rustasshn-tmux-demo.mp4
Output .terminal-rec/rustasshn-tmux-demo.txt

Require bash
Require tmux

Set Shell "bash"
Set FontSize 16
Set Width 1600
Set Height 600
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 55ms
Set Framerate 24
Set CursorBlink false
Set BorderRadius 8
Set WindowBar "Colorful"

Hide
Type "PS1='❯ ' && bash docs/tapes/rustasshn-tmux-demo.sh setup && clear"
Enter
Type "bash docs/tapes/rustasshn-tmux-demo.sh attach"
Enter
Wait+Screen@20s /rustasshn/
Sleep 500ms
Show
Sleep 1200ms

Ctrl+b
Sleep 500ms
Type "s"
Wait+Screen@20s /enter connect/
Sleep 1200ms

Type@120ms "router"
Sleep 1200ms
Wait+Screen@20s /lab-routeros/
Sleep 1400ms

Screenshot docs/media/rustasshn-tmux-demo.png
Sleep 800ms

Hide
Escape
Sleep 400ms
Type "q"
Sleep 400ms
Ctrl+b
Type "d"
Sleep 400ms
Type "bash docs/tapes/rustasshn-tmux-demo.sh cleanup"
Enter
Sleep 400ms
