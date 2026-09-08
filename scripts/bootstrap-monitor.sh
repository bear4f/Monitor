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
readonly SERVER_USER=monitor
readonly MAX_SECRET_BYTES=1024

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

# The administrator CLI has to run as the monitor user so the database keeps its
# ownership; running it as root and repairing ownership afterwards is not an
# option. runuser is preferred because minimal root-only systems frequently have
# no sudo installed at all. This is resolved before the Server is installed
# whenever a password will be set, so a missing tool cannot surface halfway
# through.
resolve_privilege_drop() {
  if command -v runuser >/dev/null; then
    PRIVILEGE_DROP=(runuser -u "$SERVER_USER" --)
  elif command -v sudo >/dev/null; then
    # -n keeps sudo from ever prompting and swallowing the password on stdin.
    PRIVILEGE_DROP=(sudo -n -u "$SERVER_USER" --)
  else
    die "runuser (util-linux) or sudo is required to set the administrator password as $SERVER_USER"
  fi
}

# GitHub redirects /releases/latest to the newest tag. --location makes curl
# actually follow that redirect, so %{url_effective} reports the tag URL rather
# than the request URL; the redirect chain is pinned to HTTPS. The result is
# only ever used after valid_release_tag accepts it.
resolve_latest_tag() {
  local location tag
  location=$(curl --fail --silent --show-error --location --max-redirs 5 --head \
    --proto '=https' --proto-redir '=https' --tlsv1.2 \
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

# Counts UTF-8 bytes the way the Server does, which validates 1..=1024 bytes.
# ${#var} counts characters in a UTF-8 locale, so the count is taken with
# LC_ALL=C; the local assignment restores the caller's locale on return.
secret_byte_length() {
  local LC_ALL=C
  SECRET_BYTES=${#SECRET_VALUE}
}

# Content is never trimmed; only the CR of a CRLF terminator is removed, and
# only where read_secret_file already specifies it.
validate_secret_bytes() {
  local description=$1
  secret_byte_length
  (( SECRET_BYTES >= 1 )) || die "$description must not be empty"
  (( SECRET_BYTES <= MAX_SECRET_BYTES )) \
    || die "$description must be at most $MAX_SECRET_BYTES bytes ($SECRET_BYTES given)"
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
  SECRET_VALUE=$value
  value=
}

forget_secret() {
  SECRET_VALUE=
  unset SECRET_VALUE
}

# Secrets and the local conditions around them are validated here, before a
# single byte is downloaded or installed: a mistyped confirmation, a
# world-readable password file, a symlinked token file, an over-long password or
# a malformed token all fail while the system is still untouched. The
# MONITOR_SERVER URL is not validated here; install-monitor.sh holds the one
# canonical validator for it and rejects a bad URL during installation.
collect_secrets() {
  if [[ $COMPONENT == server ]]; then
    if [[ $SET_PASSWORD == true ]]; then
      prompt_secret_twice "Administrator password"
    elif [[ -n $PASSWORD_FILE ]]; then
      SECRET_VALUE=$(read_secret_file "$PASSWORD_FILE" "administrator password file")
    else
      return 0
    fi
    validate_secret_bytes "administrator password"
    resolve_privilege_drop
    return 0
  fi

  [[ -n $AGENT_SERVER ]] || die "agent installation requires --server URL"
  if [[ -n $TOKEN_FILE ]]; then
    SECRET_VALUE=$(read_secret_file "$TOKEN_FILE" "agent token file")
  else
    prompt_secret_once "MONITOR_TOKEN"
  fi
  if [[ ! $SECRET_VALUE =~ ^[0-9a-f]{64}$ ]]; then
    forget_secret
    die "agent token must be exactly 64 lowercase hexadecimal characters"
  fi
}

# The installer, packaging units and release binaries must all come from the
# same revision, so the source tree for the resolved tag is always downloaded.
# There is deliberately no lookup next to this script: under
# "curl ... | sudo bash -s --" BASH_SOURCE[0] is empty, which would resolve to
# the current working directory and let a ./install-monitor.sh planted there run
# as root. Developers testing a checkout invoke scripts/install-monitor.sh
# directly instead.
prepare_installer() {
  local tarball="$WORK_DIR/source.tar.gz"
  curl --fail --location --max-redirs 5 --proto '=https' --proto-redir '=https' \
    --tlsv1.2 --silent --show-error \
    "https://github.com/$REPOSITORY/archive/refs/tags/$RELEASE_TAG.tar.gz" \
    -o "$tarball" || die "failed to download the $RELEASE_TAG source tree"
  mkdir -p -- "$WORK_DIR/source"
  tar -xzf "$tarball" -C "$WORK_DIR/source" --strip-components=1 --no-same-owner \
    || die "failed to extract the $RELEASE_TAG source tree"
  INSTALLER="$WORK_DIR/source/scripts/install-monitor.sh"
  [[ -f $INSTALLER && ! -L $INSTALLER ]] \
    || die "$RELEASE_TAG does not contain scripts/install-monitor.sh"
  [[ -d $WORK_DIR/source/packaging && ! -L $WORK_DIR/source/packaging ]] \
    || die "$RELEASE_TAG does not contain packaging/"
  chmod 0755 "$INSTALLER"
}

# Printed when no password change was requested. The command has to name a
# privilege-drop tool that actually exists on this machine, and when neither is
# present it says so instead of printing something unrunnable. An installation
# that did not ask for a password is still a successful installation.
print_password_hint() {
  printf '\nAdministrator password was not changed.\n'
  if command -v runuser >/dev/null; then
    printf 'If this is a fresh installation, set one with:\n'
    printf '  runuser -u %s -- %s --db %s admin set-password\n' \
      "$SERVER_USER" "$SERVER_BINARY" "$SERVER_DB"
  elif command -v sudo >/dev/null; then
    printf 'If this is a fresh installation, set one with:\n'
    printf '  sudo -u %s -- %s --db %s admin set-password\n' \
      "$SERVER_USER" "$SERVER_BINARY" "$SERVER_DB"
  else
    printf 'Administrator password is not configured.\n'
    printf 'Install util-linux (runuser) or sudo, then run\n'
    printf '  %s --db %s admin set-password\n' "$SERVER_BINARY" "$SERVER_DB"
    printf 'as the %s user.\n' "$SERVER_USER"
  fi
}

install_server() {
  local -a install_args=(--version "$RELEASE_TAG" --component server)
  [[ -z $LISTEN_ADDRESS ]] || install_args+=(--listen "$LISTEN_ADDRESS")
  [[ -z $SERVER_PORT ]] || install_args+=(--port "$SERVER_PORT")
  "$INSTALLER" "${install_args[@]}"

  if [[ $SET_PASSWORD == false && -z $PASSWORD_FILE ]]; then
    print_password_hint
    return 0
  fi

  # The password reaches the Server only through stdin.
  if printf '%s\n' "$SECRET_VALUE" \
    | "${PRIVILEGE_DROP[@]}" "$SERVER_BINARY" --db "$SERVER_DB" admin set-password; then
    forget_secret
    printf 'Administrator password updated.\n'
  else
    forget_secret
    die "failed to set the administrator password"
  fi
}

install_agent() {
  # install-monitor.sh consumes a root-owned 0600 environment file, so the token
  # is written to one inside the private work directory and removed afterwards.
  local staged="$WORK_DIR/monitor-agent.env"
  local previous_umask
  previous_umask=$(umask)
  umask 0077
  printf 'MONITOR_SERVER=%s\nMONITOR_TOKEN=%s\n' "$AGENT_SERVER" "$SECRET_VALUE" > "$staged"
  umask "$previous_umask"
  forget_secret
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
SECRET_BYTES=0
PRIVILEGE_DROP=()

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
      [[ -n $2 ]] || die "--listen requires a non-empty address"
      LISTEN_ADDRESS=$2
      shift 2
      ;;
    --port)
      (($# >= 2)) || die "--port requires a value"
      [[ -n $2 ]] || die "--port requires a non-empty value"
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

collect_secrets
prepare_installer
printf 'Installing Monitor %s (%s, installer from release %s).\n' \
  "$RELEASE_TAG" "$COMPONENT" "$RELEASE_TAG"

if [[ $COMPONENT == server ]]; then
  install_server
else
  install_agent
fi
