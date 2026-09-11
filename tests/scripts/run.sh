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
  update_rollback_restores_wal_generation
  update_rollback_without_wal_generation
  update_backup_is_root_only
  update_applies_schema_change
  update_refuses_foreign_backup_directory
  update_refuses_symlinked_database
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

# The Server migrates its database during startup, before it binds a listener,
# so a schema-raising release commits the migration and only then can fail the
# updater's stability check. Restoring just the binary would leave an older
# Server in front of a newer schema, which it refuses to open -- the rollback
# has to restore the database too.
#
# generation is "wal" for a pre-update database that still has an uncheckpointed
# -wal, or "nowal" for one that was stopped cleanly and has no sidecars.
prepare_schema_update() {
  local fail_flag=$1 generation=$2
  install_server >/dev/null || return $(fail "install failed")
  # Replace the installed Server with one that supports schema 1 only, and give
  # it a v0.1.2-shaped database.
  make_schema_binary /usr/local/bin/monitor-server 9.9.8 1 /nonexistent
  rm -f -- /var/lib/monitor/monitor.db /var/lib/monitor/monitor.db-wal /var/lib/monitor/monitor.db-shm
  seed_legacy_database /var/lib/monitor/monitor.db
  systemctl restart monitor-server.service >/dev/null \
    || return $(fail "the schema 1 Server could not start on its own database")
  if [[ $generation == wal ]]; then
    leave_uncheckpointed_wal /var/lib/monitor/monitor.db
    [[ -f /var/lib/monitor/monitor.db-wal ]] \
      || return $(fail "the fixture was supposed to leave a -wal behind")
  else
    drop_wal_sidecars /var/lib/monitor/monitor.db
    [[ ! -e /var/lib/monitor/monitor.db-wal ]] \
      || return $(fail "the fixture was supposed to have no -wal")
  fi
  # The release the updater will fetch supports schema 2.
  local asset="$CASE_TMP/web/bear4f/Monitor/releases/download/$TEST_TAG/monitor-server-linux-$ARCH"
  make_schema_binary "$asset" 9.9.9 2 "$fail_flag"
  (
    cd "$CASE_TMP/web/bear4f/Monitor/releases/download/$TEST_TAG"
    sha256sum "monitor-server-linux-$ARCH" "monitor-agent-linux-$ARCH" > SHA256SUMS
  )
}

backup_generation=/var/lib/monitor-update-backup/current
readonly backup_generation

run_failing_update() {
  local output
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && { printf '%s\n' "$output"; return 1; }
  printf '%s' "$output"
  return 0
}

# A. pre-update database has a WAL: the generation carries both files, the failed
#    Server's own sidecars are discarded, and the old pair comes back intact.
case_update_rollback_restores_wal_generation() {
  local fail_flag="$CASE_TMP/fail_start"
  : > "$fail_flag"
  prepare_schema_update "$fail_flag" wal || return 1

  local before_schema before_digest before_binary output
  before_schema=$(schema_version_of /var/lib/monitor/monitor.db)
  before_digest=$(database_digest /var/lib/monitor/monitor.db)
  before_binary=$(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1)
  [[ $before_schema == 1 ]] || return $(fail "fixture schema is $before_schema, expected 1")
  grep -q nagoya <<< "$before_digest" \
    || return $(fail "the row committed into the -wal is not visible before the update")

  output=$(run_failing_update) \
    || return $(fail "the update reported success even though the new Server failed to start")
  grep -q 'previous binary and pre-update database were restored and the service recovered' <<< "$output" \
    || return $(fail "unexpected failure message: $output")

  # The new Server really did migrate and then fail, so this case has something
  # to roll back.
  grep -q 'simulated post-migration startup failure' "$STATE/exec.log" \
    || return $(fail "the new Server never reached its failure point")
  grep -q 'migrated-marker' <<< "$(database_digest /var/lib/monitor/monitor.db)" \
    && return $(fail "the row the migration committed is still present after rollback")

  [[ -f $backup_generation/monitor.db && -f $backup_generation/monitor.db-wal ]] \
    || return $(fail "the generation does not hold both pre-update files")
  # 'nagoya' was committed into the -wal and never checkpointed, so it exists
  # only in that sidecar. Seeing it after the rollback is what proves the WAL
  # half of the generation was restored and not merely the main file.
  grep -q nagoya <<< "$(database_digest /var/lib/monitor/monitor.db)" \
    || return $(fail "the row that lived only in the pre-update -wal is gone")

  [[ $(schema_version_of /var/lib/monitor/monitor.db) == 1 ]] \
    || return $(fail "schema stayed at $(schema_version_of /var/lib/monitor/monitor.db) after rollback")
  [[ $(database_digest /var/lib/monitor/monitor.db) == "$before_digest" ]] \
    || return $(fail "pre-upgrade rows changed: $(database_digest /var/lib/monitor/monitor.db)")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$before_binary" ]] \
    || return $(fail "the previous binary was not restored")
  [[ $(stat -c '%U:%G' /var/lib/monitor/monitor.db) == monitor:monitor ]] \
    || return $(fail "restored database is owned by $(stat -c '%U:%G' /var/lib/monitor/monitor.db)")
  grep -qx monitor-server.service "$STATE/active" \
    || return $(fail "the service did not recover after rollback")

  # The restored pair must be usable by the restored binary, not merely present.
  systemctl restart monitor-server.service >/dev/null \
    || return $(fail "the restored binary and database cannot start together")
  return 0
}

# B. pre-update database has no WAL, and the persistent location still holds a
#    WAL from an earlier generation. That stale file must not become part of the
#    new generation and must never be replayed onto the restored database.
case_update_rollback_without_wal_generation() {
  local fail_flag="$CASE_TMP/fail_start"
  : > "$fail_flag"
  prepare_schema_update "$fail_flag" nowal || return 1

  # An earlier update left a complete generation, WAL included, in a root it had
  # marked as its own.
  install -d -o root -g root -m 0700 /var/lib/monitor-update-backup
  printf '# Managed-By: monitor-update (rollback generation) v1\n' \
    > /var/lib/monitor-update-backup/.monitor-managed
  chown root:root /var/lib/monitor-update-backup/.monitor-managed
  chmod 0600 /var/lib/monitor-update-backup/.monitor-managed
  install -d -o root -g root -m 0700 "$backup_generation"
  printf 'stale main from an earlier generation\n' > "$backup_generation/monitor.db"
  printf 'stale wal from an earlier generation\n' > "$backup_generation/monitor.db-wal"

  local before_digest before_binary output
  before_digest=$(database_digest /var/lib/monitor/monitor.db)
  before_binary=$(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1)

  output=$(run_failing_update) \
    || return $(fail "the update reported success even though the new Server failed to start")
  grep -q 'previous binary and pre-update database were restored and the service recovered' <<< "$output" \
    || return $(fail "unexpected failure message: $output")

  [[ -f $backup_generation/monitor.db ]] || return $(fail "the new generation has no main file")
  [[ ! -e $backup_generation/monitor.db-wal ]] \
    || return $(fail "the stale WAL survived into a generation taken without one")
  grep -q 'stale main' "$backup_generation/monitor.db" \
    && return $(fail "the generation still holds the earlier main file")
  [[ ! -e /var/lib/monitor/monitor.db-wal ]] \
    || return $(fail "a WAL was restored even though the generation had none")
  [[ ! -e /var/lib/monitor/monitor.db-shm ]] \
    || return $(fail "the failed Server's -shm was not removed")

  [[ $(schema_version_of /var/lib/monitor/monitor.db) == 1 ]] \
    || return $(fail "schema is $(schema_version_of /var/lib/monitor/monitor.db), expected 1")
  [[ $(database_digest /var/lib/monitor/monitor.db) == "$before_digest" ]] \
    || return $(fail "rows changed: $(database_digest /var/lib/monitor/monitor.db)")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$before_binary" ]] \
    || return $(fail "the previous binary was not restored")
  grep -qx monitor-server.service "$STATE/active" || return $(fail "the service did not recover")
  systemctl restart monitor-server.service >/dev/null \
    || return $(fail "the restored binary and database cannot start together")
  return 0
}

# C. after a successful update the service account must not be able to read or
#    alter the rollback copy it would be restored from.
case_update_backup_is_root_only() {
  prepare_schema_update "$CASE_TMP/never_created" wal || return 1
  "$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server >/dev/null \
    || return $(fail "a healthy schema update failed")

  local root=/var/lib/monitor-update-backup
  [[ $(stat -c '%U:%G:%a' "$root") == root:root:700 ]] \
    || return $(fail "$root is $(stat -c '%U:%G:%a' "$root"), expected root:root:700")
  [[ ! -L $root ]] || return $(fail "$root is a symbolic link")
  [[ -f $backup_generation/monitor.db ]] || return $(fail "the generation is missing")

  runuser -u monitor -- test -r "$root" \
    && return $(fail "the monitor account can read $root")
  runuser -u monitor -- ls "$backup_generation" >/dev/null 2>&1 \
    && return $(fail "the monitor account can list the generation")
  runuser -u monitor -- touch "$root/planted" 2>/dev/null \
    && return $(fail "the monitor account can create files in $root")
  runuser -u monitor -- rm -f "$backup_generation/monitor.db" 2>/dev/null
  [[ -f $backup_generation/monitor.db ]] \
    || return $(fail "the monitor account deleted the rollback copy")
  [[ ! -e $root/planted ]] || return $(fail "a file planted by the monitor account exists")

  # Nothing about the rollback copy lives under the Server's own state directory.
  compgen -G '/var/lib/monitor/*pre-update*' >/dev/null \
    && return $(fail "a rollback file was left inside the service-writable state directory")

  # A plain uninstall keeps the rollback copy; --purge removes it through the
  # same ownership-guarded path that removes Server data.
  "$REPO_ROOT/scripts/uninstall-monitor.sh" --component server >/dev/null \
    || return $(fail "uninstall failed")
  [[ -d $root ]] || return $(fail "a non-purging uninstall removed the rollback copy")
  [[ -d /var/lib/monitor ]] || return $(fail "a non-purging uninstall removed Server data")
  "$REPO_ROOT/scripts/uninstall-monitor.sh" --component server --purge >/dev/null \
    || return $(fail "purging uninstall failed")
  [[ ! -e $root ]] || return $(fail "--purge left the rollback copy behind")
  [[ ! -e /var/lib/monitor ]] || return $(fail "--purge left Server data behind")
  return 0
}

case_update_applies_schema_change() {
  prepare_schema_update "$CASE_TMP/never_created" wal || return 1

  local before_digest output
  before_digest=$(database_digest /var/lib/monitor/monitor.db)
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    || { printf '%s\n' "$output"; return $(fail "a healthy schema update failed"); }
  [[ $(schema_version_of /var/lib/monitor/monitor.db) == 2 ]] \
    || return $(fail "schema is $(schema_version_of /var/lib/monitor/monitor.db), expected 2")
  grep -q 'migrated-marker' <<< "$(database_digest /var/lib/monitor/monitor.db)" \
    || return $(fail "the migration did not run")
  grep -q tokyo <<< "$(database_digest /var/lib/monitor/monitor.db)" \
    || return $(fail "pre-upgrade rows were lost by a successful update")
  grep -qx monitor-server.service "$STATE/active" || return $(fail "the Server is not active")
  grep -q 'Pre-update database kept at /var/lib/monitor-update-backup/current' <<< "$output" \
    || return $(fail "the updater did not report where the pre-update copy is: $output")
  [[ $before_digest != "$(database_digest /var/lib/monitor/monitor.db)" ]] \
    || return $(fail "nothing changed, so this case proves nothing")
  return 0
}

# root:root 0700 is access control, not provenance: an unrelated directory can
# carry those bits legitimately. Without the marker this project writes, the
# updater must neither adopt the directory nor plant a marker in it, and --purge
# must never delete it.
case_update_refuses_foreign_backup_directory() {
  local root=/var/lib/monitor-update-backup
  local sentinel="$root/someone-elses-data"
  prepare_schema_update "$CASE_TMP/never_created" wal || return 1

  local before_binary before_digest output sentinel_digest
  before_binary=$(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1)
  before_digest=$(database_digest /var/lib/monitor/monitor.db)

  install -d -o root -g root -m 0700 "$root"
  printf 'unrelated backup payload\n' > "$sentinel"
  chmod 0600 "$sentinel"
  sentinel_digest=$(sha256sum "$sentinel" | cut -d' ' -f1)

  # No marker at all.
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && return $(fail "the update adopted a directory it did not create")
  grep -q 'was not created by Monitor and will not be used' <<< "$output" \
    || return $(fail "unexpected refusal: $output")
  [[ ! -e $root/.monitor-managed ]] \
    || return $(fail "a Monitor marker was planted in a foreign directory")

  # A falsified marker must not be accepted either.
  printf '# Managed-By: monitor-update (rollback generation) v99\n' > "$root/.monitor-managed"
  chown root:root "$root/.monitor-managed"
  chmod 0600 "$root/.monitor-managed"
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && return $(fail "the update accepted a falsified marker")
  grep -q 'content does not match this Monitor version' <<< "$output" \
    || return $(fail "unexpected refusal for a falsified marker: $output")

  # A world-readable marker with the right text is still wrong.
  printf '# Managed-By: monitor-update (rollback generation) v1\n' > "$root/.monitor-managed"
  chmod 0644 "$root/.monitor-managed"
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && return $(fail "the update accepted a marker with the wrong mode")
  grep -q 'must be owned by root:root with mode 0600' <<< "$output" \
    || return $(fail "unexpected refusal for a bad marker mode: $output")

  # Nothing was touched by any of the three refusals.
  [[ $(sha256sum "$sentinel" | cut -d' ' -f1) == "$sentinel_digest" ]] \
    || return $(fail "the unrelated file was modified")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$before_binary" ]] \
    || return $(fail "the Server binary was replaced despite the refusal")
  [[ $(database_digest /var/lib/monitor/monitor.db) == "$before_digest" ]] \
    || return $(fail "the database changed despite the refusal")
  [[ $(schema_version_of /var/lib/monitor/monitor.db) == 1 ]] \
    || return $(fail "the database was migrated despite the refusal")
  [[ ! -e $root/current ]] || return $(fail "a generation was written into a foreign directory")
  grep -qx monitor-server.service "$STATE/active" \
    || return $(fail "the running Server was disturbed by a refusal")

  # --purge must refuse the same directory rather than delete someone else's data.
  output=$("$REPO_ROOT/scripts/uninstall-monitor.sh" --component server --purge 2>&1) \
    && return $(fail "--purge deleted a directory Monitor does not own")
  grep -q 'refusing to remove it' <<< "$output" \
    || return $(fail "unexpected purge refusal: $output")
  [[ -d $root && $(sha256sum "$sentinel" | cut -d' ' -f1) == "$sentinel_digest" ]] \
    || return $(fail "--purge damaged the unrelated directory")
  return 0
}

# The live database directory is writable by the monitor service account, so root
# must refuse to follow anything it finds there instead of copying through it.
case_update_refuses_symlinked_database() {
  prepare_schema_update "$CASE_TMP/never_created" nowal || return 1
  local secret="$CASE_TMP/not-a-database"
  printf 'a file root can read but the updater must never copy\n' > "$secret"
  local secret_digest
  secret_digest=$(sha256sum "$secret" | cut -d' ' -f1)

  local before_binary real_digest output
  before_binary=$(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1)
  real_digest=$(sha256sum /var/lib/monitor/monitor.db | cut -d' ' -f1)

  # The database itself replaced by a symlink.
  mv -- /var/lib/monitor/monitor.db "$CASE_TMP/real.db"
  ln -s "$secret" /var/lib/monitor/monitor.db
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && return $(fail "the update followed a symlinked database")
  grep -q 'monitor.db is a symbolic link; refusing to copy through it' <<< "$output" \
    || return $(fail "unexpected refusal: $output")
  [[ $(sha256sum "$secret" | cut -d' ' -f1) == "$secret_digest" ]] \
    || return $(fail "the symlink target was modified")
  [[ ! -e /var/lib/monitor-update-backup/current ]] \
    || return $(fail "the symlink target was copied into a generation")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$before_binary" ]] \
    || return $(fail "the binary was replaced despite the refusal")
  rm -f -- /var/lib/monitor/monitor.db
  mv -- "$CASE_TMP/real.db" /var/lib/monitor/monitor.db
  chown monitor:monitor /var/lib/monitor/monitor.db

  # A real database with a symlinked WAL beside it.
  ln -s "$secret" /var/lib/monitor/monitor.db-wal
  output=$("$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server 2>&1) \
    && return $(fail "the update followed a symlinked -wal")
  grep -q 'monitor.db-wal is a symbolic link; refusing to copy through it' <<< "$output" \
    || return $(fail "unexpected refusal for a symlinked -wal: $output")
  [[ $(sha256sum "$secret" | cut -d' ' -f1) == "$secret_digest" ]] \
    || return $(fail "the symlink target was modified")
  [[ ! -e /var/lib/monitor-update-backup/current ]] \
    || return $(fail "a generation was written despite the refusal")
  [[ $(sha256sum /var/lib/monitor/monitor.db | cut -d' ' -f1) == "$real_digest" ]] \
    || return $(fail "the real database was modified")
  [[ $(sha256sum /usr/local/bin/monitor-server | cut -d' ' -f1) == "$before_binary" ]] \
    || return $(fail "the binary was replaced despite the refusal")
  grep -qx monitor-server.service "$STATE/active" \
    || return $(fail "the running Server was disturbed by a refusal")

  # With the symlink gone the same update succeeds, so the guard is not blanket.
  rm -f -- /var/lib/monitor/monitor.db-wal
  "$REPO_ROOT/scripts/update-monitor.sh" --version "$TEST_TAG" --component server >/dev/null \
    || return $(fail "a clean database was refused as well")
  [[ $(schema_version_of /var/lib/monitor/monitor.db) == 2 ]] \
    || return $(fail "the healthy update did not migrate")
  return 0
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
command -v python3 >/dev/null || { echo "python3 is required for the schema fixtures" >&2; exit 1; }

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
