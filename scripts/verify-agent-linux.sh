#!/usr/bin/env bash
set -euo pipefail
set +x

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "ERROR: Linux is required" >&2
  exit 1
fi

arch="$(uname -m)"
case "$arch" in
  x86_64|aarch64) ;;
  *)
    echo "ERROR: unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

if [[ ! -r /etc/os-release ]]; then
  echo "ERROR: /etc/os-release is not readable" >&2
  exit 1
fi

distro="unknown"
while IFS='=' read -r key value; do
  if [[ "$key" == "ID" ]]; then
    distro="${value%\"}"
    distro="${distro#\"}"
    distro="${distro%\'}"
    distro="${distro#\'}"
    break
  fi
done < /etc/os-release

echo "distro=$distro"
echo "arch=$arch"
echo "kernel=$(uname -r)"

for source in \
  /proc/sys/kernel/hostname \
  /proc/sys/kernel/osrelease \
  /proc/sys/kernel/random/boot_id \
  /proc/cpuinfo \
  /proc/stat \
  /proc/loadavg \
  /proc/meminfo \
  /proc/net/dev \
  /proc/uptime; do
  if [[ ! -r "$source" ]]; then
    echo "ERROR: collector source is not readable: $source" >&2
    exit 1
  fi
done
echo "collector_sources=PASS"

if [[ -z "${MONITOR_SERVER:-}" || -z "${MONITOR_TOKEN:-}" ]]; then
  echo "runtime=NOT RUN — MONITOR_SERVER and MONITOR_TOKEN are required"
  exit 0
fi

if [[ ${#MONITOR_TOKEN} -ne 64 || "$MONITOR_TOKEN" == *[!0-9a-f]* ]]; then
  echo "ERROR: MONITOR_TOKEN must be 64 lowercase hexadecimal characters" >&2
  exit 1
fi

agent_bin="${MONITOR_AGENT_BIN:-target/release/monitor-agent}"
duration="${MONITOR_VERIFY_SECONDS:-60}"
if [[ ! -x "$agent_bin" ]]; then
  echo "ERROR: monitor-agent binary is not executable: $agent_bin" >&2
  exit 1
fi
if [[ ! "$duration" =~ ^[1-9][0-9]*$ ]]; then
  echo "ERROR: MONITOR_VERIFY_SECONDS must be a positive integer" >&2
  exit 1
fi

work_dir="$(mktemp -d)"
agent_log="$work_dir/agent.log"
agent_pid=""
cleanup() {
  if [[ -n "$agent_pid" ]] && kill -0 "$agent_pid" 2>/dev/null; then
    kill -TERM "$agent_pid" 2>/dev/null || true
    wait "$agent_pid" 2>/dev/null || true
  fi
  rm -rf -- "$work_dir"
}
trap cleanup EXIT INT TERM

"$agent_bin" >"$agent_log" 2>&1 &
agent_pid=$!
sleep 1
if ! kill -0 "$agent_pid" 2>/dev/null; then
  echo "ERROR: monitor-agent exited during startup" >&2
  sed -n '1,20p' "$agent_log" >&2
  exit 1
fi

cmdline="$(tr '\0' ' ' <"/proc/$agent_pid/cmdline")"
if [[ "$cmdline" == *"--token"* || "$cmdline" == *"$MONITOR_TOKEN"* ]]; then
  echo "ERROR: Agent token is present in process argv" >&2
  exit 1
fi
echo "argv_token=PASS"
echo "token=[redacted]"

start_ticks="$(awk '{print $14 + $15}' "/proc/$agent_pid/stat")"
start_time="$(date +%s)"
peak_rss=0
last_fd_count=0
for ((second = 1; second <= duration; second++)); do
  sleep 1
  if ! kill -0 "$agent_pid" 2>/dev/null; then
    echo "ERROR: monitor-agent exited during verification" >&2
    sed -n '1,20p' "$agent_log" >&2
    exit 1
  fi
  rss="$(awk '/^VmRSS:/ {print $2}' "/proc/$agent_pid/status")"
  fd_count="$(find "/proc/$agent_pid/fd" -mindepth 1 -maxdepth 1 -print 2>/dev/null | wc -l | tr -d ' ')"
  if ((rss > peak_rss)); then
    peak_rss=$rss
  fi
  last_fd_count=$fd_count
  if [[ "$second" == "10" || "$second" == "30" || "$second" == "$duration" ]]; then
    echo "sample_${second}s_rss_kib=$rss fd=$fd_count"
  fi
done

if grep -Eq 'server unavailable|report failed|configuration refresh failed' "$agent_log"; then
  echo "ERROR: Agent observed a config/report transport failure" >&2
  sed -n '1,20p' "$agent_log" >&2
  exit 1
fi

end_ticks="$(awk '{print $14 + $15}' "/proc/$agent_pid/stat")"
end_time="$(date +%s)"
clock_ticks="$(getconf CLK_TCK)"
elapsed=$((end_time - start_time))
cpu_ticks=$((end_ticks - start_ticks))
echo "runtime=PASS"
echo "report_204=PASS"
echo "peak_rss_kib=$peak_rss"
echo "final_fd_count=$last_fd_count"
echo "cpu_ticks=$cpu_ticks clock_ticks_per_second=$clock_ticks elapsed_seconds=$elapsed"
echo "icmp_results=inspect Server Ping history for configured targets"
