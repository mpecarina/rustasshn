#!/usr/bin/env bash
set -euo pipefail

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}" )" && pwd)"
REPO_ROOT="$(cd "${CURRENT_DIR}/.." && pwd)"

BIN_PATH="$(tmux show -gqv @tmux_ssh_manager_bin || true)"
LAUNCH_MODE="$(tmux show -gqv @tmux_ssh_manager_launch_mode || true)"
PICKER_MODE="$(tmux show -gqv @tmux_ssh_manager_mode || true)"
IMPLICIT_SELECT="$(tmux show -gqv @tmux_ssh_manager_implicit_select || true)"
ENTER_MODE="$(tmux show -gqv @tmux_ssh_manager_enter_mode || true)"

if [[ -z "${BIN_PATH}" ]]; then
  BIN_PATH="${REPO_ROOT}/bin/rustasshn"
fi
if [[ "${BIN_PATH}" == "~/"* ]]; then
  BIN_PATH="${HOME}/${BIN_PATH:2}"
fi
if [[ -z "${LAUNCH_MODE}" ]]; then
  LAUNCH_MODE="popup"
fi

# Auto-build: rebuild when binary is missing or git commit has changed.
STAMP_FILE="${BIN_PATH}.commit"
CURRENT_COMMIT="$(cd "${REPO_ROOT}" && git rev-parse HEAD 2>/dev/null || echo unknown)"
NEEDS_BUILD=0

if [[ ! -x "${BIN_PATH}" ]]; then
  NEEDS_BUILD=1
elif [[ ! -f "${STAMP_FILE}" ]]; then
  NEEDS_BUILD=1
elif [[ "$(cat "${STAMP_FILE}" 2>/dev/null)" != "${CURRENT_COMMIT}" ]]; then
  NEEDS_BUILD=1
fi

if [[ "${NEEDS_BUILD}" -eq 1 ]]; then
  tmux display-message "rustasshn: building..."
  mkdir -p "$(dirname "${BIN_PATH}")"
  if (cd "${REPO_ROOT}" && cargo build --release --locked) 2>/dev/null; then
    cp "${REPO_ROOT}/target/release/rustasshn" "${BIN_PATH}" 2>/dev/null || cp "${REPO_ROOT}/target/release/tmux-ssh-manager" "${BIN_PATH}"
    chmod +x "${BIN_PATH}" || true
    echo "${CURRENT_COMMIT}" > "${STAMP_FILE}"
  else
    tmux display-message -d 5000 "rustasshn: build failed  run 'cd ${REPO_ROOT} && cargo build --release' manually"
    exit 1
  fi
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

if [[ "${LAUNCH_MODE}" == "popup" ]]; then
  if tmux display-popup -E -w 90% -h 80% -- "${BIN_PATH}" "${BIN_ARGS[@]+${BIN_ARGS[@]}}"; then
    exit 0
  fi
fi

tmux new-window -n "ssh-manager" "${BIN_PATH}" "${BIN_ARGS[@]+${BIN_ARGS[@]}}"
