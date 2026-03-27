#!/usr/bin/env bash
set -euo pipefail

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}" )" && pwd)"
REPO_ROOT="$(cd "${CURRENT_DIR}/.." && pwd)"

BIN_PATH="$(tmux show -gqv @rustasshn_bin || true)"
LAUNCH_MODE="$(tmux show -gqv @rustasshn_launch_mode || true)"
PICKER_MODE="$(tmux show -gqv @rustasshn_mode || true)"
IMPLICIT_SELECT="$(tmux show -gqv @rustasshn_implicit_select || true)"
ENTER_MODE="$(tmux show -gqv @rustasshn_enter_mode || true)"

if [[ -z "${BIN_PATH}" ]]; then
  BIN_PATH="${REPO_ROOT}/bin/rustasshn"
fi
if [[ "${BIN_PATH}" == "~/"* ]]; then
  BIN_PATH="${HOME}/${BIN_PATH:2}"
fi
if [[ -z "${LAUNCH_MODE}" ]]; then
  LAUNCH_MODE="popup"
fi

SHELL_BIN="${SHELL:-sh}"

if [[ ! -x "${BIN_PATH}" ]]; then
  tmux display-message -d 5000 "rustasshn: missing ${BIN_PATH} (plugin install incomplete)"
  exit 1
fi

BIN_ARGS=()
if [[ -n "${PICKER_MODE}" ]]; then
  BIN_ARGS+=(--mode "${PICKER_MODE}")
fi
if [[ "${IMPLICIT_SELECT}" == "off" || "${IMPLICIT_SELECT}" == "false" ]]; then
  BIN_ARGS+=(--implicit-select=false)
fi
if [[ -n "${ENTER_MODE}" ]]; then
  BIN_ARGS+=(--enter-mode "${ENTER_MODE}")
fi

CMD=("${BIN_PATH}")
if [[ ${#BIN_ARGS[@]} -gt 0 ]]; then
  CMD+=("${BIN_ARGS[@]}")
fi
CMD_STR=""
printf -v CMD_STR '%q ' "${CMD[@]}"

if [[ "${LAUNCH_MODE}" == "popup" ]]; then
  ORIGIN_PANE="$(tmux display-message -p '#{pane_id}' || true)"
  ORIGIN_PATH="$(tmux display-message -p '#{pane_current_path}' || true)"

  POPUP_CMD=(
    env
    "RUSTASSHN_ORIGIN_PANE=${ORIGIN_PANE}"
    "RUSTASSHN_ORIGIN_PATH=${ORIGIN_PATH}"
    "${BIN_PATH}"
  )
  if [[ ${#BIN_ARGS[@]} -gt 0 ]]; then
    POPUP_CMD+=("${BIN_ARGS[@]}")
  fi

  if tmux display-popup -E -w 90% -h 80% -- "${POPUP_CMD[@]}"; then
    exit 0
  fi
fi

if [[ "${LAUNCH_MODE}" == "pane" ]]; then
  # Run selector in the current pane (replaces current process).
  # Keep the pane alive after rustasshn exits.
  tmux respawn-pane -k -c "#{pane_current_path}" -- "${SHELL_BIN}" -lc "${CMD_STR}; exec \"${SHELL_BIN}\" -l"
  exit 0
fi

if [[ "${LAUNCH_MODE}" == "window" ]]; then
  tmux new-window -n "rustasshn" -- "${SHELL_BIN}" -lc "${CMD_STR}; exec \"${SHELL_BIN}\" -l"
  exit 0
fi

# Fallback
tmux new-window -n "rustasshn" -- "${SHELL_BIN}" -lc "${CMD_STR}; exec \"${SHELL_BIN}\" -l"
