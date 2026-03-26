#!/usr/bin/env bash
set -euo pipefail

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}" )" && pwd)"
KEY_BIND="$(tmux show -gqv @tmux_ssh_manager_key || true)"

if [[ -z "${KEY_BIND}" ]]; then
  KEY_BIND="s"
fi

tmux bind-key "${KEY_BIND}" run-shell "${CURRENT_DIR}/scripts/rustasshn.tmux"
