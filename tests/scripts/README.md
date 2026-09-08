# Installer regression tests

Deterministic Bash tests for the install / update / uninstall lifecycle.

```sh
sudo tests/scripts/run.sh            # every case
sudo tests/scripts/run.sh NAME ...   # selected cases
```

Each case runs in its own mount namespace with overlay mounts over `/etc`,
`/usr/local/bin` and `/var/lib`, so the real scripts under `scripts/` execute
unmodified at their real absolute paths while nothing on the host changes.
systemd, `ss`, GitHub and the release binaries are replaced by small fakes on
`PATH`; the privilege drop uses the real `runuser`.

Requirements: root, Linux, `unshare`, overlayfs, `runuser`.

This is test-only tooling. It is never installed, never downloaded by the
installers, and adds no runtime dependency.

**Real systemd is not exercised here.** Unit activation, drop-in parsing and
service restart on a real host remain a separate manual verification step.
