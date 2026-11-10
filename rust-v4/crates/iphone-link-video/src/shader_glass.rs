//! WinUI 3 Liquid Glass HLSL shader, SDF bevel refraction, and color conversion matrices.

/// Raw HLSL shader code for Liquid Glass background refraction and Gaussian blur.
pub const LIQUID_GLASS_HLSL: &str = r#"
cbuffer P : register(b0)
{
    float2 winSize;
    float2 srcSize;
    float2 pad;
    float  sigma;
    float  dark;
    float4 dir;
    float4 light;
    int    count;
    float  lens;
    float  rimK;
    float  scale;
    float4 rc[8];
    float4 rp[8];
};

Texture2D src : register(t0);
SamplerState smp : register(s0);

struct VO { float4 pos : SV_Position; float2 uv : TEXCOORD0; };

VO vs(uint id : SV_VertexID)
{
    VO o;
    float2 t = float2((id << 1) & 2, id & 2);
    o.uv = t;
    o.pos = float4(t * float2(2, -2) + float2(-1, 1), 0, 1);
    return o;
}

float4 blur(VO i) : SV_Target
{
    float s = max(sigma, 0.01);
    int r = (int)min(ceil(s * 2.5), 24.0);
    float4 acc = 0;
    float ws = 0;
    for (int k = -r; k <= r; k++) {
        float w = exp(-(k * k) / (2 * s * s));
        acc += src.SampleLevel(smp, i.uv + dir.xy * k, 0) * w;
        ws += w;
    }
    return acc / ws;
}

float sdRR(float2 p, float2 h, float r)
{
    float2 q = abs(p) - h + r;
    return length(max(q, 0)) + min(max(q.x, q.y), 0) - r;
}

float2 sdNormal(float2 p, float2 h, float r)
{
    const float e = 0.5;
    float2 g = float2(sdRR(p + float2(e, 0), h, r) - sdRR(p - float2(e, 0), h, r),
                      sdRR(p + float2(0, e), h, r) - sdRR(p - float2(0, e), h, r));
    return g / max(length(g), 1e-5);
}

float refractShift(float slope, float eta)
{
    float th = atan(slope);
    return tan(th - asin(sin(th) / eta));
}

float4 comp(VO i) : SV_Target
{
    float2 p = i.uv * winSize;
    float2 off = 0;
    float rim = 0;
    float cov = 1;
    for (int k = 0; k < count; k++) {
        float2 c = rc[k].xy;
        float2 h = rc[k].zw;
        float r = rp[k].x;
        float bz = max(rp[k].y, 1);
        float st = rp[k].z;
        float d = sdRR(p - c, h, r);
        if (k == 0) cov = saturate(0.5 - d);
        if (d < 0.5 && d > -bz) {
            float u = saturate(-d / bz);
            float a = 1 - u;
            float slope = min(a / sqrt(max(1 - a * a, 1e-3)), 6.0);
            float2 n = sdNormal(p - c, h, r);
            off += n * refractShift(slope, 1.5) * bz * 0.55 * st * lens;
            float edge = exp(min(d, 0) / (1.4 * scale));
            float l = max(dot(n, light.xy), 0) + 0.35 * max(dot(n, -light.xy), 0);
            rim = max(rim, edge * l * st);
        }
    }
    float2 uv = (p + pad + off) / srcSize;
    float2 dc = off * 0.06 / srcSize;
    float3 col = float3(src.Sample(smp, uv + dc).r, src.Sample(smp, uv).g, src.Sample(smp, uv - dc).b);
    col += rim * rimK;
    return float4(col * cov, cov);
}
"#;

/// Raw HLSL shader code for NV12 video surface to BGRA8 unorm color conversion.
pub const NV12_TO_BGRA_HLSL: &str = r#"
Texture2D<float> lumaTex : register(t0);
Texture2D<float2> chromaTex : register(t1);
SamplerState smp : register(s0);

struct VS_OUTPUT {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD0;
};

// BT.709 Standard Color Matrix
static const float3x3 YUV2RGB = {
    1.164383,  0.000000,  1.792741,
    1.164383, -0.213249, -0.532909,
    1.164383,  2.112402,  0.000000
};

float4 main(VS_OUTPUT input) : SV_TARGET
{
    float y = lumaTex.Sample(smp, input.uv) - (16.0 / 255.0);
    float2 uv = chromaTex.Sample(smp, input.uv) - (128.0 / 255.0);
    float3 yuv = float3(y, uv.x, uv.y);
    float3 rgb = mul(YUV2RGB, yuv);
    return float4(saturate(rgb), 1.0);
}
"#;

/// Shape parameters for rounded rectangle SDF evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassShape {
    pub center_x: f32,
    pub center_y: f32,
    pub half_w: f32,
    pub half_h: f32,
    pub corner_radius: f32,
    pub bezel_width: f32,
    pub refraction_strength: f32,
}

impl Default for GlassShape {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_y: 0.0,
            half_w: 100.0,
            half_h: 200.0,
            corner_radius: 16.0,
            bezel_width: 8.0,
            refraction_strength: 1.0,
        }
    }
}

/// Pipeline configuration for Liquid Glass rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct GlassEffectConfig {
    pub blur_sigma: f32,
    pub lens_refraction: bool,
    pub dark_theme: bool,
    pub dpi_scale: f32,
    pub shapes: Vec<GlassShape>,
}

impl Default for GlassEffectConfig {
    fn default() -> Self {
        Self {
            blur_sigma: 12.0,
            lens_refraction: true,
            dark_theme: true,
            dpi_scale: 1.0,
            shapes: vec![GlassShape::default()],
        }
    }
}

/// Converts BT.709 YUV color components to RGB values in range 0.0 to 1.0.
pub fn bt709_yuv_to_rgb(y: f32, u: f32, v: f32) -> (f32, f32, f32) {
    let y_adj = (y - (16.0 / 255.0)).max(0.0);
    let u_adj = u - (128.0 / 255.0);
    let v_adj = v - (128.0 / 255.0);

    let r = 1.164383 * y_adj + 1.792741 * v_adj;
    let g = 1.164383 * y_adj - 0.213249 * u_adj - 0.532909 * v_adj;
    let b = 1.164383 * y_adj + 2.112402 * u_adj;

    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shader_sources_contain_expected_tokens() {
        assert!(LIQUID_GLASS_HLSL.contains("sdRR"));
        assert!(LIQUID_GLASS_HLSL.contains("refractShift"));
        assert!(LIQUID_GLASS_HLSL.contains("comp(VO i)"));
        assert!(NV12_TO_BGRA_HLSL.contains("YUV2RGB"));
        assert!(NV12_TO_BGRA_HLSL.contains("lumaTex"));
    }

    #[test]
    fn test_bt709_yuv_to_rgb_conversion() {
        // Black level: Y = 16/255, U = 128/255, V = 128/255 -> RGB (0, 0, 0)
        let (r, g, b) = bt709_yuv_to_rgb(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        assert!((r - 0.0).abs() < 1e-4);
        assert!((g - 0.0).abs() < 1e-4);
        assert!((b - 0.0).abs() < 1e-4);

        // White level: Y = 235/255, U = 128/255, V = 128/255 -> RGB (~1.0, ~1.0, ~1.0)
        let (r_w, g_w, b_w) = bt709_yuv_to_rgb(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        assert!((r_w - 1.0).abs() < 0.02);
        assert!((g_w - 1.0).abs() < 0.02);
        assert!((b_w - 1.0).abs() < 0.02);
    }

    #[test]
    fn test_glass_effect_config_default() {
        let cfg = GlassEffectConfig::default();
        assert_eq!(cfg.blur_sigma, 12.0);
        assert!(cfg.lens_refraction);
        assert!(cfg.dark_theme);
        assert_eq!(cfg.shapes.len(), 1);
    }
}
