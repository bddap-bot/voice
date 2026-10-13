use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const PERIOD: Duration = Duration::from_secs(1);
/// Busiest processes reported.
const PROCESSES: usize = 8;
const NAME: usize = 16;
/// Reading every process's descriptors costs more than the rest of a sample, and GPU clients come and go rarely.
const RESCAN: Duration = Duration::from_secs(10);
/// USER_HZ, the unit of /proc times, is 100 on every Linux the overlay runs on.
const TICKS: f32 = 100.0;

struct Core {
    clock: PathBuf,
    most: f32,
    times: (u64, u64),
}

/// One open GPU client, read through one of its descriptors.
struct Client {
    pid: u32,
    fdinfo: PathBuf,
    busy: u64,
}

/// The device's own resource readings, the live data an object asks for with `"feed": "device"` (OBJECTS.md).
pub struct Device {
    cores: Vec<Core>,
    processes: HashMap<u32, u64>,
    clients: Vec<Client>,
    scanned: Option<Instant>,
    sampled: Instant,
    cpu_zones: Vec<PathBuf>,
    gpu_zones: Vec<PathBuf>,
    gpu_clock: Option<(PathBuf, f32)>,
    page: f32,
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn number(path: &Path) -> Option<f32> {
    read(path)?.trim().parse().ok()
}

/// Busy and total ticks of every `cpuN` line, in order.
fn core_times() -> Vec<(u64, u64)> {
    let Some(stat) = read(Path::new("/proc/stat")) else { return Vec::new() };
    stat.lines()
        .filter(|line| line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .map(|line| {
            let fields: Vec<u64> = line.split_whitespace().skip(1).filter_map(|field| field.parse().ok()).collect();
            let idle = fields.get(3).copied().unwrap_or(0) + fields.get(4).copied().unwrap_or(0);
            let total: u64 = fields.iter().take(8).sum();
            (total - idle, total)
        })
        .collect()
}

/// Name, CPU ticks and resident pages from `/proc/<pid>/stat`; the name may hold spaces and parentheses, so it runs to the last `)`.
fn process(pid: u32) -> Option<(String, u64, u64)> {
    let stat = read(&PathBuf::from(format!("/proc/{pid}/stat")))?;
    let (head, tail) = stat.rsplit_once(')')?;
    let name = head.split_once('(')?.1.to_owned();
    let fields: Vec<&str> = tail.split_whitespace().collect();
    let field = |at: usize| fields.get(at).and_then(|field| field.parse::<u64>().ok());
    Some((name, field(11)? + field(12)?, field(21)?))
}

/// The client id, GPU nanoseconds summed over engines and memory in bytes of one DRM descriptor's fdinfo.
fn drm(fdinfo: &Path) -> Option<(u64, u64, u64)> {
    let text = read(fdinfo)?;
    let (mut id, mut busy, mut memory) = (None, 0, 0);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let mut words = value.split_whitespace();
        let amount = words.next().and_then(|word| word.parse::<u64>().ok()).unwrap_or(0);
        match key {
            "drm-client-id" => id = Some(amount),
            key if key.starts_with("drm-engine-") && !key.starts_with("drm-engine-capacity") => busy += amount,
            key if key.starts_with("drm-total-") => {
                memory += amount * match words.next() {
                    Some("KiB") => 1 << 10,
                    Some("MiB") => 1 << 20,
                    Some("GiB") => 1 << 30,
                    _ => 1,
                }
            }
            _ => {}
        }
    }
    Some((id?, busy, memory))
}

fn pids() -> Vec<u32> {
    fs::read_dir("/proc").map(|entries| entries.filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default()
}

fn hottest(zones: &[PathBuf]) -> f32 {
    zones.iter().filter_map(|zone| number(zone)).fold(0.0, f32::max) / 1000.0
}

impl Device {
    pub fn new() -> Device {
        let cores = core_times()
            .into_iter()
            .enumerate()
            .map(|(at, times)| {
                let directory = PathBuf::from(format!("/sys/devices/system/cpu/cpu{at}/cpufreq"));
                Core { most: number(&directory.join("cpuinfo_max_freq")).unwrap_or(0.0) * 1000.0, clock: directory.join("scaling_cur_freq"), times }
            })
            .collect();
        let (mut cpu_zones, mut gpu_zones) = (Vec::new(), Vec::new());
        for zone in fs::read_dir("/sys/class/thermal").into_iter().flatten().flatten() {
            let kind = read(&zone.path().join("type")).unwrap_or_default();
            if kind.starts_with("cpu") {
                cpu_zones.push(zone.path().join("temp"));
            } else if kind.starts_with("gpu") {
                gpu_zones.push(zone.path().join("temp"));
            }
        }
        let gpu_clock = fs::read_dir("/sys/class/devfreq").into_iter().flatten().flatten().map(|entry| entry.path()).find(|path| path.to_string_lossy().contains("gpu")).map(|path| {
            let most = number(&path.join("max_freq")).unwrap_or(0.0);
            (path.join("cur_freq"), most)
        });
        let page = read(Path::new("/proc/self/smaps")).and_then(|smaps| smaps.lines().find_map(|line| line.strip_prefix("KernelPageSize:")?.split_whitespace().next()?.parse::<f32>().ok())).unwrap_or(4.0) * 1024.0;
        let mut device = Device { cores, processes: HashMap::new(), clients: Vec::new(), scanned: None, sampled: Instant::now(), cpu_zones, gpu_zones, gpu_clock, page };
        device.sample();
        device
    }

    /// Every process's DRM descriptors, one per client; the descriptors of other users' processes are unreadable and skipped.
    fn scan(&mut self) {
        let mut seen = HashSet::new();
        let previous: HashMap<PathBuf, u64> = self.clients.drain(..).map(|client| (client.fdinfo, client.busy)).collect();
        for pid in pids() {
            let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
            for fd in fds.flatten() {
                if !fs::read_link(fd.path()).is_ok_and(|target| target.starts_with("/dev/dri")) {
                    continue;
                }
                let fdinfo = PathBuf::from(format!("/proc/{pid}/fdinfo/{}", fd.file_name().to_string_lossy()));
                let Some((id, busy, _)) = drm(&fdinfo) else { continue };
                if seen.insert(id) {
                    let busy = previous.get(&fdinfo).copied().unwrap_or(busy);
                    self.clients.push(Client { pid, fdinfo, busy });
                }
            }
        }
    }

    /// The readings since the last sample, laid out as OBJECTS.md says.
    pub fn sample(&mut self) -> Vec<u8> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled).as_secs_f32().max(1e-3);
        self.sampled = now;
        if self.scanned.is_none_or(|scanned| now.duration_since(scanned) >= RESCAN) {
            self.scan();
            self.scanned = Some(now);
        }

        let meminfo = read(Path::new("/proc/meminfo")).unwrap_or_default();
        let kib = |key: &str| meminfo.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(':')?.split_whitespace().next()?.parse::<f32>().ok()).unwrap_or(0.0) * 1024.0;

        let mut gpu_by_pid: HashMap<u32, u64> = HashMap::new();
        let (mut gpu_busy, mut gpu_memory) = (0, 0);
        self.clients.retain_mut(|client| {
            let Some((_, busy, memory)) = drm(&client.fdinfo) else { return false };
            let spent = busy.saturating_sub(client.busy);
            client.busy = busy;
            *gpu_by_pid.entry(client.pid).or_default() += spent;
            gpu_busy += spent;
            gpu_memory += memory;
            true
        });
        let fraction = |nanoseconds: u64| (nanoseconds as f32 / 1e9 / elapsed).clamp(0.0, 1.0);

        let mut processes: Vec<(String, f32, f32, f32)> = Vec::new();
        let mut ticks = HashMap::with_capacity(self.processes.len());
        for pid in pids() {
            let Some((name, spent, pages)) = process(pid) else { continue };
            ticks.insert(pid, spent);
            let Some(&before) = self.processes.get(&pid) else { continue };
            processes.push((name, spent.saturating_sub(before) as f32 / TICKS / elapsed, pages as f32 * self.page, fraction(gpu_by_pid.get(&pid).copied().unwrap_or(0))));
        }
        self.processes = ticks;
        processes.sort_by(|a, b| (b.1 + b.3).total_cmp(&(a.1 + a.3)));
        processes.truncate(PROCESSES);

        let times = core_times();
        let header = [self.cores.len() as u32, processes.len() as u32];
        let (gpu_clock, gpu_most) = self.gpu_clock.as_ref().map_or((0.0, 0.0), |(clock, most)| (number(clock).unwrap_or(0.0), *most));
        let mut out = Vec::with_capacity(512);
        for word in header {
            out.extend_from_slice(&word.to_le_bytes());
        }
        let put_all = |values: &[f32], out: &mut Vec<u8>| values.iter().for_each(|value| out.extend_from_slice(&value.to_le_bytes()));
        put_all(&[kib("MemTotal"), kib("MemAvailable"), fraction(gpu_busy), gpu_memory as f32, gpu_clock, gpu_most, hottest(&self.cpu_zones), hottest(&self.gpu_zones)], &mut out);
        for (core, now) in self.cores.iter_mut().zip(times.iter().copied().chain(std::iter::repeat((0, 0)))) {
            let (busy, total) = (now.0.saturating_sub(core.times.0), now.1.saturating_sub(core.times.1));
            core.times = now;
            let busy = if total == 0 { 0.0 } else { busy as f32 / total as f32 };
            put_all(&[busy, number(&core.clock).unwrap_or(0.0) * 1000.0, core.most], &mut out);
        }
        for (name, cpu, resident, gpu) in processes {
            put_all(&[cpu, resident, gpu], &mut out);
            let mut field = [0u8; NAME];
            let bytes = name.as_bytes();
            let length = bytes.len().min(NAME);
            field[..length].copy_from_slice(&bytes[..length]);
            out.extend_from_slice(&field);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn float(bytes: &[u8], at: usize) -> f32 {
        f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_sample_reads_this_machine_s_cores_memory_and_busiest_processes() {
        let mut device = Device::new();
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut spin = 0u64;
        while Instant::now() < deadline {
            spin = std::hint::black_box(spin.wrapping_add(1));
        }
        let bytes = device.sample();
        let (cores, processes) = (word(&bytes, 0) as usize, word(&bytes, 4) as usize);
        assert!(cores >= 1);
        assert!((1..=PROCESSES).contains(&processes), "{processes} processes");
        assert_eq!(bytes.len(), 8 + 8 * 4 + cores * 12 + processes * (12 + NAME));
        let (total, available) = (float(&bytes, 8), float(&bytes, 12));
        assert!(total > 1e8 && available > 0.0 && available <= total, "memory {available} of {total}");
        let busiest: f32 = (0..cores).map(|core| float(&bytes, 40 + core * 12)).fold(0.0, f32::max);
        assert!(busiest > 0.2, "one core spun for the whole sample: {busiest}");
        let at = 40 + cores * 12;
        let names: Vec<String> = (0..processes).map(|process| String::from_utf8_lossy(&bytes[at + process * 28 + 12..at + process * 28 + 28]).trim_end_matches('\0').to_owned()).collect();
        let own = std::fs::read_to_string("/proc/self/comm").unwrap().trim().to_owned();
        assert!(names.iter().any(|name| own.starts_with(name.as_str())), "this spinning process is among the busiest: {names:?}");
        assert!(float(&bytes, at) > 0.2, "and leads them: {}", float(&bytes, at));
    }
}
