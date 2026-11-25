//! Jitter buffer + audio output. The phone's audio is played into an output device;
//! when that device is VB-Cable's "CABLE Input", every program sees it as the
//! microphone "CABLE Output".

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat};

use crate::proto::SAMPLE_RATE;

/// How much audio we keep buffered before playing (absorbs Wi-Fi jitter).
const TARGET: usize = (SAMPLE_RATE as usize) * 60 / 1000; // 60 ms
/// If the buffer grows past this, drop the excess so latency can't creep up.
const MAX: usize = (SAMPLE_RATE as usize) * 250 / 1000; // 250 ms

pub struct AudioBuf {
    q: VecDeque<f32>,
    playing: bool,
    pos: f64,
    /// 1.0 = 100 %
    pub volume: f32,
    pub muted: bool,
    /// Voice pitch in semitones (0 = off). Applied in the network thread.
    pub pitch: f32,
    /// Peak output level since the GUI last read it.
    pub level: f32,
}

pub struct AudioShared(Mutex<AudioBuf>);

impl AudioShared {
    pub fn new(volume: f32) -> Arc<Self> {
        Arc::new(AudioShared(Mutex::new(AudioBuf {
            q: VecDeque::with_capacity(MAX + 4096),
            playing: false,
            pos: 0.0,
            volume,
            muted: false,
            pitch: 0.0,
            level: 0.0,
        })))
    }

    pub fn lock(&self) -> MutexGuard<'_, AudioBuf> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Append mono 48 kHz samples.
    pub fn push_samples(&self, samples: &[f32]) {
        let mut b = self.lock();
        b.q.extend(samples.iter().copied());
        if b.q.len() > MAX {
            let excess = b.q.len() - TARGET;
            b.q.drain(..excess);
        }
    }

    pub fn clear(&self) {
        let mut b = self.lock();
        b.q.clear();
        b.playing = false;
        b.pos = 0.0;
    }
}

/// Slightly speed up / slow down playback to keep the buffer near TARGET.
/// This hides the small clock difference between phone and PC (max ±0.4 %, inaudible).
fn adapted_ratio(b: &AudioBuf, base: f64) -> f64 {
    if !b.playing {
        return base;
    }
    let err = (b.q.len() as f64 - TARGET as f64) / (SAMPLE_RATE as f64 * 0.1);
    base * (1.0 + err.clamp(-1.0, 1.0) * 0.004)
}

/// One output sample, resampled from 48 kHz with linear interpolation.
fn next_sample(b: &mut AudioBuf, ratio: f64) -> f32 {
    if !b.playing {
        if b.q.len() >= TARGET {
            b.playing = true;
            b.pos = 0.0;
        } else {
            return 0.0;
        }
    }
    if b.q.len() < 2 {
        // Underrun: wait until the buffer refills.
        b.playing = false;
        return 0.0;
    }
    let s0 = b.q[0];
    let s1 = b.q[1];
    let v = s0 + (s1 - s0) * b.pos as f32;
    b.pos += ratio;
    while b.pos >= 1.0 && !b.q.is_empty() {
        b.q.pop_front();
        b.pos -= 1.0;
    }
    let gain = if b.muted { 0.0 } else { b.volume };
    let out = (v * gain).clamp(-1.0, 1.0);
    if out.abs() > b.level {
        b.level = out.abs();
    }
    out
}

pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn is_virtual_cable(name: &str) -> bool {
    name.to_lowercase().contains("cable input")
}

/// Open the output stream. `wanted` = saved device name; falls back to VB-Cable, then the default device.
pub fn start_output(
    wanted: Option<&str>,
    shared: Arc<AudioShared>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let find = |pred: &dyn Fn(&str) -> bool| -> Option<cpal::Device> {
        host.output_devices()
            .ok()?
            .find(|d| d.name().map(|n| pred(&n)).unwrap_or(false))
    };

    let device = wanted
        .and_then(|w| find(&|n: &str| n == w))
        .or_else(|| find(&is_virtual_cable))
        .or_else(|| host.default_output_device())
        .ok_or_else(|| "No audio output device found".to_string())?;
    let name = device.name().unwrap_or_else(|_| "Unknown device".into());

    let supported = device
        .default_output_config()
        .map_err(|e| format!("{name}: {e}"))?;
    let format = supported.sample_format();
    let config = supported.config();

    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, shared),
        SampleFormat::I16 => build::<i16>(&device, &config, shared),
        SampleFormat::U16 => build::<u16>(&device, &config, shared),
        SampleFormat::I32 => build::<i32>(&device, &config, shared),
        other => Err(format!("Unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| format!("{name}: {e}"))?;
    Ok((stream, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<AudioShared>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let base_ratio = SAMPLE_RATE as f64 / config.sample_rate.0 as f64;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let mut b = shared.lock();
                let ratio = adapted_ratio(&b, base_ratio);
                for frame in data.chunks_mut(channels) {
                    let s = T::from_sample(next_sample(&mut b, ratio));
                    for x in frame.iter_mut() {
                        *x = s;
                    }
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}
