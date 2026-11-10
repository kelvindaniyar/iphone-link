//! Direct3D 11 hardware context probe and Latest-Frame-Wins triple-buffering SharedFrameRing.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const D3D11_SDK_VERSION: u32 = 7;
pub const D3D_DRIVER_TYPE_HARDWARE: u32 = 1;
pub const D3D11_CREATE_DEVICE_BGRA_SUPPORT: u32 = 0x20;
pub const D3D11_CREATE_DEVICE_VIDEO_SUPPORT: u32 = 0x800;

pub const D3D_FEATURE_LEVEL_11_0: u32 = 0xb000;
pub const D3D_FEATURE_LEVEL_11_1: u32 = 0xb100;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

// IID_IDXGIFactory1 = {770aae78-f26f-4dba-a829-253c83d1b387}
pub const IID_IDXGI_FACTORY1: Guid = Guid {
    data1: 0x770aae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

#[repr(C)]
pub struct DxgiAdapterDesc1 {
    pub description: [u16; 128],
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsys_id: u32,
    pub revision: u32,
    pub dedicated_video_memory: usize,
    pub dedicated_system_memory: usize,
    pub shared_system_memory: usize,
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
    pub flags: u32,
}

impl Default for DxgiAdapterDesc1 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

type FnCreateDXGIFactory1 = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
type FnD3D11CreateDevice = unsafe extern "system" fn(
    *mut c_void, // pAdapter
    u32,         // DriverType
    *mut c_void, // Software
    u32,         // Flags
    *const u32,  // pFeatureLevels
    u32,         // FeatureLevels
    u32,         // SDKVersion
    *mut *mut c_void, // ppDevice
    *mut u32,    // pFeatureLevel
    *mut *mut c_void, // ppImmediateContext
) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D3d11HardwareInfo {
    pub adapter_name: String,
    pub feature_level: String,
    pub feature_level_raw: u32,
    pub dedicated_vram_mb: usize,
    pub hardware_device_created: bool,
}

/// Probe real Direct3D 11 and DXGI hardware adapter on Windows.
pub fn probe_d3d11_hardware_device() -> Result<D3d11HardwareInfo, String> {
    let dxgi_wide = to_wide("dxgi.dll");
    let d3d11_wide = to_wide("d3d11.dll");

    let h_dxgi = unsafe { LoadLibraryW(dxgi_wide.as_ptr()) };
    if h_dxgi.is_null() {
        return Err("Could not load dxgi.dll".to_string());
    }

    let h_d3d11 = unsafe { LoadLibraryW(d3d11_wide.as_ptr()) };
    if h_d3d11.is_null() {
        unsafe { FreeLibrary(h_dxgi) };
        return Err("Could not load d3d11.dll".to_string());
    }

    let mut adapter_name = "Unknown Adapter".to_string();
    let mut vram_bytes = 0usize;

    unsafe {
        let p_create_factory1 = GetProcAddress(h_dxgi, b"CreateDXGIFactory1\0".as_ptr());
        if !p_create_factory1.is_null() {
            let create_factory1: FnCreateDXGIFactory1 = std::mem::transmute(p_create_factory1);
            let mut p_factory: *mut c_void = std::ptr::null_mut();

            if create_factory1(&IID_IDXGI_FACTORY1, &mut p_factory) == 0 && !p_factory.is_null() {
                // IDXGIFactory1 vtable: method 12 is EnumAdapters1
                let vtable = *(p_factory as *const *const *const c_void);
                let enum_adapters1_ptr = *vtable.add(12);
                let enum_adapters1: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                    std::mem::transmute(enum_adapters1_ptr);

                let mut p_adapter: *mut c_void = std::ptr::null_mut();
                if enum_adapters1(p_factory, 0, &mut p_adapter) == 0 && !p_adapter.is_null() {
                    // IDXGIAdapter1 vtable: method 10 is GetDesc1
                    let adapter_vtable = *(p_adapter as *const *const *const c_void);
                    let get_desc1_ptr = *adapter_vtable.add(10);
                    let get_desc1: unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32 =
                        std::mem::transmute(get_desc1_ptr);

                    let mut desc = DxgiAdapterDesc1::default();
                    if get_desc1(p_adapter, &mut desc) == 0 {
                        let len = desc.description.iter().position(|&c| c == 0).unwrap_or(128);
                        adapter_name = String::from_utf16_lossy(&desc.description[..len]);
                        vram_bytes = desc.dedicated_video_memory;
                    }

                    // Release p_adapter (method 2 on IUnknown)
                    let release_adapter_ptr = *adapter_vtable.add(2);
                    let release_adapter: unsafe extern "system" fn(*mut c_void) -> u32 =
                        std::mem::transmute(release_adapter_ptr);
                    release_adapter(p_adapter);
                }

                // Release p_factory (method 2 on IUnknown)
                let release_factory_ptr = *vtable.add(2);
                let release_factory: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(release_factory_ptr);
                release_factory(p_factory);
            }
        }
    }

    // Now test D3D11CreateDevice with D3D_DRIVER_TYPE_HARDWARE
    let mut feature_level_raw = 0u32;
    let mut hw_ok = false;

    unsafe {
        let p_create_device = GetProcAddress(h_d3d11, b"D3D11CreateDevice\0".as_ptr());
        if !p_create_device.is_null() {
            let create_device: FnD3D11CreateDevice = std::mem::transmute(p_create_device);
            let mut p_device: *mut c_void = std::ptr::null_mut();
            let mut p_context: *mut c_void = std::ptr::null_mut();
            let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;

            let hr = create_device(
                std::ptr::null_mut(),
                D3D_DRIVER_TYPE_HARDWARE,
                std::ptr::null_mut(),
                flags,
                levels.as_ptr(),
                levels.len() as u32,
                D3D11_SDK_VERSION,
                &mut p_device,
                &mut feature_level_raw,
                &mut p_context,
            );

            if hr == 0 && !p_device.is_null() {
                hw_ok = true;
                // Release context and device
                if !p_context.is_null() {
                    let ctx_vtable = *(p_context as *const *const *const c_void);
                    let rel_ctx_ptr = *ctx_vtable.add(2);
                    let rel_ctx: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(rel_ctx_ptr);
                    rel_ctx(p_context);
                }
                let dev_vtable = *(p_device as *const *const *const c_void);
                let rel_dev_ptr = *dev_vtable.add(2);
                let rel_dev: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(rel_dev_ptr);
                rel_dev(p_device);
            }
        }

        FreeLibrary(h_d3d11);
        FreeLibrary(h_dxgi);
    }

    let feature_str = match feature_level_raw {
        0xb100 => "Direct3D 11.1 (0xb100)",
        0xb000 => "Direct3D 11.0 (0xb000)",
        0xa100 => "Direct3D 10.1 (0xa100)",
        0xa000 => "Direct3D 10.0 (0xa000)",
        other => return Ok(D3d11HardwareInfo {
            adapter_name,
            feature_level: format!("0x{:x}", other),
            feature_level_raw: other,
            dedicated_vram_mb: vram_bytes / (1024 * 1024),
            hardware_device_created: hw_ok,
        }),
    };

    Ok(D3d11HardwareInfo {
        adapter_name,
        feature_level: feature_str.to_string(),
        feature_level_raw,
        dedicated_vram_mb: vram_bytes / (1024 * 1024),
        hardware_device_created: hw_ok,
    })
}

/// A decoded frame slot in the shared triple-buffer ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSlot {
    pub slot_idx: usize,
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub timestamp: Instant,
    pub data: Vec<u8>,
}

pub const RING_SLOT_COUNT: usize = 3;
pub const MAX_RING_QUEUE: usize = 2;

#[derive(Debug)]
struct RingInner {
    slots: [Option<FrameSlot>; RING_SLOT_COUNT],
    queue: VecDeque<usize>,
    showing: Option<usize>,
    next_seq: u64,
    total_dropped: u64,
}

impl Default for RingInner {
    fn default() -> Self {
        Self {
            slots: [None, None, None],
            queue: VecDeque::new(),
            showing: None,
            next_seq: 1,
            total_dropped: 0,
        }
    }
}

/// Thread-safe Latest-Frame-Wins 3-slot triple buffering ring buffer.
/// Guarantees decoder is never blocked and stale frames are automatically dropped.
#[derive(Debug, Clone)]
pub struct SharedFrameRing {
    inner: Arc<Mutex<RingInner>>,
}

impl Default for SharedFrameRing {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RingInner::default())),
        }
    }
}

impl SharedFrameRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a new decoded frame. Returns (sequence, frames_dropped_this_push).
    pub fn push(&self, width: u32, height: u32, data: Vec<u8>) -> (u64, u64) {
        let mut g = self.inner.lock().unwrap();
        let seq = g.next_seq;
        g.next_seq += 1;

        // Find slot index that is neither showing nor in queue
        let target_slot = (0..RING_SLOT_COUNT)
            .find(|&i| !g.queue.contains(&i) && g.showing != Some(i))
            .unwrap_or(0);

        g.slots[target_slot] = Some(FrameSlot {
            slot_idx: target_slot,
            width,
            height,
            sequence: seq,
            timestamp: Instant::now(),
            data,
        });

        g.queue.push_back(target_slot);

        // Latest-Frame-Wins: drop stale waiting frames if queue exceeds MAX_RING_QUEUE
        let mut dropped = 0u64;
        while g.queue.len() > MAX_RING_QUEUE {
            if let Some(stale) = g.queue.pop_front() {
                g.slots[stale] = None;
                dropped += 1;
                g.total_dropped += 1;
            }
        }

        (seq, dropped)
    }

    /// Retrieve the newest frame for the display refresh.
    /// Returns the latest available frame and drains older stale frames.
    pub fn next(&self) -> Option<FrameSlot> {
        let mut g = self.inner.lock().unwrap();
        if let Some(newest_idx) = g.queue.pop_back() {
            // Drop any remaining older frames in the queue
            while let Some(stale) = g.queue.pop_front() {
                g.slots[stale] = None;
                g.total_dropped += 1;
            }
            g.showing = Some(newest_idx);
            return g.slots[newest_idx].clone();
        }
        // If queue is empty, repeat last showing frame if available
        g.showing.and_then(|idx| g.slots[idx].clone())
    }

    pub fn total_dropped(&self) -> u64 {
        self.inner.lock().unwrap().total_dropped
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.queue.clear();
        g.showing = None;
        for s in g.slots.iter_mut() {
            *s = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_d3d11_hardware_probe() {
        let res = probe_d3d11_hardware_device();
        assert!(res.is_ok(), "Probe failed: {:?}", res.err());
        let info = res.unwrap();
        assert!(info.hardware_device_created);
        assert!(!info.adapter_name.is_empty());
        assert!(info.feature_level_raw >= D3D_FEATURE_LEVEL_11_0);
        println!("Tested hardware GPU: {} (Feature: {})", info.adapter_name, info.feature_level);
    }

    #[test]
    fn test_latest_frame_wins_ring() {
        let ring = SharedFrameRing::new();

        // Push 3 frames in rapid succession (seq 1, seq 2, seq 3)
        let (seq1, _) = ring.push(1216, 2656, vec![1, 1, 1]);
        let (seq2, _) = ring.push(1216, 2656, vec![2, 2, 2]);
        let (seq3, _) = ring.push(1216, 2656, vec![3, 3, 3]);

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);

        // next() must immediately yield the latest frame (seq 3)
        let latest = ring.next();
        assert!(latest.is_some());
        let frame = latest.unwrap();
        assert_eq!(frame.sequence, 3);
        assert_eq!(frame.data, vec![3, 3, 3]);
    }
}
