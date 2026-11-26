//! Video pipeline for iPhone Link: Direct3D 11 hardware context, Media Foundation decoder,
//! auto-rotation, edge cropping, and Liquid Glass shaders.

pub mod autorotate;
pub mod d3d11_context;
pub mod decoder;
pub mod mf_decoder;
pub mod nv12_hd_shader;
pub mod shader_glass;
pub mod video_hwnd;

pub use video_hwnd::{StageKeyCallback, StageKeyEvent, StagePointerCallback, StagePointerEvent};
