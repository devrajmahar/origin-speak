//! Lightweight hardware detection for CLI setup/model recommendation.
//!
//! Detection uses Rust/std plus direct OS APIs and never launches a shell or
//! helper process. Accelerator probing is intentionally explicit about unknown
//! state until a native DXGI/Metal/Vulkan capability probe is integrated.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemCapabilities {
    pub os: &'static str,
    pub arch: &'static str,
    pub logical_cpus: usize,
    pub memory_bytes: Option<u64>,
    pub accelerator: AcceleratorCapability,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceleratorCapability {
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRecommendation {
    pub model: &'static str,
    pub rationale: String,
    pub conservative: bool,
}

impl SystemCapabilities {
    pub fn detect() -> Self {
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            logical_cpus: std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1),
            memory_bytes: total_memory_bytes(),
            accelerator: AcceleratorCapability::Unknown {
                reason: "native GPU/accelerator probing is not integrated in the CLI manager yet"
                    .to_string(),
            },
        }
    }

    /// Pick a latency-oriented local Whisper model using only capabilities we
    /// can verify without starting the runtime. Unknown RAM/GPU state chooses a
    /// conservative model rather than assuming hardware exists.
    pub fn recommend_model(&self) -> ModelRecommendation {
        let gib = self.memory_bytes.map(|bytes| bytes / (1024 * 1024 * 1024));
        let cpus = self.logical_cpus.max(1);

        let (model, rationale, conservative) = match gib {
            Some(memory) if memory < 4 || cpus <= 2 => (
                "tiny.en",
                format!(
                    "{memory} GiB RAM and {cpus} logical CPU(s): prioritize low memory use and startup latency"
                ),
                false,
            ),
            Some(memory) if memory < 8 || cpus <= 4 => (
                "base.en",
                format!(
                    "{memory} GiB RAM and {cpus} logical CPU(s): balanced local transcription footprint"
                ),
                false,
            ),
            Some(memory) if memory < 16 || cpus <= 6 => (
                "small.en",
                format!(
                    "{memory} GiB RAM and {cpus} logical CPU(s): enough headroom for improved accuracy without a large-model startup cost"
                ),
                false,
            ),
            Some(memory) => (
                "small.en",
                format!(
                    "{memory} GiB RAM and {cpus} logical CPU(s): small.en remains the latency-first default until accelerator capability is verified"
                ),
                matches!(self.accelerator, AcceleratorCapability::Unknown { .. }),
            ),
            None => (
                "base.en",
                format!(
                    "RAM capacity unavailable; {cpus} logical CPU(s) detected, so setup uses a conservative balanced model"
                ),
                true,
            ),
        };

        ModelRecommendation {
            model,
            rationale,
            conservative,
        }
    }

    pub fn memory_gib(&self) -> Option<u64> {
        self.memory_bytes.map(|bytes| bytes / (1024 * 1024 * 1024))
    }
}

impl fmt::Display for AcceleratorCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { reason } => write!(f, "unknown ({reason})"),
        }
    }
}

#[cfg(target_os = "windows")]
fn total_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0 && status.total_phys > 0).then_some(status.total_phys)
}

#[cfg(target_os = "macos")]
fn total_memory_bytes() -> Option<u64> {
    use std::ffi::{c_char, c_void};

    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> i32;
    }

    let name = b"hw.memsize\0";
    let mut value = 0_u64;
    let mut len = std::mem::size_of::<u64>();
    let result = unsafe {
        sysctlbyname(
            name.as_ptr().cast(),
            (&mut value as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && len == std::mem::size_of::<u64>() && value > 0).then_some(value)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn total_memory_bytes() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?;
    let kib = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    kib.checked_mul(1024)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn total_memory_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(cpus: usize, gib: Option<u64>) -> SystemCapabilities {
        SystemCapabilities {
            os: "test",
            arch: "test",
            logical_cpus: cpus,
            memory_bytes: gib.map(|value| value * 1024 * 1024 * 1024),
            accelerator: AcceleratorCapability::Unknown {
                reason: "test".to_string(),
            },
        }
    }

    #[test]
    fn low_memory_prefers_tiny() {
        assert_eq!(caps(8, Some(3)).recommend_model().model, "tiny.en");
    }

    #[test]
    fn midrange_prefers_base_or_small_by_verified_resources() {
        assert_eq!(caps(4, Some(8)).recommend_model().model, "base.en");
        assert_eq!(caps(8, Some(12)).recommend_model().model, "small.en");
    }

    #[test]
    fn unknown_memory_never_assumes_large_hardware() {
        let recommendation = caps(16, None).recommend_model();
        assert_eq!(recommendation.model, "base.en");
        assert!(recommendation.conservative);
    }
}
