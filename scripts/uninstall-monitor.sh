#!/usr/bin/env bash
set -euo pipefail

readonly SERVER_BINARY=/usr/local/bin/monitor-server
readonly AGENT_BINARY=/usr/local/bin/monitor-agent
readonly SERVER_UNIT=/etc/systemd/system/monitor-server.service
readonly SERVER_UNIT_NAME=monitor-server.service
readonly AGENT_UNIT=/etc/systemd/system/monitor-agent.service
readonly AGENT_ENV=/etc/monitor-agent.env
readonly SERVER_DATA=/var/lib/monitor
readonly UPDATE_BACKUP_ROOT=/var/lib/monitor-update-backup
readonly UPDATE_BACKUP_MARKER=/var/lib/monitor-update-backup/.monitor-managed
readonly UPDATE_BACKUP_MARKER_CONTENT='# Managed-By: monitor-update (rollback generation) v1'
readonly SERVER_DROPIN_DIR=/etc/systemd/system/monitor-server.service.d
readonly SERVER_DROPIN=/etc/systemd/system/monitor-server.service.d/10-monitor-listen.conf
readonly SERVER_DROPIN_MARKER='# Managed-By: monitor-install (listener)'

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
ensure_service_manager() {
  systemctl show-environment >/dev/null 2>&1 || die "systemd is not running"
}
assert_unit_owned() {
  local unit=$1 binary=$2
  [[ -e $unit ]] || return 0
  [[ -f $unit && -r $unit && ! -L $unit ]] || die "$unit is not a readable regular file"
  grep -Eq "^ExecStart=$binary([[:space:]]|$)" "$unit" \
    || die "$unit is not a recognized Monitor unit; refusing to remove it"
}
assert_component_owned() {
  local unit=$1 binary=$2
  if [[ -e $unit ]]; then
    assert_unit_owned "$unit" "$binary"
    [[ -e $binary ]] || return 0
    [[ -x $binary ]] || die "$binary is not executable; refusing to remove it"
  elif [[ -e $binary ]]; then
    die "$binary has no recognized Monitor unit; refusing to remove it"
  fi
}
stop_disable_unit() {
  local unit_name=$1 unit_path=$2 binary=$3
  assert_unit_owned "$unit_path" "$binary" || return 0
  if systemctl is-active --quiet "$unit_name"; then
    systemctl stop "$unit_name" >/dev/null || die "failed to stop $unit_name"
  fi
  if systemctl is-active --quiet "$unit_name"; then
    die "$unit_name remains active after stop"
  fi
  local enabled_state
  enabled_state=$(systemctl is-enabled "$unit_name" 2>/dev/null || true)
  case $enabled_state in
    enabled|enabled-runtime|linked|linked-runtime|alias)
      systemctl disable "$unit_name" >/dev/null || die "failed to disable $unit_name"
      ;;
  esac
  enabled_state=$(systemctl is-enabled "$unit_name" 2>/dev/null || true)
  case $enabled_state in
    enabled|enabled-runtime|linked|linked-runtime|alias)
      die "$unit_name remains enabled after disable"
      ;;
  esac
}
remove_unit_if_owned() {
  local unit=$1 binary=$2
  [[ -e $unit ]] || return 0
  assert_unit_owned "$unit" "$binary"
  rm -f -- "$unit"
}
# systemd loads drop-ins from several unit lookup paths, not only from
# /etc/systemd/system. Its own view is authoritative when it can answer, and the
# standard directories are scanned as well. Same understanding as install and
# update.
server_dropin_files() {
  local paths dir file
  {
    paths=$(systemctl show -p DropInPaths --value "$SERVER_UNIT_NAME" 2>/dev/null || true)
    if [[ -n $paths ]]; then
      # DropInPaths is a space separated list and unit paths carry no spaces.
      # shellcheck disable=SC2086
      printf '%s\n' $paths
    fi
    for dir in /etc/systemd/system /run/systemd/system \
      /usr/local/lib/systemd/system /usr/lib/systemd/system /lib/systemd/system; do
      for file in "$dir/$SERVER_UNIT_NAME.d"/*.conf; do
        if [[ -e $file ]]; then
          printf '%s\n' "$file"
        fi
      done
    done
  } | sort -u
}
# A drop-in we did not write that overrides ExecStart means the unit no longer
# starts what this uninstaller thinks it owns, so nothing may be stopped or
# deleted on that assumption. The foreign file is never touched; resolving it is
# the operator's call. A drop-in that leaves ExecStart alone is unrelated: it
# neither blocks the uninstall nor gets removed.
assert_no_foreign_execstart_dropin() {
  local file
  while IFS= read -r file; do
    [[ -n $file && -f $file ]] || continue
    if [[ $file == "$SERVER_DROPIN" ]]; then
      continue
    fi
    if grep -Eq '^[[:space:]]*ExecStart=' "$file" 2>/dev/null; then
      die "$file overrides ExecStart for $SERVER_UNIT_NAME; resolve it before uninstalling"
    fi
  done < <(server_dropin_files)
  return 0
}
# Validates the drop-in state before anything is stopped or deleted, so an
# unexpected state never leaves a half-uninstalled Server behind. Only the
# listener drop-in this project writes is ever a candidate for removal;
# unrelated drop-ins are not removed.
preflight_managed_dropin() {
  [[ ! -L $SERVER_DROPIN_DIR ]] || die "$SERVER_DROPIN_DIR must not be a symbolic link"
  [[ -e $SERVER_DROPIN || -L $SERVER_DROPIN ]] || return 0
  [[ ! -L $SERVER_DROPIN ]] || die "$SERVER_DROPIN is a symbolic link; refusing to remove it"
  [[ -f $SERVER_DROPIN ]] || die "$SERVER_DROPIN is not a regular file; refusing to remove it"
  [[ -r $SERVER_DROPIN ]] || die "$SERVER_DROPIN is not readable; refusing to remove it"
  [[ $(head -n 1 -- "$SERVER_DROPIN") == "$SERVER_DROPIN_MARKER" ]] \
    || die "$SERVER_DROPIN was not written by the Monitor installer; refusing to remove it"
}
remove_managed_dropin() {
  preflight_managed_dropin
  [[ -e $SERVER_DROPIN ]] || return 0
  rm -f -- "$SERVER_DROPIN"
  rmdir -- "$SERVER_DROPIN_DIR" 2>/dev/null || true
}

remove_binary_if_owned() {
  local binary=$1 unit=$2
  [[ -e $binary ]] || return 0
  assert_component_owned "$unit" "$binary"
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
ensure_service_manager

if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  assert_component_owned "$SERVER_UNIT" "$SERVER_BINARY"
  preflight_managed_dropin
  assert_no_foreign_execstart_dropin
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  assert_component_owned "$AGENT_UNIT" "$AGENT_BINARY"
fi

if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  stop_disable_unit monitor-server.service "$SERVER_UNIT" "$SERVER_BINARY"
  remove_binary_if_owned "$SERVER_BINARY" "$SERVER_UNIT"
  remove_unit_if_owned "$SERVER_UNIT" "$SERVER_BINARY"
  remove_managed_dropin
  if [[ $PURGE == true && -e $SERVER_DATA ]]; then
    [[ $SERVER_DATA == /var/lib/monitor && -d $SERVER_DATA ]] || die "unexpected Server data path"
    rm -rf -- "$SERVER_DATA"
  fi
  # The rollback copies written by update-monitor.sh live in their own root-owned
  # directory, so purging Server data has to remove them too. root:root 0700 is
  # not proof of origin, so the marker update-monitor.sh writes must match before
  # anything here deletes a root-owned tree: a path collision must never turn
  # --purge into destruction of someone else's data.
  if [[ $PURGE == true && ( -e $UPDATE_BACKUP_ROOT || -L $UPDATE_BACKUP_ROOT ) ]]; then
    [[ $UPDATE_BACKUP_ROOT == /var/lib/monitor-update-backup ]] \
      || die "unexpected update backup path"
    [[ -d $UPDATE_BACKUP_ROOT && ! -L $UPDATE_BACKUP_ROOT ]] \
      || die "$UPDATE_BACKUP_ROOT is not a directory; refusing to remove it"
    [[ $(stat -c '%u:%g:%a' -- "$UPDATE_BACKUP_ROOT") == 0:0:700 ]] \
      || die "$UPDATE_BACKUP_ROOT is not owned by root:root with mode 0700; refusing to remove it"
    [[ -f $UPDATE_BACKUP_MARKER && ! -L $UPDATE_BACKUP_MARKER ]] \
      || die "$UPDATE_BACKUP_ROOT has no Monitor marker; refusing to remove it"
    [[ $(stat -c '%u:%g:%a' -- "$UPDATE_BACKUP_MARKER") == 0:0:600 ]] \
      || die "$UPDATE_BACKUP_MARKER is not owned by root:root with mode 0600; refusing to remove it"
    [[ $(< "$UPDATE_BACKUP_MARKER") == "$UPDATE_BACKUP_MARKER_CONTENT" ]] \
      || die "$UPDATE_BACKUP_MARKER was not written by the Monitor updater; refusing to remove it"
    rm -rf -- "$UPDATE_BACKUP_ROOT"
  fi
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  stop_disable_unit monitor-agent.service "$AGENT_UNIT" "$AGENT_BINARY"
  remove_binary_if_owned "$AGENT_BINARY" "$AGENT_UNIT"
  remove_unit_if_owned "$AGENT_UNIT" "$AGENT_BINARY"
  rm -f -- "$AGENT_ENV"
fi
systemctl daemon-reload
printf 'Monitor uninstall complete (%s); Server data %s.\n' \
  "$COMPONENT" "$([[ $PURGE == true ]] && echo purged || echo preserved)"
