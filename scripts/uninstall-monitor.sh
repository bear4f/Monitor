#!/usr/bin/env bash
set -euo pipefail

readonly SERVER_BINARY=/usr/local/bin/monitor-server
readonly AGENT_BINARY=/usr/local/bin/monitor-agent
readonly SERVER_UNIT=/etc/systemd/system/monitor-server.service
readonly AGENT_UNIT=/etc/systemd/system/monitor-agent.service
readonly AGENT_ENV=/etc/monitor-agent.env
readonly SERVER_DATA=/var/lib/monitor

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'EOF'
Usage: uninstall-monitor.sh [--component server|agent|all] [--purge]

The Server database is preserved unless --purge is explicitly supplied.
EOF
}
require_root() { [[ "$(id -u)" -eq 0 ]] || die "run as root"; }
require_linux() {
  [[ "$(uname -s)" == Linux ]] || die "Linux is required"
  command -v systemctl >/dev/null || die "systemctl is required"
}
assert_unit_owned() {
  local unit=$1 binary=$2
  [[ -e $unit ]] || return 0
  grep -Fq "ExecStart=$binary" "$unit" \
    || die "$unit is not a recognized Monitor unit; refusing to remove it"
}
stop_unit() {
  local unit=$1 binary=$2
  [[ -e $unit ]] || return 0
  assert_unit_owned "$unit" "$binary"
  systemctl disable --now "$unit" >/dev/null 2>&1 || true
}
remove_unit_if_owned() {
  local unit=$1 binary=$2
  [[ -e $unit ]] || return 0
  assert_unit_owned "$unit" "$binary"
  rm -f -- "$unit"
}
assert_binary_owned() {
  local binary=$1 unit=$2
  [[ -e $binary ]] || return 0
  [[ -x $binary && -e $unit ]] \
    || die "$binary has no recognized Monitor unit; refusing to remove it"
  grep -Fq "ExecStart=$binary" "$unit" \
    || die "$binary has no recognized Monitor unit; refusing to remove it"
}
remove_binary_if_owned() {
  local binary=$1 unit=$2
  [[ -e $binary ]] || return 0
  assert_binary_owned "$binary" "$unit"
  rm -f -- "$binary"
}

COMPONENT=all
PURGE=false
while (($#)); do
  case $1 in
    --component)
      (($# >= 2)) || die "--component requires server, agent, or all"
      COMPONENT=$2
      shift 2
      ;;
    --purge)
      PURGE=true
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) die "unknown argument: $1" ;;
  esac
done
[[ $COMPONENT == server || $COMPONENT == agent || $COMPONENT == all ]] || die "invalid component"
[[ $PURGE == true && $COMPONENT == agent ]] \
  && die "--purge applies to Server data; choose --component server or all"
require_root
require_linux

if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  assert_unit_owned "$SERVER_UNIT" "$SERVER_BINARY"
  assert_binary_owned "$SERVER_BINARY" "$SERVER_UNIT"
  stop_unit monitor-server.service "$SERVER_BINARY"
  remove_binary_if_owned "$SERVER_BINARY" "$SERVER_UNIT"
  remove_unit_if_owned "$SERVER_UNIT" "$SERVER_BINARY"
  if [[ $PURGE == true && -e $SERVER_DATA ]]; then
    [[ $SERVER_DATA == /var/lib/monitor && -d $SERVER_DATA ]] || die "unexpected Server data path"
    rm -rf -- "$SERVER_DATA"
  fi
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  assert_unit_owned "$AGENT_UNIT" "$AGENT_BINARY"
  assert_binary_owned "$AGENT_BINARY" "$AGENT_UNIT"
  stop_unit monitor-agent.service "$AGENT_BINARY"
  remove_binary_if_owned "$AGENT_BINARY" "$AGENT_UNIT"
  remove_unit_if_owned "$AGENT_UNIT" "$AGENT_BINARY"
  rm -f -- "$AGENT_ENV"
fi
systemctl daemon-reload
printf 'Monitor uninstall complete (%s); Server data %s.\n' \
  "$COMPONENT" "$([[ $PURGE == true ]] && echo purged || echo preserved)"
