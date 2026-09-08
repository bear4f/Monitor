#!/usr/bin/env bash
set -uo pipefail

# Deterministic regression tests for the Monitor installer lifecycle.
#
#   sudo tests/scripts/run.sh            run every case
#   sudo tests/scripts/run.sh NAME ...   run selected cases
#
# Each case runs in its own mount namespace over overlay mounts, so the real
# scripts under scripts/ execute unmodified at their real absolute paths while
# nothing on the host changes. systemd, ss, GitHub and the release binaries are
# faked; the privilege drop uses the real runuser. This is test-only tooling and
# is never installed.
#
# Real systemd is not exercised here. Unit activation on a real host stays a
# separate, manual verification step.

SELF=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/run.sh
readonly SELF
# shellcheck source=lib/sandbox.sh
source "$(dirname -- "$SELF")/lib/sandbox.sh"

CASES=(
  latest_release_redirect
  cwd_installer_never_executed
  default_listener
  custom_listener
  same_port_reinstall
  occupied_different_port_refused
  foreign_execstart_dropin_refused
  update_preserves_managed_listener
  update_rejects_foreign_execstart
  uninstall_preflight_before_mutation
  uninstall_rejects_foreign_execstart
  password_file_ownership_and_mode
  password_byte_length
  privilege_drop_without_sudo
  privilege_drop_missing
  password_hint_matches_available_tools
  agent_secret_not_in_argv
  invalid_token_refused
)

# --- cases -------------------------------------------------------------------

bootstrap() {
  # Always through "bash -s --", which is how the documented pipeline runs it
  # and the shape in which BASH_SOURCE[0] is empty.
  bash -s -- "$@" < "$REPO_ROOT/scripts/bootstrap-monitor.sh"
}

case_latest_release_redirect() {
  local output resolved
  # Negative control: the stand-in only resolves the redirect when --location is
  # passed, exactly like the real curl, so this case fails if the script ever
  # drops that flag again.
  resolved=$(curl --fail --silent --head --proto '=https' -o /dev/null \
    -w '%{url_effective}' https://github.com/bear4f/Monitor/releases/latest)
  [[ $resolved == https://github.com/bear4f/Monitor/releases/latest ]] \
    || return $(fail "a HEAD without --location must not resolve the redirect")
  resolved=$(curl --fail --silent --head --location --proto '=https' -o /dev/null \
    -w '%{url_effective}' https://github.com/bear4f/Monitor/releases/latest)
  [[ $resolved == "https://github.com/bear4f/Monitor/releases/tag/$LATEST_TAG" ]] \
    || return $(fail "the redirect stand-in is not wired up: $resolved")

  output=$(cd "$CASE_TMP" && bootstrap server 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "bootstrap failed"); }
  grep -q "Installing Monitor $LATEST_TAG " <<< "$output" \
    || return $(fail "resolved tag is not $LATEST_TAG: $output")
  grep -qx "https://github.com/bear4f/Monitor/archive/refs/tags/$LATEST_TAG.tar.gz" \
    "$STATE/curl.log" \
    || return $(fail "release tarball for $LATEST_TAG was never requested")
  [[ -x /usr/local/bin/monitor-server ]] || return $(fail "Server was not installed")
}

case_cwd_installer_never_executed() {
  local work="$CASE_TMP/cwd"
  mkdir -p "$work"
  cat > "$work/install-monitor.sh" <<SENTINEL
#!/bin/sh
touch "$CASE_TMP/SENTINEL_EXECUTED"
exit 0
SENTINEL
  chmod 0755 "$work/install-monitor.sh"
  local output
  output=$(cd "$work" && bootstrap server 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "bootstrap failed"); }
  [[ ! -e $CASE_TMP/SENTINEL_EXECUTED ]] \
    || return $(fail "the install-monitor.sh in the working directory was executed")
  grep -qx "https://github.com/bear4f/Monitor/archive/refs/tags/$LATEST_TAG.tar.gz" \
    "$STATE/curl.log" \
    || return $(fail "the release installer was not downloaded")
  [[ -x /usr/local/bin/monitor-server ]] || return $(fail "Server was not installed")
}

case_default_listener() {
  install_server >/dev/null || return $(fail "install failed")
  [[ ! -e $managed_dropin ]] \
    || return $(fail "a drop-in was written for the default listener")
  [[ ! -d /etc/systemd/system/monitor-server.service.d ]] \
    || return $(fail "an empty drop-in directory was left behind")
  grep -q '127.0.0.1:25774' /etc/systemd/system/monitor-server.service \
    || return $(fail "unit does not carry the default listener")
  cmp -s "$REPO_ROOT/packaging/monitor-server.service" \
    /etc/systemd/system/monitor-server.service \
    || return $(fail "installed unit is not byte-identical to packaging/")
  grep -q '^25774 ' "$STATE/listeners" || return $(fail "Server is not listening on 25774")
}

case_custom_listener() {
  install_server --listen 0.0.0.0 --port 25776 >/dev/null || return $(fail "install failed")
  [[ -f $managed_dropin ]] || return $(fail "drop-in was not written")
  [[ $(head -n 1 "$managed_dropin") == '# Managed-By: monitor-install (listener)' ]] \
    || return $(fail "drop-in marker is missing")
  [[ $(dropin_socket) == 0.0.0.0:25776 ]] \
    || return $(fail "drop-in listener is $(dropin_socket)")
  grep -qx 'ExecStart=' "$managed_dropin" \
    || return $(fail "drop-in does not reset the shipped ExecStart")
  cmp -s "$REPO_ROOT/packaging/monitor-server.service" \
    /etc/systemd/system/monitor-server.service \
    || return $(fail "installed unit is not byte-identical to packaging/")
  grep -q '^25776 ' "$STATE/listeners" || return $(fail "Server is not listening on 25776")

  # An explicit default restores the shipped unit and removes the drop-in.
  install_server --listen 127.0.0.1 --port 25774 >/dev/null \
    || return $(fail "restoring the default failed")
  [[ ! -e $managed_dropin ]] || return $(fail "drop-in survived a default install")
}

case_same_port_reinstall() {
  install_server >/dev/null || return $(fail "first install failed")
  install_server >/dev/null || return $(fail "reinstall on the same port was refused")
  grep -q '^25774 ' "$STATE/listeners" || return $(fail "Server is not listening on 25774")

  install_server --port 25776 >/dev/null || return $(fail "migration to a free port was refused")
  [[ $(dropin_socket) == 127.0.0.1:25776 ]] || return $(fail "listener was not migrated")
  install_server >/dev/null || return $(fail "reinstall did not preserve the listener")
  [[ $(dropin_socket) == 127.0.0.1:25776 ]] || return $(fail "reinstall reset the listener")
}

case_occupied_different_port_refused() {
  install_server >/dev/null || return $(fail "install failed")
  seed_foreign_listener 30000
  local output
  output=$(install_server --listen 0.0.0.0 --port 30000 2>&1) \
    && return $(fail "install onto an occupied foreign port was allowed")
  grep -q 'already in use by another service' <<< "$output" \
    || return $(fail "unexpected refusal: $output")
  kill -0 "$FOREIGN_PID" 2>/dev/null || return $(fail "the unknown listener was killed")
  grep -qx monitor-server.service "$STATE/active" \
    || return $(fail "the running Server was disturbed")
  [[ ! -e $managed_dropin ]] || return $(fail "a drop-in was written despite the refusal")
  grep -q '^25774 ' "$STATE/listeners" || return $(fail "the Server left its own port")
  kill "$FOREIGN_PID" 2>/dev/null
  wait "$FOREIGN_PID" 2>/dev/null
  return 0
}

case_foreign_execstart_dropin_refused() {
  mkdir -p /etc/systemd/system/monitor-server.service.d
  printf '[Service]\nExecStart=\nExecStart=/usr/bin/false\n' \
    > /etc/systemd/system/monitor-server.service.d/20-foreign.conf
  local output
  output=$(install_server 2>&1) && return $(fail "install through a foreign override was allowed")
  grep -q 'overrides ExecStart' <<< "$output" || return $(fail "unexpected refusal: $output")
  [[ ! -e /usr/local/bin/monitor-server ]] \
    || return $(fail "a binary was installed despite the refusal")
  [[ ! -e /etc/systemd/system/monitor-server.service ]] \
    || return $(fail "a unit was written despite the refusal")
  [[ -f /etc/systemd/system/monitor-server.service.d/20-foreign.conf ]] \
    || return $(fail "the unrelated drop-in was removed")

  # A drop-in that does not touch ExecStart is none of our business.
  rm -f /etc/systemd/system/monitor-server.service.d/20-foreign.conf
  printf '[Service]\nMemoryMax=256M\n' \
    > /etc/systemd/system/monitor-server.service.d/20-unrelated.conf
  install_server >/dev/null || return $(fail "an unrelated drop-in blocked the install")
  [[ -f /etc/systemd/system/monitor-server.service.d/20-unrelated.conf ]] \
    || return $(fail "the unrelated drop-in was removed")
}

case_update_preserves_managed_listener() {
  install_server --port 25776 >/dev/null || return $(fail "install failed")
  local before
  before=$(sha256sum "$managed_dropin" | cut -d' ' -f1)
  "$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server >/dev/null \
    || return $(fail "update failed")
  [[ $(sha256sum "$managed_dropin" | cut -d' ' -f1) == "$before" ]] \
    || return $(fail "update rewrote the managed drop-in")
  grep -q '^25776 ' "$STATE/listeners" || return $(fail "Server left its configured port")
}

case_update_rejects_foreign_execstart() {
  install_server >/dev/null || return $(fail "install failed")
  local before output
  before=$(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1)
  mkdir -p /etc/systemd/system/monitor-server.service.d
  printf '[Service]\nExecStart=\nExecStart=/usr/bin/false\n' \
    > /etc/systemd/system/monitor-server.service.d/20-foreign.conf
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && return $(fail "update through a foreign override was allowed")
  grep -q 'overrides ExecStart' <<< "$output" || return $(fail "unexpected refusal: $output")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$before" ]] \
    || return $(fail "the binary was replaced despite the refusal")
}

case_uninstall_preflight_before_mutation() {
  install_server --port 25776 >/dev/null || return $(fail "install failed")
  printf '[Service]\nExecStart=\nExecStart=/usr/local/bin/monitor-server --listen 127.0.0.1:25776 --db /var/lib/monitor/monitor.db\n' \
    > "$managed_dropin"
  local output
  output=$("$REPO_ROOT/scripts/uninstall-monitor.sh" --component server 2>&1) \
    && return $(fail "uninstall removed an unmarked drop-in")
  grep -q 'was not written by the Monitor installer' <<< "$output" \
    || return $(fail "unexpected refusal: $output")
  [[ -x /usr/local/bin/monitor-server ]] \
    || return $(fail "the binary was removed before the drop-in check")
  [[ -f /etc/systemd/system/monitor-server.service ]] \
    || return $(fail "the unit was removed before the drop-in check")
  grep -qx monitor-server.service "$STATE/active" \
    || return $(fail "the service was stopped before the drop-in check")

  # With the marker restored the uninstall completes and removes only ours.
  {
    printf '# Managed-By: monitor-install (listener)\n'
    printf '[Service]\nExecStart=\n'
    printf 'ExecStart=/usr/local/bin/monitor-server --listen 127.0.0.1:25776 --db /var/lib/monitor/monitor.db\n'
  } > "$managed_dropin"
  printf '[Service]\nMemoryMax=256M\n' \
    > /etc/systemd/system/monitor-server.service.d/20-unrelated.conf
  "$REPO_ROOT/scripts/uninstall-monitor.sh" --component server >/dev/null \
    || return $(fail "uninstall failed")
  [[ ! -e /usr/local/bin/monitor-server ]] || return $(fail "the binary survived uninstall")
  [[ ! -e $managed_dropin ]] || return $(fail "the managed drop-in survived uninstall")
  [[ -f /etc/systemd/system/monitor-server.service.d/20-unrelated.conf ]] \
    || return $(fail "an unrelated drop-in was removed")
}

case_uninstall_rejects_foreign_execstart() {
  install_server --port 25776 >/dev/null || return $(fail "install failed")
  local binary_hash unit_hash dropin_hash foreign_hash output
  binary_hash=$(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1)
  unit_hash=$(sha256sum /etc/systemd/system/monitor-server.service | cut -d' ' -f1)
  dropin_hash=$(sha256sum "$managed_dropin" | cut -d' ' -f1)

  local foreign=/etc/systemd/system/monitor-server.service.d/20-foreign.conf
  printf '[Service]\nExecStart=\nExecStart=/usr/bin/some-foreign-program\n' > "$foreign"
  foreign_hash=$(sha256sum "$foreign" | cut -d' ' -f1)

  output=$("$REPO_ROOT/scripts/uninstall-monitor.sh" --component server 2>&1) \
    && return $(fail "uninstall through a foreign ExecStart override was allowed")
  grep -q 'overrides ExecStart' <<< "$output" || return $(fail "unexpected refusal: $output")

  grep -qx monitor-server.service "$STATE/active" \
    || return $(fail "the service was stopped before the refusal")
  grep -qx monitor-server.service "$STATE/enabled" \
    || return $(fail "the service was disabled before the refusal")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$binary_hash" ]] \
    || return $(fail "the binary was touched before the refusal")
  [[ $(sha256sum /etc/systemd/system/monitor-server.service | cut -d' ' -f1) == "$unit_hash" ]] \
    || return $(fail "the unit was touched before the refusal")
  [[ $(sha256sum "$managed_dropin" | cut -d' ' -f1) == "$dropin_hash" ]] \
    || return $(fail "the managed drop-in was touched before the refusal")
  [[ $(sha256sum "$foreign" | cut -d' ' -f1) == "$foreign_hash" ]] \
    || return $(fail "the foreign drop-in was modified")
  [[ -d /var/lib/monitor ]] || return $(fail "Server data was removed before the refusal")

  # --purge must refuse just as early, before any data is deleted.
  output=$("$REPO_ROOT/scripts/uninstall-monitor.sh" --component server --purge 2>&1) \
    && return $(fail "--purge through a foreign ExecStart override was allowed")
  [[ -d /var/lib/monitor ]] || return $(fail "Server data was purged despite the refusal")

  # A drop-in that leaves ExecStart alone is unrelated: it must neither block
  # the uninstall nor be removed by it.
  rm -f -- "$foreign"
  local unrelated=/etc/systemd/system/monitor-server.service.d/20-unrelated.conf
  printf '[Service]\nMemoryMax=256M\n' > "$unrelated"
  "$REPO_ROOT/scripts/uninstall-monitor.sh" --component server >/dev/null \
    || return $(fail "an unrelated drop-in blocked the uninstall")
  [[ ! -e /usr/local/bin/monitor-server ]] || return $(fail "the binary survived uninstall")
  [[ ! -e /etc/systemd/system/monitor-server.service ]] \
    || return $(fail "the unit survived uninstall")
  [[ ! -e $managed_dropin ]] || return $(fail "the managed drop-in survived uninstall")
  [[ -f $unrelated ]] || return $(fail "the unrelated drop-in was removed")
}

case_password_hint_matches_available_tools() {
  local output limited
  # No password requested: the install succeeds and the hint has to name a tool
  # that exists here.
  output=$(bootstrap server --version "$TEST_TAG" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "install without a password failed"); }
  grep -q 'runuser -u monitor -- /usr/local/bin/monitor-server' <<< "$output" \
    || return $(fail "the hint does not name runuser: $output")

  "$REPO_ROOT/scripts/uninstall-monitor.sh" --component server --purge >/dev/null \
    || return $(fail "cleanup uninstall failed")

  limited=$(path_without sudo runuser)
  output=$(PATH=$limited bootstrap server --version "$TEST_TAG" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "install without runuser or sudo failed"); }
  grep -q 'Administrator password is not configured' <<< "$output" \
    || return $(fail "no honest hint without runuser or sudo: $output")
  grep -q 'Install util-linux (runuser) or sudo' <<< "$output" \
    || return $(fail "the hint does not say what to install: $output")
  grep -qE '(^|[^-])\bsudo -u monitor' <<< "$output" \
    && return $(fail "the hint printed a sudo command that does not exist here")
  [[ -x /usr/local/bin/monitor-server ]] \
    || return $(fail "a valid installation was refused for want of a password tool")
  return 0
}

case_password_file_ownership_and_mode() {
  local secret="$CASE_TMP/pw"
  printf 'correct horse battery staple\n' > "$secret"
  chmod 0644 "$secret"
  local output
  output=$(bootstrap server --version "$TEST_TAG" --admin-password-file "$secret" 2>&1) \
    && return $(fail "a 0644 password file was accepted")
  grep -q 'must have mode 0600' <<< "$output" || return $(fail "unexpected refusal: $output")
  [[ ! -e /usr/local/bin/monitor-server ]] \
    || return $(fail "the Server was installed before the password file was validated")

  ln -s "$secret" "$CASE_TMP/pw-link"
  output=$(bootstrap server --version "$TEST_TAG" --admin-password-file "$CASE_TMP/pw-link" 2>&1) \
    && return $(fail "a symlinked password file was accepted")
  grep -q 'must not be a symbolic link' <<< "$output" || return $(fail "unexpected refusal: $output")
  [[ ! -e /usr/local/bin/monitor-server ]] \
    || return $(fail "the Server was installed before the password file was validated")

  chmod 0600 "$secret"
  output=$(bootstrap server --version "$TEST_TAG" --admin-password-file "$secret" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "a valid password file was rejected"); }
  grep -q 'set-password bytes=28 ' "$STATE/admin.log" \
    || return $(fail "the password did not reach the Server on stdin: $(cat "$STATE/admin.log")")
  grep -q 'admin set-password' "$STATE/admin.log" || return $(fail "no admin invocation recorded")
  grep -q 'horse battery' "$STATE/admin.log" \
    && return $(fail "the password appeared in a recorded argv")
  grep -q 'horse battery' <<< "$output" && return $(fail "the password was printed")
  # The privilege drop must have left root behind.
  grep -q 'uid=0' "$STATE/admin.log" \
    && return $(fail "the admin CLI ran as root instead of the monitor user")
  return 0
}

case_password_byte_length() {
  local secret="$CASE_TMP/pw" output
  # Exactly 1024 bytes is accepted; 1025 is refused before installation.
  head -c 1024 /dev/zero | tr '\0' 'a' > "$secret"
  printf '\n' >> "$secret"
  chmod 0600 "$secret"
  output=$(bootstrap server --version "$TEST_TAG" --admin-password-file "$secret" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "1024 bytes was refused"); }
  grep -q 'set-password bytes=1024 ' "$STATE/admin.log" \
    || return $(fail "1024 bytes did not reach the Server: $(cat "$STATE/admin.log")")

  head -c 1025 /dev/zero | tr '\0' 'a' > "$secret"
  printf '\n' >> "$secret"
  chmod 0600 "$secret"
  output=$(bootstrap server --version "$TEST_TAG" --admin-password-file "$secret" 2>&1) \
    && return $(fail "1025 bytes was accepted")
  grep -q 'at most 1024 bytes (1025 given)' <<< "$output" \
    || return $(fail "unexpected refusal: $output")

  # Multibyte UTF-8 is counted in bytes, not characters: 400 x 3 bytes = 1200.
  local i=0
  : > "$secret"
  while (( i < 400 )); do printf '中' >> "$secret"; i=$((i + 1)); done
  printf '\n' >> "$secret"
  chmod 0600 "$secret"
  output=$(LANG=C.UTF-8 LC_ALL=C.UTF-8 bootstrap server --version "$TEST_TAG" \
    --admin-password-file "$secret" 2>&1) \
    && return $(fail "1200 bytes of UTF-8 was accepted")
  grep -q '(1200 given)' <<< "$output" \
    || return $(fail "UTF-8 length was counted in characters: $output")

  # 300 characters of the same text is 900 bytes and must be accepted.
  i=0
  : > "$secret"
  while (( i < 300 )); do printf '中' >> "$secret"; i=$((i + 1)); done
  printf '\n' >> "$secret"
  chmod 0600 "$secret"
  output=$(LANG=C.UTF-8 LC_ALL=C.UTF-8 bootstrap server --version "$TEST_TAG" \
    --admin-password-file "$secret" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "900 bytes of UTF-8 was refused"); }
  grep -q 'set-password bytes=900 ' "$STATE/admin.log" \
    || return $(fail "900 bytes did not reach the Server: $(cat "$STATE/admin.log")")
}

# Builds a PATH that contains every ordinary tool except the named ones.
path_without() {
  local excluded=" $* " dir file base
  mkdir -p "$CASE_TMP/limited"
  rm -f "$CASE_TMP/limited"/*
  for dir in /usr/local/bin /usr/bin /bin /usr/sbin /sbin; do
    [[ -d $dir ]] || continue
    for file in "$dir"/*; do
      base=${file##*/}
      [[ $excluded == *" $base "* ]] && continue
      [[ -e $CASE_TMP/limited/$base ]] && continue
      ln -s "$file" "$CASE_TMP/limited/$base" 2>/dev/null || true
    done
  done
  chmod 0755 "$CASE_TMP/limited"
  printf '%s' "$CASE_TMP/bin:$CASE_TMP/limited"
}

case_privilege_drop_without_sudo() {
  local secret="$CASE_TMP/pw" output limited
  printf 'a-perfectly-fine-password\n' > "$secret"
  chmod 0600 "$secret"
  limited=$(path_without sudo)
  command -v runuser >/dev/null || return $(fail "runuser is required to run this case")
  output=$(PATH=$limited bootstrap server --version "$TEST_TAG" \
    --admin-password-file "$secret" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "install without sudo failed"); }
  grep -q 'set-password bytes=25 ' "$STATE/admin.log" \
    || return $(fail "the password never reached the Server: $(cat "$STATE/admin.log")")
  grep -q 'uid=0' "$STATE/admin.log" \
    && return $(fail "the admin CLI ran as root instead of the monitor user")
  return 0
}

case_privilege_drop_missing() {
  local secret="$CASE_TMP/pw" output limited
  printf 'a-perfectly-fine-password\n' > "$secret"
  chmod 0600 "$secret"
  limited=$(path_without sudo runuser)
  output=$(PATH=$limited bootstrap server --version "$TEST_TAG" \
    --admin-password-file "$secret" 2>&1) \
    && return $(fail "install proceeded with no way to drop privileges")
  grep -q 'runuser (util-linux) or sudo is required' <<< "$output" \
    || return $(fail "unexpected refusal: $output")
  [[ ! -e /usr/local/bin/monitor-server ]] \
    || return $(fail "the Server was installed before the privilege drop was validated")
}

case_agent_secret_not_in_argv() {
  local token=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  local file="$CASE_TMP/token" output
  printf '%s\n' "$token" > "$file"
  chmod 0600 "$file"
  output=$(bootstrap agent --version "$TEST_TAG" --server https://monitor.example.com \
    --token-file "$file" 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "agent install failed"); }
  grep -q "$token" <<< "$output" && return $(fail "the token was printed")
  grep -rq "$token" "$STATE" && return $(fail "the token was recorded in a process argv")
  [[ $(stat -c '%u:%g:%a' /etc/monitor-agent.env) == 0:0:600 ]] \
    || return $(fail "the agent environment file has the wrong ownership or mode")
  grep -qx "MONITOR_TOKEN=$token" /etc/monitor-agent.env \
    || return $(fail "the token did not reach the agent environment file")
  compgen -G "$CASE_TMP/../*/monitor-agent.env" >/dev/null \
    && return $(fail "a staged token file was left behind")
  grep -qx monitor-agent.service "$STATE/active" || return $(fail "the Agent was not started")
  return 0
}

case_invalid_token_refused() {
  local file="$CASE_TMP/token" output
  printf 'not-a-token\n' > "$file"
  chmod 0600 "$file"
  output=$(bootstrap agent --version "$TEST_TAG" --server https://monitor.example.com \
    --token-file "$file" 2>&1) \
    && return $(fail "an invalid token was accepted")
  grep -q '64 lowercase hexadecimal' <<< "$output" || return $(fail "unexpected refusal: $output")
  [[ ! -e /usr/local/bin/monitor-agent ]] \
    || return $(fail "the Agent was installed before the token was validated")
  [[ ! -e /etc/monitor-agent.env ]] || return $(fail "an environment file was written")

  # Uppercase hex is rejected too; the Server never accepts it.
  printf '%s\n' 0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef > "$file"
  output=$(bootstrap agent --version "$TEST_TAG" --server https://monitor.example.com \
    --token-file "$file" 2>&1) \
    && return $(fail "an uppercase token was accepted")

  # A secret passed as an argument is refused outright.
  output=$(bootstrap agent --version "$TEST_TAG" --server https://monitor.example.com \
    --token 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef 2>&1) \
    && return $(fail "--token was accepted")
  grep -q 'secrets must never appear in command arguments' <<< "$output" \
    || return $(fail "unexpected refusal: $output")
}

# --- runner ------------------------------------------------------------------

run_case() {
  local name=$1
  CASE_TMP=$(mktemp -d)
  export CASE_TMP
  sandbox_init
  cd "$CASE_TMP"
  "case_$name"
}

if [[ ${1-} == --case ]]; then
  run_case "$2"
  exit $?
fi

[[ "$(id -u)" -eq 0 ]] || { echo "run as root" >&2; exit 1; }
[[ "$(uname -s)" == Linux ]] || { echo "Linux is required" >&2; exit 1; }
command -v unshare >/dev/null || { echo "unshare (util-linux) is required" >&2; exit 1; }

selected=("$@")
((${#selected[@]})) || selected=("${CASES[@]}")

passed=0
failed=0
failures=()
for name in "${selected[@]}"; do
  printf '%-40s' "$name"
  if output=$(unshare -m --propagation private "$SELF" --case "$name" 2>&1); then
    printf 'PASS\n'
    passed=$((passed + 1))
  else
    printf 'FAIL\n'
    printf '%s\n' "$output" | sed 's/^/    /'
    failed=$((failed + 1))
    failures+=("$name")
  fi
done

printf '\n%s passed, %s failed\n' "$passed" "$failed"
((failed == 0)) || { printf 'failed: %s\n' "${failures[*]}"; exit 1; }
