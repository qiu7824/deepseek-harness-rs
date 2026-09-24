//! Opt-in allocation accounting for isolated diagnostic builds. No block
//! contents, prompts, addresses or credentials are recorded. This is requested
//! Rust heap size, not physical memory; Windows process counters remain the gate.
use std::alloc::{GlobalAlloc, Layout};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const BINS: usize = 32;
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static COUNTS: [AtomicU64; BINS] = [const { AtomicU64::new(0) }; BINS];
static BYTES: [AtomicU64; BINS] = [const { AtomicU64::new(0) }; BINS];
static REPORT: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

fn bucket(size: usize) -> usize {
    (usize::BITS - size.max(1).leading_zeros()).min((BINS - 1) as u32) as usize
}
fn allocated(size: usize) {
    let size = size as u64;
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
    let index = bucket(size as usize);
    COUNTS[index].fetch_add(1, Ordering::Relaxed);
    BYTES[index].fetch_add(size, Ordering::Relaxed);
}
fn released(size: usize) {
    LIVE.fetch_sub(size as u64, Ordering::Relaxed);
    let index = bucket(size);
    COUNTS[index].fetch_sub(1, Ordering::Relaxed);
    BYTES[index].fetch_sub(size as u64, Ordering::Relaxed);
}
pub struct MeasuredAllocator;
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        released(layout.size());
        unsafe { mimalloc::MiMalloc.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let resized = unsafe { mimalloc::MiMalloc.realloc(pointer, layout, size) };
        if !resized.is_null() {
            released(layout.size());
            allocated(size);
        }
        resized
    }
}
pub fn initialize() {
    REPORT.get_or_init(|| {
        std::env::var_os("DSH_ALLOCATION_REPORT").map(|path| {
            // Refuse replacing previous evidence, including from a codec child.
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .expect("allocation diagnostic output must be a new, writable file");
            Mutex::new(file)
        })
    });
    // Private helpers inherit the environment but must never overwrite the
    // parent report. Remove it before any subprocess can be created.
    unsafe {
        std::env::remove_var("DSH_ALLOCATION_REPORT");
    }
}
pub fn sample() {
    let Some(Some(output)) = REPORT.get() else {
        return;
    };
    let live = LIVE.load(Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed);
    let counts = std::array::from_fn::<_, BINS, _>(|i| COUNTS[i].load(Ordering::Relaxed));
    let bytes = std::array::from_fn::<_, BINS, _>(|i| BYTES[i].load(Ordering::Relaxed));
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Samples are approximate across threads; no global allocator lock is held.
    let row = serde_json::json!({"timeMs":time,"pid":std::process::id(),"liveRequestedBytes":live,"peakRequestedBytes":peak,"sizeClassUpperExclusiveCounts":counts,"sizeClassRequestedBytes":bytes});
    if let Ok(mut output) = output.lock() {
        let _ = writeln!(output, "{row}");
        let _ = output.flush();
    }
}
