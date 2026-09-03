#!/usr/bin/env bash
# Build and optionally run the documentation-only SSH inventory used by host-picker-demo.tape.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
demo_home="${repo_root}/.terminal-rec/rustasshn-home"
demo_config_home="${repo_root}/.terminal-rec/rustasshn-config"

rm -rf -- "${demo_home}/.ssh" "${demo_config_home}/rustasshn"
mkdir -p "${demo_home}/.ssh" "${demo_config_home}/rustasshn"

printf '%s\n' \
  'Host bastion' \
  '  HostName 192.0.2.5' \
  '  User netops' \
  '' \
  'Host lab-routeros' \
  '  HostName 192.0.2.10' \
  '  User admin' \
  '  ProxyJump bastion' \
  '' \
  'Host prod-db-01' \
  '  HostName 198.51.100.21' \
  '  User platform' \
  '  ProxyJump bastion' \
  '' \
  'Host prod-edge-01' \
  '  HostName 198.51.100.10' \
  '  User netops' \
  '  ProxyJump bastion' \
  '' \
  'Host prod-web-01' \
  '  HostName 198.51.100.31' \
  '  User platform' \
  '  ProxyJump bastion' \
  '' \
  'Host staging-web-01' \
  '  HostName 203.0.113.31' \
  '  User platform' \
  > "${demo_home}/.ssh/config"

chmod 600 "${demo_home}/.ssh/config"

if [[ "${1:-run}" == "setup" ]]; then
  exit 0
fi

HOME="${demo_home}" \
XDG_CONFIG_HOME="${demo_config_home}" \
TMUX="${TMUX:-terminal-rec-demo}" \
exec "${repo_root}/bin/rustasshn"
