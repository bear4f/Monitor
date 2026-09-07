#!/usr/bin/env bash
set -euo pipefail

readonly REPOSITORY="bear4f/Monitor"
readonly SERVER_BINARY=/usr/local/bin/monitor-server
readonly AGENT_BINARY=/usr/local/bin/monitor-agent

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
  curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error \
    "https://github.com/$REPOSITORY/releases/download/$RELEASE_TAG/$asset" -o "$destination"
}
verify_checksum() {
  local asset=$1 file=$2 sums=$3 expected actual
  expected=$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$sums")
  [[ $expected =~ ^[[:xdigit:]]{64}$ ]] || die "SHA256SUMS has no valid entry for $asset"
  actual=$(sha256sum "$file" | awk '{ print $1 }')
  [[ $actual == "$expected" ]] || die "checksum mismatch for $asset"
}
installed_component() {
  local component=$1 binary=$2 unit=$3
  [[ -x $binary && -f $unit && -r $unit && ! -L $unit ]] \
    || die "$component is not installed as a complete Monitor component"
  grep -Eq "^ExecStart=$binary([[:space:]]|$)" "$unit" \
    || die "$unit is not a recognized Monitor unit for $component"
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
  local was_active=false
  if systemctl is-active --quiet "$unit"; then
    was_active=true
  fi
  cp -p -- "$binary" "$backup"
  stage_binary "$source" "$binary"
  if [[ $was_active == true ]]; then
    if ! systemctl restart "$unit" || ! service_is_stable "$unit"; then
      rollback_binary "$backup" "$binary" \
        || die "$component restart failed and previous binary could not be restored"
      if systemctl restart "$unit" && service_is_stable "$unit"; then
        die "$component restart failed; previous binary restored and service recovered"
      fi
      die "$component restart failed; previous binary was restored but service recovery failed"
    fi
  fi
  printf '%s updated to %s\n' "$component" "$RELEASE_VERSION"
}

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
  installed_component server "$SERVER_BINARY" /etc/systemd/system/monitor-server.service
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
  [[ "$("$WORK_DIR/monitor-server" --version 2>/dev/null)" == "monitor-server $RELEASE_VERSION" ]] || die "Server version mismatch"
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  agent_asset="monitor-agent-linux-$ASSET_ARCH"
  download_asset "$agent_asset" "$WORK_DIR/monitor-agent"
  verify_checksum "$agent_asset" "$WORK_DIR/monitor-agent" "$WORK_DIR/SHA256SUMS"
  [[ "$("$WORK_DIR/monitor-agent" --version 2>/dev/null)" == "monitor-agent $RELEASE_VERSION" ]] || die "Agent version mismatch"
fi

if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  update_component Server monitor-server.service "$SERVER_BINARY" "$WORK_DIR/monitor-server"
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  update_component Agent monitor-agent.service "$AGENT_BINARY" "$WORK_DIR/monitor-agent"
fi
printf 'Monitor %s update complete (%s).\n' "$RELEASE_TAG" "$COMPONENT"
