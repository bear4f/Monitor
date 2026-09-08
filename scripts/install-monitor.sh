#!/usr/bin/env bash
set -euo pipefail

readonly REPOSITORY="bear4f/Monitor"
readonly DEFAULT_LISTEN_ADDRESS=127.0.0.1
readonly DEFAULT_SERVER_PORT=25774
readonly SERVER_BINARY=/usr/local/bin/monitor-server
readonly AGENT_BINARY=/usr/local/bin/monitor-agent
readonly SERVER_UNIT=/etc/systemd/system/monitor-server.service
readonly SERVER_UNIT_NAME=monitor-server.service
readonly AGENT_UNIT=/etc/systemd/system/monitor-agent.service
readonly AGENT_ENV=/etc/monitor-agent.env
readonly SERVER_DATA=/var/lib/monitor
readonly SERVER_DB=/var/lib/monitor/monitor.db
readonly SERVER_DROPIN_DIR=/etc/systemd/system/monitor-server.service.d
readonly SERVER_DROPIN=/etc/systemd/system/monitor-server.service.d/10-monitor-listen.conf
readonly SERVER_DROPIN_MARKER='# Managed-By: monitor-install (listener)'

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: install-monitor.sh --version VERSION [--component server|agent|all]
       [--listen ADDRESS] [--port PORT] [--agent-env PATH]

Install release binaries and their systemd units. VERSION is required and
must identify an explicit GitHub Release (for example v0.1.1).

--listen and --port change where the Server listens. The default is
127.0.0.1:25774. The listener is applied through a managed systemd drop-in so
the shipped unit stays byte-identical and auditable. When neither flag is
given an existing managed listener is preserved, so reinstalling never resets
a customised port.
EOF
}

require_root() {
  [[ "$(id -u)" -eq 0 ]] || die "run as root"
}

require_linux() {
  [[ "$(uname -s)" == Linux ]] || die "Linux is required"
  command -v systemctl >/dev/null || die "systemctl is required"
  command -v curl >/dev/null || die "curl is required"
  command -v sha256sum >/dev/null || die "sha256sum is required"
  command -v ss >/dev/null || die "ss (iproute2) is required for port collision checks"
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
  [[ $RELEASE_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] \
    || die "invalid release version: $value"
}

architecture_asset() {
  case "$(uname -m)" in
    x86_64) ASSET_ARCH=amd64 ;;
    aarch64|arm64) ASSET_ARCH=arm64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
  esac
}

download_asset() {
  local asset=$1
  local destination=$2
  local base_url="https://github.com/$REPOSITORY/releases/download/$RELEASE_TAG"
  curl --fail --location --max-redirs 5 --proto '=https' --proto-redir '=https' \
    --tlsv1.2 --silent --show-error "$base_url/$asset" -o "$destination"
}

verify_checksum() {
  local asset=$1
  local file=$2
  local sums=$3
  local expected actual
  expected=$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$sums")
  [[ $expected =~ ^[[:xdigit:]]{64}$ ]] || die "SHA256SUMS has no valid entry for $asset"
  actual=$(sha256sum "$file" | awk '{ print $1 }')
  [[ $actual == "$expected" ]] || die "checksum mismatch for $asset"
}

ensure_service_manager() {
  systemctl show-environment >/dev/null 2>&1 || die "systemd is not running"
}

preflight_systemd_unit() {
  local unit_name=$1 unit_path=$2 binary=$3
  local load_state fragment_path
  load_state=$(systemctl show -p LoadState --value "$unit_name" 2>/dev/null || true)
  fragment_path=$(systemctl show -p FragmentPath --value "$unit_name" 2>/dev/null || true)
  if [[ -n $fragment_path && $fragment_path != "(null)" ]]; then
    [[ $fragment_path == "$unit_path" ]] \
      || die "$unit_name resolves to unrelated systemd fragment $fragment_path"
    assert_unit_owned "$unit_path" "$binary" \
      || die "$unit_name does not resolve to a recognized Monitor unit"
  elif [[ -n $load_state && $load_state != not-found ]]; then
    die "$unit_name is already provided by systemd without the expected Monitor fragment"
  fi
}

ensure_user() {
  local user=$1
  if ! getent passwd "$user" >/dev/null; then
    useradd --system --user-group --home-dir /nonexistent --shell /usr/sbin/nologin "$user"
  fi
  local home shell primary_group
  home=$(getent passwd "$user" | cut -d: -f6)
  shell=$(getent passwd "$user" | cut -d: -f7)
  primary_group=$(id -gn "$user")
  [[ $home == /nonexistent ]] \
    || die "existing $user account has incompatible home directory"
  [[ $shell == /usr/sbin/nologin || $shell == /sbin/nologin ]] \
    || die "existing $user account has an interactive shell"
  [[ $primary_group == "$user" ]] \
    || die "existing $user account has an incompatible primary group"
}

write_unit_safely() {
  local source=$1
  local destination=$2
  [[ -r $source ]] || die "missing unit source: $source"
  [[ ! -L $destination ]] || die "$destination must not be a symbolic link"
  if [[ -e $destination ]] && ! cmp -s "$source" "$destination"; then
    die "$destination already exists and is not the frozen Monitor unit"
  fi
  install -o root -g root -m 0644 "$source" "$destination"
}

valid_port() {
  local value=$1
  [[ $value =~ ^[0-9]{1,5}$ ]] || return 1
  (( 10#$value >= 1 && 10#$value <= 65535 ))
}

# Formats ADDRESS and PORT the way Rust's SocketAddr parses them, bracketing
# IPv6 literals. Reuses the address validators the Agent URL check already uses
# so there is only one notion of a valid literal in this script.
compose_listen_socket() {
  local address=$1 port=$2
  if valid_ipv4_literal "$address"; then
    LISTEN_SOCKET="$address:$port"
  elif valid_ipv6_literal "$address"; then
    LISTEN_SOCKET="[$address]:$port"
  else
    die "invalid --listen address: $address"
  fi
}

dropin_is_managed() {
  local file=$1
  [[ -f $file && ! -L $file ]] || return 1
  [[ $(head -n 1 -- "$file") == "$SERVER_DROPIN_MARKER" ]]
}

# Reads back the listener from our own drop-in so a reinstall or update without
# --listen/--port keeps whatever the operator already chose.
managed_dropin_socket() {
  local line
  dropin_is_managed "$SERVER_DROPIN" || return 1
  line=$(grep -m1 -E "^ExecStart=$SERVER_BINARY[[:space:]]" "$SERVER_DROPIN" 2>/dev/null) || return 1
  [[ $line =~ --listen[[:space:]]+([^[:space:]]+) ]] || return 1
  printf '%s' "${BASH_REMATCH[1]}"
}

split_socket() {
  local socket=$1
  if [[ $socket == \[*\]:* ]]; then
    SOCKET_ADDRESS=${socket%%]:*}
    SOCKET_ADDRESS=${SOCKET_ADDRESS#[}
    SOCKET_PORT=${socket##*]:}
  else
    SOCKET_ADDRESS=${socket%:*}
    SOCKET_PORT=${socket##*:}
  fi
  valid_port "$SOCKET_PORT" || return 1
  valid_ipv4_literal "$SOCKET_ADDRESS" || valid_ipv6_literal "$SOCKET_ADDRESS" || return 1
}

# systemd loads drop-ins from several unit lookup paths, not only from
# /etc/systemd/system. Its own view is authoritative when it can answer, and the
# standard directories are scanned as well so a unit systemd has not loaded yet
# is still covered.
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

# Any drop-in we did not write that also sets ExecStart would silently fight
# ours, so refuse rather than guess. Unrelated drop-ins are left untouched.
assert_no_conflicting_dropin() {
  local file
  [[ ! -L $SERVER_DROPIN_DIR ]] || die "$SERVER_DROPIN_DIR must not be a symbolic link"
  [[ ! -L $SERVER_DROPIN ]] || die "$SERVER_DROPIN must not be a symbolic link"
  if [[ -e $SERVER_DROPIN ]] && ! dropin_is_managed "$SERVER_DROPIN"; then
    die "$SERVER_DROPIN exists but was not written by this installer"
  fi
  while IFS= read -r file; do
    [[ -n $file && -f $file ]] || continue
    if [[ $file == "$SERVER_DROPIN" ]]; then
      continue
    fi
    if grep -Eq '^[[:space:]]*ExecStart=' "$file" 2>/dev/null; then
      die "$file overrides ExecStart for $SERVER_UNIT_NAME; resolve it before configuring the Monitor listener"
    fi
  done < <(server_dropin_files)
  return 0
}

resolve_server_listener() {
  local existing address port
  address=$LISTEN_ADDRESS_ARG
  port=$SERVER_PORT_ARG
  if [[ -z $address || -z $port ]]; then
    if existing=$(managed_dropin_socket) && split_socket "$existing"; then
      [[ -n $address ]] || address=$SOCKET_ADDRESS
      [[ -n $port ]] || port=$SOCKET_PORT
    fi
  fi
  [[ -n $address ]] || address=$DEFAULT_LISTEN_ADDRESS
  [[ -n $port ]] || port=$DEFAULT_SERVER_PORT
  valid_port "$port" || die "invalid --port: $port"
  compose_listen_socket "$address" "$port"
  EFFECTIVE_ADDRESS=$address
  EFFECTIVE_PORT=$port
}

warn_public_listener() {
  case $EFFECTIVE_ADDRESS in
    0.0.0.0|::|0:0:0:0:0:0:0:0)
      cat >&2 <<'EOF'
WARNING: the Server will accept connections from any address.
Monitor itself serves HTTP.
Use HTTPS through a trusted reverse proxy for administrator login.
Direct public HTTP administrator login is intentionally unsupported.
EOF
      ;;
  esac
}

apply_server_listener() {
  assert_no_conflicting_dropin
  if [[ $LISTEN_SOCKET == "$DEFAULT_LISTEN_ADDRESS:$DEFAULT_SERVER_PORT" ]]; then
    # The shipped unit already starts on the default socket, so the managed
    # drop-in is removed instead of restating it.
    remove_managed_dropin
    return 0
  fi
  [[ ! -L $SERVER_DROPIN ]] || die "$SERVER_DROPIN must not be a symbolic link"
  if [[ -e $SERVER_DROPIN ]] && ! dropin_is_managed "$SERVER_DROPIN"; then
    die "$SERVER_DROPIN exists but was not written by this installer"
  fi
  install -d -o root -g root -m 0755 "$SERVER_DROPIN_DIR"
  local temporary="$SERVER_DROPIN.new.$$"
  cat > "$temporary" <<EOF
$SERVER_DROPIN_MARKER
# Regenerated by install-monitor.sh; edit through --listen/--port instead.
[Service]
ExecStart=
ExecStart=$SERVER_BINARY --listen $LISTEN_SOCKET --db $SERVER_DB
EOF
  chown root:root "$temporary"
  chmod 0644 "$temporary"
  mv -f -- "$temporary" "$SERVER_DROPIN"
}

remove_managed_dropin() {
  [[ -e $SERVER_DROPIN || -L $SERVER_DROPIN ]] || return 0
  dropin_is_managed "$SERVER_DROPIN" \
    || die "$SERVER_DROPIN exists but was not written by this installer"
  rm -f -- "$SERVER_DROPIN"
  rmdir -- "$SERVER_DROPIN_DIR" 2>/dev/null || true
}

port_is_listening() {
  ss -Hln "sport = :$EFFECTIVE_PORT" 2>/dev/null | grep -q .
}

# Pids that hold a listening socket on PORT, one per line. Empty when ss cannot
# report process information.
port_listener_pids() {
  ss -Hlnp "sport = :$1" 2>/dev/null \
    | grep -o 'pid=[0-9]\+' | cut -d= -f2 | sort -u
}

# The socket the installed Server is currently configured to use, or failure
# when no recognized Monitor unit is installed. The managed drop-in wins over
# the shipped unit because that is how systemd resolves the effective ExecStart.
configured_monitor_socket() {
  local socket line
  if socket=$(managed_dropin_socket); then
    printf '%s' "$socket"
    return 0
  fi
  # A file at the managed path that we did not write may carry any listener at
  # all, so the effective socket is unknown rather than the shipped default.
  [[ ! -e $SERVER_DROPIN ]] || return 1
  [[ -f $SERVER_UNIT && ! -L $SERVER_UNIT ]] || return 1
  line=$(grep -m1 -E "^ExecStart=$SERVER_BINARY[[:space:]]" "$SERVER_UNIT" 2>/dev/null) || return 1
  [[ $line =~ --listen[[:space:]]+([^[:space:]]+) ]] || return 1
  printf '%s' "${BASH_REMATCH[1]}"
}

# An occupied port is only acceptable when the listener provably belongs to the
# Monitor Server that is already configured for that same port. A busy port that
# Monitor is not currently configured for always refuses, even while
# monitor-server.service is active somewhere else. Nothing is ever killed and
# ownership is never inferred from "the service happens to be running".
check_server_port() {
  port_is_listening || return 0
  local current pids main_pid
  current=$(configured_monitor_socket) \
    || die "port $EFFECTIVE_PORT is occupied by an unknown service; refusing to replace it"
  split_socket "$current" \
    || die "port $EFFECTIVE_PORT is occupied and the installed Monitor listener could not be read"
  [[ $SOCKET_PORT == "$EFFECTIVE_PORT" ]] \
    || die "port $EFFECTIVE_PORT is already in use by another service; Monitor Server is configured for $current"
  systemctl is-active --quiet "$SERVER_UNIT_NAME" \
    || die "port $EFFECTIVE_PORT is occupied by an unknown service; refusing to replace it"
  [[ -f $SERVER_UNIT ]] \
    || die "port $EFFECTIVE_PORT is occupied but Monitor Server ownership is unrecognized"
  assert_unit_owned "$SERVER_UNIT" "$SERVER_BINARY" \
    || die "port $EFFECTIVE_PORT is occupied but Monitor Server ownership is unrecognized"
  main_pid=$(systemctl show -p MainPID --value "$SERVER_UNIT_NAME" 2>/dev/null || true)
  pids=$(port_listener_pids "$EFFECTIVE_PORT")
  if [[ -n $pids ]]; then
    [[ $main_pid =~ ^[0-9]+$ ]] && (( main_pid > 0 )) \
      || die "port $EFFECTIVE_PORT is occupied but $SERVER_UNIT_NAME has no main process"
    [[ $pids == "$main_pid" ]] \
      || die "port $EFFECTIVE_PORT is held by a process outside $SERVER_UNIT_NAME; refusing to continue"
  fi
}

install_binary() {
  local source=$1
  local destination=$2
  local temporary="${destination}.new.$$"
  install -o root -g root -m 0755 "$source" "$temporary"
  mv -f -- "$temporary" "$destination"
}

ensure_binary_path_safe() {
  local binary=$1 unit=$2
  [[ -e $binary ]] || return 0
  [[ -x $binary && -e $unit ]] \
    || die "$binary already exists without a recognized Monitor unit; refusing to replace it"
  grep -Eq "^ExecStart=$binary([[:space:]]|$)" "$unit" \
    || die "$binary already exists without a recognized Monitor unit; refusing to replace it"
}

valid_agent_environment_content() {
  local file=$1
  [[ -f $file && -r $file ]] || return 1
  local line server_count=0 token_count=0 server_value= token_value=
  while IFS= read -r line || [[ -n $line ]]; do
    if [[ $line =~ ^[[:space:]]*$ || $line =~ ^[[:space:]]*[#\;] ]]; then
      continue
    fi
    case $line in
      MONITOR_SERVER=*)
        server_count=$((server_count + 1))
        [[ $server_count -eq 1 ]] || return 1
        server_value=${line#MONITOR_SERVER=}
        ;;
      MONITOR_TOKEN=*)
        token_count=$((token_count + 1))
        [[ $token_count -eq 1 ]] || return 1
        token_value=${line#MONITOR_TOKEN=}
        ;;
      *)
        return 1
        ;;
    esac
  done < "$file"
  [[ $server_count -eq 1 && $token_count -eq 1 ]] || return 1
  valid_agent_server_url "$server_value" || return 1
  [[ $token_value =~ ^[0-9a-f]{64}$ ]]
}

valid_ipv4_literal() {
  local value=$1 octet
  local -a octets
  IFS=. read -r -a octets <<< "$value"
  [[ ${#octets[@]} == 4 ]] || return 1
  for octet in "${octets[@]}"; do
    [[ $octet =~ ^[0-9]{1,3}$ ]] || return 1
    (( 10#$octet <= 255 )) || return 1
  done
}

valid_ipv6_literal() {
  local value=$1 side group count=0 ipv4_tail=
  local -a groups
  if [[ $value == *.* ]]; then
    [[ $value == *:* ]] || return 1
    ipv4_tail=${value##*:}
    valid_ipv4_literal "$ipv4_tail" || return 1
    value=${value%:*}:v4
  fi
  [[ $value == *:* && $value != *[^0-9A-Fa-f:v]* && $value != *:::* ]] || return 1
  if [[ $value == *::* ]]; then
    [[ ${value#*::} != *::* ]] || return 1
    for side in "${value%%::*}" "${value#*::}"; do
      [[ -z $side ]] && continue
      IFS=: read -r -a groups <<< "$side"
      for group in "${groups[@]}"; do
        if [[ $group == v4 ]]; then
          count=$((count + 2))
        else
          [[ $group =~ ^[0-9A-Fa-f]{1,4}$ ]] || return 1
          count=$((count + 1))
        fi
      done
    done
    (( count < 8 ))
  else
    IFS=: read -r -a groups <<< "$value"
    count=0
    for group in "${groups[@]}"; do
      if [[ $group == v4 ]]; then
        count=$((count + 2))
      else
        [[ $group =~ ^[0-9A-Fa-f]{1,4}$ ]] || return 1
        count=$((count + 1))
      fi
    done
    (( count == 8 ))
  fi
}

valid_agent_server_url() {
  local value=$1 authority host port
  if [[ $value == http://* ]]; then
    authority=${value#http://}
  elif [[ $value == https://* ]]; then
    authority=${value#https://}
  else
    return 1
  fi
  [[ $authority != *'?'* && $authority != *'#'* && $authority != *'@'* ]] || return 1
  if [[ $authority == */ ]]; then
    authority=${authority%/}
  fi
  [[ -n $authority && $authority != */* ]] || return 1

  if [[ $authority == \[* ]]; then
    [[ $authority =~ ^\[([0-9A-Fa-f:.]+)\](:([0-9]+))?$ ]] || return 1
    host=${BASH_REMATCH[1]}
    valid_ipv6_literal "$host" || return 1
    port=${BASH_REMATCH[3]-}
  else
    [[ $authority != *:*:* ]] || return 1
    if [[ $authority == *:* ]]; then
      host=${authority%:*}
      port=${authority##*:}
    else
      host=$authority
      port=
    fi
    [[ $host =~ ^[A-Za-z0-9.-]+$ && -n $host ]] || return 1
  fi
  if [[ -n $port ]]; then
    [[ $port =~ ^[0-9]{1,5}$ ]] || return 1
    (( 10#$port >= 1 && 10#$port <= 65535 )) || return 1
  elif [[ $authority == *: && $authority != \]* ]]; then
    return 1
  fi
}

valid_agent_environment() {
  local file=$1
  [[ -f $file ]] || return 1
  [[ $(stat -c '%u:%g:%a' "$file") == 0:0:600 ]] || return 1
  valid_agent_environment_content "$file"
}

assert_unit_owned() {
  local unit_path=$1 binary=$2
  [[ -e $unit_path ]] || return 1
  [[ -f $unit_path && -r $unit_path && ! -L $unit_path ]] \
    || die "$unit_path is not a readable regular file"
  grep -Eq "^ExecStart=$binary([[:space:]]|$)" "$unit_path" \
    || die "$unit_path is not a recognized Monitor unit"
}

stop_disable_unit() {
  local unit_name=$1 unit_path=$2 binary=$3
  assert_unit_owned "$unit_path" "$binary" || return 0
  if systemctl is-active --quiet "$unit_name"; then
    systemctl stop "$unit_name" >/dev/null \
      || die "failed to stop $unit_name"
  fi
  if systemctl is-active --quiet "$unit_name"; then
    die "$unit_name remains active after stop"
  fi
  local enabled_state
  enabled_state=$(systemctl is-enabled "$unit_name" 2>/dev/null || true)
  if [[ $enabled_state == enabled || $enabled_state == enabled-runtime || $enabled_state == linked || $enabled_state == linked-runtime || $enabled_state == alias ]]; then
    systemctl disable "$unit_name" >/dev/null \
      || die "failed to disable $unit_name"
  fi
  enabled_state=$(systemctl is-enabled "$unit_name" 2>/dev/null || true)
  case $enabled_state in
    enabled|enabled-runtime|linked|linked-runtime|alias)
      die "$unit_name remains enabled after disable"
      ;;
  esac
}

install_server() {
  local unit_source="$SCRIPT_ROOT/../packaging/monitor-server.service"
  check_server_port
  ensure_binary_path_safe "$SERVER_BINARY" "$SERVER_UNIT"
  ensure_user monitor
  [[ -d $SERVER_DATA ]] || install -d -o monitor -g monitor -m 0750 "$SERVER_DATA"
  [[ -d $SERVER_DATA ]] || die "$SERVER_DATA is not a directory"
  [[ ! -L $SERVER_DATA ]] || die "$SERVER_DATA must not be a symbolic link"
  write_unit_safely "$unit_source" "$SERVER_UNIT"
  apply_server_listener
  install_binary "$WORK_DIR/monitor-server" "$SERVER_BINARY"
  systemctl daemon-reload
  systemctl enable monitor-server.service >/dev/null
  systemctl restart monitor-server.service
  systemctl is-active --quiet monitor-server.service \
    || die "monitor-server failed to start; inspect journalctl -u monitor-server"
  cat <<EOF

Monitor Server installed successfully.

Version:
$RELEASE_VERSION

Listen:
$LISTEN_SOCKET

Database:
$SERVER_DB

Service:
monitor-server.service

Next:
Configure HTTPS reverse proxy.
Set administrator password if not already configured.
EOF
}

install_agent() {
  local unit_source="$SCRIPT_ROOT/../packaging/monitor-agent.service"
  ensure_binary_path_safe "$AGENT_BINARY" "$AGENT_UNIT"
  ensure_user monitor-agent
  write_unit_safely "$unit_source" "$AGENT_UNIT"
  install_binary "$WORK_DIR/monitor-agent" "$AGENT_BINARY"

  if [[ -n $AGENT_ENV_SOURCE ]]; then
    [[ -f $AGENT_ENV_SOURCE ]] || die "agent environment file not found: $AGENT_ENV_SOURCE"
    install -o root -g root -m 0600 "$AGENT_ENV_SOURCE" "$AGENT_ENV"
  elif [[ ! -e $AGENT_ENV ]]; then
    install -o root -g root -m 0600 "$SCRIPT_ROOT/../packaging/monitor-agent.env.example" "$AGENT_ENV"
  fi

  systemctl daemon-reload
  if valid_agent_environment "$AGENT_ENV"; then
    systemctl enable monitor-agent.service >/dev/null
    systemctl restart monitor-agent.service
    systemctl is-active --quiet monitor-agent.service \
      || die "monitor-agent failed to start; inspect journalctl -u monitor-agent"
    local agent_server
    agent_server=$(sed -n 's/^MONITOR_SERVER=//p' "$AGENT_ENV" | head -n 1)
    cat <<EOF

Monitor Agent installed successfully.

Version:
$RELEASE_VERSION

Server:
$agent_server

Service:
monitor-agent.service

Status:
active
EOF
  else
    stop_disable_unit monitor-agent.service "$AGENT_UNIT" "$AGENT_BINARY"
    printf 'Monitor Agent installed but not started: configure %s as root:root mode 0600.\n' "$AGENT_ENV"
  fi
}

VERSION=
COMPONENT=all
AGENT_ENV_SOURCE=
LISTEN_ADDRESS_ARG=
SERVER_PORT_ARG=
LISTEN_SOCKET=
EFFECTIVE_ADDRESS=
EFFECTIVE_PORT=$DEFAULT_SERVER_PORT
SOCKET_ADDRESS=
SOCKET_PORT=
while (($#)); do
  case $1 in
    --version)
      (($# >= 2)) || die "--version requires a value"
      VERSION=$2
      shift 2
      ;;
    --listen)
      (($# >= 2)) || die "--listen requires an address"
      [[ -n $2 ]] || die "--listen requires a non-empty address"
      LISTEN_ADDRESS_ARG=$2
      shift 2
      ;;
    --port)
      (($# >= 2)) || die "--port requires a value"
      [[ -n $2 ]] || die "--port requires a non-empty value"
      SERVER_PORT_ARG=$2
      shift 2
      ;;
    --component)
      (($# >= 2)) || die "--component requires server, agent, or all"
      COMPONENT=$2
      shift 2
      ;;
    --agent-env)
      (($# >= 2)) || die "--agent-env requires a path"
      AGENT_ENV_SOURCE=$2
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
if [[ $COMPONENT == agent ]]; then
  [[ -z $LISTEN_ADDRESS_ARG && -z $SERVER_PORT_ARG ]] \
    || die "--listen and --port apply to the Server component"
fi
[[ $COMPONENT == server || $COMPONENT == agent || $COMPONENT == all ]] \
  || die "--component must be server, agent, or all"
require_root
require_linux
ensure_service_manager
normalize_version "$VERSION"
architecture_asset

SCRIPT_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

if [[ $COMPONENT == server || $COMPONENT == all ]]; then
  resolve_server_listener
  warn_public_listener
  # Both of these must run before anything is downloaded or written, so a
  # foreign ExecStart override or a busy port stops the install untouched.
  assert_no_conflicting_dropin
  check_server_port
  preflight_systemd_unit "$SERVER_UNIT_NAME" "$SERVER_UNIT" "$SERVER_BINARY"
  if [[ -e $SERVER_UNIT ]]; then
    assert_unit_owned "$SERVER_UNIT" "$SERVER_BINARY"
  fi
  ensure_binary_path_safe "$SERVER_BINARY" "$SERVER_UNIT"
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  preflight_systemd_unit monitor-agent.service "$AGENT_UNIT" "$AGENT_BINARY"
  if [[ -e $AGENT_UNIT ]]; then
    assert_unit_owned "$AGENT_UNIT" "$AGENT_BINARY"
  fi
  ensure_binary_path_safe "$AGENT_BINARY" "$AGENT_UNIT"
  agent_environment_valid=false
  if [[ -n $AGENT_ENV_SOURCE ]]; then
    [[ -f $AGENT_ENV_SOURCE ]] || die "agent environment file not found: $AGENT_ENV_SOURCE"
    valid_agent_environment_content "$AGENT_ENV_SOURCE" \
      || die "agent environment file has invalid MONITOR_SERVER or MONITOR_TOKEN"
    agent_environment_valid=true
  elif [[ -e $AGENT_ENV && ! -f $AGENT_ENV ]]; then
    die "$AGENT_ENV is not a regular file"
  elif valid_agent_environment "$AGENT_ENV"; then
    agent_environment_valid=true
  fi
  if [[ $agent_environment_valid != true ]]; then
    stop_disable_unit monitor-agent.service "$AGENT_UNIT" "$AGENT_BINARY"
  fi
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
  [[ "$("$WORK_DIR/monitor-server" --version 2>/dev/null)" == "monitor-server $RELEASE_VERSION" ]] \
    || die "downloaded Server version does not match $RELEASE_VERSION"
fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then
  agent_asset="monitor-agent-linux-$ASSET_ARCH"
  download_asset "$agent_asset" "$WORK_DIR/monitor-agent"
  verify_checksum "$agent_asset" "$WORK_DIR/monitor-agent" "$WORK_DIR/SHA256SUMS"
  chmod 0755 "$WORK_DIR/monitor-agent"
  [[ "$("$WORK_DIR/monitor-agent" --version 2>/dev/null)" == "monitor-agent $RELEASE_VERSION" ]] \
    || die "downloaded Agent version does not match $RELEASE_VERSION"
fi

if [[ $COMPONENT == server || $COMPONENT == all ]]; then install_server; fi
if [[ $COMPONENT == agent || $COMPONENT == all ]]; then install_agent; fi
printf 'Monitor %s installation complete (%s).\n' "$RELEASE_TAG" "$COMPONENT"
