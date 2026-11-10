//! Viewport statistics (Settings > Viewport > Statistics): frame time and FPS measured by the app, the GPU time of
//! the viewport's passes (timestamp queries, in the renderer), and the CPU / GPU usage of the ShardStroke process, sampled
//! once a second on a background thread (Windows: process times, and the "GPU Engine" performance counters Task
//! Manager reads -- the 3D and compute engines of this process).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Usage of the process, in percent: CPU over all cores, GPU of its busiest engine type (3D or compute).
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    /// Dedicated video memory used by this process, bytes.
    pub vram: Option<u64>,
}

/// How often the numbers on screen change (a per-frame value changes too fast to read).
const REFRESH: Duration = Duration::from_millis(500);

/// The numbers on screen: averages over the last `REFRESH`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shown {
    pub fps: usize,
    /// Interval between frames, ms.
    pub frame_ms: Option<f32>,
    /// CPU time of a frame (draw + submit), ms.
    pub cpu_ms: Option<f32>,
    /// GPU time of the viewport's passes, ms.
    pub gpu_ms: Option<f32>,
}

/// What the overlay shows.
pub struct Stats {
    last: Instant,
    /// Frame intervals of the last second.
    frames: Vec<(Instant, f32)>,
    /// Sums and counts of the CPU and GPU frame times since the numbers on screen were refreshed.
    cpu_acc: (f32, u32),
    gpu_acc: (f32, u32),
    shown: Shown,
    shown_at: Instant,
    usage: Arc<Mutex<Usage>>,
    sampler: Option<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            last: Instant::now(),
            frames: Vec::new(),
            cpu_acc: (0.0, 0),
            gpu_acc: (0.0, 0),
            shown: Shown::default(),
            shown_at: Instant::now(),
            usage: Arc::default(),
            sampler: None,
            stop: Arc::default(),
        }
    }
}

impl Stats {
    /// A frame was presented; `work` is the CPU time it took, `gpu_ms` the GPU time of the last measured one.
    pub fn frame(&mut self, work: Duration, gpu_ms: Option<f32>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32() * 1000.0;
        self.last = now;
        self.cpu_acc.0 += work.as_secs_f32() * 1000.0;
        self.cpu_acc.1 += 1;
        if let Some(g) = gpu_ms {
            self.gpu_acc.0 += g;
            self.gpu_acc.1 += 1;
        }
        self.frames.push((now, dt));
        self.frames.retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(1));
        if now.duration_since(self.shown_at) >= REFRESH {
            let avg = |(sum, n): (f32, u32)| (n > 0).then(|| sum / n as f32);
            self.shown = Shown {
                fps: self.frames.len(),
                frame_ms: (self.frames.len() > 1)
                    .then(|| self.frames.iter().skip(1).map(|f| f.1).sum::<f32>() / (self.frames.len() - 1) as f32),
                cpu_ms: avg(self.cpu_acc),
                gpu_ms: avg(self.gpu_acc).or(self.shown.gpu_ms),
            };
            self.cpu_acc = (0.0, 0);
            self.gpu_acc = (0.0, 0);
            self.shown_at = now;
        }
    }

    /// The numbers to show (refreshed every `REFRESH`).
    pub fn shown(&self) -> Shown {
        self.shown
    }

    pub fn usage(&self) -> Usage {
        self.usage.lock().map(|u| *u).unwrap_or_default()
    }

    /// Start or stop the usage sampler.
    pub fn set_sampling(&mut self, on: bool) {
        if on && self.sampler.is_none() {
            self.stop.store(false, std::sync::atomic::Ordering::Relaxed);
            let (usage, stop) = (self.usage.clone(), self.stop.clone());
            self.sampler = Some(std::thread::spawn(move || sample_loop(usage, stop)));
        } else if !on {
            if let Some(h) = self.sampler.take() {
                self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                drop(h);
            }
        }
    }
}

#[cfg(windows)]
fn sample_loop(usage: Arc<Mutex<Usage>>, stop: Arc<std::sync::atomic::AtomicBool>) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Performance::*;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessTimes};

    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let ft = |f: &FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 * 100e-9;
    let process_seconds = || -> Option<f64> {
        let mut t = [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 4];
        // SAFETY: valid out pointers for the four times of the current process
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut t[0], &mut t[1], &mut t[2], &mut t[3]) };
        (ok != 0).then(|| ft(&t[2]) + ft(&t[3]))
    };
    // GPU: every "GPU Engine" instance; this process's are named pid_<pid>_..._engtype_<kind>
    let pid = unsafe { GetCurrentProcessId() };
    let prefix = format!("pid_{pid}_");
    let mut query: isize = 0;
    let mut counter: isize = 0;
    let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
    // SAFETY: PDH handles are owned by this thread and closed below
    let gpu_ok = unsafe {
        PdhOpenQueryW(std::ptr::null(), 0, &mut query) == 0 && PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) == 0
    };
    // video memory: "GPU Process Memory" instances are named pid_<pid>_luid_..._phys_<n>
    let mut mem_counter: isize = 0;
    let mem_path: Vec<u16> = "\\GPU Process Memory(*)\\Dedicated Usage\0".encode_utf16().collect();
    let mem_ok = gpu_ok && unsafe { PdhAddEnglishCounterW(query, mem_path.as_ptr(), 0, &mut mem_counter) == 0 };
    if gpu_ok {
        unsafe { PdhCollectQueryData(query) };
    }
    let mut last = (Instant::now(), process_seconds());
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(1000));
        let now = (Instant::now(), process_seconds());
        let cpu = match (last.1, now.1) {
            (Some(a), Some(b)) => Some(((b - a) / now.0.duration_since(last.0).as_secs_f64() / cores * 100.0) as f32),
            _ => None,
        };
        last = now;
        let gpu = if gpu_ok {
            // SAFETY: two-call pattern of PdhGetFormattedCounterArrayW with a buffer of the size it asks for
            unsafe {
                PdhCollectQueryData(query);
                let (mut size, mut count) = (0u32, 0u32);
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
                let mut buf = vec![0u8; size as usize];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if size > 0 && PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, items) == 0 {
                    let mut by_kind: std::collections::HashMap<String, f64> = Default::default();
                    for i in 0..count as usize {
                        let it = &*items.add(i);
                        let mut len = 0;
                        while *it.szName.add(len) != 0 {
                            len += 1;
                        }
                        let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, len));
                        if !name.starts_with(&prefix) {
                            continue;
                        }
                        let kind = name.rsplit("engtype_").next().unwrap_or("").to_string();
                        *by_kind.entry(kind).or_default() += it.FmtValue.Anonymous.doubleValue;
                    }
                    Some(by_kind.values().cloned().fold(0.0, f64::max).min(100.0) as f32)
                } else {
                    None
                }
            }
        } else {
            None
        };
        let vram = if mem_ok {
            // SAFETY: same two-call pattern; the query was collected above for both counters
            unsafe {
                let (mut size, mut count) = (0u32, 0u32);
                PdhGetFormattedCounterArrayW(mem_counter, PDH_FMT_LARGE, &mut size, &mut count, std::ptr::null_mut());
                let mut buf = vec![0u8; size as usize];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if size > 0 && PdhGetFormattedCounterArrayW(mem_counter, PDH_FMT_LARGE, &mut size, &mut count, items) == 0 {
                    let mut total = 0u64;
                    for i in 0..count as usize {
                        let it = &*items.add(i);
                        let mut len = 0;
                        while *it.szName.add(len) != 0 {
                            len += 1;
                        }
                        let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, len));
                        if name.starts_with(&prefix) {
                            total += it.FmtValue.Anonymous.largeValue.max(0) as u64;
                        }
                    }
                    Some(total)
                } else {
                    None
                }
            }
        } else {
            None
        };
        if let Ok(mut u) = usage.lock() {
            *u = Usage { cpu, gpu, vram };
        }
    }
    if gpu_ok {
        unsafe { PdhCloseQuery(query) };
    }
}

#[cfg(not(windows))]
fn sample_loop(_usage: Arc<Mutex<Usage>>, _stop: Arc<std::sync::atomic::AtomicBool>) {}
