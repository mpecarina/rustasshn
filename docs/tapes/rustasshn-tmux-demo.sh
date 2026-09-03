#!/usr/bin/env bash
# Create the isolated tmux session used by rustasshn-tmux-demo.tape.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
socket_name="rustasshn-terminal-rec"
session_name="rustasshn-demo"
demo_home="${repo_root}/.terminal-rec/rustasshn-home"
demo_config_home="${repo_root}/.terminal-rec/rustasshn-config"

tmux_client() {
  tmux -L "${socket_name}" "$@"
}

cleanup() {
  tmux_client kill-server >/dev/null 2>&1 || true
}

setup() {
  cleanup
  bash "${repo_root}/docs/tapes/host-picker-demo.sh" setup

  HOME="${demo_home}" \
  XDG_CONFIG_HOME="${demo_config_home}" \
  SHELL="/bin/bash" \
  tmux_client -f /dev/null new-session -d -s "${session_name}" -n shell \
    -x 150 -y 28 -c "${repo_root}" \
    "env PS1='❯ ' bash --noprofile --norc"

  tmux_client set-environment -g HOME "${demo_home}"
  tmux_client set-environment -g XDG_CONFIG_HOME "${demo_config_home}"
  tmux_client set-environment -g SHELL /bin/bash
  tmux_client set-option -g automatic-rename off
  tmux_client set-option -g default-terminal tmux-256color
  tmux_client set-option -g mouse off
  tmux_client set-option -g status on
  tmux_client set-option -g status-style 'bg=#1e1e2e,fg=#cdd6f4'
  tmux_client set-option -g status-left '#[bold,fg=#89b4fa] rustasshn #[default]'
  tmux_client set-option -g status-right '#[fg=#a6adc8]%H:%M '
  tmux_client set-option -g popup-border-style 'fg=#89b4fa'
  tmux_client set-option -g @rustasshn_bin "${repo_root}/bin/rustasshn"
  tmux_client set-option -g @rustasshn_key s
  tmux_client set-option -g @rustasshn_launch_mode popup
  tmux_client set-option -g @rustasshn_mode search
  tmux_client set-option -g @rustasshn_implicit_select true
  tmux_client set-option -g @rustasshn_enter_mode o
  tmux_client bind-key s run-shell "${repo_root}/scripts/rustasshn.tmux"
  tmux_client send-keys -t "${session_name}:shell" clear Enter
}

case "${1:-}" in
  setup)
    setup
    ;;
  attach)
    exec tmux -L "${socket_name}" attach-session -t "${session_name}"
    ;;
  cleanup)
    cleanup
    ;;
  *)
    printf 'usage: %s setup|attach|cleanup\n' "$0" >&2
    exit 2
    ;;
esac
