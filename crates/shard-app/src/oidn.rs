//! Intel Open Image Denoise (Apache 2.0, `third_party/oidn`): the neural denoiser run on the path-traced picture once
//! the view stops (see `pathtrace.rs`). The library is loaded at run time from the executable's folder, so the
//! program works without it (the option is then shown as not installed). It picks the best device itself: CUDA on
//! NVIDIA, HIP on AMD, SYCL on Intel GPUs, else the CPU.
//!
//! The work runs on a thread of its own: `Worker::submit` hands it the accumulation and the denoiser guides of the
//! path tracer, `Worker::poll` returns the denoised picture (half floats, ready to upload).

use std::ffi::{c_char, c_void, CStr};
use std::sync::mpsc;

type Handle = *mut c_void;

const DEVICE_TYPE_DEFAULT: i32 = 0;
const FORMAT_FLOAT3: i32 = 3;

struct Api {
    _lib: libloading::Library,
    new_device: unsafe extern "C" fn(i32) -> Handle,
    commit_device: unsafe extern "C" fn(Handle),
    release_device: unsafe extern "C" fn(Handle),
    device_error: unsafe extern "C" fn(Handle, *mut *const c_char) -> i32,
    device_int: unsafe extern "C" fn(Handle, *const c_char) -> i32,
    new_buffer: unsafe extern "C" fn(Handle, usize) -> Handle,
    release_buffer: unsafe extern "C" fn(Handle),
    write_buffer: unsafe extern "C" fn(Handle, usize, usize, *const c_void),
    read_buffer: unsafe extern "C" fn(Handle, usize, usize, *mut c_void),
    new_filter: unsafe extern "C" fn(Handle, *const c_char) -> Handle,
    release_filter: unsafe extern "C" fn(Handle),
    set_image: unsafe extern "C" fn(Handle, *const c_char, Handle, i32, usize, usize, usize, usize, usize),
    set_bool: unsafe extern "C" fn(Handle, *const c_char, bool),
    commit_filter: unsafe extern "C" fn(Handle),
    execute_filter: unsafe extern "C" fn(Handle),
}

fn lib_path() -> Option<std::path::PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join("OpenImageDenoise.dll");
    p.exists().then_some(p)
}

/// Whether the library is installed next to the program.
pub fn available() -> bool {
    lib_path().is_some()
}

impl Api {
    fn load() -> Result<Api, String> {
        let path = lib_path().ok_or("OpenImageDenoise.dll not found")?;
        unsafe {
            let lib = libloading::Library::new(&path).map_err(|e| e.to_string())?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| e.to_string())?
                };
            }
            Ok(Api {
                new_device: sym!("oidnNewDevice"),
                commit_device: sym!("oidnCommitDevice"),
                release_device: sym!("oidnReleaseDevice"),
                device_error: sym!("oidnGetDeviceError"),
                device_int: sym!("oidnGetDeviceInt"),
                new_buffer: sym!("oidnNewBuffer"),
                release_buffer: sym!("oidnReleaseBuffer"),
                write_buffer: sym!("oidnWriteBuffer"),
                read_buffer: sym!("oidnReadBuffer"),
                new_filter: sym!("oidnNewFilter"),
                release_filter: sym!("oidnReleaseFilter"),
                set_image: sym!("oidnSetFilterImage"),
                set_bool: sym!("oidnSetFilterBool"),
                commit_filter: sym!("oidnCommitFilter"),
                execute_filter: sym!("oidnExecuteFilter"),
                _lib: lib,
            })
        }
    }
}

/// A device with the "RT" filter set up for one picture size (colour, albedo, normal in; colour out).
struct Oidn {
    api: Api,
    device: Handle,
    filter: Handle,
    /// color, albedo, normal, output
    bufs: [Handle; 4],
    size: (u32, u32),
}

impl Oidn {
    fn new() -> Result<Oidn, String> {
        let api = Api::load()?;
        unsafe {
            let device = (api.new_device)(DEVICE_TYPE_DEFAULT);
            if device.is_null() {
                return Err("no Open Image Denoise device".into());
            }
            (api.commit_device)(device);
            let o = Oidn { api, device, filter: std::ptr::null_mut(), bufs: [std::ptr::null_mut(); 4], size: (0, 0) };
            o.check()?;
            let kind = ["default", "CPU", "SYCL", "CUDA", "HIP", "Metal"].get((o.api.device_int)(device, c"type".as_ptr()) as usize).copied().unwrap_or("?");
            eprintln!("[oidn] device: {kind}");
            Ok(o)
        }
    }

    fn check(&self) -> Result<(), String> {
        let mut msg: *const c_char = std::ptr::null();
        let code = unsafe { (self.api.device_error)(self.device, &mut msg) };
        if code == 0 {
            return Ok(());
        }
        let text = if msg.is_null() { String::new() } else { unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned() };
        Err(format!("Open Image Denoise error {code}: {text}"))
    }

    fn release_images(&mut self) {
        unsafe {
            if !self.filter.is_null() {
                (self.api.release_filter)(self.filter);
                self.filter = std::ptr::null_mut();
            }
            for b in &mut self.bufs {
                if !b.is_null() {
                    (self.api.release_buffer)(*b);
                    *b = std::ptr::null_mut();
                }
            }
        }
    }

    fn denoise(&mut self, size: (u32, u32), color: &[f32], albedo: &[f32], normal: &[f32]) -> Result<Vec<f32>, String> {
        let bytes = size.0 as usize * size.1 as usize * 12;
        unsafe {
            if self.size != size || self.filter.is_null() {
                self.release_images();
                for b in &mut self.bufs {
                    *b = (self.api.new_buffer)(self.device, bytes);
                }
                self.check()?;
                let f = (self.api.new_filter)(self.device, c"RT".as_ptr());
                let (w, h) = (size.0 as usize, size.1 as usize);
                for (name, b) in [(c"color", 0), (c"albedo", 1), (c"normal", 2), (c"output", 3)] {
                    (self.api.set_image)(f, name.as_ptr(), self.bufs[b], FORMAT_FLOAT3, w, h, 0, 0, 0);
                }
                (self.api.set_bool)(f, c"hdr".as_ptr(), true);
                // the guides come from one ray through the pixel centre: free of noise
                (self.api.set_bool)(f, c"cleanAux".as_ptr(), true);
                (self.api.commit_filter)(f);
                self.filter = f;
                self.size = size;
                self.check()?;
            }
            for (b, data) in [(0, color), (1, albedo), (2, normal)] {
                (self.api.write_buffer)(self.bufs[b], 0, bytes, data.as_ptr() as *const c_void);
            }
            (self.api.execute_filter)(self.filter);
            let mut out = vec![0f32; bytes / 4];
            (self.api.read_buffer)(self.bufs[3], 0, bytes, out.as_mut_ptr() as *mut c_void);
            self.check()?;
            Ok(out)
        }
    }
}

impl Drop for Oidn {
    fn drop(&mut self) {
        self.release_images();
        unsafe { (self.api.release_device)(self.device) };
    }
}

/// The path tracer's state at one sample count: `accum` (colour sum, coverage sum) and `feat` (guides, the layout
/// of `pathtrace.rs`), `samples` deep.
pub struct Job {
    pub key: u64,
    pub samples: u32,
    pub size: (u32, u32),
    pub accum: Vec<[f32; 4]>,
    pub feat: Vec<[u32; 4]>,
}

/// A denoised picture: linear RGB and coverage as half floats, row by row.
pub struct Done {
    pub key: u64,
    pub samples: u32,
    pub size: (u32, u32),
    pub rgba: Vec<half::f16>,
}

pub struct Worker {
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Result<Done, String>>,
}

impl Worker {
    /// The thread with the library loaded (None when it is not installed).
    pub fn spawn() -> Option<Worker> {
        if !available() {
            return None;
        }
        let (tx, jobs) = mpsc::channel::<Job>();
        let (done, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("oidn".into())
            .spawn(move || {
                let mut oidn = match Oidn::new() {
                    Ok(o) => o,
                    Err(e) => {
                        let _ = done.send(Err(e));
                        return;
                    }
                };
                while let Ok(mut job) = jobs.recv() {
                    // only the latest picture matters
                    while let Ok(j) = jobs.try_recv() {
                        job = j;
                    }
                    let t0 = std::time::Instant::now();
                    let r = run(&mut oidn, &job);
                    if let Ok(d) = &r {
                        if std::env::var_os("SHARD_TRACE").is_some() {
                            eprintln!("[oidn] {}x{} at {} samples in {:.0} ms", d.size.0, d.size.1, d.samples, t0.elapsed().as_secs_f32() * 1000.0);
                        }
                    }
                    if done.send(r).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Worker { tx, rx })
    }

    /// Queue a picture; false when the thread is gone (the library failed to start).
    pub fn submit(&self, job: Job) -> bool {
        self.tx.send(job).is_ok()
    }

    /// A finished picture or an error, if one is ready.
    pub fn poll(&self) -> Option<Result<Done, String>> {
        self.rx.try_recv().ok()
    }
}

fn oct_dec(v: u32) -> [f32; 3] {
    let x = ((v & 0xffff) as u16 as i16 as f32 / 32767.0).max(-1.0);
    let y = ((v >> 16) as u16 as i16 as f32 / 32767.0).max(-1.0);
    let z = 1.0 - x.abs() - y.abs();
    let (x, y) = if z < 0.0 { ((1.0 - y.abs()) * x.signum(), (1.0 - x.abs()) * y.signum()) } else { (x, y) };
    let l = (x * x + y * y + z * z).sqrt().max(1e-12);
    [x / l, y / l, z / l]
}

fn run(oidn: &mut Oidn, job: &Job) -> Result<Done, String> {
    let n = job.samples.max(1) as f32;
    let px = job.accum.len();
    let (mut color, mut albedo, mut normal) = (vec![0f32; px * 3], vec![0f32; px * 3], vec![0f32; px * 3]);
    for i in 0..px {
        let a = job.accum[i];
        let f = job.feat[i];
        let c = [a[0] / n, a[1] / n, a[2] / n];
        color[i * 3..i * 3 + 3].copy_from_slice(&c);
        if f[3] != 0 {
            let alb = f[2].to_le_bytes();
            albedo[i * 3..i * 3 + 3].copy_from_slice(&[alb[0] as f32 / 255.0, alb[1] as f32 / 255.0, alb[2] as f32 / 255.0]);
            normal[i * 3..i * 3 + 3].copy_from_slice(&oct_dec(f[0]));
        } else {
            // background: the environment itself is the albedo, no normal
            albedo[i * 3..i * 3 + 3].copy_from_slice(&c.map(|v| v.clamp(0.0, 1.0)));
        }
    }
    let out = oidn.denoise(job.size, &color, &albedo, &normal)?;
    let mut rgba = Vec::with_capacity(px * 4);
    for i in 0..px {
        let o = &out[i * 3..i * 3 + 3];
        rgba.extend([o[0], o[1], o[2], job.accum[i][3] / n].map(|v| half::f16::from_f32(v.max(0.0))));
    }
    Ok(Done { key: job.key, samples: job.samples, size: job.size, rgba })
}
