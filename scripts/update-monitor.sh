#!/usr/bin/env bash
set -euo pipefail

readonly REPOSITORY="bear4f/Monitor"
readonly SERVER_BINARY=/usr/local/bin/monitor-server
readonly AGENT_BINARY=/usr/local/bin/monitor-agent
readonly SERVER_DB=/var/lib/monitor/monitor.db
# Deliberately outside /var/lib/monitor: the shipped unit gives the monitor
# service account StateDirectory=monitor and ReadWritePaths=/var/lib/monitor, so
# a fixed rollback path in there would be both a path the service user can
# replace under a root writer and a copy the freshly upgraded Server could
# delete after startup. This directory is root:root 0700, so the service account
# can neither traverse nor modify it.
readonly SERVER_DATA=/var/lib/monitor
readonly BACKUP_ROOT=/var/lib/monitor-update-backup
readonly BACKUP_GENERATION=/var/lib/monitor-update-backup/current
readonly BACKUP_STAGING=/var/lib/monitor-update-backup/staging
# root:root 0700 proves who may write here, not who created it: an unrelated
# directory can legitimately carry those bits. Ownership is therefore asserted
# the same way the managed systemd listener drop-in asserts it, with a marker
# this project writes and checks byte for byte.
readonly BACKUP_MARKER=/var/lib/monitor-update-backup/.monitor-managed
readonly BACKUP_MARKER_CONTENT='# Managed-By: monitor-update (rollback generation) v1'
readonly SERVER_UNIT=/etc/systemd/system/monitor-server.service
readonly SERVER_UNIT_NAME=monitor-server.service
readonly SERVER_DROPIN_DIR=/etc/systemd/system/monitor-server.service.d
readonly SERVER_DROPIN=/etc/systemd/system/monitor-server.service.d/10-monitor-listen.conf
readonly SERVER_DROPIN_MARKER='# Managed-By: monitor-install (listener)'

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'EOF'
Usage: update-monitor.sh --version VERSION [--component server|agent|all]

Update an installed Monitor binary from the exact GitHub Release VERSION.
EOF
}
require_root() { [[ "$(id -u)" -eq 0 ]] || die "run as root"; }
require_linux() {
  [[ "$(uname -s)" == Linux ]] || die "Linux is required"
  command -v systemctl >/dev/null || die "systemctl is required"
  command -v curl >/dev/null || die "curl is required"
  command -v sha256sum >/dev/null || die "sha256sum is required"
}
ensure_service_manager() {
  systemctl show-environment >/dev/null 2>&1 || die "systemd is not running"
}
normalize_version() {
  local value=$1
  if [[ $value == v* ]]; then
    RELEASE_TAG=$value
    RELEASE_VERSION=${value#v}
  else
    RELEASE_TAG=v$value
    RELEASE_VERSION=$value
  fi
  [[ $RELEASE_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || die "invalid release version: $value"
}
architecture_asset() {
  case "$(uname -m)" in
    x86_64) ASSET_ARCH=amd64 ;;
    aarch64|arm64) ASSET_ARCH=arm64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
  esac
}
download_asset() {
  local asset=$1 destination=$2
  curl --fail --location --max-redirs 5 --proto '=https' --proto-redir '=https' \
    --tlsv1.2 --silent --show-error \
    "https://github.com/$REPOSITORY/releases/download/$RELEASE_TAG/$asset" -o "$destination"
}
verify_checksum() {
  local asset=$1 file=$2 sums=$3 expected actual
  expected=$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$sums")
  [[ $expected =~ ^[[:xdigit:]]{64}$ ]] || die "SHA256SUMS has no valid entry for $asset"
  actual=$(sha256sum "$file" | awk '{ print $1 }')
  [[ $actual == "$expected" ]] || die "checksum mismatch for $asset"
}
# The listener drop-in is never written or removed here, so a managed listener
# survives every update untouched. What this must not do is restart the Server
# through an ExecStart override it does not recognize, so the drop-in state is
# validated before any binary is downloaded or staged.
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
assert_listener_dropin_safe() {
  local file
  [[ ! -L $SERVER_DROPIN_DIR ]] || die "$SERVER_DROPIN_DIR must not be a symbolic link"
  if [[ -e $SERVER_DROPIN || -L $SERVER_DROPIN ]]; then
    [[ -f $SERVER_DROPIN && ! -L $SERVER_DROPIN ]] \
      || die "$SERVER_DROPIN is not a regular file; refusing to update through it"
    [[ $(head -n 1 -- "$SERVER_DROPIN") == "$SERVER_DROPIN_MARKER" ]] \
      || die "$SERVER_DROPIN was not written by the Monitor installer; refusing to update through it"
    grep -Eq "^ExecStart=$SERVER_BINARY([[:space:]]|$)" "$SERVER_DROPIN" \
      || die "$SERVER_DROPIN does not start $SERVER_BINARY; refusing to update through it"
  fi
  while IFS= read -r file; do
    [[ -n $file && -f $file ]] || continue
    if [[ $file == "$SERVER_DROPIN" ]]; then
      continue
    fi
    if grep -Eq '^[[:space:]]*ExecStart=' "$file" 2>/dev/null; then
      die "$file overrides ExecStart for $SERVER_UNIT_NAME; resolve it before updating"
    fi
  done < <(server_dropin_files)
  return 0
}
installed_component() {
  local component=$1 binary=$2 unit=$3
  [[ -x $binary && -f $unit && -r $unit && ! -L $unit ]] \
    || die "$component is not installed as a complete Monitor component"
  grep -Eq "^ExecStart=$binary([[:space:]]|$)" "$unit" \
    || die "$unit is not a recognized Monitor unit for $component"
}
# A Server release may raise the SQLite user_version, and the Server migrates the
# database while starting up -- before it binds its listener, so long before the
# stability check below can judge the new version. Restoring only the binary
# would then strand an old Server in front of a newer schema it refuses to open,
# which is exactly the state the rollback claims to prevent. The pre-update
# database is therefore copied first and restored together with the binary.
#
# The copy is taken with the service stopped, so the database has no writer. In
# WAL mode the main file alone is not a complete database: committed frames may
# still sit in -wal, so the pair is copied and restored together. -shm is a
# rebuildable index into -wal and is deliberately not copied.
# The live database directory is writable by the monitor service account, so root
# must not follow anything it finds there. Symlinks are rejected outright rather
# than resolved: a copy through one would read a file of the service account's
# choosing into the protected generation. This runs before the service is
# stopped, so a refusal leaves the installation exactly as it was.
assert_server_database_safe() {
  [[ ! -L $SERVER_DATA ]] || die "$SERVER_DATA must not be a symbolic link"
  [[ -d $SERVER_DATA ]] || die "$SERVER_DATA is not a directory"
  if [[ -e $SERVER_DB || -L $SERVER_DB ]]; then
    [[ ! -L $SERVER_DB ]] || die "$SERVER_DB is a symbolic link; refusing to copy through it"
    [[ -f $SERVER_DB ]] || die "$SERVER_DB is not a regular file"
  fi
  if [[ -e ${SERVER_DB}-wal || -L ${SERVER_DB}-wal ]]; then
    [[ ! -L ${SERVER_DB}-wal ]] \
      || die "${SERVER_DB}-wal is a symbolic link; refusing to copy through it"
    [[ -f ${SERVER_DB}-wal ]] || die "${SERVER_DB}-wal is not a regular file"
  fi
}

assert_backup_root_managed() {
  [[ ! -L $BACKUP_ROOT ]] || die "$BACKUP_ROOT must not be a symbolic link"
  [[ -d $BACKUP_ROOT ]] || die "$BACKUP_ROOT is not a directory"
  [[ $(stat -c '%u:%g:%a' -- "$BACKUP_ROOT") == 0:0:700 ]] \
    || die "$BACKUP_ROOT must be owned by root:root with mode 0700"
  [[ -e $BACKUP_MARKER || -L $BACKUP_MARKER ]] \
    || die "$BACKUP_ROOT has no $BACKUP_MARKER marker; it was not created by Monitor and will not be used"
  [[ -f $BACKUP_MARKER && ! -L $BACKUP_MARKER ]] \
    || die "$BACKUP_MARKER is not a regular file"
  [[ $(stat -c '%u:%g:%a' -- "$BACKUP_MARKER") == 0:0:600 ]] \
    || die "$BACKUP_MARKER must be owned by root:root with mode 0600"
  [[ $(< "$BACKUP_MARKER") == "$BACKUP_MARKER_CONTENT" ]] \
    || die "$BACKUP_MARKER content does not match this Monitor version; refusing to use $BACKUP_ROOT"
  [[ ! -L $BACKUP_GENERATION ]] || die "$BACKUP_GENERATION must not be a symbolic link"
}

# Creates the backup root only when the path is free. An existing directory is
# never adopted just because its uid, gid and mode happen to match; without the
# marker it belongs to something else and no marker is planted in it.
prepare_backup_root() {
  if [[ -e $BACKUP_ROOT || -L $BACKUP_ROOT ]]; then
    assert_backup_root_managed
    return 0
  fi
  install -d -o root -g root -m 0700 "$BACKUP_ROOT" \
    || die "failed to create $BACKUP_ROOT"
  printf '%s\n' "$BACKUP_MARKER_CONTENT" > "$BACKUP_MARKER.new.$$" \
    || die "failed to write $BACKUP_MARKER"
  chown root:root "$BACKUP_MARKER.new.$$" || die "failed to own $BACKUP_MARKER"
  chmod 0600 "$BACKUP_MARKER.new.$$" || die "failed to set the mode of $BACKUP_MARKER"
  mv -f -- "$BACKUP_MARKER.new.$$" "$BACKUP_MARKER" || die "failed to publish $BACKUP_MARKER"
  assert_backup_root_managed
}

# One backup is one generation, held in its own directory. The whole directory is
# replaced, so a WAL copied by an earlier update cannot survive next to a newer
# main database: a generation taken from a database with no -wal simply has no
# -wal file in it. The generation is staged completely and only then published,
# so a failed copy never destroys the generation already on disk.
backup_database() {
  BACKUP_TAKEN=false
  [[ -e $SERVER_DB ]] || return 0
  [[ -f $SERVER_DB && ! -L $SERVER_DB ]] || die "$SERVER_DB is not a regular file"

  rm -rf -- "$BACKUP_STAGING" || return 1
  install -d -o root -g root -m 0700 "$BACKUP_STAGING" || return 1
  cp -p -- "$SERVER_DB" "$BACKUP_STAGING/monitor.db" || return 1
  if [[ -f ${SERVER_DB}-wal ]]; then
    cp -p -- "${SERVER_DB}-wal" "$BACKUP_STAGING/monitor.db-wal" || return 1
  fi
  # The live ownership and mode are recorded so a restore reinstates exactly what
  # the service account needs, instead of trusting the copy to have kept it.
  stat -c '%u:%g:%a' -- "$SERVER_DB" > "$BACKUP_STAGING/database-owner" || return 1

  local replaced="$BACKUP_GENERATION.replaced"
  rm -rf -- "$replaced" || return 1
  if [[ -d $BACKUP_GENERATION ]]; then
    mv -- "$BACKUP_GENERATION" "$replaced" || return 1
  fi
  mv -- "$BACKUP_STAGING" "$BACKUP_GENERATION" || return 1
  rm -rf -- "$replaced"
  BACKUP_TAKEN=true
}

# Any -wal left by the new Server describes the migrated schema, so it is
# discarded before the pre-update files go back; replaying it onto the restored
# main file would reintroduce the migration this is undoing. -shm is removed too
# so SQLite rebuilds it from whatever the restored generation actually is.
restore_database() {
  [[ ${BACKUP_TAKEN-false} == true ]] || return 0
  [[ -f $BACKUP_GENERATION/monitor.db ]] \
    || die "$BACKUP_GENERATION/monitor.db is missing; cannot restore"

  rm -f -- "${SERVER_DB}-wal" "${SERVER_DB}-shm" || return 1
  cp -p -- "$BACKUP_GENERATION/monitor.db" "$SERVER_DB.restore.$$" \
    && mv -f -- "$SERVER_DB.restore.$$" "$SERVER_DB" \
    || return 1
  if [[ -f $BACKUP_GENERATION/monitor.db-wal ]]; then
    cp -p -- "$BACKUP_GENERATION/monitor.db-wal" "${SERVER_DB}-wal.restore.$$" \
      && mv -f -- "${SERVER_DB}-wal.restore.$$" "${SERVER_DB}-wal" \
      || return 1
  fi
  # A generation taken without a WAL must leave no live WAL behind.
  if [[ ! -f $BACKUP_GENERATION/monitor.db-wal && -e ${SERVER_DB}-wal ]]; then
    die "a stale ${SERVER_DB}-wal survived a WAL-less rollback"
  fi

  local owner
  if owner=$(< "$BACKUP_GENERATION/database-owner"); then
    chown -- "${owner%:*}" "$SERVER_DB" || return 1
    chmod -- "${owner##*:}" "$SERVER_DB" || return 1
    if [[ -f ${SERVER_DB}-wal ]]; then
      chown -- "${owner%:*}" "${SERVER_DB}-wal" || return 1
      chmod -- "${owner##*:}" "${SERVER_DB}-wal" || return 1
    fi
  fi
}

stage_binary() {
  local source=$1 destination=$2
  local temporary="${destination}.new.$$"
  install -o root -g root -m 0755 "$source" "$temporary"
  mv -f -- "$temporary" "$destination"
}
rollback_binary() {
  local backup=$1 destination=$2
  local temporary="${destination}.rollback.$$"
  install -o root -g root -m 0755 "$backup" "$temporary"
  mv -f -- "$temporary" "$destination"
}
service_is_stable() {
  local unit=$1 attempt
  for attempt in 1 2 3; do
    systemctl is-active --quiet "$unit" || return 1
    [[ $attempt -eq 3 ]] || sleep 1
  done
}
update_component() {
  local component=$1 unit=$2 binary=$3 source=$4
  local backup="$WORK_DIR/$component.previous"
  local is_server=false restored="previous binary"
  if [[ $component == Server ]]; then
    is_server=true
    restored="previous binary and pre-update database"
  fi
  local was_active=false
  if systemctl is-active --quiet "$unit"; then
    was_active=true
  fi
  cp -p -- "$binary" "$backup"

  # Stop, copy, swap, start -- rather than one restart -- so the database copy
  # happens in the window where the Server is not running and cannot write.
  if [[ $was_active == true ]]; then
    systemctl stop "$unit" >/dev/null || die "failed to stop $unit"
    if systemctl is-active --quiet "$unit"; then
      die "$unit remains active after stop"
    fi
  fi
  if [[ $is_server == true ]] && ! backup_database; then
    if [[ $was_active == true ]]; then
      systemctl start "$unit" >/dev/null 2>&1 || true
    fi
    die "failed to stage a rollback copy of $SERVER_DB; nothing was changed"
  fi
  stage_binary "$source" "$binary"

  if [[ $was_active == true ]]; then
    if ! systemctl start "$unit" || ! service_is_stable "$unit"; then
      systemctl stop "$unit" >/dev/null 2>&1 || true
      rollback_binary "$backup" "$binary" \
        || die "$component start failed and the previous binary could not be restored"
      if [[ $is_server == true ]] && ! restore_database; then
        die "$component start failed and the pre-update database could not be restored from $BACKUP_GENERATION"
      fi
      if systemctl start "$unit" && service_is_stable "$unit"; then
        die "$component start failed; the $restored were restored and the service recovered"
      fi
      die "$component start failed; the $restored were restored but the service did not recover; inspect journalctl -u ${unit%.service}"
    fi
  fi
  printf '%s updated to %s\n' "$component" "$RELEASE_VERSION"
}

BACKUP_TAKEN=false
VERSION=
COMPONENT=all
while (($#)); do
  case $1 in
    --version)
      (($# >= 2)) || die "--version requires a value"
      VERSION=$2
      shift 2
      ;;
    --component)
      (($# >= 2)) || die "--component requires server, agent, or all"
      COMPONENT=$2
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ -n $VERSION ]] || die "--version is required"
[[ $COMPONENT == server || $COMPONENT == agent || $COMPONENT == all ]] || die "invalid component"
require_root
require_linux
ensure_service_manager
normalize_version "$VERSION"
architecture_asset
if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  installed_component server "$SERVER_BINARY" "$SERVER_UNIT"
  assert_listener_dropin_safe
  # Both of these refuse before anything is downloaded, stopped or swapped.
  assert_server_database_safe
  prepare_backup_root
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  installed_component agent "$AGENT_BINARY" /etc/systemd/system/monitor-agent.service
fi

WORK_DIR=$(mktemp -d)
cleanup() { rm -rf -- "$WORK_DIR"; }
trap cleanup EXIT
download_asset SHA256SUMS "$WORK_DIR/SHA256SUMS"
if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  server_asset="monitor-server-linux-$ASSET_ARCH"
  download_asset "$server_asset" "$WORK_DIR/monitor-server"
  verify_checksum "$server_asset" "$WORK_DIR/monitor-server" "$WORK_DIR/SHA256SUMS"
  chmod 0755 "$WORK_DIR/monitor-server"
  [[ "$("$WORK_DIR/monitor-server" --version 2>/dev/null)" == "monitor-server $RELEASE_VERSION" ]] || die "Server version mismatch"
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  agent_asset="monitor-agent-linux-$ASSET_ARCH"
  download_asset "$agent_asset" "$WORK_DIR/monitor-agent"
  verify_checksum "$agent_asset" "$WORK_DIR/monitor-agent" "$WORK_DIR/SHA256SUMS"
  chmod 0755 "$WORK_DIR/monitor-agent"
  [[ "$("$WORK_DIR/monitor-agent" --version 2>/dev/null)" == "monitor-agent $RELEASE_VERSION" ]] || die "Agent version mismatch"
fi

if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  update_component Server monitor-server.service "$SERVER_BINARY" "$WORK_DIR/monitor-server"
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  update_component Agent monitor-agent.service "$AGENT_BINARY" "$WORK_DIR/monitor-agent"
fi
printf 'Monitor %s update complete (%s).\n' "$RELEASE_TAG" "$COMPONENT"
if [[ ${BACKUP_TAKEN-false} == true ]]; then
  printf 'Pre-update database kept at %s; it matches the previously installed Server.\n' \
    "$BACKUP_GENERATION"
  printf 'Downgrading the Server later requires restoring it, because an older Server refuses a newer schema.\n'
fi
