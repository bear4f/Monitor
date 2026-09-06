use std::{error::Error, fmt};

use monitor_common::AgentReport;

#[cfg(any(target_os = "linux", test))]
use monitor_common::{CpuReport, DiskReport, MemoryReport, NetworkReport, OsReport};
#[cfg(any(target_os = "linux", test))]
use std::time::Instant;

#[cfg(any(target_os = "linux", test))]
const JS_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticInfo {
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel: String,
    pub architecture: String,
    pub virtualization: String,
    pub cpu_model: String,
    pub cpu_cores: i64,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicSample {
    pub cpu_usage: f64,
    pub load_1: f64,
    pub load_5: f64,
    pub load_15: f64,
    pub memory_total: i64,
    pub memory_used: i64,
    pub swap_total: i64,
    pub swap_used: i64,
    pub disk_total: i64,
    pub disk_used: i64,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub rx_rate: i64,
    pub tx_rate: i64,
    pub uptime_seconds: i64,
    pub process_count: i64,
}

#[derive(Debug)]
pub struct CollectorError(String);

impl CollectorError {
    fn parse(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CollectorError {}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuCounters {
    total: u64,
    idle: u64,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Default)]
struct CpuTracker {
    previous: Option<CpuCounters>,
}

#[cfg(any(target_os = "linux", test))]
impl CpuTracker {
    fn baseline(&mut self, counters: CpuCounters) {
        self.previous = Some(counters);
    }

    fn sample(&mut self, current: CpuCounters) -> f64 {
        let Some(previous) = self.previous.replace(current) else {
            return 0.0;
        };
        let Some(total_delta) = current.total.checked_sub(previous.total) else {
            return 0.0;
        };
        let Some(idle_delta) = current.idle.checked_sub(previous.idle) else {
            return 0.0;
        };
        if total_delta == 0 || idle_delta > total_delta {
            return 0.0;
        }
        ((total_delta - idle_delta) as f64 / total_delta as f64 * 100.0).clamp(0.0, 100.0)
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkCounters {
    rx: u64,
    tx: u64,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Default)]
struct NetworkTracker {
    previous: Option<(NetworkCounters, Instant)>,
}

#[cfg(any(target_os = "linux", test))]
impl NetworkTracker {
    fn baseline(&mut self, counters: NetworkCounters, now: Instant) {
        self.previous = Some((counters, now));
    }

    fn sample(
        &mut self,
        current: NetworkCounters,
        now: Instant,
    ) -> Result<(i64, i64), CollectorError> {
        let Some((previous, previous_at)) = self.previous.replace((current, now)) else {
            return Ok((0, 0));
        };
        let elapsed = now.saturating_duration_since(previous_at).as_secs_f64();
        if elapsed <= 0.0 {
            return Ok((0, 0));
        }
        Ok((
            rate(current.rx, previous.rx, elapsed)?,
            rate(current.tx, previous.tx, elapsed)?,
        ))
    }
}

#[cfg(target_os = "linux")]
pub struct SystemCollector {
    static_info: StaticInfo,
    cpu: CpuTracker,
    network: NetworkTracker,
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub struct SystemCollector;

#[cfg(target_os = "linux")]
impl SystemCollector {
    pub fn new() -> Result<Self, CollectorError> {
        Ok(Self {
            static_info: linux::collect_static()?,
            cpu: CpuTracker::default(),
            network: NetworkTracker::default(),
        })
    }

    pub fn static_info(&self) -> &StaticInfo {
        &self.static_info
    }

    pub fn initialize_baselines(&mut self) -> Result<(), CollectorError> {
        self.cpu.baseline(linux::read_cpu()?);
        self.network
            .baseline(linux::read_network()?, Instant::now());
        Ok(())
    }

    pub fn collect_report(&mut self) -> Result<AgentReport, CollectorError> {
        let dynamic = linux::collect_dynamic(&mut self.cpu, &mut self.network)?;
        Ok(build_report(&self.static_info, dynamic))
    }
}

#[cfg(not(target_os = "linux"))]
impl SystemCollector {
    pub fn new() -> Result<Self, CollectorError> {
        Err(CollectorError::parse("monitor-agent supports Linux only"))
    }

    pub fn static_info(&self) -> &StaticInfo {
        unreachable!("unsupported platform has no static information")
    }

    pub fn initialize_baselines(&mut self) -> Result<(), CollectorError> {
        Err(CollectorError::parse("monitor-agent supports Linux only"))
    }

    pub fn collect_report(&mut self) -> Result<AgentReport, CollectorError> {
        Err(CollectorError::parse("monitor-agent supports Linux only"))
    }
}

#[cfg(any(target_os = "linux", test))]
fn build_report(static_info: &StaticInfo, dynamic: DynamicSample) -> AgentReport {
    AgentReport {
        protocol_version: 1,
        agent_version: crate::VERSION.to_owned(),
        boot_id: static_info.boot_id.clone(),
        hostname: static_info.hostname.clone(),
        os: OsReport {
            name: static_info.os_name.clone(),
            version: static_info.os_version.clone(),
            kernel: static_info.kernel.clone(),
            architecture: static_info.architecture.clone(),
            virtualization: static_info.virtualization.clone(),
        },
        cpu: CpuReport {
            model: static_info.cpu_model.clone(),
            cores: static_info.cpu_cores,
            usage: dynamic.cpu_usage,
            load_1: dynamic.load_1,
            load_5: dynamic.load_5,
            load_15: dynamic.load_15,
        },
        memory: MemoryReport {
            total: dynamic.memory_total,
            used: dynamic.memory_used,
            swap_total: dynamic.swap_total,
            swap_used: dynamic.swap_used,
        },
        disk: DiskReport {
            total: dynamic.disk_total,
            used: dynamic.disk_used,
        },
        network: NetworkReport {
            rx_bytes: dynamic.rx_bytes,
            tx_bytes: dynamic.tx_bytes,
            rx_rate: dynamic.rx_rate,
            tx_rate: dynamic.tx_rate,
        },
        uptime_seconds: dynamic.uptime_seconds,
        process_count: dynamic.process_count,
        pings: Vec::new(),
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_os_release(input: &str) -> Result<(String, String), CollectorError> {
    let mut name = None;
    let mut version_id = None;
    let mut version = None;
    for line in input.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let value = unquote_os_release(raw_value.trim())?;
        match key {
            "NAME" => name = Some(value),
            "VERSION_ID" => version_id = Some(value),
            "VERSION" => version = Some(value),
            _ => {}
        }
    }
    let name = name.ok_or_else(|| CollectorError::parse("/etc/os-release is missing NAME"))?;
    let version = version_id.or(version).unwrap_or_default();
    validate_text("OS name", &name, 1, 128)?;
    validate_text("OS version", &version, 0, 128)?;
    Ok((name, version))
}

#[cfg(any(target_os = "linux", test))]
fn unquote_os_release(value: &str) -> Result<String, CollectorError> {
    if value.starts_with('"') || value.starts_with('\'') {
        let quote = value.as_bytes()[0];
        if value.len() < 2 || value.as_bytes()[value.len() - 1] != quote {
            return Err(CollectorError::parse("unterminated os-release value"));
        }
        let inner = &value[1..value.len() - 1];
        if quote == b'\'' {
            return Ok(inner.to_owned());
        }
        let mut decoded = String::with_capacity(inner.len());
        let mut escaped = false;
        for character in inner.chars() {
            if escaped {
                decoded.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                decoded.push(character);
            }
        }
        if escaped {
            return Err(CollectorError::parse("invalid os-release escape"));
        }
        Ok(decoded)
    } else {
        Ok(value.to_owned())
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_cpu_info(input: &str) -> Result<(String, i64), CollectorError> {
    let mut cores = 0_u64;
    let mut model_name = None;
    let mut processor_name = None;
    let mut hardware = None;
    let mut implementer = None;
    let mut part = None;
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "processor" if value.bytes().all(|byte| byte.is_ascii_digit()) => cores += 1,
            "model name" if model_name.is_none() => model_name = Some(value.to_owned()),
            "Processor" if processor_name.is_none() => processor_name = Some(value.to_owned()),
            "Hardware" if hardware.is_none() => hardware = Some(value.to_owned()),
            "CPU implementer" if implementer.is_none() => implementer = Some(value.to_owned()),
            "CPU part" if part.is_none() => part = Some(value.to_owned()),
            _ => {}
        }
    }
    if !(1..=4096).contains(&cores) {
        return Err(CollectorError::parse("invalid logical CPU count"));
    }
    let model = model_name
        .or(processor_name)
        .or(hardware)
        .or_else(|| match (implementer, part) {
            (Some(implementer), Some(part)) => Some(format!("ARM {implementer} {part}")),
            _ => None,
        })
        .unwrap_or_else(|| "Unknown CPU".to_owned());
    validate_text("CPU model", &model, 1, 255)?;
    Ok((model, cores as i64))
}

#[cfg(any(target_os = "linux", test))]
fn parse_cpu_stat(input: &str) -> Result<CpuCounters, CollectorError> {
    let line = input
        .lines()
        .next()
        .ok_or_else(|| CollectorError::parse("/proc/stat is empty"))?;
    let mut fields = line.split_whitespace();
    if fields.next() != Some("cpu") {
        return Err(CollectorError::parse("/proc/stat lacks aggregate CPU row"));
    }
    let values = fields
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CollectorError::parse("invalid /proc/stat counter"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() < 4 {
        return Err(CollectorError::parse("incomplete /proc/stat CPU row"));
    }
    // Linux guest counters are already included in user/nice, so only the first eight fields
    // participate in total time.
    let total = values.iter().take(8).try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| CollectorError::parse("CPU counter overflow"))
    })?;
    let idle = values[3]
        .checked_add(values.get(4).copied().unwrap_or(0))
        .ok_or_else(|| CollectorError::parse("CPU idle counter overflow"))?;
    Ok(CpuCounters { total, idle })
}

#[cfg(any(target_os = "linux", test))]
fn parse_loadavg(input: &str) -> Result<(f64, f64, f64), CollectorError> {
    let mut fields = input.split_whitespace();
    let mut next = || {
        let value = fields
            .next()
            .ok_or_else(|| CollectorError::parse("incomplete /proc/loadavg"))?
            .parse::<f64>()
            .map_err(|_| CollectorError::parse("invalid /proc/loadavg value"))?;
        if !value.is_finite() || value < 0.0 {
            return Err(CollectorError::parse("invalid /proc/loadavg range"));
        }
        Ok(value)
    };
    Ok((next()?, next()?, next()?))
}

#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(input: &str) -> Result<(i64, i64, i64, i64), CollectorError> {
    let mut values = std::collections::HashMap::new();
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(number) = value.split_whitespace().next() else {
            continue;
        };
        let kilobytes = number
            .parse::<u64>()
            .map_err(|_| CollectorError::parse("invalid /proc/meminfo value"))?;
        values.insert(key, kilobytes);
    }
    let get = |key| {
        values
            .get(key)
            .copied()
            .ok_or_else(|| CollectorError::parse(format!("/proc/meminfo is missing {key}")))
    };
    let total = kb_to_bytes(get("MemTotal")?)?;
    let available = match values.get("MemAvailable").copied() {
        Some(value) => kb_to_bytes(value)?,
        None => ["MemFree", "Buffers", "Cached"]
            .into_iter()
            .try_fold(0_u64, |sum, key| {
                sum.checked_add(kb_to_bytes(get(key)?)?)
                    .ok_or_else(|| CollectorError::parse("memory fallback overflow"))
            })?,
    };
    let used = total
        .checked_sub(available)
        .ok_or_else(|| CollectorError::parse("available memory exceeds total"))?;
    let swap_total = kb_to_bytes(get("SwapTotal")?)?;
    let swap_free = kb_to_bytes(get("SwapFree")?)?;
    let swap_used = swap_total
        .checked_sub(swap_free)
        .ok_or_else(|| CollectorError::parse("free swap exceeds total"))?;
    Ok((
        safe_i64(total, "memory total")?,
        safe_i64(used, "memory used")?,
        safe_i64(swap_total, "swap total")?,
        safe_i64(swap_used, "swap used")?,
    ))
}

#[cfg(any(target_os = "linux", test))]
fn parse_uptime(input: &str) -> Result<i64, CollectorError> {
    let seconds = input
        .split_whitespace()
        .next()
        .ok_or_else(|| CollectorError::parse("/proc/uptime is empty"))?
        .parse::<f64>()
        .map_err(|_| CollectorError::parse("invalid /proc/uptime"))?;
    if !seconds.is_finite() || seconds < 0.0 || seconds.floor() > JS_SAFE_INTEGER_MAX as f64 {
        return Err(CollectorError::parse("invalid /proc/uptime range"));
    }
    Ok(seconds.floor() as i64)
}

#[cfg(any(target_os = "linux", test))]
fn is_process_directory_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(any(target_os = "linux", test))]
fn parse_ipv4_default_route(input: &str) -> Option<String> {
    input
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 8 || fields[1] != "00000000" {
                return None;
            }
            let flags = u32::from_str_radix(fields[3], 16).ok()?;
            if flags & 1 == 0 {
                return None;
            }
            let metric = fields[6].parse::<u64>().ok()?;
            Some((metric, fields[0].to_owned()))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, interface)| interface)
}

#[cfg(any(target_os = "linux", test))]
fn parse_ipv6_default_route(input: &str) -> Option<String> {
    input
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 10
                || fields[0] != "00000000000000000000000000000000"
                || fields[1] != "00"
            {
                return None;
            }
            let flags = u32::from_str_radix(fields[8], 16).ok()?;
            if flags & 1 == 0 {
                return None;
            }
            let metric = u64::from_str_radix(fields[5], 16).ok()?;
            Some((metric, fields[9].to_owned()))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, interface)| interface)
}

#[cfg(any(target_os = "linux", test))]
fn parse_net_dev(input: &str, interface: Option<&str>) -> Result<NetworkCounters, CollectorError> {
    let mut total = NetworkCounters { rx: 0, tx: 0 };
    let mut found = false;
    for line in input.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let selected = interface.map_or(name != "lo", |selected| name == selected);
        if !selected {
            continue;
        }
        let fields = values.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 16 {
            return Err(CollectorError::parse("incomplete /proc/net/dev row"));
        }
        let rx = fields[0]
            .parse::<u64>()
            .map_err(|_| CollectorError::parse("invalid RX counter"))?;
        let tx = fields[8]
            .parse::<u64>()
            .map_err(|_| CollectorError::parse("invalid TX counter"))?;
        total.rx = total
            .rx
            .checked_add(rx)
            .ok_or_else(|| CollectorError::parse("RX counter overflow"))?;
        total.tx = total
            .tx
            .checked_add(tx)
            .ok_or_else(|| CollectorError::parse("TX counter overflow"))?;
        found = true;
        if interface.is_some() {
            break;
        }
    }
    if !found {
        return Err(CollectorError::parse(match interface {
            Some(_) => "default route interface is absent from /proc/net/dev",
            None => "no non-loopback interface in /proc/net/dev",
        }));
    }
    safe_i64(total.rx, "RX counter")?;
    safe_i64(total.tx, "TX counter")?;
    Ok(total)
}

#[cfg(any(target_os = "linux", test))]
fn detect_virtualization(product: &str, vendor: &str, hypervisor: &str, cgroup: &str) -> String {
    let joined = format!("{product}\n{vendor}\n{hypervisor}\n{cgroup}").to_ascii_lowercase();
    for (needle, label) in [
        ("docker", "docker"),
        ("lxc", "lxc"),
        ("virtualbox", "virtualbox"),
        ("vmware", "vmware"),
        ("xen", "xen"),
        ("kvm", "kvm"),
        ("qemu", "qemu"),
    ] {
        if joined.contains(needle) {
            return label.to_owned();
        }
    }
    "unknown".to_owned()
}

#[cfg(any(target_os = "linux", test))]
fn rate(current: u64, previous: u64, elapsed: f64) -> Result<i64, CollectorError> {
    let Some(delta) = current.checked_sub(previous) else {
        return Ok(0);
    };
    let value = (delta as f64 / elapsed).round();
    if !value.is_finite() || value < 0.0 || value > JS_SAFE_INTEGER_MAX as f64 {
        return Err(CollectorError::parse(
            "network rate exceeds safe integer range",
        ));
    }
    Ok(value as i64)
}

#[cfg(any(target_os = "linux", test))]
fn kb_to_bytes(value: u64) -> Result<u64, CollectorError> {
    value
        .checked_mul(1024)
        .ok_or_else(|| CollectorError::parse("kilobyte conversion overflow"))
}

#[cfg(any(target_os = "linux", test))]
fn safe_i64(value: u64, field: &str) -> Result<i64, CollectorError> {
    if value > JS_SAFE_INTEGER_MAX {
        Err(CollectorError::parse(format!(
            "{field} exceeds the JSON safe integer range"
        )))
    } else {
        Ok(value as i64)
    }
}

#[cfg(any(target_os = "linux", test))]
fn validate_text(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), CollectorError> {
    let length = value.trim().chars().count();
    if (minimum..=maximum).contains(&length) {
        Ok(())
    } else {
        Err(CollectorError::parse(format!("invalid {field}")))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{ffi::CString, fs, time::Instant};

    use super::*;

    pub(super) fn collect_static() -> Result<StaticInfo, CollectorError> {
        let hostname = read_trimmed("/proc/sys/kernel/hostname")?;
        validate_text("hostname", &hostname, 1, 255)?;
        let (os_name, os_version) = parse_os_release(&read("/etc/os-release")?)?;
        let kernel = read_trimmed("/proc/sys/kernel/osrelease")?;
        validate_text("kernel", &kernel, 1, 255)?;
        let architecture = architecture()?;
        let (cpu_model, cpu_cores) = parse_cpu_info(&read("/proc/cpuinfo")?)?;
        let boot_id = read_trimmed("/proc/sys/kernel/random/boot_id")?;
        validate_text("boot ID", &boot_id, 1, 64)?;
        let virtualization = detect_virtualization(
            &read_optional("/sys/class/dmi/id/product_name"),
            &read_optional("/sys/class/dmi/id/sys_vendor"),
            &read_optional("/sys/hypervisor/type"),
            &read_optional("/proc/1/cgroup"),
        );
        Ok(StaticInfo {
            hostname,
            os_name,
            os_version,
            kernel,
            architecture,
            virtualization,
            cpu_model,
            cpu_cores,
            boot_id,
        })
    }

    pub(super) fn read_cpu() -> Result<CpuCounters, CollectorError> {
        parse_cpu_stat(&read("/proc/stat")?)
    }

    pub(super) fn read_network() -> Result<NetworkCounters, CollectorError> {
        let ipv4 = read("/proc/net/route")?;
        let interface = parse_ipv4_default_route(&ipv4).or_else(|| {
            read("/proc/net/ipv6_route")
                .ok()
                .and_then(|routes| parse_ipv6_default_route(&routes))
        });
        parse_net_dev(&read("/proc/net/dev")?, interface.as_deref())
    }

    pub(super) fn collect_dynamic(
        cpu: &mut CpuTracker,
        network: &mut NetworkTracker,
    ) -> Result<DynamicSample, CollectorError> {
        let cpu_usage = cpu.sample(read_cpu()?);
        let (load_1, load_5, load_15) = parse_loadavg(&read("/proc/loadavg")?)?;
        let (memory_total, memory_used, swap_total, swap_used) =
            parse_meminfo(&read("/proc/meminfo")?)?;
        let (disk_total, disk_used) = disk_usage()?;
        let counters = read_network()?;
        let (rx_rate, tx_rate) = network.sample(counters, Instant::now())?;
        let uptime_seconds = parse_uptime(&read("/proc/uptime")?)?;
        let process_count = process_count()?;
        Ok(DynamicSample {
            cpu_usage,
            load_1,
            load_5,
            load_15,
            memory_total,
            memory_used,
            swap_total,
            swap_used,
            disk_total,
            disk_used,
            rx_bytes: safe_i64(counters.rx, "RX counter")?,
            tx_bytes: safe_i64(counters.tx, "TX counter")?,
            rx_rate,
            tx_rate,
            uptime_seconds,
            process_count,
        })
    }

    fn read(path: &str) -> Result<String, CollectorError> {
        fs::read_to_string(path)
            .map_err(|error| CollectorError::parse(format!("failed to read {path}: {error}")))
    }

    fn read_trimmed(path: &str) -> Result<String, CollectorError> {
        Ok(read(path)?.trim().to_owned())
    }

    fn read_optional(path: &str) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    fn architecture() -> Result<String, CollectorError> {
        #[cfg(target_arch = "x86_64")]
        {
            Ok("x86_64".to_owned())
        }
        #[cfg(target_arch = "aarch64")]
        {
            Ok("aarch64".to_owned())
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Err(CollectorError::parse("unsupported Linux architecture"))
        }
    }

    fn disk_usage() -> Result<(i64, i64), CollectorError> {
        let path = CString::new("/").expect("root path has no interior NUL");
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is a valid NUL-terminated string and `stats` points to writable storage.
        let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
        if result != 0 {
            return Err(CollectorError::parse(format!(
                "statvfs(/) failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: successful statvfs initialized the output structure.
        let stats = unsafe { stats.assume_init() };
        let blocks = stats.f_blocks;
        let free = stats.f_bfree;
        let fragment_size = stats.f_frsize;
        let total = blocks
            .checked_mul(fragment_size)
            .ok_or_else(|| CollectorError::parse("disk total overflow"))?;
        let used = blocks
            .checked_sub(free)
            .and_then(|value| value.checked_mul(fragment_size))
            .ok_or_else(|| CollectorError::parse("disk used overflow"))?;
        Ok((safe_i64(total, "disk total")?, safe_i64(used, "disk used")?))
    }

    fn process_count() -> Result<i64, CollectorError> {
        let mut count = 0_u64;
        for entry in fs::read_dir("/proc")
            .map_err(|error| CollectorError::parse(format!("failed to read /proc: {error}")))?
        {
            let entry = entry.map_err(|error| {
                CollectorError::parse(format!("failed to inspect /proc entry: {error}"))
            })?;
            if is_process_directory_name(&entry.file_name().to_string_lossy())
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                count = count
                    .checked_add(1)
                    .ok_or_else(|| CollectorError::parse("process count overflow"))?;
            }
        }
        safe_i64(count, "process count")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const FIXTURES: &str = "../tests/fixtures";

    fn fixture(name: &str) -> &'static str {
        match name {
            "os-release-debian" => include_str!("../tests/fixtures/os-release-debian"),
            "os-release-ubuntu" => include_str!("../tests/fixtures/os-release-ubuntu"),
            "os-release-alpine" => include_str!("../tests/fixtures/os-release-alpine"),
            "proc-stat-1" => include_str!("../tests/fixtures/proc-stat-1"),
            "proc-stat-2" => include_str!("../tests/fixtures/proc-stat-2"),
            "proc-meminfo" => include_str!("../tests/fixtures/proc-meminfo"),
            "proc-loadavg" => include_str!("../tests/fixtures/proc-loadavg"),
            "proc-net-dev" => include_str!("../tests/fixtures/proc-net-dev"),
            "proc-net-route" => include_str!("../tests/fixtures/proc-net-route"),
            "proc-net-ipv6-route" => include_str!("../tests/fixtures/proc-net-ipv6-route"),
            "proc-cpuinfo-x86" => include_str!("../tests/fixtures/proc-cpuinfo-x86"),
            "proc-cpuinfo-arm" => include_str!("../tests/fixtures/proc-cpuinfo-arm"),
            "boot-id" => include_str!("../tests/fixtures/boot-id"),
            "virtualization-cgroup" => include_str!("../tests/fixtures/virtualization-cgroup"),
            _ => panic!("unknown fixture in {FIXTURES}: {name}"),
        }
    }

    #[test]
    fn parses_debian_ubuntu_alpine_and_quoted_os_release_values() {
        assert_eq!(
            parse_os_release(fixture("os-release-debian")).expect("Debian os-release"),
            ("Debian GNU/Linux".to_owned(), "13".to_owned())
        );
        assert_eq!(
            parse_os_release(fixture("os-release-ubuntu")).expect("Ubuntu os-release"),
            ("Ubuntu".to_owned(), "24.04".to_owned())
        );
        assert_eq!(
            parse_os_release(fixture("os-release-alpine")).expect("Alpine os-release"),
            ("Alpine Linux".to_owned(), "3.21.3".to_owned())
        );
    }

    #[test]
    fn parses_x86_and_arm_cpu_models_and_cores() {
        assert_eq!(
            parse_cpu_info(fixture("proc-cpuinfo-x86")).expect("x86 cpuinfo"),
            ("AMD EPYC 9654".to_owned(), 2)
        );
        assert_eq!(
            parse_cpu_info(fixture("proc-cpuinfo-arm")).expect("ARM cpuinfo"),
            ("ARM 0x41 0xd0c".to_owned(), 2)
        );
    }

    #[test]
    fn cpu_first_normal_and_reset_samples_are_safe() {
        let first = parse_cpu_stat(fixture("proc-stat-1")).expect("first stat");
        let second = parse_cpu_stat(fixture("proc-stat-2")).expect("second stat");
        let mut tracker = CpuTracker::default();
        assert_eq!(tracker.sample(first), 0.0);
        assert!((tracker.sample(second) - 60.0).abs() < 0.001);
        assert_eq!(tracker.sample(first), 0.0);
    }

    #[test]
    fn explicit_cpu_and_network_baselines_feed_the_first_report() {
        let first = parse_cpu_stat(fixture("proc-stat-1")).expect("first stat");
        let second = parse_cpu_stat(fixture("proc-stat-2")).expect("second stat");
        let mut cpu = CpuTracker::default();
        cpu.baseline(first);
        assert!((cpu.sample(second) - 60.0).abs() < 0.001);

        let start = Instant::now();
        let mut network = NetworkTracker::default();
        network.baseline(NetworkCounters { rx: 10, tx: 20 }, start);
        assert_eq!(
            network
                .sample(
                    NetworkCounters { rx: 30, tx: 60 },
                    start + Duration::from_secs(2)
                )
                .expect("network after baseline"),
            (10, 20)
        );
    }

    #[test]
    fn parses_memory_available_fallback_swap_load_and_uptime() {
        assert_eq!(
            parse_meminfo(fixture("proc-meminfo")).expect("meminfo"),
            (1_024_000, 614_400, 204_800, 153_600)
        );
        let fallback = "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 250 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n";
        assert_eq!(
            parse_meminfo(fallback).expect("fallback meminfo"),
            (1_024_000, 614_400, 0, 0)
        );
        assert_eq!(
            parse_loadavg(fixture("proc-loadavg")).expect("loadavg"),
            (0.12, 0.34, 0.56)
        );
        assert_eq!(parse_uptime("123.99 456.0").expect("uptime"), 123);
    }

    #[test]
    fn process_directory_predicate_is_ascii_digits_only() {
        assert!(is_process_directory_name("1"));
        assert!(is_process_directory_name("12345"));
        assert!(!is_process_directory_name(""));
        assert!(!is_process_directory_name("self"));
        assert!(!is_process_directory_name("12a"));
        assert!(!is_process_directory_name("１２"));
    }

    #[test]
    fn selects_lowest_metric_ipv4_then_ipv6_default_route() {
        assert_eq!(
            parse_ipv4_default_route(fixture("proc-net-route")).as_deref(),
            Some("ens3")
        );
        assert_eq!(
            parse_ipv6_default_route(fixture("proc-net-ipv6-route")).as_deref(),
            Some("eth1")
        );
    }

    #[test]
    fn parses_selected_interface_and_fallback_non_loopback_sum() {
        let input = fixture("proc-net-dev");
        assert_eq!(
            parse_net_dev(input, Some("ens3")).expect("selected interface"),
            NetworkCounters { rx: 1000, tx: 2000 }
        );
        assert_eq!(
            parse_net_dev(input, None).expect("fallback aggregate"),
            NetworkCounters { rx: 4000, tx: 6000 }
        );
    }

    #[test]
    fn network_first_normal_and_directional_rollbacks_are_safe() {
        let start = Instant::now();
        let mut tracker = NetworkTracker::default();
        assert_eq!(
            tracker
                .sample(NetworkCounters { rx: 100, tx: 200 }, start)
                .expect("first network sample"),
            (0, 0)
        );
        assert_eq!(
            tracker
                .sample(
                    NetworkCounters { rx: 300, tx: 500 },
                    start + Duration::from_secs(2)
                )
                .expect("normal network sample"),
            (100, 150)
        );
        assert_eq!(
            tracker
                .sample(
                    NetworkCounters { rx: 10, tx: 700 },
                    start + Duration::from_secs(4)
                )
                .expect("RX rollback"),
            (0, 100)
        );
        assert_eq!(
            tracker
                .sample(
                    NetworkCounters { rx: 110, tx: 5 },
                    start + Duration::from_secs(6)
                )
                .expect("TX rollback"),
            (50, 0)
        );
    }

    #[test]
    fn rejects_safe_integer_overflow() {
        assert!(safe_i64(JS_SAFE_INTEGER_MAX + 1, "fixture").is_err());
        assert!(
            parse_meminfo(&format!(
                "MemTotal: {} kB\nMemAvailable: 0 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB",
                JS_SAFE_INTEGER_MAX
            ))
            .is_err()
        );
    }

    #[test]
    fn virtualization_heuristics_are_display_only_and_stable() {
        assert_eq!(detect_virtualization("KVM", "QEMU", "", ""), "kvm");
        assert_eq!(
            detect_virtualization("", "", "", fixture("virtualization-cgroup")),
            "docker"
        );
        assert_eq!(
            detect_virtualization("bare metal", "ACME", "", ""),
            "unknown"
        );
        assert_eq!(fixture("boot-id").trim().len(), 36);
    }

    #[test]
    fn generated_report_keeps_pings_empty() {
        let static_info = StaticInfo {
            hostname: "node".to_owned(),
            os_name: "Debian".to_owned(),
            os_version: "13".to_owned(),
            kernel: "6.12".to_owned(),
            architecture: "x86_64".to_owned(),
            virtualization: "kvm".to_owned(),
            cpu_model: "CPU".to_owned(),
            cpu_cores: 1,
            boot_id: "boot".to_owned(),
        };
        let report = build_report(
            &static_info,
            DynamicSample {
                cpu_usage: 0.0,
                load_1: 0.0,
                load_5: 0.0,
                load_15: 0.0,
                memory_total: 1,
                memory_used: 0,
                swap_total: 0,
                swap_used: 0,
                disk_total: 1,
                disk_used: 0,
                rx_bytes: 1,
                tx_bytes: 2,
                rx_rate: 0,
                tx_rate: 0,
                uptime_seconds: 1,
                process_count: 1,
            },
        );
        assert!(report.pings.is_empty());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_runtime_reports_the_supported_platform() {
        assert_eq!(
            SystemCollector::new()
                .expect_err("non-Linux must be unsupported")
                .to_string(),
            "monitor-agent supports Linux only"
        );
    }
}
