//! Adaptive audio jitter buffer with volume scaling, soft-clipping limiter, and lip-sync offset.

use std::collections::VecDeque;
use crate::AudioBufferPreset;

pub const DEFAULT_SAMPLE_RATE: u32 = 48000;
pub const STEREO_CHANNELS: usize = 2;
pub const MIN_LIP_SYNC_OFFSET_MS: i32 = -250;
pub const MAX_LIP_SYNC_OFFSET_MS: i32 = 250;

/// Adaptive audio ring buffer designed for low-latency WASAPI rendering.
pub struct AdaptiveAudioBuffer {
    queue: VecDeque<i16>,
    preset: AudioBufferPreset,
    sample_rate: u32,
    channels: usize,
    volume_percent: u32,
    lip_sync_offset_ms: i32,
    underrun_count: u64,
    overrun_drop_count: u64,
    last_sample_left: i16,
    last_sample_right: i16,
}

impl AdaptiveAudioBuffer {
    /// Initialize a new audio buffer with the given latency preset.
    pub fn new(preset: AudioBufferPreset) -> Self {
        Self {
            queue: VecDeque::with_capacity(48000),
            preset,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: STEREO_CHANNELS,
            volume_percent: 100,
            lip_sync_offset_ms: 0,
            underrun_count: 0,
            overrun_drop_count: 0,
            last_sample_left: 0,
            last_sample_right: 0,
        }
    }

    /// Set latency preset (Low: 30ms, Normal: 60ms, High: 120ms).
    pub fn set_preset(&mut self, preset: AudioBufferPreset) {
        self.preset = preset;
    }

    pub fn preset(&self) -> AudioBufferPreset {
        self.preset
    }

    /// Set volume scaling percentage (0% to 200%).
    pub fn set_volume(&mut self, percent: u32) {
        self.volume_percent = percent.min(200);
    }

    pub fn volume(&self) -> u32 {
        self.volume_percent
    }

    /// Set lip-sync offset in milliseconds (clamped to [-250, +250]).
    pub fn set_lip_sync_offset_ms(&mut self, offset_ms: i32) {
        self.lip_sync_offset_ms = offset_ms.clamp(MIN_LIP_SYNC_OFFSET_MS, MAX_LIP_SYNC_OFFSET_MS);
    }

    pub fn lip_sync_offset_ms(&self) -> i32 {
        self.lip_sync_offset_ms
    }

    /// Target buffer capacity in samples (frames * channels) based on preset and lip-sync offset.
    pub fn target_samples(&self) -> usize {
        let base_ms = self.preset.latency_ms() as i32;
        let effective_ms = (base_ms + self.lip_sync_offset_ms).max(10) as u32;
        let frames = (self.sample_rate * effective_ms) / 1000;
        (frames as usize) * self.channels
    }

    /// Maximum capacity before dropping overrun frames (3x target or at least 200ms).
    pub fn max_capacity_samples(&self) -> usize {
        let min_max_frames = (self.sample_rate * 200) / 1000;
        let min_max_samples = (min_max_frames as usize) * self.channels;
        (self.target_samples() * 3).max(min_max_samples)
    }

    /// Push newly decoded PCM samples into the buffer.
    /// Handles overrun protection by dropping oldest samples if limit exceeded.
    pub fn push_samples(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }

        let max_cap = self.max_capacity_samples();
        let new_total = self.queue.len() + samples.len();

        if new_total > max_cap {
            let drop_count = new_total - max_cap;
            let aligned_drop = (drop_count / self.channels) * self.channels;
            if aligned_drop > 0 {
                self.queue.drain(..aligned_drop);
                self.overrun_drop_count += (aligned_drop / self.channels) as u64;
            }
        }

        self.queue.extend(samples.iter().copied());
    }

    /// Pull audio samples for WASAPI rendering, applying volume gain, soft-clipping limiter,
    /// and underrun comfort silence with fade-out.
    pub fn pull_samples(&mut self, requested_count: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(requested_count);
        let available = self.queue.len();

        if available >= requested_count {
            for _ in 0..requested_count {
                if let Some(s) = self.queue.pop_front() {
                    let scaled = self.apply_gain_and_limiter(s);
                    out.push(scaled);
                }
            }
            if out.len() >= 2 {
                self.last_sample_left = out[out.len() - 2];
                self.last_sample_right = out[out.len() - 1];
            }
        } else {
            // Buffer underrun condition
            self.underrun_count += 1;

            // Drain all available samples
            for _ in 0..available {
                if let Some(s) = self.queue.pop_front() {
                    out.push(self.apply_gain_and_limiter(s));
                }
            }

            // Apply smooth fade-out over remaining available samples or comfort silence
            let missing = requested_count.saturating_sub(available);
            let fade_pairs = (16.min(missing) / 2).max(1);

            let cur_l = self.last_sample_left as f32;
            let cur_r = self.last_sample_right as f32;

            for k in 1..=fade_pairs {
                let factor = 1.0 - (k as f32 / fade_pairs as f32);
                out.push((cur_l * factor) as i16);
                out.push((cur_r * factor) as i16);
            }

            // Pad remainder with true comfort silence (zeroes)
            while out.len() < requested_count {
                out.push(0);
            }

            self.last_sample_left = 0;
            self.last_sample_right = 0;
        }

        out
    }

    /// Apply linear volume scaling or hyperbolic tangent soft-clipping limiter above 100%.
    fn apply_gain_and_limiter(&self, sample: i16) -> i16 {
        if self.volume_percent == 0 {
            return 0;
        }

        let gain = self.volume_percent as f32 / 100.0;
        let val = sample as f32 * gain;

        if self.volume_percent <= 100 {
            val.clamp(-32768.0, 32767.0) as i16
        } else {
            // Soft-clipping saturation function to prevent digital harsh distortion
            let norm = val / 32768.0;
            let sat = norm / (1.0 + norm.abs());
            (sat * 32767.0).clamp(-32768.0, 32767.0) as i16
        }
    }

    /// Number of samples currently queued in buffer.
    pub fn available_samples(&self) -> usize {
        self.queue.len()
    }

    /// Total count of buffer underrun occurrences.
    pub fn underrun_count(&self) -> u64 {
        self.underrun_count
    }

    /// Total count of frames dropped due to buffer overrun.
    pub fn overrun_drop_count(&self) -> u64 {
        self.overrun_drop_count
    }

    /// Clear all audio samples and reset underrun/overrun metrics.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.underrun_count = 0;
        self.overrun_drop_count = 0;
        self.last_sample_left = 0;
        self.last_sample_right = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_pull_basic() {
        let mut buf = AdaptiveAudioBuffer::new(AudioBufferPreset::Normal);
        let samples = vec![1000, -1000, 2000, -2000];
        buf.push_samples(&samples);
        assert_eq!(buf.available_samples(), 4);

        let pulled = buf.pull_samples(4);
        assert_eq!(pulled, samples);
        assert_eq!(buf.available_samples(), 0);
        assert_eq!(buf.underrun_count(), 0);
    }

    #[test]
    fn test_volume_scaling_and_limiter() {
        let mut buf = AdaptiveAudioBuffer::new(AudioBufferPreset::Low);

        // Mute (0%)
        buf.set_volume(0);
        buf.push_samples(&[10000, -10000]);
        let pulled = buf.pull_samples(2);
        assert_eq!(pulled, vec![0, 0]);

        // Normal (100%)
        buf.set_volume(100);
        buf.push_samples(&[10000, -10000]);
        let pulled = buf.pull_samples(2);
        assert_eq!(pulled, vec![10000, -10000]);

        // Boost (150%) with soft-clipping limiter (should not overflow i16 range)
        buf.set_volume(150);
        buf.push_samples(&[30000, -30000]);
        let pulled = buf.pull_samples(2);
        assert!(pulled[0] > 0 && pulled[0] < 32767);
        assert!(pulled[1] < 0 && pulled[1] > -32768);
    }

    #[test]
    fn test_underrun_handling_and_comfort_silence() {
        let mut buf = AdaptiveAudioBuffer::new(AudioBufferPreset::Low);
        buf.push_samples(&[500, -500]);

        // Request 6 samples when only 2 are available
        let pulled = buf.pull_samples(6);
        assert_eq!(pulled.len(), 6);
        assert_eq!(buf.underrun_count(), 1);
        // The last samples should fade to silence / 0
        assert_eq!(pulled[pulled.len() - 1], 0);
    }

    #[test]
    fn test_lip_sync_offset_clamp() {
        let mut buf = AdaptiveAudioBuffer::new(AudioBufferPreset::Normal);
        buf.set_lip_sync_offset_ms(500);
        assert_eq!(buf.lip_sync_offset_ms(), MAX_LIP_SYNC_OFFSET_MS);

        buf.set_lip_sync_offset_ms(-400);
        assert_eq!(buf.lip_sync_offset_ms(), MIN_LIP_SYNC_OFFSET_MS);
    }
}
