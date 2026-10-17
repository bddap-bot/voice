use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PERIOD: Duration = Duration::from_secs(1);
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
    root: PathBuf,
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
fn core_times(root: &Path) -> Vec<(u64, u64)> {
    let Some(stat) = read(&root.join("proc/stat")) else { return Vec::new() };
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
fn process(root: &Path, pid: u32) -> Option<(String, u64, u64)> {
    let stat = read(&root.join(format!("proc/{pid}/stat")))?;
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

fn pids(root: &Path) -> Vec<u32> {
    fs::read_dir(root.join("proc")).map(|entries| entries.filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default()
}

fn hottest(zones: &[PathBuf]) -> f32 {
    zones.iter().filter_map(|zone| number(zone)).fold(0.0, f32::max) / 1000.0
}

impl Device {
    pub fn new(root: &Path, now: Instant) -> Device {
        let cores = core_times(root)
            .into_iter()
            .enumerate()
            .map(|(at, times)| {
                let directory = root.join(format!("sys/devices/system/cpu/cpu{at}/cpufreq"));
                Core { most: number(&directory.join("cpuinfo_max_freq")).unwrap_or(0.0) * 1000.0, clock: directory.join("scaling_cur_freq"), times }
            })
            .collect();
        let (mut cpu_zones, mut gpu_zones) = (Vec::new(), Vec::new());
        for zone in fs::read_dir(root.join("sys/class/thermal")).into_iter().flatten().flatten() {
            let kind = read(&zone.path().join("type")).unwrap_or_default();
            if kind.starts_with("cpu") {
                cpu_zones.push(zone.path().join("temp"));
            } else if kind.starts_with("gpu") {
                gpu_zones.push(zone.path().join("temp"));
            }
        }
        let gpu_clock = fs::read_dir(root.join("sys/class/devfreq")).into_iter().flatten().flatten().filter(|entry| entry.file_name().to_string_lossy().contains("gpu")).map(|entry| entry.path()).next().map(|path| {
            let most = number(&path.join("max_freq")).unwrap_or(0.0);
            (path.join("cur_freq"), most)
        });
        let page = read(&root.join("proc/self/smaps")).and_then(|smaps| smaps.lines().find_map(|line| line.strip_prefix("KernelPageSize:")?.split_whitespace().next()?.parse::<f32>().ok())).unwrap_or(4.0) * 1024.0;
        let mut device = Device { root: root.to_owned(), cores, processes: HashMap::new(), clients: Vec::new(), scanned: None, sampled: now, cpu_zones, gpu_zones, gpu_clock, page };
        device.measure(now);
        device
    }

    /// Every process's DRM descriptors, one per client; the descriptors of other users' processes are unreadable and skipped.
    fn scan(&mut self) {
        let mut seen = HashSet::new();
        let previous: HashMap<PathBuf, u64> = self.clients.drain(..).map(|client| (client.fdinfo, client.busy)).collect();
        for pid in pids(&self.root) {
            let Ok(fds) = fs::read_dir(self.root.join(format!("proc/{pid}/fd"))) else { continue };
            for fd in fds.flatten() {
                if !fs::read_link(fd.path()).is_ok_and(|target| target.starts_with("/dev/dri")) {
                    continue;
                }
                let fdinfo = self.root.join(format!("proc/{pid}/fdinfo/{}", fd.file_name().to_string_lossy()));
                let Some((id, busy, _)) = drm(&fdinfo) else { continue };
                if seen.insert(id) {
                    let busy = previous.get(&fdinfo).copied().unwrap_or(busy);
                    self.clients.push(Client { pid, fdinfo, busy });
                }
            }
        }
    }

    /// The readings since the last sample, laid out as OBJECTS.md says, once a `PERIOD` has passed since it.
    pub fn sample(&mut self, now: Instant) -> Option<Vec<u8>> {
        (now.duration_since(self.sampled) >= PERIOD).then(|| self.measure(now))
    }

    fn measure(&mut self, now: Instant) -> Vec<u8> {
        let elapsed = now.duration_since(self.sampled).as_secs_f32().max(1e-3);
        self.sampled = now;
        if self.scanned.is_none_or(|scanned| now.duration_since(scanned) >= RESCAN) {
            self.scan();
            self.scanned = Some(now);
        }

        let meminfo = read(&self.root.join("proc/meminfo")).unwrap_or_default();
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
        for pid in pids(&self.root) {
            let Some((name, spent, pages)) = process(&self.root, pid) else { continue };
            ticks.insert(pid, spent);
            let Some(&before) = self.processes.get(&pid) else { continue };
            processes.push((name, spent.saturating_sub(before) as f32 / TICKS / elapsed, pages as f32 * self.page, fraction(gpu_by_pid.get(&pid).copied().unwrap_or(0))));
        }
        self.processes = ticks;
        processes.sort_by(|a, b| (b.1 + b.3).total_cmp(&(a.1 + a.3)));
        processes.truncate(PROCESSES);

        let times = core_times(&self.root);
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

    fn put(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn stat(name: &str, ticks: u64, pages: u64) -> String {
        let mut fields = vec![0; 22];
        (fields[11], fields[21]) = (ticks, pages);
        format!("1 ({name}) S {}", fields[1..].iter().map(u64::to_string).collect::<Vec<_>>().join(" "))
    }

    /// Two cores, a busy process, a GPU client and one born between the samples, at `seconds` into the fixture's life.
    fn machine(root: &Path, seconds: u64) {
        put(root, "proc/stat", &format!("cpu  0 0 0 0 0 0 0 0\ncpu0 {} 0 100 {} 0 0 0 0\ncpu1 100 0 100 {} 0 0 0 0\nintr 0\n", 100 + 50 * seconds, 700 + 50 * seconds, 700 + 150 * seconds));
        put(root, "proc/meminfo", "MemTotal:       16000000 kB\nMemFree:  1000 kB\nMemAvailable:    4000000 kB\n");
        put(root, "proc/10/stat", &stat("a (busy) proc", 1000 + 150 * seconds, 300));
        put(root, "proc/20/stat", &stat("gpu-client", 20 * seconds, 100));
        put(root, "proc/20/fdinfo/3", &format!("pos:\t0\ndrm-client-id:\t7\ndrm-engine-gfx:\t{} ns\ndrm-engine-capacity-gfx:\t9\ndrm-total-vram:\t2 MiB\n", 500_000_000 * seconds));
        if seconds > 0 {
            put(root, "proc/30/stat", &stat("newborn", 10_000, 1));
        }
    }

    #[test]
    fn a_sample_reads_cores_memory_gpu_and_busiest_processes_since_the_last() {
        let root = std::env::temp_dir().join(format!("voice-vr-device-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        machine(&root, 0);
        fs::create_dir_all(root.join("proc/20/fd")).unwrap();
        std::os::unix::fs::symlink("/dev/dri/renderD128", root.join("proc/20/fd/3")).unwrap();
        put(&root, "sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", "3000000\n");
        put(&root, "sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "1500000\n");
        put(&root, "sys/class/thermal/thermal_zone0/type", "cpu-thermal\n");
        put(&root, "sys/class/thermal/thermal_zone0/temp", "45000\n");
        put(&root, "sys/class/thermal/thermal_zone1/type", "gpu-thermal\n");
        put(&root, "sys/class/thermal/thermal_zone1/temp", "50000\n");
        put(&root, "sys/class/devfreq/fb000000.gpu/cur_freq", "400000000\n");
        put(&root, "sys/class/devfreq/fb000000.gpu/max_freq", "800000000\n");
        let born = Instant::now();
        let mut device = Device::new(&root, born);
        machine(&root, 1);
        assert!(device.sample(born + PERIOD / 2).is_none(), "nothing until a period has passed");
        let bytes = device.sample(born + PERIOD).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!((word(&bytes, 0), word(&bytes, 4)), (2, 2), "two cores; the process born since the last sample has nothing to compare yet");
        assert_eq!(bytes.len(), 8 + 8 * 4 + 2 * 12 + 2 * (12 + NAME));
        let floats = |at: usize, count: usize| (0..count).map(|index| float(&bytes, at + 4 * index)).collect::<Vec<_>>();
        assert_eq!(floats(8, 8), [16000000.0 * 1024.0, 4000000.0 * 1024.0, 0.5, 2.0 * 1024.0 * 1024.0, 400000000.0, 800000000.0, 45.0, 50.0]);
        assert_eq!(floats(40, 6), [0.5, 1.5e9, 3e9, 0.0, 0.0, 0.0], "per core: busy fraction, clock and top clock; the second core has no cpufreq");
        let at = 40 + 2 * 12;
        let name = |process: usize| String::from_utf8_lossy(&bytes[at + process * 28 + 12..at + process * 28 + 28]).trim_end_matches('\0').to_owned();
        assert_eq!((name(0), floats(at, 3)), ("a (busy) proc".to_owned(), vec![1.5, 300.0 * 4096.0, 0.0]));
        assert_eq!((name(1), floats(at + 28, 3)), ("gpu-client".to_owned(), vec![0.2, 100.0 * 4096.0, 0.5]));
    }
}
