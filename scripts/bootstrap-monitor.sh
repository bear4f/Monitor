#!/usr/bin/env bash
set -euo pipefail

# Thin orchestration layer over install-monitor.sh. It resolves a release tag,
# collects secrets from the terminal without letting them reach argv, the
# environment, logs, or shell history, and then hands the real work to
# install-monitor.sh. Binary download, checksum verification, unit ownership
# and service management all stay in that one installer.

readonly REPOSITORY="bear4f/Monitor"
readonly SERVER_BINARY=/usr/local/bin/monitor-server
readonly SERVER_DB=/var/lib/monitor/monitor.db

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: bootstrap-monitor.sh server [--listen ADDRESS] [--port PORT]
                                   [--set-admin-password | --admin-password-file PATH]
                                   [--version vX.Y.Z]
       bootstrap-monitor.sh agent  --server URL
                                   [--token-file PATH] [--version vX.Y.Z]

Installs Monitor from a GitHub Release. Without --version the latest stable
release is resolved automatically.

Server defaults to 127.0.0.1:25774 with the database at
/var/lib/monitor/monitor.db.

Secrets are never accepted as command-line arguments. The administrator
password and the agent token are read from the terminal with echo disabled, or
from a root-owned file with mode 0600.
EOF
}

require_root() {
  [[ "$(id -u)" -eq 0 ]] || die "run as root"
}

require_tools() {
  local tool
  for tool in curl tar sha256sum systemctl ss; do
    command -v "$tool" >/dev/null || die "$tool is required"
  done
}

# GitHub redirects /releases/latest to the newest tag, so the tag can be read
# from the Location header without jq or any other parser.
resolve_latest_tag() {
  local location tag
  location=$(curl --fail --silent --show-error --head --proto '=https' --tlsv1.2 \
    -o /dev/null -w '%{url_effective}' \
    "https://github.com/$REPOSITORY/releases/latest") \
    || die "failed to resolve the latest release"
  tag=${location##*/tag/}
  [[ $tag != "$location" && -n $tag ]] || die "could not read a release tag from $location"
  printf '%s' "$tag"
}

valid_release_tag() {
  [[ $1 =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

# Reads a secret file that must be root-owned, mode 0600 and a real file, then
# returns only its first line with CR/LF removed and nothing else trimmed.
read_secret_file() {
  local path=$1 description=$2 value
  [[ -e $path ]] || die "$description not found: $path"
  [[ ! -L $path ]] || die "$description must not be a symbolic link: $path"
  [[ -f $path ]] || die "$description must be a regular file: $path"
  [[ $(stat -c '%u:%g' -- "$path") == 0:0 ]] || die "$description must be owned by root:root"
  [[ $(stat -c '%a' -- "$path") == 600 ]] || die "$description must have mode 0600"
  IFS= read -r value < "$path" || true
  value=${value%$'\r'}
  [[ -n $value ]] || die "$description is empty"
  printf '%s' "$value"
}

# Prompts twice on the controlling terminal with echo disabled. The value never
# becomes an argument, an exported variable, or a temporary file.
prompt_secret_twice() {
  local prompt=$1 first second
  [[ -r /dev/tty ]] || die "$prompt requires an interactive terminal"
  printf '%s: ' "$prompt" > /dev/tty
  IFS= read -r -s first < /dev/tty || die "failed to read $prompt"
  printf '\n' > /dev/tty
  printf 'Confirm %s: ' "$prompt" > /dev/tty
  IFS= read -r -s second < /dev/tty || die "failed to read $prompt"
  printf '\n' > /dev/tty
  [[ $first == "$second" ]] || die "$prompt entries do not match"
  [[ -n $first ]] || die "$prompt must not be empty"
  (( ${#first} <= 1024 )) || die "$prompt exceeds 1024 bytes"
  SECRET_VALUE=$first
  first=
  second=
}

prompt_secret_once() {
  local prompt=$1 value
  [[ -r /dev/tty ]] || die "$prompt requires an interactive terminal"
  printf '%s: ' "$prompt" > /dev/tty
  IFS= read -r -s value < /dev/tty || die "failed to read $prompt"
  printf '\n' > /dev/tty
  [[ -n $value ]] || die "$prompt must not be empty"
  SECRET_VALUE=$value
  value=
}

# Uses the installer next to this script when run from a checkout, otherwise
# downloads the source tree for the resolved tag so the installer, packaging
# and release binaries all come from the same revision.
prepare_installer() {
  local local_installer="$SCRIPT_ROOT/install-monitor.sh"
  if [[ -f $local_installer ]]; then
    INSTALLER=$local_installer
    INSTALLER_SOURCE="local checkout"
    return 0
  fi
  local tarball="$WORK_DIR/source.tar.gz"
  curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error \
    "https://github.com/$REPOSITORY/archive/refs/tags/$RELEASE_TAG.tar.gz" \
    -o "$tarball" || die "failed to download the $RELEASE_TAG source tree"
  mkdir -p -- "$WORK_DIR/source"
  tar -xzf "$tarball" -C "$WORK_DIR/source" --strip-components=1 \
    || die "failed to extract the $RELEASE_TAG source tree"
  INSTALLER="$WORK_DIR/source/scripts/install-monitor.sh"
  [[ -f $INSTALLER ]] || die "$RELEASE_TAG does not contain scripts/install-monitor.sh"
  chmod 0755 "$INSTALLER"
  INSTALLER_SOURCE="release $RELEASE_TAG"
}

install_server() {
  local -a install_args=(--version "$RELEASE_TAG" --component server)
  [[ -z $LISTEN_ADDRESS ]] || install_args+=(--listen "$LISTEN_ADDRESS")
  [[ -z $SERVER_PORT ]] || install_args+=(--port "$SERVER_PORT")
  "$INSTALLER" "${install_args[@]}"

  if [[ $SET_PASSWORD == true ]]; then
    prompt_secret_twice "Administrator password"
  elif [[ -n $PASSWORD_FILE ]]; then
    SECRET_VALUE=$(read_secret_file "$PASSWORD_FILE" "administrator password file")
  else
    printf '\nAdministrator password was not changed.\n'
    printf 'If this is a fresh installation, set one with:\n'
    printf '  sudo -u monitor %s --db %s admin set-password\n' "$SERVER_BINARY" "$SERVER_DB"
    return 0
  fi

  # The password reaches the Server only through stdin.
  if printf '%s\n' "$SECRET_VALUE" \
    | sudo -u monitor "$SERVER_BINARY" --db "$SERVER_DB" admin set-password; then
    SECRET_VALUE=
    unset SECRET_VALUE
    printf 'Administrator password updated.\n'
  else
    SECRET_VALUE=
    unset SECRET_VALUE
    die "failed to set the administrator password"
  fi
}

install_agent() {
  [[ -n $AGENT_SERVER ]] || die "agent installation requires --server URL"
  if [[ -n $TOKEN_FILE ]]; then
    SECRET_VALUE=$(read_secret_file "$TOKEN_FILE" "agent token file")
  else
    prompt_secret_once "MONITOR_TOKEN"
  fi
  [[ $SECRET_VALUE =~ ^[0-9a-f]{64}$ ]] || {
    SECRET_VALUE=
    unset SECRET_VALUE
    die "agent token must be exactly 64 lowercase hexadecimal characters"
  }

  # install-monitor.sh consumes a root-owned 0600 environment file, so the token
  # is written to one inside the private work directory and removed afterwards.
  local staged="$WORK_DIR/monitor-agent.env"
  local previous_umask
  previous_umask=$(umask)
  umask 0077
  printf 'MONITOR_SERVER=%s\nMONITOR_TOKEN=%s\n' "$AGENT_SERVER" "$SECRET_VALUE" > "$staged"
  umask "$previous_umask"
  SECRET_VALUE=
  unset SECRET_VALUE
  chown root:root "$staged"
  chmod 0600 "$staged"

  if "$INSTALLER" --version "$RELEASE_TAG" --component agent --agent-env "$staged"; then
    rm -f -- "$staged"
  else
    rm -f -- "$staged"
    die "agent installation failed"
  fi
}

COMPONENT=
LISTEN_ADDRESS=
SERVER_PORT=
AGENT_SERVER=
TOKEN_FILE=
PASSWORD_FILE=
SET_PASSWORD=false
RELEASE_TAG=
SECRET_VALUE=

(($#)) || { usage; exit 1; }
case $1 in
  server|agent) COMPONENT=$1; shift ;;
  --help|-h) usage; exit 0 ;;
  *) die "first argument must be server or agent" ;;
esac

while (($#)); do
  case $1 in
    --listen)
      (($# >= 2)) || die "--listen requires an address"
      LISTEN_ADDRESS=$2
      shift 2
      ;;
    --port)
      (($# >= 2)) || die "--port requires a value"
      SERVER_PORT=$2
      shift 2
      ;;
    --server)
      (($# >= 2)) || die "--server requires a URL"
      AGENT_SERVER=$2
      shift 2
      ;;
    --token-file)
      (($# >= 2)) || die "--token-file requires a path"
      TOKEN_FILE=$2
      shift 2
      ;;
    --admin-password-file)
      (($# >= 2)) || die "--admin-password-file requires a path"
      PASSWORD_FILE=$2
      shift 2
      ;;
    --set-admin-password)
      SET_PASSWORD=true
      shift
      ;;
    --version)
      (($# >= 2)) || die "--version requires a value"
      RELEASE_TAG=$2
      shift 2
      ;;
    --token|--password|--admin-password)
      die "$1 is not supported; secrets must never appear in command arguments"
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ $SET_PASSWORD == true && -n $PASSWORD_FILE ]] \
  && die "--set-admin-password and --admin-password-file are mutually exclusive"
if [[ $COMPONENT == agent ]]; then
  [[ -z $LISTEN_ADDRESS && -z $SERVER_PORT ]] \
    || die "--listen and --port apply to the Server component"
  [[ $SET_PASSWORD == false && -z $PASSWORD_FILE ]] \
    || die "administrator password options apply to the Server component"
else
  [[ -z $AGENT_SERVER && -z $TOKEN_FILE ]] \
    || die "--server and --token-file apply to the Agent component"
fi

require_root
require_tools

SCRIPT_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
if [[ -z $RELEASE_TAG ]]; then
  RELEASE_TAG=$(resolve_latest_tag)
fi
valid_release_tag "$RELEASE_TAG" || die "invalid release tag: $RELEASE_TAG"

WORK_DIR=$(mktemp -d)
cleanup() {
  SECRET_VALUE=
  rm -rf -- "$WORK_DIR"
}
trap cleanup EXIT
chmod 0700 "$WORK_DIR"

prepare_installer
printf 'Installing Monitor %s (%s, installer from %s).\n' \
  "$RELEASE_TAG" "$COMPONENT" "$INSTALLER_SOURCE"

if [[ $COMPONENT == server ]]; then
  install_server
else
  install_agent
fi
