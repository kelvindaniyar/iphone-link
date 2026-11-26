//! Adaptive Multi-Tap D3D11 NV12 -> BGRA8 Pixel Shader (Zero-Copy VRAM, Exact BT.709 Studio->Full sRGB,
//! 4-Way Rotation, Half-Texel Aperture Clamp, & `1.15x..4.0x` Anti-Aliasing Lanczos-2 Downscaling).

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::d3d11_context::{
    ComPtr, D3d11Box, D3d11MappedSubresource, LiveD3d11Gpu, Rect, VideoColorInfo,
    VideoColorMatrix, D3D11_BIND_CONSTANT_BUFFER, D3D11_CPU_ACCESS_WRITE, D3D11_MAP_WRITE_DISCARD,
    D3D11_USAGE_DYNAMIC,
};

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

const D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST: u32 = 4;
const D3D11_FILTER_MIN_MAG_MIP_LINEAR: u32 = 21;
const D3D11_TEXTURE_ADDRESS_CLAMP: u32 = 3;
const D3D11_COMPARISON_NEVER: u32 = 1;
const D3D11_COMPARISON_ALWAYS: u32 = 8;
const D3D11_FILL_SOLID: u32 = 3;
const D3D11_CULL_NONE: u32 = 1;
const D3D11_BLEND_ZERO: u32 = 1;
const D3D11_BLEND_ONE: u32 = 2;
const D3D11_BLEND_OP_ADD: u32 = 1;
const D3D11_COLOR_WRITE_ENABLE_ALL: u8 = 15;
const D3D11_DEPTH_WRITE_MASK_ZERO: u32 = 0;
const D3D11_STENCIL_OP_KEEP: u32 = 1;
const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 2048;
const D3DCOMPILE_OPTIMIZATION_LEVEL3: u32 = 32768;

const MAX_CACHED_NV12_SRVS: usize = 8;

/// Constant buffer layout shared identically between Rust and HLSL (`cbuffer HdNv12Params : register(b0)`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdNv12ShaderParams {
    pub src_rect: [f32; 4],    // c0: [src_left, src_top, src_right, src_bottom] (source px)
    pub tex_size: [f32; 2],    // c1.xy: [tex_w, tex_h]
    pub dst_size: [f32; 2],    // c1.zw: [dest_w, dest_h]
    pub rotation_mode: u32,    // c2.x: 0=0°, 1=90° CW, 2=180°, 3=270° CW
    pub color_flags: u32,      // c2.y: bit0=full_range, bit1=bt601, bit2=bt2020
    pub sharpen_strength: f32, // c2.z: reserved micro-acutance (0.0 default)
    pub pad: u32,              // c2.w
}

const _: () = assert!(std::mem::size_of::<HdNv12ShaderParams>() == 48);

#[inline]
pub fn video_color_info_to_shader_flags(color: VideoColorInfo) -> u32 {
    let mut flags = if color.full_range { 1u32 } else { 0u32 };
    match color.matrix {
        VideoColorMatrix::Bt709 => {}
        VideoColorMatrix::Bt601 => flags |= 2,
        VideoColorMatrix::Bt2020 => flags |= 4,
    }
    flags
}

#[inline]
pub fn rotation_deg_to_mode(rotation_deg: u16) -> u32 {
    match rotation_deg % 360 {
        90 => 1,
        180 => 2,
        270 => 3,
        _ => 0,
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct D3d11BufferDesc {
    byte_width: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access_flags: u32,
    misc_flags: u32,
    structure_byte_stride: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11SamplerDesc {
    filter: u32,
    address_u: u32,
    address_v: u32,
    address_w: u32,
    mip_lod_bias: f32,
    max_anisotropy: u32,
    comparison_func: u32,
    border_color: [f32; 4],
    min_lod: f32,
    max_lod: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11RasterizerDesc {
    fill_mode: u32,
    cull_mode: u32,
    front_counter_clockwise: i32,
    depth_bias: i32,
    depth_bias_clamp: f32,
    slope_scaled_depth_bias: f32,
    depth_clip_enable: i32,
    scissor_enable: i32,
    multisample_enable: i32,
    antialiased_line_enable: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11RenderTargetBlendDesc {
    blend_enable: i32,
    src_blend: u32,
    dest_blend: u32,
    blend_op: u32,
    src_blend_alpha: u32,
    dest_blend_alpha: u32,
    blend_op_alpha: u32,
    render_target_write_mask: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11BlendDesc {
    alpha_to_coverage_enable: i32,
    independent_blend_enable: i32,
    render_target: [D3d11RenderTargetBlendDesc; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11DepthStencilOpDesc {
    stencil_fail_op: u32,
    stencil_depth_fail_op: u32,
    stencil_pass_op: u32,
    stencil_func: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11DepthStencilDesc {
    depth_enable: i32,
    depth_write_mask: u32,
    depth_func: u32,
    stencil_enable: i32,
    stencil_read_mask: u8,
    stencil_write_mask: u8,
    front_face: D3d11DepthStencilOpDesc,
    back_face: D3d11DepthStencilOpDesc,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct D3d11Viewport {
    top_left_x: f32,
    top_left_y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
}

/// High-quality NV12 -> BGRA8 HLSL shader source:
/// - Fullscreen triangle vertex shader (`SV_VertexID`, 0 vertex buffers)
/// - 4-way exact rotation (`0`, `90° CW`, `180°`, `270° CW`)
/// - Half-texel aperture clamp (0% mod-16 padding bleed)
/// - Separable adaptive multi-tap Lanczos-2 downscaling (`1.15x..4x+`) & Catmull-Rom 1:1/upscaling
/// - Left-cosited NV12 chroma reconstruction
/// - Exact BT.709 Studio (`16..235`) -> Full sRGB (`0..255`) + Full/BT.601/BT.2020 variants
pub const HD_NV12_TO_BGRA_HLSL: &str = r#"
cbuffer HdNv12Params : register(b0)
{
    float4 src_rect;
    float2 tex_size;
    float2 dst_size;
    uint   rotation_mode;
    uint   color_flags;
    float  sharpen_strength;
    uint   pad;
};

Texture2D<float>  luma_tex   : register(t0);
Texture2D<float2> chroma_tex : register(t1);
SamplerState      linear_smp : register(s0);

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

VsOut vs_main(uint id : SV_VertexID)
{
    VsOut o;
    float2 uv = float2((id << 1) & 2, id & 2);
    o.uv  = uv;
    o.pos = float4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return o;
}

float lanczos2(float x)
{
    float ax = abs(x);
    if (ax < 1e-4)
        return 1.0;
    if (ax >= 2.0)
        return 0.0;
    float px = 3.14159265 * ax;
    return (sin(px) / px) * (sin(px * 0.5) / (px * 0.5));
}

float catmull_rom(float x)
{
    float ax = abs(x);
    if (ax < 1.0)
        return ((1.5 * ax - 2.5) * ax * ax) + 1.0;
    if (ax < 2.0)
        return (((-0.5 * ax + 2.5) * ax - 4.0) * ax) + 2.0;
    return 0.0;
}

float sample_luma_hq(float2 src_px, float2 scale_xy, float4 clamp_uv, float2 inv_tex)
{
    if (scale_xy.x <= 1.15 && scale_xy.y <= 1.15)
    {
        float2 p = src_px - 0.5;
        float2 f = frac(p);
        float2 b = floor(p);

        float wx[4];
        float wy[4];
        float u_tap[4];
        float v_tap[4];
        float wx_sum = 0.0;
        float wy_sum = 0.0;

        [unroll]
        for (int i = 0; i < 4; ++i)
        {
            float k = float(i - 1);
            wx[i] = catmull_rom(k - f.x);
            wy[i] = catmull_rom(k - f.y);
            wx_sum += wx[i];
            wy_sum += wy[i];
            u_tap[i] = clamp((b.x + k + 0.5) * inv_tex.x, clamp_uv.x, clamp_uv.z);
            v_tap[i] = clamp((b.y + k + 0.5) * inv_tex.y, clamp_uv.y, clamp_uv.w);
        }

        float acc = 0.0;
        [unroll]
        for (int iy = 0; iy < 4; ++iy)
        {
            float row_acc = 0.0;
            [unroll]
            for (int ix = 0; ix < 4; ++ix)
            {
                row_acc += luma_tex.SampleLevel(linear_smp, float2(u_tap[ix], v_tap[iy]), 0.0) * wx[ix];
            }
            acc += row_acc * wy[iy];
        }
        return saturate(acc / max(wx_sum * wy_sum, 1e-5));
    }

    bool do_cas = (sharpen_strength > 1e-4);
    int rx = clamp((int)ceil(1.8 * max(scale_xy.x, 1.0)), 2, 8);
    int ry = clamp((int)ceil(1.8 * max(scale_xy.y, 1.0)), 2, 8);
    int nx = rx * 2;
    int ny = ry * 2;
    float sx = max(scale_xy.x, 1.0);
    float sy = max(scale_xy.y, 1.0);
    float2 b = floor(src_px - 0.5);

    float wx[16];
    float wy[16];
    float bx[16];
    float by[16];
    float u_tap[16];
    float v_tap[16];
    for (int z = 0; z < 16; ++z)
    {
        wx[z] = 0.0;
        wy[z] = 0.0;
        bx[z] = 0.0;
        by[z] = 0.0;
        u_tap[z] = clamp_uv.x;
        v_tap[z] = clamp_uv.y;
    }

    float wx_sum = 0.0;
    float bx_sum = 0.0;
    [loop]
    for (int ix = 0; ix < nx; ++ix)
    {
        int k = ix - rx + 1;
        float tap_px = b.x + float(k) + 0.5;
        float dx = (tap_px - src_px.x) / sx;
        float w = lanczos2(dx);
        wx[ix] = w;
        wx_sum += w;
        if (do_cas)
        {
            float bwx = exp(-0.75 * dx * dx);
            bx[ix] = bwx;
            bx_sum += bwx;
        }
        u_tap[ix] = clamp(tap_px * inv_tex.x, clamp_uv.x, clamp_uv.z);
    }

    float wy_sum = 0.0;
    float by_sum = 0.0;
    [loop]
    for (int iy = 0; iy < ny; ++iy)
    {
        int k = iy - ry + 1;
        float tap_px = b.y + float(k) + 0.5;
        float dy = (tap_px - src_px.y) / sy;
        float w = lanczos2(dy);
        wy[iy] = w;
        wy_sum += w;
        if (do_cas)
        {
            float bwy = exp(-0.75 * dy * dy);
            by[iy] = bwy;
            by_sum += bwy;
        }
        v_tap[iy] = clamp(tap_px * inv_tex.y, clamp_uv.y, clamp_uv.w);
    }

    float acc = 0.0;
    float blur_acc = 0.0;
    float cmin = 1.0;
    float cmax = 0.0;
    bool has_local = false;
    [loop]
    for (int iy2 = 0; iy2 < ny; ++iy2)
    {
        float wy_val = wy[iy2];
        float by_val = by[iy2];
        float v_val = v_tap[iy2];
        bool ly_in = do_cas && (by_val >= 0.309);
        float row_acc = 0.0;
        float row_blur = 0.0;
        [loop]
        for (int ix2 = 0; ix2 < nx; ++ix2)
        {
            float s = luma_tex.SampleLevel(linear_smp, float2(u_tap[ix2], v_val), 0.0);
            row_acc += s * wx[ix2];
            if (do_cas)
            {
                float bwx_val = bx[ix2];
                row_blur += s * bwx_val;
                if (ly_in && (bwx_val >= 0.309))
                {
                    cmin = min(cmin, s);
                    cmax = max(cmax, s);
                    has_local = true;
                }
            }
        }
        acc += row_acc * wy_val;
        if (do_cas)
        {
            blur_acc += row_blur * by_val;
        }
    }

    float y_l = saturate(acc / max(wx_sum * wy_sum, 1e-5));
    if (do_cas && has_local && (cmax > cmin) && ((bx_sum * by_sum) > 1e-5))
    {
        float y_b = saturate(blur_acc / max(bx_sum * by_sum, 1e-5));
        float contrast = cmax - cmin;
        float gate = saturate((contrast - 0.02) / 0.08);
        float y_sharp = y_l + (sharpen_strength * gate) * (y_l - y_b);
        float margin = min(0.04, 0.075 * contrast);
        return clamp(y_sharp, max(0.0, cmin - margin), min(1.0, cmax + margin));
    }
    return y_l;
}

float2 sample_chroma_hq(float2 src_px, float2 scale_xy, float4 clamp_uv, float2 inv_tex)
{
    // Left-cosited NV12 chroma: chroma center (cx, cy) corresponds to luma center (2*cx + 0.5, 2*cy + 1.0)
    // in pixel coords, or bilinear texture coords ((src_px.x + 0.5) / tex_w, src_px.y / tex_h).
    float csx = max(scale_xy.x * 0.5, 1.0);
    float csy = max(scale_xy.y * 0.5, 1.0);

    if (csx <= 1.15 && csy <= 1.15)
    {
        float2 uv = float2((src_px.x + 0.5) * inv_tex.x, src_px.y * inv_tex.y);
        uv = clamp(uv, clamp_uv.xy, clamp_uv.zw);
        return chroma_tex.SampleLevel(linear_smp, uv, 0.0);
    }

    float2 c_px = float2((src_px.x + 0.5) * 0.5, src_px.y * 0.5);
    float2 cb = floor(c_px - 0.5);
    int rx = clamp((int)ceil(1.8 * csx), 2, 4);
    int ry = clamp((int)ceil(1.8 * csy), 2, 4);
    int nx = rx * 2;
    int ny = ry * 2;

    float wx[8];
    float wy[8];
    float u_tap[8];
    float v_tap[8];
    for (int z = 0; z < 8; ++z)
    {
        wx[z] = 0.0;
        wy[z] = 0.0;
        u_tap[z] = clamp_uv.x;
        v_tap[z] = clamp_uv.y;
    }

    float wx_sum = 0.0;
    [loop]
    for (int ix = 0; ix < nx; ++ix)
    {
        int k = ix - rx + 1;
        float tap_c = cb.x + float(k) + 0.5;
        float w = lanczos2((tap_c - c_px.x) / csx);
        wx[ix] = w;
        wx_sum += w;
        u_tap[ix] = clamp((tap_c * 2.0) * inv_tex.x, clamp_uv.x, clamp_uv.z);
    }

    float wy_sum = 0.0;
    [loop]
    for (int iy = 0; iy < ny; ++iy)
    {
        int k = iy - ry + 1;
        float tap_c = cb.y + float(k) + 0.5;
        float w = lanczos2((tap_c - c_px.y) / csy);
        wy[iy] = w;
        wy_sum += w;
        v_tap[iy] = clamp((tap_c * 2.0) * inv_tex.y, clamp_uv.y, clamp_uv.w);
    }

    float2 acc = float2(0.0, 0.0);
    [loop]
    for (int iy2 = 0; iy2 < ny; ++iy2)
    {
        float wy_val = wy[iy2];
        float v_val = v_tap[iy2];
        float2 row_acc = float2(0.0, 0.0);
        [loop]
        for (int ix2 = 0; ix2 < nx; ++ix2)
        {
            row_acc += chroma_tex.SampleLevel(linear_smp, float2(u_tap[ix2], v_val), 0.0) * wx[ix2];
        }
        acc += row_acc * wy_val;
    }
    return saturate(acc / max(wx_sum * wy_sum, 1e-5));
}

float4 ps_main(VsOut inp) : SV_Target
{
    float2 inv_tex = 1.0 / max(tex_size, float2(2.0, 2.0));
    float u = inp.uv.x;
    float v = inp.uv.y;

    float2 mapped_uv;
    if (rotation_mode == 1)
    {
        mapped_uv = float2(v, 1.0 - u);
    }
    else if (rotation_mode == 2)
    {
        mapped_uv = float2(1.0 - u, 1.0 - v);
    }
    else if (rotation_mode == 3)
    {
        mapped_uv = float2(1.0 - v, u);
    }
    else
    {
        mapped_uv = float2(u, v);
    }

    float2 src_px = float2(
        lerp(src_rect.x, src_rect.z, mapped_uv.x),
        lerp(src_rect.y, src_rect.w, mapped_uv.y)
    );

    float src_w = max(src_rect.z - src_rect.x, 1.0);
    float src_h = max(src_rect.w - src_rect.y, 1.0);
    float eff_dst_w = (rotation_mode == 1 || rotation_mode == 3) ? max(dst_size.y, 1.0) : max(dst_size.x, 1.0);
    float eff_dst_h = (rotation_mode == 1 || rotation_mode == 3) ? max(dst_size.x, 1.0) : max(dst_size.y, 1.0);
    float2 scale_xy = float2(src_w / eff_dst_w, src_h / eff_dst_h);

    float min_yu = (src_rect.x + 0.5) * inv_tex.x;
    float max_yu = max(min_yu, (src_rect.z - 0.5) * inv_tex.x);
    float min_yv = (src_rect.y + 0.5) * inv_tex.y;
    float max_yv = max(min_yv, (src_rect.w - 0.5) * inv_tex.y);
    float4 luma_clamp = float4(min_yu, min_yv, max_yu, max_yv);

    float min_cu = (src_rect.x + 1.0) * inv_tex.x;
    float max_cu = max(min_cu, (src_rect.z - 1.0) * inv_tex.x);
    float min_cv = (src_rect.y + 1.0) * inv_tex.y;
    float max_cv = max(min_cv, (src_rect.w - 1.0) * inv_tex.y);
    float4 chroma_clamp = float4(min_cu, min_cv, max_cu, max_cv);

    float y_val = sample_luma_hq(src_px, scale_xy, luma_clamp, inv_tex);
    float2 uv_val = sample_chroma_hq(src_px, scale_xy, chroma_clamp, inv_tex);

    float y_lin;
    float cb;
    float cr;
    if ((color_flags & 1u) != 0u)
    {
        y_lin = y_val;
        cb = uv_val.x - (128.0 / 255.0);
        cr = uv_val.y - (128.0 / 255.0);
    }
    else
    {
        y_lin = saturate((y_val - (16.0 / 255.0)) * (255.0 / 219.0));
        cb = (uv_val.x - (128.0 / 255.0)) * (255.0 / 224.0);
        cr = (uv_val.y - (128.0 / 255.0)) * (255.0 / 224.0);
    }

    float3 rgb;
    if ((color_flags & 2u) != 0u)
    {
        // BT.601
        rgb.r = y_lin + 1.402000 * cr;
        rgb.g = y_lin - 0.344136 * cb - 0.714136 * cr;
        rgb.b = y_lin + 1.772000 * cb;
    }
    else if ((color_flags & 4u) != 0u)
    {
        // BT.2020
        rgb.r = y_lin + 1.474600 * cr;
        rgb.g = y_lin - 0.164553 * cb - 0.571353 * cr;
        rgb.b = y_lin + 1.881400 * cb;
    }
    else
    {
        // BT.709 default (standard iOS HEVC & AirPlay H.264)
        rgb.r = y_lin + 1.574800 * cr;
        rgb.g = y_lin - 0.187324 * cb - 0.468124 * cr;
        rgb.b = y_lin + 1.855600 * cb;
    }

    return float4(saturate(rgb), 1.0);
}
"#;

type FnD3DCompile = unsafe extern "system" fn(
    p_src_data: *const c_void,
    src_data_size: usize,
    p_source_name: *const u8,
    p_defines: *const c_void,
    p_include: *const c_void,
    p_entrypoint: *const u8,
    p_target: *const u8,
    flags1: u32,
    flags2: u32,
    pp_code: *mut *mut c_void,
    pp_error_msgs: *mut *mut c_void,
) -> i32;

fn compile_hlsl_stage(
    d3d_compile: FnD3DCompile,
    entry: &[u8],
    targets: &[&[u8]],
) -> Result<Vec<u8>, String> {
    let src_bytes = HD_NV12_TO_BGRA_HLSL.as_bytes();
    let src_name = b"nv12_hd_shader.hlsl\0";
    let flags1 = D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3;
    let mut last_err = String::from("D3DCompile failed");

    for &target in targets {
        let mut p_code: *mut c_void = std::ptr::null_mut();
        let mut p_err: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            d3d_compile(
                src_bytes.as_ptr() as *const c_void,
                src_bytes.len(),
                src_name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                entry.as_ptr(),
                target.as_ptr(),
                flags1,
                0,
                &mut p_code,
                &mut p_err,
            )
        };
        let code_blob = unsafe { ComPtr::from_raw(p_code) };
        let err_blob = unsafe { ComPtr::from_raw(p_err) };
        if hr >= 0 {
            if let Some(blob) = code_blob {
                unsafe {
                    let vt = blob.vtable();
                    let get_ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 =
                        std::mem::transmute(*vt.add(3));
                    let get_sz: unsafe extern "system" fn(*mut c_void) -> usize =
                        std::mem::transmute(*vt.add(4));
                    let ptr = get_ptr(blob.as_ptr());
                    let sz = get_sz(blob.as_ptr());
                    if !ptr.is_null() && sz > 0 {
                        return Ok(std::slice::from_raw_parts(ptr, sz).to_vec());
                    }
                }
            }
        }
        let mut err_msg = format!("HRESULT=0x{:08X}", hr as u32);
        if let Some(eb) = err_blob {
            unsafe {
                let vt = eb.vtable();
                let get_ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 =
                    std::mem::transmute(*vt.add(3));
                let get_sz: unsafe extern "system" fn(*mut c_void) -> usize =
                    std::mem::transmute(*vt.add(4));
                let ptr = get_ptr(eb.as_ptr());
                let sz = get_sz(eb.as_ptr());
                if !ptr.is_null() && sz > 0 {
                    let s = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, sz));
                    err_msg = format!("{err_msg}: {}", s.trim());
                }
            }
        }
        last_err = err_msg;
    }
    Err(last_err)
}

/// Compiles `HD_NV12_TO_BGRA_HLSL` once per process via dynamically loaded `d3dcompiler_47.dll`,
/// caches the compact DXBC bytecode, and immediately unloads `d3dcompiler_47.dll` (`FreeLibrary`)
/// so the 4.6 MB compiler DLL leaves the process working set!
pub fn compile_hd_nv12_shaders_cached() -> Result<&'static (Vec<u8>, Vec<u8>), String> {
    static BYTECODE: OnceLock<Result<(Vec<u8>, Vec<u8>), String>> = OnceLock::new();
    let res = BYTECODE.get_or_init(|| {
        let wide: Vec<u16> = "d3dcompiler_47.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let h_mod = unsafe { LoadLibraryW(wide.as_ptr()) };
        if h_mod.is_null() {
            return Err("Failed to load d3dcompiler_47.dll".to_string());
        }
        let compile_res = unsafe {
            let p_sym = GetProcAddress(h_mod, b"D3DCompile\0".as_ptr());
            if p_sym.is_null() {
                Err("D3DCompile export missing in d3dcompiler_47.dll".to_string())
            } else {
                let d3d_compile: FnD3DCompile = std::mem::transmute(p_sym);
                let vs = compile_hlsl_stage(d3d_compile, b"vs_main\0", &[b"vs_5_0\0", b"vs_4_0\0"])?;
                let ps = compile_hlsl_stage(d3d_compile, b"ps_main\0", &[b"ps_5_0\0", b"ps_4_0\0"])?;
                Ok((vs, ps))
            }
        };
        unsafe {
            FreeLibrary(h_mod);
        }
        compile_res
    });
    res.as_ref().map_err(|e| e.clone())
}

struct CachedNv12Srvs {
    tex_ptr: usize,
    tex_w: u32,
    tex_h: u32,
    srv_y: ComPtr,
    srv_uv: ComPtr,
}

/// Hardware Direct3D 11 pixel-shader renderer for NV12 -> BGRA8 presentation with
/// adaptive multi-tap Lanczos-2 downscale anti-aliasing, Catmull-Rom 1:1/upscale,
/// 4-way rotation, and exact BT.709 Studio -> Full sRGB conversion.
pub struct HdNv12ShaderRenderer {
    device_ptr: usize,
    vs: ComPtr,
    ps: ComPtr,
    sampler: ComPtr,
    cbuf: ComPtr,
    rasterizer: ComPtr,
    blend: ComPtr,
    depth_stencil: ComPtr,
    srv_cache: Vec<CachedNv12Srvs>,
}

unsafe impl Send for HdNv12ShaderRenderer {}
unsafe impl Sync for HdNv12ShaderRenderer {}

impl HdNv12ShaderRenderer {
    pub fn new(gpu: &LiveD3d11Gpu) -> Result<Self, String> {
        let (vs_bc, ps_bc) = compile_hd_nv12_shaders_cached()?;
        let dev = gpu.device();
        unsafe {
            let vt = dev.vtable();

            let create_vs: unsafe extern "system" fn(
                *mut c_void,
                *const c_void,
                usize,
                *mut c_void,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(12));
            let mut p_vs: *mut c_void = std::ptr::null_mut();
            let hr_vs = create_vs(
                dev.as_ptr(),
                vs_bc.as_ptr() as *const c_void,
                vs_bc.len(),
                std::ptr::null_mut(),
                &mut p_vs,
            );
            if hr_vs < 0 || p_vs.is_null() {
                return Err(format!("CreateVertexShader failed: 0x{:08X}", hr_vs as u32));
            }
            let vs = ComPtr::from_raw(p_vs).ok_or("null ID3D11VertexShader")?;

            let create_ps: unsafe extern "system" fn(
                *mut c_void,
                *const c_void,
                usize,
                *mut c_void,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(15));
            let mut p_ps: *mut c_void = std::ptr::null_mut();
            let hr_ps = create_ps(
                dev.as_ptr(),
                ps_bc.as_ptr() as *const c_void,
                ps_bc.len(),
                std::ptr::null_mut(),
                &mut p_ps,
            );
            if hr_ps < 0 || p_ps.is_null() {
                return Err(format!("CreatePixelShader failed: 0x{:08X}", hr_ps as u32));
            }
            let ps = ComPtr::from_raw(p_ps).ok_or("null ID3D11PixelShader")?;

            let create_sampler: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11SamplerDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(23));
            let samp_desc = D3d11SamplerDesc {
                filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                address_u: D3D11_TEXTURE_ADDRESS_CLAMP,
                address_v: D3D11_TEXTURE_ADDRESS_CLAMP,
                address_w: D3D11_TEXTURE_ADDRESS_CLAMP,
                mip_lod_bias: 0.0,
                max_anisotropy: 1,
                comparison_func: D3D11_COMPARISON_NEVER,
                border_color: [0.0; 4],
                min_lod: 0.0,
                max_lod: f32::MAX,
            };
            let mut p_samp: *mut c_void = std::ptr::null_mut();
            let hr_s = create_sampler(dev.as_ptr(), &samp_desc, &mut p_samp);
            if hr_s < 0 || p_samp.is_null() {
                return Err(format!("CreateSamplerState failed: 0x{:08X}", hr_s as u32));
            }
            let sampler = ComPtr::from_raw(p_samp).ok_or("null ID3D11SamplerState")?;

            let create_buf: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11BufferDesc,
                *const c_void,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(3));
            let cbuf_desc = D3d11BufferDesc {
                byte_width: std::mem::size_of::<HdNv12ShaderParams>() as u32,
                usage: D3D11_USAGE_DYNAMIC,
                bind_flags: D3D11_BIND_CONSTANT_BUFFER,
                cpu_access_flags: D3D11_CPU_ACCESS_WRITE,
                misc_flags: 0,
                structure_byte_stride: 0,
            };
            let mut p_cbuf: *mut c_void = std::ptr::null_mut();
            let hr_cb = create_buf(dev.as_ptr(), &cbuf_desc, std::ptr::null(), &mut p_cbuf);
            if hr_cb < 0 || p_cbuf.is_null() {
                return Err(format!("CreateBuffer(cbuf) failed: 0x{:08X}", hr_cb as u32));
            }
            let cbuf = ComPtr::from_raw(p_cbuf).ok_or("null ID3D11Buffer")?;

            let create_rs: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11RasterizerDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(22));
            let rs_desc = D3d11RasterizerDesc {
                fill_mode: D3D11_FILL_SOLID,
                cull_mode: D3D11_CULL_NONE,
                front_counter_clockwise: 0,
                depth_bias: 0,
                depth_bias_clamp: 0.0,
                slope_scaled_depth_bias: 0.0,
                depth_clip_enable: 1,
                scissor_enable: 0,
                multisample_enable: 0,
                antialiased_line_enable: 0,
            };
            let mut p_rs: *mut c_void = std::ptr::null_mut();
            let hr_rs = create_rs(dev.as_ptr(), &rs_desc, &mut p_rs);
            if hr_rs < 0 || p_rs.is_null() {
                return Err(format!("CreateRasterizerState failed: 0x{:08X}", hr_rs as u32));
            }
            let rasterizer = ComPtr::from_raw(p_rs).ok_or("null ID3D11RasterizerState")?;

            let create_bs: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11BlendDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(20));
            let rt_blend = D3d11RenderTargetBlendDesc {
                blend_enable: 0,
                src_blend: D3D11_BLEND_ONE,
                dest_blend: D3D11_BLEND_ZERO,
                blend_op: D3D11_BLEND_OP_ADD,
                src_blend_alpha: D3D11_BLEND_ONE,
                dest_blend_alpha: D3D11_BLEND_ZERO,
                blend_op_alpha: D3D11_BLEND_OP_ADD,
                render_target_write_mask: D3D11_COLOR_WRITE_ENABLE_ALL,
            };
            let bs_desc = D3d11BlendDesc {
                alpha_to_coverage_enable: 0,
                independent_blend_enable: 0,
                render_target: [rt_blend; 8],
            };
            let mut p_bs: *mut c_void = std::ptr::null_mut();
            let hr_bs = create_bs(dev.as_ptr(), &bs_desc, &mut p_bs);
            if hr_bs < 0 || p_bs.is_null() {
                return Err(format!("CreateBlendState failed: 0x{:08X}", hr_bs as u32));
            }
            let blend = ComPtr::from_raw(p_bs).ok_or("null ID3D11BlendState")?;

            let create_ds: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11DepthStencilDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(21));
            let ds_op = D3d11DepthStencilOpDesc {
                stencil_fail_op: D3D11_STENCIL_OP_KEEP,
                stencil_depth_fail_op: D3D11_STENCIL_OP_KEEP,
                stencil_pass_op: D3D11_STENCIL_OP_KEEP,
                stencil_func: D3D11_COMPARISON_ALWAYS,
            };
            let ds_desc = D3d11DepthStencilDesc {
                depth_enable: 0,
                depth_write_mask: D3D11_DEPTH_WRITE_MASK_ZERO,
                depth_func: D3D11_COMPARISON_ALWAYS,
                stencil_enable: 0,
                stencil_read_mask: 0xFF,
                stencil_write_mask: 0xFF,
                front_face: ds_op,
                back_face: ds_op,
            };
            let mut p_ds: *mut c_void = std::ptr::null_mut();
            let hr_ds = create_ds(dev.as_ptr(), &ds_desc, &mut p_ds);
            if hr_ds < 0 || p_ds.is_null() {
                return Err(format!("CreateDepthStencilState failed: 0x{:08X}", hr_ds as u32));
            }
            let depth_stencil = ComPtr::from_raw(p_ds).ok_or("null ID3D11DepthStencilState")?;

            Ok(Self {
                device_ptr: dev.as_ptr() as usize,
                vs,
                ps,
                sampler,
                cbuf,
                rasterizer,
                blend,
                depth_stencil,
                srv_cache: Vec::with_capacity(MAX_CACHED_NV12_SRVS),
            })
        }
    }

    pub fn clear_srv_cache(&mut self) {
        self.srv_cache.clear();
    }

    pub fn srv_cache_len(&self) -> usize {
        self.srv_cache.len()
    }

    fn get_or_create_srvs(
        &mut self,
        gpu: &LiveD3d11Gpu,
        nv12_tex: &ComPtr,
        tex_w: u32,
        tex_h: u32,
    ) -> Result<(ComPtr, ComPtr), String> {
        let ptr_key = nv12_tex.as_ptr() as usize;
        if let Some(pos) = self.srv_cache.iter().position(|e| e.tex_ptr == ptr_key) {
            if self.srv_cache[pos].tex_w == tex_w && self.srv_cache[pos].tex_h == tex_h {
                return Ok((
                    self.srv_cache[pos].srv_y.clone(),
                    self.srv_cache[pos].srv_uv.clone(),
                ));
            }
            self.srv_cache.remove(pos);
        }

        let (srv_y, srv_uv) = gpu
            .create_nv12_y_uv_srvs(nv12_tex)
            .map_err(|hr| format!("create_nv12_y_uv_srvs failed: 0x{:08X}", hr as u32))?;
        if self.srv_cache.len() >= MAX_CACHED_NV12_SRVS {
            self.srv_cache.remove(0);
        }
        self.srv_cache.push(CachedNv12Srvs {
            tex_ptr: ptr_key,
            tex_w,
            tex_h,
            srv_y: srv_y.clone(),
            srv_uv: srv_uv.clone(),
        });
        Ok((srv_y, srv_uv))
    }

    /// Renders `nv12_tex` directly to an existing `ID3D11RenderTargetView` with zero CPU copies.
    /// Immediately unbinds SRVs and RTV before returning so DXGI `ResizeBuffers` is never blocked!
    #[allow(clippy::too_many_arguments)]
    pub fn render_to_rtv(
        &mut self,
        gpu: &LiveD3d11Gpu,
        nv12_tex: &ComPtr,
        rtv: &ComPtr,
        tex_w: u32,
        tex_h: u32,
        src_rect: Rect,
        dst_rect: Rect,
        rotation_deg: u16,
        color_info: VideoColorInfo,
    ) -> Result<(), String> {
        self.render_to_rtv_with_sharpen(
            gpu,
            nv12_tex,
            rtv,
            tex_w,
            tex_h,
            src_rect,
            dst_rect,
            rotation_deg,
            color_info,
            0.0,
        )
    }

    /// Renders `nv12_tex` directly to an `ID3D11RenderTargetView` with configurable zero-extra-tap
    /// Luma-Only CAS (`0.0` for AirPlay H.264, `0.28` calibrated for CoreDevice HEVC downscales).
    #[allow(clippy::too_many_arguments)]
    pub fn render_to_rtv_with_sharpen(
        &mut self,
        gpu: &LiveD3d11Gpu,
        nv12_tex: &ComPtr,
        rtv: &ComPtr,
        tex_w: u32,
        tex_h: u32,
        src_rect: Rect,
        dst_rect: Rect,
        rotation_deg: u16,
        color_info: VideoColorInfo,
        sharpen_strength: f32,
    ) -> Result<(), String> {
        if gpu.device().as_ptr() as usize != self.device_ptr {
            return Err("HdNv12ShaderRenderer device mismatch".to_string());
        }
        let dst_w = (dst_rect.right - dst_rect.left).max(1) as u32;
        let dst_h = (dst_rect.bottom - dst_rect.top).max(1) as u32;
        let tw = tex_w.max(2);
        let th = tex_h.max(2);

        let (srv_y, srv_uv) = self.get_or_create_srvs(gpu, nv12_tex, tw, th)?;

        let params = HdNv12ShaderParams {
            src_rect: [
                src_rect.left as f32,
                src_rect.top as f32,
                src_rect.right.max(src_rect.left + 1) as f32,
                src_rect.bottom.max(src_rect.top + 1) as f32,
            ],
            tex_size: [tw as f32, th as f32],
            dst_size: [dst_w as f32, dst_h as f32],
            rotation_mode: rotation_deg_to_mode(rotation_deg),
            color_flags: video_color_info_to_shader_flags(color_info),
            sharpen_strength: sharpen_strength.clamp(0.0, 0.50),
            pad: 0,
        };

        let ctx = gpu.context();
        let mapped: D3d11MappedSubresource = gpu
            .map_texture(&self.cbuf, 0, D3D11_MAP_WRITE_DISCARD)
            .map_err(|hr| format!("Map(cbuf) failed: 0x{:08X}", hr as u32))?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                &params as *const HdNv12ShaderParams as *const u8,
                mapped.p_data as *mut u8,
                std::mem::size_of::<HdNv12ShaderParams>(),
            );
        }
        gpu.unmap_texture(&self.cbuf, 0);

        unsafe {
            let vt = ctx.vtable();
            let ps_set_srv: unsafe extern "system" fn(*mut c_void, u32, u32, *const *mut c_void) =
                std::mem::transmute(*vt.add(8));
            let ps_set_shader: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const *mut c_void,
                u32,
            ) = std::mem::transmute(*vt.add(9));
            let ps_set_smp: unsafe extern "system" fn(*mut c_void, u32, u32, *const *mut c_void) =
                std::mem::transmute(*vt.add(10));
            let vs_set_shader: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const *mut c_void,
                u32,
            ) = std::mem::transmute(*vt.add(11));
            let draw: unsafe extern "system" fn(*mut c_void, u32, u32) =
                std::mem::transmute(*vt.add(13));
            let ps_set_cbuf: unsafe extern "system" fn(*mut c_void, u32, u32, *const *mut c_void) =
                std::mem::transmute(*vt.add(16));
            let ia_set_layout: unsafe extern "system" fn(*mut c_void, *mut c_void) =
                std::mem::transmute(*vt.add(17));
            let ia_set_topo: unsafe extern "system" fn(*mut c_void, u32) =
                std::mem::transmute(*vt.add(24));
            let om_set_rt: unsafe extern "system" fn(
                *mut c_void,
                u32,
                *const *mut c_void,
                *mut c_void,
            ) = std::mem::transmute(*vt.add(33));
            let om_set_blend: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *const f32,
                u32,
            ) = std::mem::transmute(*vt.add(35));
            let om_set_ds: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) =
                std::mem::transmute(*vt.add(36));
            let rs_set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) =
                std::mem::transmute(*vt.add(43));
            let rs_set_vp: unsafe extern "system" fn(*mut c_void, u32, *const D3d11Viewport) =
                std::mem::transmute(*vt.add(44));

            let vp = D3d11Viewport {
                top_left_x: dst_rect.left as f32,
                top_left_y: dst_rect.top as f32,
                width: dst_w as f32,
                height: dst_h as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };

            let rtvs = [rtv.as_ptr()];
            let cbufs = [self.cbuf.as_ptr()];
            let srvs = [srv_y.as_ptr(), srv_uv.as_ptr()];
            let smps = [self.sampler.as_ptr()];

            ia_set_layout(ctx.as_ptr(), std::ptr::null_mut());
            ia_set_topo(ctx.as_ptr(), D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            vs_set_shader(ctx.as_ptr(), self.vs.as_ptr(), std::ptr::null(), 0);
            ps_set_shader(ctx.as_ptr(), self.ps.as_ptr(), std::ptr::null(), 0);
            ps_set_cbuf(ctx.as_ptr(), 0, 1, cbufs.as_ptr());
            ps_set_srv(ctx.as_ptr(), 0, 2, srvs.as_ptr());
            ps_set_smp(ctx.as_ptr(), 0, 1, smps.as_ptr());
            rs_set_state(ctx.as_ptr(), self.rasterizer.as_ptr());
            rs_set_vp(ctx.as_ptr(), 1, &vp);
            om_set_blend(ctx.as_ptr(), self.blend.as_ptr(), std::ptr::null(), 0xFFFF_FFFF);
            om_set_ds(ctx.as_ptr(), self.depth_stencil.as_ptr(), 0);
            om_set_rt(ctx.as_ptr(), 1, rtvs.as_ptr(), std::ptr::null_mut());

            draw(ctx.as_ptr(), 3, 0);

            // Mandatory clean unbind before Present / ResizeBuffers!
            let null_srvs: [*mut c_void; 2] = [std::ptr::null_mut(), std::ptr::null_mut()];
            let null_rtvs: [*mut c_void; 1] = [std::ptr::null_mut()];
            ps_set_srv(ctx.as_ptr(), 0, 2, null_srvs.as_ptr());
            om_set_rt(ctx.as_ptr(), 1, null_rtvs.as_ptr(), std::ptr::null_mut());
        }

        let _ = D3d11Box::default();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_to_texture(
        &mut self,
        gpu: &LiveD3d11Gpu,
        nv12_tex: &ComPtr,
        dst_bgra_tex: &ComPtr,
        tex_w: u32,
        tex_h: u32,
        src_rect: Rect,
        dst_rect: Rect,
        rotation_deg: u16,
        color_info: VideoColorInfo,
    ) -> Result<(), String> {
        let rtv = gpu
            .create_rtv(dst_bgra_tex)
            .map_err(|hr| format!("create_rtv(dst_bgra) failed: 0x{:08X}", hr as u32))?;
        self.render_to_rtv(
            gpu, nv12_tex, &rtv, tex_w, tex_h, src_rect, dst_rect, rotation_deg, color_info,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d11_context::{
        D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_DEFAULT,
        D3D11_USAGE_STAGING,
    };
    use crate::mf_decoder::write_nv12_from_rgb_sampler;

    fn upload_raw_nv12_to_default_tex(gpu: &LiveD3d11Gpu, w: u32, h: u32, nv12: &[u8]) -> ComPtr {
        let up = gpu.create_nv12_texture(w, h, true).expect("dynamic nv12");
        let dst = gpu.create_nv12_texture(w, h, false).expect("default nv12");
        let mp = gpu
            .map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD)
            .expect("map dynamic nv12");
        let rows = (h as usize) + (h as usize) / 2;
        let cp = (w as usize).min(mp.row_pitch as usize);
        unsafe {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(r * w as usize),
                    (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                    cp,
                );
            }
        }
        gpu.unmap_texture(&up, 0);
        gpu.copy_resource(&dst, &up);
        dst
    }

    fn build_uniform_nv12(w: u32, h: u32, y_val: u8, u_val: u8, v_val: u8) -> Vec<u8> {
        let y_len = (w * h) as usize;
        let uv_len = y_len / 2;
        let mut out = vec![y_val; y_len + uv_len];
        for i in (0..uv_len).step_by(2) {
            out[y_len + i] = u_val;
            out[y_len + i + 1] = v_val;
        }
        out
    }

    fn render_and_readback_bgra_with_sharpen(
        gpu: &LiveD3d11Gpu,
        sh: &mut HdNv12ShaderRenderer,
        nv12_tex: &ComPtr,
        tex_w: u32,
        tex_h: u32,
        src_rect: Rect,
        out_w: u32,
        out_h: u32,
        rot_deg: u16,
        color: VideoColorInfo,
        sharpen: f32,
    ) -> Vec<u8> {
        let rt = gpu
            .create_bgra_texture(out_w, out_h, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET, 0)
            .expect("bgra rt");
        let rtv = gpu.create_rtv(&rt).expect("rtv");
        gpu.clear_rtv(&rtv, [0.0, 0.0, 0.0, 1.0]);
        sh.render_to_rtv_with_sharpen(
            gpu,
            nv12_tex,
            &rtv,
            tex_w,
            tex_h,
            src_rect,
            Rect {
                left: 0,
                top: 0,
                right: out_w as i32,
                bottom: out_h as i32,
            },
            rot_deg,
            color,
            sharpen,
        )
        .expect("render_to_rtv_with_sharpen");
        drop(rtv);

        let stg = gpu
            .create_bgra_texture(out_w, out_h, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ)
            .expect("staging bgra");
        gpu.copy_resource(&stg, &rt);
        let mp = gpu.map_texture(&stg, 0, D3D11_MAP_READ).expect("map read");
        let mut pixels = vec![0u8; (out_w * out_h * 4) as usize];
        unsafe {
            for r in 0..out_h as usize {
                let sp = (mp.p_data as *const u8).add(r * mp.row_pitch as usize);
                let dp = &mut pixels[r * (out_w as usize) * 4..(r + 1) * (out_w as usize) * 4];
                std::ptr::copy_nonoverlapping(sp, dp.as_mut_ptr(), (out_w as usize) * 4);
            }
        }
        gpu.unmap_texture(&stg, 0);
        pixels
    }

    fn render_and_readback_bgra(
        gpu: &LiveD3d11Gpu,
        sh: &mut HdNv12ShaderRenderer,
        nv12_tex: &ComPtr,
        tex_w: u32,
        tex_h: u32,
        src_rect: Rect,
        out_w: u32,
        out_h: u32,
        rot_deg: u16,
        color: VideoColorInfo,
    ) -> Vec<u8> {
        render_and_readback_bgra_with_sharpen(
            gpu, sh, nv12_tex, tex_w, tex_h, src_rect, out_w, out_h, rot_deg, color, 0.0,
        )
    }

    #[inline]
    fn px_rgb(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8) {
        let off = ((y * w + x) * 4) as usize;
        (buf[off + 2], buf[off + 1], buf[off])
    }

    #[test]
    fn test_tahap280_d3d11_hd_nv12_shader_compiles_and_binds_real_nv12_srv() {
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let (vs, ps) = compile_hd_nv12_shaders_cached().expect("d3dcompiler_47 compile");
        assert!(!vs.is_empty() && !ps.is_empty());

        let sh = HdNv12ShaderRenderer::new(&gpu).expect("HdNv12ShaderRenderer::new");
        assert_eq!(sh.srv_cache_len(), 0);

        let nv12 = build_uniform_nv12(64, 64, 128, 128, 128);
        let tex = upload_raw_nv12_to_default_tex(&gpu, 64, 64, &nv12);
        let (srv_y, srv_uv) = gpu.create_nv12_y_uv_srvs(&tex).expect("real NV12 R8+R8G8 SRVs");
        assert!(!srv_y.as_ptr().is_null());
        assert!(!srv_uv.as_ptr().is_null());
    }

    #[test]
    fn test_tahap280_bt709_studio_to_full_srgb_color_accuracy_zero_washout() {
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let mut sh = HdNv12ShaderRenderer::new(&gpu).expect("HdNv12ShaderRenderer");

        let studio = VideoColorInfo {
            matrix: VideoColorMatrix::Bt709,
            full_range: false,
        };
        let rect = Rect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };

        // 1. Studio Black (Y=16, UV=128) -> (0, 0, 0) +/- 2 (ZERO washout!)
        let blk_tex = upload_raw_nv12_to_default_tex(&gpu, 32, 32, &build_uniform_nv12(32, 32, 16, 128, 128));
        let blk_out = render_and_readback_bgra(&gpu, &mut sh, &blk_tex, 32, 32, rect, 32, 32, 0, studio);
        let (br, bg, bb) = px_rgb(&blk_out, 32, 16, 16);
        assert!(br <= 2 && bg <= 2 && bb <= 2, "Studio black washed out: ({br},{bg},{bb})");

        // 2. Studio White (Y=235, UV=128) -> (255, 255, 255) +/- 2
        let wht_tex = upload_raw_nv12_to_default_tex(&gpu, 32, 32, &build_uniform_nv12(32, 32, 235, 128, 128));
        let wht_out = render_and_readback_bgra(&gpu, &mut sh, &wht_tex, 32, 32, rect, 32, 32, 0, studio);
        let (wr, wg, wb) = px_rgb(&wht_out, 32, 16, 16);
        assert!(wr >= 253 && wg >= 253 && wb >= 253, "Studio white dim: ({wr},{wg},{wb})");

        // 3. BT.709 Studio Primary Red, Green, Blue (ITU-R BT.709 limited)
        for &(tr, tg, tb) in &[(255u8, 0u8, 0u8), (0, 255, 0), (0, 0, 255)] {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(32, 32, &mut nv12, |_, _| (tr, tg, tb));
            let tex = upload_raw_nv12_to_default_tex(&gpu, 32, 32, &nv12);
            let out = render_and_readback_bgra(&gpu, &mut sh, &tex, 32, 32, rect, 32, 32, 0, studio);
            let (ar, ag, ab) = px_rgb(&out, 32, 16, 16);
            assert!(
                (ar as i32 - tr as i32).abs() <= 6
                    && (ag as i32 - tg as i32).abs() <= 6
                    && (ab as i32 - tb as i32).abs() <= 6,
                "BT.709 primary mismatch target=({tr},{tg},{tb}) got=({ar},{ag},{ab})"
            );
        }

        // 4. Full Range BT.709 (Y=0 -> 0, Y=255 -> 255)
        let full = VideoColorInfo {
            matrix: VideoColorMatrix::Bt709,
            full_range: true,
        };
        let f_blk_tex = upload_raw_nv12_to_default_tex(&gpu, 32, 32, &build_uniform_nv12(32, 32, 0, 128, 128));
        let f_blk = render_and_readback_bgra(&gpu, &mut sh, &f_blk_tex, 32, 32, rect, 32, 32, 0, full);
        assert_eq!(px_rgb(&f_blk, 32, 16, 16), (0, 0, 0));
    }

    #[test]
    fn test_tahap280_four_way_rotation_0_90cw_180_270cw_exact_corner_mapping() {
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let mut sh = HdNv12ShaderRenderer::new(&gpu).expect("HdNv12ShaderRenderer");

        // 64x64 4-quadrant NV12 source: TL=Red, TR=Green, BL=Blue, BR=Yellow
        let (w, h) = (64u32, 64u32);
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12, |x, y| match (x < 32, y < 32) {
            (true, true) => (255, 0, 0),     // TL = Red
            (false, true) => (0, 255, 0),    // TR = Green
            (true, false) => (0, 0, 255),    // BL = Blue
            (false, false) => (255, 255, 0), // BR = Yellow
        });
        let tex = upload_raw_nv12_to_default_tex(&gpu, w, h, &nv12);
        let rect = Rect {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        let col = VideoColorInfo::default();

        let is_red = |rgb: (u8, u8, u8)| rgb.0 > 220 && rgb.1 < 35 && rgb.2 < 35;
        let is_green = |rgb: (u8, u8, u8)| rgb.1 > 220 && rgb.0 < 35 && rgb.2 < 35;
        let is_blue = |rgb: (u8, u8, u8)| rgb.2 > 220 && rgb.0 < 35 && rgb.1 < 35;
        let is_yellow = |rgb: (u8, u8, u8)| rgb.0 > 220 && rgb.1 > 220 && rgb.2 < 35;

        // 0°: TL=Red, TR=Green, BL=Blue, BR=Yellow
        let r0 = render_and_readback_bgra(&gpu, &mut sh, &tex, w, h, rect, 64, 64, 0, col);
        assert!(is_red(px_rgb(&r0, 64, 10, 10)), "0° TL={:?}", px_rgb(&r0, 64, 10, 10));
        assert!(is_green(px_rgb(&r0, 64, 54, 10)), "0° TR={:?}", px_rgb(&r0, 64, 54, 10));
        assert!(is_blue(px_rgb(&r0, 64, 10, 54)), "0° BL={:?}", px_rgb(&r0, 64, 10, 54));
        assert!(is_yellow(px_rgb(&r0, 64, 54, 54)), "0° BR={:?}", px_rgb(&r0, 64, 54, 54));

        // 90° CW: TL=Blue, TR=Red, BR=Green, BL=Yellow
        let r90 = render_and_readback_bgra(&gpu, &mut sh, &tex, w, h, rect, 64, 64, 90, col);
        assert!(is_blue(px_rgb(&r90, 64, 10, 10)), "90° TL={:?}", px_rgb(&r90, 64, 10, 10));
        assert!(is_red(px_rgb(&r90, 64, 54, 10)), "90° TR={:?}", px_rgb(&r90, 64, 54, 10));
        assert!(is_green(px_rgb(&r90, 64, 54, 54)), "90° BR={:?}", px_rgb(&r90, 64, 54, 54));
        assert!(is_yellow(px_rgb(&r90, 64, 10, 54)), "90° BL={:?}", px_rgb(&r90, 64, 10, 54));

        // 180°: TL=Yellow, TR=Blue, BL=Green, BR=Red
        let r180 = render_and_readback_bgra(&gpu, &mut sh, &tex, w, h, rect, 64, 64, 180, col);
        assert!(is_yellow(px_rgb(&r180, 64, 10, 10)), "180° TL={:?}", px_rgb(&r180, 64, 10, 10));
        assert!(is_blue(px_rgb(&r180, 64, 54, 10)), "180° TR={:?}", px_rgb(&r180, 64, 54, 10));
        assert!(is_green(px_rgb(&r180, 64, 10, 54)), "180° BL={:?}", px_rgb(&r180, 64, 10, 54));
        assert!(is_red(px_rgb(&r180, 64, 54, 54)), "180° BR={:?}", px_rgb(&r180, 64, 54, 54));

        // 270° CW (= 90° CCW): TL=Green, TR=Yellow, BR=Blue, BL=Red
        let r270 = render_and_readback_bgra(&gpu, &mut sh, &tex, w, h, rect, 64, 64, 270, col);
        assert!(is_green(px_rgb(&r270, 64, 10, 10)), "270° TL={:?}", px_rgb(&r270, 64, 10, 10));
        assert!(is_yellow(px_rgb(&r270, 64, 54, 10)), "270° TR={:?}", px_rgb(&r270, 64, 54, 10));
        assert!(is_blue(px_rgb(&r270, 64, 54, 54)), "270° BR={:?}", px_rgb(&r270, 64, 54, 54));
        assert!(is_red(px_rgb(&r270, 64, 10, 54)), "270° BL={:?}", px_rgb(&r270, 64, 10, 54));
    }

    #[test]
    fn test_tahap280_non_mod16_994x2160_inside_1008x2160_zero_padding_bleed() {
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let mut sh = HdNv12ShaderRenderer::new(&gpu).expect("HdNv12ShaderRenderer");

        // 1008x2160 coded NV12 where clean region 0..994 is pure white and padding 994..1008 is bright green/magenta!
        let (coded_w, coded_h, vis_w, vis_h) = (1008u32, 2160u32, 994u32, 2160u32);
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(coded_w, coded_h, &mut nv12, |x, y| {
            if x < vis_w {
                (255, 255, 255)
            } else if (y & 4) == 0 {
                (0, 255, 0)
            } else {
                (255, 0, 255)
            }
        });
        let tex = upload_raw_nv12_to_default_tex(&gpu, coded_w, coded_h, &nv12);
        let src_rect = Rect {
            left: 0,
            top: 0,
            right: vis_w as i32,
            bottom: vis_h as i32,
        };
        let out = render_and_readback_bgra(
            &gpu,
            &mut sh,
            &tex,
            coded_w,
            coded_h,
            src_rect,
            519,
            1125,
            0,
            VideoColorInfo::default(),
        );

        // Check rightmost output columns (x = 517, 518) across full vertical height -> 0% padding bleed!
        for y in (0..1125u32).step_by(17) {
            for x in [517u32, 518u32] {
                let (r, g, b) = px_rgb(&out, 519, x, y);
                assert!(
                    r >= 248 && g >= 248 && b >= 248,
                    "Non-mod16 padding bleed at ({x},{y}): RGB=({r},{g},{b})"
                );
            }
        }
    }

    #[test]
    fn test_tahap280_adaptive_2_34x_downscale_preserves_1px_hairlines_without_aliasing_drop() {
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let mut sh = HdNv12ShaderRenderer::new(&gpu).expect("HdNv12ShaderRenderer");

        // 1216x2656 frame with 1px white vertical hairlines AND 1px white horizontal hairlines across varied subpixel phases
        let (w, h) = (1216u32, 2656u32);
        let v_lines = [100u32, 120, 141, 163, 184, 205, 226, 247, 268, 289];
        let h_lines = [400u32, 420, 441, 463, 484, 505, 526, 547, 568, 589];
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12, |x, y| {
            let is_v = y >= 50 && y <= 350 && v_lines.contains(&x);
            let is_h = x >= 400 && x <= 800 && h_lines.contains(&y);
            if is_v || is_h {
                (255, 255, 255)
            } else {
                (0, 0, 0)
            }
        });
        let tex = upload_raw_nv12_to_default_tex(&gpu, w, h, &nv12);
        let src_rect = Rect {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        let col = VideoColorInfo::default();

        // 1. 2.34x downscale (150% DPI window: 1216x2656 -> 519x1125)
        let out_150 = render_and_readback_bgra(&gpu, &mut sh, &tex, w, h, src_rect, 519, 1125, 0, col);
        let scale_x = w as f32 / 519.0;
        let scale_y = h as f32 / 1125.0;
        let v_peaks: Vec<u8> = v_lines
            .iter()
            .map(|&lx| {
                let cx = ((lx as f32 + 0.5) / scale_x - 0.5).round() as i32;
                let mut best = 0u8;
                for dx in -1..=1 {
                    let x = (cx + dx).clamp(0, 518) as u32;
                    best = best.max(px_rgb(&out_150, 519, x, 85).0);
                }
                best
            })
            .collect();
        let h_peaks: Vec<u8> = h_lines
            .iter()
            .map(|&ly| {
                let cy = ((ly as f32 + 0.5) / scale_y - 0.5).round() as i32;
                let mut best = 0u8;
                for dy in -1..=1 {
                    let y = (cy + dy).clamp(0, 1124) as u32;
                    best = best.max(px_rgb(&out_150, 519, 256, y).0);
                }
                best
            })
            .collect();
        let vmin = *v_peaks.iter().min().unwrap();
        let vmax = *v_peaks.iter().max().unwrap();
        let hmin = *h_peaks.iter().min().unwrap();
        let hmax = *h_peaks.iter().max().unwrap();
        assert!(
            vmin >= 55 && (vmin as f32 / vmax.max(1) as f32) >= 0.55,
            "2.34x vertical 1px hairlines aliased/dropped: {v_peaks:?}"
        );
        assert!(
            hmin >= 55 && (hmin as f32 / hmax.max(1) as f32) >= 0.55,
            "2.34x horizontal 1px hairlines aliased/dropped: {h_peaks:?}"
        );

        // 2. 3.51x downscale (100% DPI window: 1216x2656 -> 346x750)
        let out_100 = render_and_readback_bgra(&gpu, &mut sh, &tex, w, h, src_rect, 346, 750, 0, col);
        let scale_x100 = w as f32 / 346.0;
        let v_peaks100: Vec<u8> = v_lines
            .iter()
            .map(|&lx| {
                let cx = ((lx as f32 + 0.5) / scale_x100 - 0.5).round() as i32;
                let mut best = 0u8;
                for dx in -1..=1 {
                    let x = (cx + dx).clamp(0, 345) as u32;
                    best = best.max(px_rgb(&out_100, 346, x, 55).0);
                }
                best
            })
            .collect();
        let vmin100 = *v_peaks100.iter().min().unwrap();
        let vmax100 = *v_peaks100.iter().max().unwrap();
        assert!(
            vmin100 >= 40 && (vmin100 as f32 / vmax100.max(1) as f32) >= 0.65,
            "3.51x vertical 1px hairlines aliased/dropped: {v_peaks100:?}"
        );
    }

    #[test]
    fn test_tahap281_luma_only_cas_lifts_text_contrast_zero_color_shift_zero_halo() {
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let mut sh = HdNv12ShaderRenderer::new(&gpu).expect("HdNv12ShaderRenderer");
        let studio = VideoColorInfo {
            matrix: VideoColorMatrix::Bt709,
            full_range: false,
        };
        let rect32 = Rect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        let (w, h) = (1216u32, 2656u32);
        let full_rect = Rect {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };

        // 1. Flat BT.709 Studio Black/White & Primaries under sharpen=0.28 -> ZERO washout & ZERO color shift
        let blk_tex = upload_raw_nv12_to_default_tex(&gpu, 32, 32, &build_uniform_nv12(32, 32, 16, 128, 128));
        let blk_out = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &blk_tex, 32, 32, rect32, 32, 32, 0, studio, 0.28);
        let (br, bg, bb) = px_rgb(&blk_out, 32, 16, 16);
        assert!(br <= 2 && bg <= 2 && bb <= 2, "CAS Studio black drifted: ({br},{bg},{bb})");

        let wht_tex = upload_raw_nv12_to_default_tex(&gpu, 32, 32, &build_uniform_nv12(32, 32, 235, 128, 128));
        let wht_out = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &wht_tex, 32, 32, rect32, 32, 32, 0, studio, 0.28);
        let (wr, wg, wb) = px_rgb(&wht_out, 32, 16, 16);
        assert!(wr >= 253 && wg >= 253 && wb >= 253, "CAS Studio white drifted: ({wr},{wg},{wb})");

        for &(tr, tg, tb) in &[(255u8, 0u8, 0u8), (0, 255, 0), (0, 0, 255)] {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(64, 64, &mut nv12, |_, _| (tr, tg, tb));
            let tex = upload_raw_nv12_to_default_tex(&gpu, 64, 64, &nv12);
            let r64 = Rect { left: 0, top: 0, right: 64, bottom: 64 };
            // Also test 2.34x downscale branch on flat color
            let out = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &tex, 64, 64, r64, 27, 27, 0, studio, 0.28);
            let (ar, ag, ab) = px_rgb(&out, 27, 13, 13);
            assert!(
                (ar as i32 - tr as i32).abs() <= 6
                    && (ag as i32 - tg as i32).abs() <= 6
                    && (ab as i32 - tb as i32).abs() <= 6,
                "CAS BT.709 primary mismatch target=({tr},{tg},{tb}) got=({ar},{ag},{ab})"
            );
        }

        // 2. 1px hairlines at 2.34x (519x1125) and 3.51x (346x750) stay anti-aliased & uniform with sharpen=0.28
        let v_lines = [100u32, 120, 141, 163, 184, 205, 226, 247, 268, 289];
        let mut nv12_hair = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12_hair, |x, y| {
            if y >= 50 && y <= 350 && v_lines.contains(&x) {
                (255, 255, 255)
            } else {
                (0, 0, 0)
            }
        });
        let tex_hair = upload_raw_nv12_to_default_tex(&gpu, w, h, &nv12_hair);
        let out_234 = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &tex_hair, w, h, full_rect, 519, 1125, 0, studio, 0.28);
        let sx234 = w as f32 / 519.0;
        let peaks234: Vec<u8> = v_lines
            .iter()
            .map(|&lx| {
                let cx = ((lx as f32 + 0.5) / sx234 - 0.5).round() as i32;
                (-1..=1)
                    .map(|dx| px_rgb(&out_234, 519, (cx + dx).clamp(0, 518) as u32, 85).0)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let vmin234 = *peaks234.iter().min().unwrap();
        let vmax234 = *peaks234.iter().max().unwrap();
        assert!(
            vmin234 >= 55 && (vmin234 as f32 / vmax234.max(1) as f32) >= 0.55,
            "CAS 2.34x 1px hairlines failed threshold: {peaks234:?}"
        );

        let out_351 = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &tex_hair, w, h, full_rect, 346, 750, 0, studio, 0.28);
        let sx351 = w as f32 / 346.0;
        let peaks351: Vec<u8> = v_lines
            .iter()
            .map(|&lx| {
                let cx = ((lx as f32 + 0.5) / sx351 - 0.5).round() as i32;
                (-1..=1)
                    .map(|dx| px_rgb(&out_351, 346, (cx + dx).clamp(0, 345) as u32, 55).0)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let vmin351 = *peaks351.iter().min().unwrap();
        let vmax351 = *peaks351.iter().max().unwrap();
        assert!(
            vmin351 >= 40 && (vmin351 as f32 / vmax351.max(1) as f32) >= 0.65,
            "CAS 3.51x 1px hairlines failed threshold: {peaks351:?}"
        );

        // 3. Softened HEVC text strokes (2.34x): CAS=0.28 MUST measurably lift stroke contrast vs CAS=0.0 (>= +6%)
        let stroke_centers = [200u32, 220, 241, 263];
        let mut nv12_text = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12_text, |x, y| {
            let mut val = 0u8;
            if (100..=300).contains(&y) {
                for &cx0 in &stroke_centers {
                    let dx = x as i32 - cx0 as i32;
                    let v = match dx.abs() {
                        0 => 242,
                        1 => 166,
                        2 => 51,
                        3 => 13,
                        _ => 0,
                    };
                    if v > val {
                        val = v;
                    }
                }
            }
            (val, val, val)
        });
        let tex_text = upload_raw_nv12_to_default_tex(&gpu, w, h, &nv12_text);
        let base_out = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &tex_text, w, h, full_rect, 519, 1125, 0, studio, 0.0);
        let cas_out = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &tex_text, w, h, full_rect, 519, 1125, 0, studio, 0.28);
        let avg_peak = |buf: &[u8]| -> f32 {
            let sum: u32 = stroke_centers
                .iter()
                .map(|&lx| {
                    let cx = ((lx as f32 + 0.5) / sx234 - 0.5).round() as i32;
                    (-1..=1)
                        .map(|dx| px_rgb(buf, 519, (cx + dx).clamp(0, 518) as u32, 85).0 as u32)
                        .max()
                        .unwrap_or(0)
                })
                .sum();
            sum as f32 / stroke_centers.len() as f32
        };
        let base_text_avg = avg_peak(&base_out);
        let cas_text_avg = avg_peak(&cas_out);
        let lift_pct = ((cas_text_avg - base_text_avg) / base_text_avg.max(1.0)) * 100.0;
        assert!(
            cas_text_avg >= base_text_avg * 1.06,
            "Luma CAS 0.28 must lift 2.34x text contrast >= +6%: base={base_text_avg:.2} cas={cas_text_avg:.2} (+{lift_pct:.2}%)"
        );

        // 4. Hard gray step edge 51 -> 204 at x=608 (2.34x): overshoot/undershoot halo strictly <= 8%
        let mut nv12_step = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12_step, |x, _| {
            if x < 608 {
                (51, 51, 51)
            } else {
                (204, 204, 204)
            }
        });
        let tex_step = upload_raw_nv12_to_default_tex(&gpu, w, h, &nv12_step);
        let step_out = render_and_readback_bgra_with_sharpen(&gpu, &mut sh, &tex_step, w, h, full_rect, 519, 1125, 0, studio, 0.28);
        let mut max_over = 0i32;
        let mut max_under = 0i32;
        for ox in 240..=280u32 {
            let (r, _, _) = px_rgb(&step_out, 519, ox, 200);
            max_over = max_over.max(r as i32 - 204);
            max_under = max_under.max(51 - r as i32);
        }
        let halo_pct = (max_over.max(max_under).max(0) as f32 / 153.0) * 100.0;
        assert!(
            halo_pct <= 8.0,
            "CAS 0.28 halo overshoot exceeded 8%: over={max_over}, under={max_under}, halo={halo_pct:.2}%"
        );

        eprintln!(
            "TAHAP281_GPU_CAS_METRICS base_text={:.2} cas_text={:.2} lift_pct={:.2} halo_pct={:.2} hair234_min={} hair351_min={}",
            base_text_avg, cas_text_avg, lift_pct, halo_pct, vmin234, vmin351
        );
    }
}
