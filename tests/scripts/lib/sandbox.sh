#!/usr/bin/env bash
# Sandbox helpers for the installer regression tests.
#
# Every case runs in its own mount namespace with overlay mounts over /etc,
# /usr/local/bin and /var/lib, so the real scripts run unmodified at their real
# absolute paths and nothing on the host is changed. GitHub, systemd, ss and the
# release binaries are replaced by small fakes on PATH.
#
# This is test-only tooling. Nothing here is installed or shipped.

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)
readonly REPO_ROOT
readonly TEST_VERSION=9.9.9
readonly TEST_TAG=v9.9.9
readonly LATEST_TAG=v1.2.3

fail() {
  printf 'ASSERT: %s\n' "$*" >&2
  return 1
}

# --- namespace ---------------------------------------------------------------

overlay_mount() {
  local target=$1 name=$2
  mkdir -p "$CASE_TMP/ovl/$name/upper" "$CASE_TMP/ovl/$name/work" "$target"
  mount -t overlay "overlay-$name" \
    -o "lowerdir=$target,upperdir=$CASE_TMP/ovl/$name/upper,workdir=$CASE_TMP/ovl/$name/work" \
    "$target"
}

sandbox_init() {
  mount --make-rprivate / 2>/dev/null || true
  # The Server CLI runs as the monitor user, so it has to be able to traverse
  # into the case directory to reach the fake binary's log.
  chmod 0755 "$CASE_TMP"
  overlay_mount /etc etc
  overlay_mount /usr/local/bin usrlocalbin
  overlay_mount /var/lib varlib
  mkdir -p /etc/systemd/system

  # The installer refuses to create a user with an unexpected shape, so the
  # accounts are pre-created exactly as install-monitor.sh would create them.
  local user
  for user in monitor monitor-agent; do
    if ! getent passwd "$user" >/dev/null; then
      local uid=31000
      [[ $user == monitor-agent ]] && uid=31001
      printf '%s:x:%s:\n' "$user" "$uid" >> /etc/group
      printf '%s:x:%s:%s::/nonexistent:/usr/sbin/nologin\n' "$user" "$uid" "$uid" >> /etc/passwd
    fi
  done

  STATE="$CASE_TMP/state"
  mkdir -p "$STATE"
  : > "$STATE/active"
  : > "$STATE/enabled"
  : > "$STATE/listeners"
  : > "$STATE/curl.log"
  : > "$STATE/exec.log"
  : > "$STATE/admin.log"
  echo 1000 > "$STATE/pidseq"
  # The Server CLI runs as the monitor user, so its log has to be writable by it.
  chmod 0777 "$STATE"
  chmod 0666 "$STATE/admin.log"

  build_fakes
  build_release
  PATH="$CASE_TMP/bin:$PATH"
  export PATH STATE
}

# --- fakes -------------------------------------------------------------------

build_fakes() {
  mkdir -p "$CASE_TMP/bin"

  cat > "$CASE_TMP/bin/systemctl" <<FAKE
#!/usr/bin/env bash
# Minimal systemd stand-in: unit state, drop-in resolution and the listening
# socket a started unit would own.
set -uo pipefail
STATE="$STATE"
FAKE
  cat >> "$CASE_TMP/bin/systemctl" <<'FAKE'
unit_file() { printf '/etc/systemd/system/%s' "$1"; }

dropins() {
  local f
  for f in "/etc/systemd/system/$1.d"/*.conf; do
    [[ -e $f ]] && printf '%s\n' "$f"
  done
  return 0
}

effective_exec() {
  local unit=$1 file line exec_line=
  file=$(unit_file "$unit")
  [[ -f $file ]] || return 1
  exec_line=$(grep -m1 '^ExecStart=' "$file" 2>/dev/null || true)
  while IFS= read -r file; do
    [[ -n $file ]] || continue
    local candidate=
    while IFS= read -r line; do
      candidate=$line
    done < <(grep '^ExecStart=' "$file" 2>/dev/null || true)
    [[ -n $candidate ]] && exec_line=$candidate
  done < <(dropins "$unit")
  printf '%s' "${exec_line#ExecStart=}"
}

listen_port() {
  local exec_line=$1 socket
  [[ $exec_line =~ --listen[[:space:]]+([^[:space:]]+) ]] || return 1
  socket=${BASH_REMATCH[1]}
  printf '%s' "${socket##*:}"
}

is_active() { grep -qxF "$1" "$STATE/active"; }

set_active() {
  is_active "$1" || printf '%s\n' "$1" >> "$STATE/active"
}

clear_active() {
  local keep
  keep=$(grep -vxF "$1" "$STATE/active" || true)
  printf '%s' "$keep" > "$STATE/active"
  [[ -s $STATE/active ]] && printf '\n' >> "$STATE/active"
  return 0
}

next_pid() {
  local pid
  pid=$(< "$STATE/pidseq")
  printf '%s' $((pid + 1)) > "$STATE/pidseq"
  printf '%s' $((pid + 1))
}

drop_listeners_for_unit() {
  local unit=$1 keep
  keep=$(grep -v " $unit\$" "$STATE/listeners" || true)
  printf '%s' "$keep" > "$STATE/listeners"
  [[ -s $STATE/listeners ]] && printf '\n' >> "$STATE/listeners"
  return 0
}

start_unit() {
  local unit=$1 exec_line port pid
  exec_line=$(effective_exec "$unit") || { printf 'no unit %s\n' "$unit" >&2; exit 1; }
  printf 'exec %s :: %s\n' "$unit" "$exec_line" >> "$STATE/exec.log"
  # The real Server opens and migrates its database during startup, before it
  # binds a listener, so starting a unit here actually runs its ExecStart and a
  # non-zero exit means the unit did not come up.
  # shellcheck disable=SC2086
  if ! $exec_line >> "$STATE/exec.log" 2>&1; then
    clear_active "$unit"
    drop_listeners_for_unit "$unit"
    return 1
  fi
  drop_listeners_for_unit "$unit"
  pid=$(next_pid)
  printf '%s %s\n' "$pid" "$unit" > "$STATE/mainpid.$unit"
  if port=$(listen_port "$exec_line"); then
    printf '%s %s %s\n' "$port" "$pid" "$unit" >> "$STATE/listeners"
  fi
  set_active "$unit"
}

case ${1-} in
  show-environment) exit 0 ;;
  daemon-reload) exit 0 ;;
esac

case ${1-} in
  show)
    property=; unit=
    shift
    while (($#)); do
      case $1 in
        -p) property=$2; shift 2 ;;
        --value) shift ;;
        *) unit=$1; shift ;;
      esac
    done
    case $property in
      LoadState)
        if [[ -f $(unit_file "$unit") ]]; then echo loaded; else echo not-found; fi ;;
      FragmentPath)
        if [[ -f $(unit_file "$unit") ]]; then unit_file "$unit"; echo; else echo; fi ;;
      DropInPaths)
        mapfile -t found < <(dropins "$unit")
        printf '%s\n' "${found[*]-}" ;;
      MainPID)
        if [[ -f $STATE/mainpid.$unit ]] && is_active "$unit"; then
          awk '{print $1}' "$STATE/mainpid.$unit"
        else
          echo 0
        fi ;;
      *) echo ;;
    esac
    exit 0
    ;;
  is-active)
    shift
    [[ ${1-} == --quiet ]] && shift
    is_active "${1-}" && exit 0 || exit 3
    ;;
  is-enabled)
    shift
    if grep -qxF "${1-}" "$STATE/enabled"; then echo enabled; exit 0; fi
    echo disabled; exit 1
    ;;
  enable)
    shift
    grep -qxF "${1-}" "$STATE/enabled" || printf '%s\n' "${1-}" >> "$STATE/enabled"
    exit 0
    ;;
  disable)
    shift
    keep=$(grep -vxF "${1-}" "$STATE/enabled" || true)
    printf '%s' "$keep" > "$STATE/enabled"
    [[ -s $STATE/enabled ]] && printf '\n' >> "$STATE/enabled"
    exit 0
    ;;
  stop)
    shift
    clear_active "${1-}"
    drop_listeners_for_unit "${1-}"
    exit 0
    ;;
  start|restart)
    shift
    start_unit "${1-}" || exit 1
    exit 0
    ;;
esac
exit 0
FAKE

  cat > "$CASE_TMP/bin/ss" <<FAKE
#!/usr/bin/env bash
set -uo pipefail
STATE="$STATE"
FAKE
  cat >> "$CASE_TMP/bin/ss" <<'FAKE'
want_pid=false
port=
for arg in "$@"; do
  case $arg in
    -*p*) want_pid=true ;;
  esac
  if [[ $arg == sport\ =\ :* ]]; then port=${arg##*:}; fi
done
[[ -n $port ]] || exit 0
while read -r listen_port listen_pid owner; do
  [[ -n ${listen_port-} ]] || continue
  [[ $listen_port == "$port" ]] || continue
  if [[ $want_pid == true ]]; then
    printf 'tcp   LISTEN 0 4096 0.0.0.0:%s 0.0.0.0:* users:(("%s",pid=%s,fd=7))\n' \
      "$listen_port" "${owner:-unknown}" "$listen_pid"
  else
    printf 'tcp   LISTEN 0 4096 0.0.0.0:%s 0.0.0.0:*\n' "$listen_port"
  fi
done < "$STATE/listeners"
exit 0
FAKE

  cat > "$CASE_TMP/bin/curl" <<FAKE
#!/usr/bin/env bash
# Faithful enough stand-in for the two ways the scripts call curl: a HEAD that
# reports %{url_effective}, and a download to -o. Redirects are only followed
# when --location is present, exactly like the real curl.
set -uo pipefail
STATE="$STATE"
WEB="$CASE_TMP/web"
FAKE
  cat >> "$CASE_TMP/bin/curl" <<'FAKE'
follow=false
head=false
output=
url=
write_out=
while (($#)); do
  case $1 in
    --location|-L) follow=true; shift ;;
    --head|-I) head=true; shift ;;
    -o) output=$2; shift 2 ;;
    -w) write_out=$2; shift 2 ;;
    --max-redirs|--proto|--proto-redir) shift 2 ;;
    --fail|--silent|--show-error|--tlsv1.2|-fsSL) shift ;;
    -*) shift ;;
    *) url=$1; shift ;;
  esac
done
printf '%s\n' "$url" >> "$STATE/curl.log"
[[ -n $url ]] || exit 2

resolved=$url
if [[ $follow == true && -f $WEB/redirects ]]; then
  while read -r from to; do
    [[ $from == "$url" ]] && resolved=$to
  done < "$WEB/redirects"
fi

path=${resolved#https://github.com/}
path=${path#https://raw.githubusercontent.com/}
asset="$WEB/$path"

if [[ $head == true ]]; then
  # A HEAD without --location must not resolve the redirect.
  if [[ $write_out == '%{url_effective}' ]]; then printf '%s' "$resolved"; fi
  [[ -n $output ]] && : > "$output"
  exit 0
fi

[[ -f $asset ]] || exit 22
if [[ -n $output ]]; then cp -- "$asset" "$output"; else cat -- "$asset"; fi
exit 0
FAKE

  chmod 0755 "$CASE_TMP/bin/systemctl" "$CASE_TMP/bin/ss" "$CASE_TMP/bin/curl"
}

# --- fake release ------------------------------------------------------------

# Builds a release the fake curl can serve: the two binaries, SHA256SUMS, and a
# source tarball holding the real scripts/ and packaging/ from this checkout.
build_release() {
  local web="$CASE_TMP/web"
  local arch
  case "$(uname -m)" in
    x86_64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) arch=amd64 ;;
  esac
  ARCH=$arch
  mkdir -p "$web/bear4f/Monitor/releases/download/$TEST_TAG"
  mkdir -p "$web/bear4f/Monitor/releases/download/$LATEST_TAG"
  mkdir -p "$web/bear4f/Monitor/archive/refs/tags"

  printf 'https://github.com/bear4f/Monitor/releases/latest https://github.com/bear4f/Monitor/releases/tag/%s\n' \
    "$LATEST_TAG" > "$web/redirects"

  local tag
  for tag in "$TEST_TAG" "$LATEST_TAG"; do
    local version=${tag#v}
    make_fake_binary monitor-server "$version" \
      "$web/bear4f/Monitor/releases/download/$tag/monitor-server-linux-$arch"
    make_fake_binary monitor-agent "$version" \
      "$web/bear4f/Monitor/releases/download/$tag/monitor-agent-linux-$arch"
    (
      cd "$web/bear4f/Monitor/releases/download/$tag"
      sha256sum "monitor-server-linux-$arch" "monitor-agent-linux-$arch" > SHA256SUMS
    )
    make_source_tarball "$tag" "$web/bear4f/Monitor/archive/refs/tags/$tag.tar.gz"
  done
}

make_fake_binary() {
  local name=$1 version=$2 destination=$3
  cat > "$destination" <<BINARY
#!/usr/bin/env bash
set -uo pipefail
LOG="$STATE"
BINARY
  cat >> "$destination" <<BINARY
NAME=$name
VERSION=$version
BINARY
  cat >> "$destination" <<'BINARY'
printf '%s argv: %s\n' "$NAME" "$*" >> "$LOG/admin.log"
if [[ ${1-} == --version ]]; then
  printf '%s %s\n' "$NAME" "$VERSION"
  exit 0
fi
if [[ ${*: -2} == "admin set-password" ]]; then
  secret=$(cat)
  bytes=$(printf '%s' "$secret" | LC_ALL=C wc -c)
  digest=$(printf '%s' "$secret" | sha256sum | cut -c1-16)
  printf 'set-password bytes=%s digest=%s uid=%s\n' "$bytes" "$digest" "$(id -u)" \
    >> "$LOG/admin.log"
  exit 0
fi
exit 0
BINARY
  chmod 0755 "$destination"
}

# A fake Server that does the one startup step that matters to the updater:
# open the database, refuse a schema newer than it supports, migrate a schema
# older than it supports, then either stay up or fail. When it fails after
# migrating it exits through os._exit so the committed frames stay in -wal and
# are never checkpointed -- the state that makes a naive main-file-only backup
# wrong.
make_schema_binary() {
  local destination=$1 version=$2 supported=$3 fail_flag=$4
  local helper="$destination.startup.py"
  cat > "$helper" <<'HELPER'
import os, sqlite3, sys
db, supported, fail_flag = sys.argv[1], int(sys.argv[2]), sys.argv[3]
connection = sqlite3.connect(db)
connection.execute("PRAGMA journal_mode=WAL")
installed = connection.execute("PRAGMA user_version").fetchone()[0]
if installed > supported:
    sys.stderr.write("unsupported schema version %d > %d\n" % (installed, supported))
    sys.exit(1)
if installed < supported:
    connection.execute(
        "ALTER TABLE nodes ADD COLUMN traffic_reset_mode TEXT NOT NULL DEFAULT 'monthly'"
    )
    connection.execute("INSERT INTO nodes (id, name) VALUES (900, 'migrated-marker')")
    connection.execute("PRAGMA user_version=%d" % supported)
    connection.commit()
if os.path.exists(fail_flag):
    sys.stderr.write("simulated post-migration startup failure\n")
    os._exit(1)
connection.close()
HELPER
  cat > "$destination" <<BINARY
#!/usr/bin/env bash
set -uo pipefail
LOG="$STATE"
NAME=monitor-server
VERSION=$version
SUPPORTED=$supported
FAIL_FLAG="$fail_flag"
HELPER_PY="$helper"
BINARY
  cat >> "$destination" <<'BINARY'
printf '%s argv: %s\n' "$NAME" "$*" >> "$LOG/admin.log"
if [[ ${1-} == --version ]]; then
  printf '%s %s\n' "$NAME" "$VERSION"
  exit 0
fi
database=
previous=
for argument in "$@"; do
  [[ $previous == --db ]] && database=$argument
  previous=$argument
done
[[ -n $database ]] || exit 0
exec python3 "$HELPER_PY" "$database" "$SUPPORTED" "$FAIL_FLAG"
BINARY
  chmod 0755 "$destination"
}

# A database shaped like the v0.1.2 schema: user_version 1, WAL, and rows in the
# tables the migration must not disturb.
seed_legacy_database() {
  local path=$1
  install -d -o monitor -g monitor -m 0750 "$(dirname -- "$path")"
  python3 - "$path" <<'SEED'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
for statement in (
    "PRAGMA journal_mode=WAL",
    "CREATE TABLE nodes (id INTEGER PRIMARY KEY, name TEXT NOT NULL) STRICT",
    "CREATE TABLE traffic_totals (node_id INTEGER PRIMARY KEY, rx_total_bytes INTEGER NOT NULL, tx_total_bytes INTEGER NOT NULL) STRICT",
    "CREATE TABLE ping_targets (id INTEGER PRIMARY KEY, name TEXT NOT NULL, host TEXT NOT NULL) STRICT",
    "CREATE TABLE ping_history (node_id INTEGER NOT NULL, bucket_ts INTEGER NOT NULL, target_id INTEGER NOT NULL, sample_count INTEGER NOT NULL, PRIMARY KEY (node_id, bucket_ts, target_id)) STRICT, WITHOUT ROWID",
    "INSERT INTO nodes VALUES (1, 'tokyo'), (2, 'osaka')",
    "INSERT INTO traffic_totals VALUES (1, 111111, 222222), (2, 333333, 444444)",
    "INSERT INTO ping_targets VALUES (1, 'cf v4', '1.1.1.1')",
    "INSERT INTO ping_history VALUES (1, 60, 1, 4), (1, 120, 1, 4), (2, 60, 1, 3)",
    "PRAGMA user_version=1",
):
    connection.execute(statement)
connection.commit()
connection.close()
SEED
  chown monitor:monitor "$path" "$path-wal" "$path-shm" 2>/dev/null || true
}

# Leaves committed frames in -wal by exiting without closing the connection, the
# way a Server killed mid-life does. A cleanly stopped SQLite deletes its -wal,
# so this is how a pre-update database ends up with one.
leave_uncheckpointed_wal() {
  local path=$1
  python3 - "$path" <<'WAL'
import os, sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute("PRAGMA journal_mode=WAL")
connection.execute("INSERT INTO nodes VALUES (3, 'nagoya')")
connection.commit()
os._exit(0)
WAL
  chown monitor:monitor "$path" "$path-wal" "$path-shm" 2>/dev/null || true
}

# A cleanly stopped database has no sidecars at all.
drop_wal_sidecars() {
  rm -f -- "$1-wal" "$1-shm"
}

# Every SQLite connection to a WAL database touches its sidecars: it recreates a
# missing -wal and rewrites -shm, and a read-write connection closing as the last
# one checkpoints and deletes -wal. Inspecting the live database would therefore
# destroy the very generation these cases are about, so readers work on a copy.
copy_database_for_reading() {
  local source=$1 destination
  destination=$(mktemp -d)/monitor.db
  cp -- "$source" "$destination"
  [[ -f $source-wal ]] && cp -- "$source-wal" "$destination-wal"
  printf '%s' "$destination"
}

schema_version_of() {
  local copy
  copy=$(copy_database_for_reading "$1")
  python3 -c "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('PRAGMA user_version').fetchone()[0])" "$copy"
  rm -rf -- "$(dirname -- "$copy")"
}

database_digest() {
  local copy
  copy=$(copy_database_for_reading "$1")
  python3 - "$copy" <<'DIGEST'
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
parts = []
for table in ("nodes", "traffic_totals", "ping_targets", "ping_history"):
    count = connection.execute("SELECT count(*) FROM " + table).fetchone()[0]
    parts.append("%s=%d" % (table, count))
parts.append("nodes:" + repr(connection.execute("SELECT id, name FROM nodes ORDER BY id").fetchall()))
parts.append("totals:" + repr(connection.execute("SELECT * FROM traffic_totals ORDER BY node_id").fetchall()))
parts.append("ping:" + repr(connection.execute("SELECT * FROM ping_history ORDER BY node_id, bucket_ts").fetchall()))
print(" | ".join(parts))
DIGEST
  rm -rf -- "$(dirname -- "$copy")"
}

make_source_tarball() {
  local tag=$1 destination=$2
  local stage="$CASE_TMP/stage/$tag/Monitor-${tag#v}"
  rm -rf -- "$CASE_TMP/stage/$tag"
  mkdir -p "$stage"
  cp -a "$REPO_ROOT/scripts" "$stage/scripts"
  cp -a "$REPO_ROOT/packaging" "$stage/packaging"
  tar -czf "$destination" -C "$CASE_TMP/stage/$tag" "Monitor-${tag#v}"
}

# --- helpers used by cases ---------------------------------------------------

install_server() {
  "$REPO_ROOT/scripts/install-monitor.sh" --version "$TEST_TAG" --component server "$@"
}

seed_foreign_listener() {
  local port=$1
  sleep 600 &
  FOREIGN_PID=$!
  printf '%s %s foreign\n' "$port" "$FOREIGN_PID" >> "$STATE/listeners"
}

managed_dropin=/etc/systemd/system/monitor-server.service.d/10-monitor-listen.conf
readonly managed_dropin

dropin_socket() {
  grep -m1 -o -- '--listen [^ ]*' "$managed_dropin" | cut -d' ' -f2
}
