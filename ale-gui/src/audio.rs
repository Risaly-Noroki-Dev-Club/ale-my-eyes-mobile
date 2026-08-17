use oboe::{
    AudioInputCallback, AudioInputStreamSafe, AudioStream, AudioStreamAsync, AudioStreamBase,
    ChannelCount, DataCallbackResult, Input, Mono,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const REQUESTED_SAMPLE_RATE_HZ: i32 = 48_000;
const MAX_BUFFER_SECONDS: usize = 2;
const STARTUP_BUFFER_LIMIT: usize = 96_000 * MAX_BUFFER_SECONDS;

pub struct Recorder {
    stream: AudioStreamAsync<Input, RecorderCallback>,
    samples: Arc<Mutex<VecDeque<f32>>>,
    overflowed: Arc<AtomicBool>,
    sample_rate_hz: u32,
    channels: u16,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        use oboe::{AudioStreamBuilder, PerformanceMode, SharingMode};

        let samples = Arc::new(Mutex::new(VecDeque::new()));
        let overflowed = Arc::new(AtomicBool::new(false));
        let max_buffered_samples = Arc::new(AtomicUsize::new(STARTUP_BUFFER_LIMIT));
        let mut stream = AudioStreamBuilder::default()
            .set_input()
            .set_performance_mode(PerformanceMode::LowLatency)
            .set_sharing_mode(SharingMode::Shared)
            .set_format::<f32>()
            .set_channel_count::<Mono>()
            .set_sample_rate(REQUESTED_SAMPLE_RATE_HZ)
            .set_callback(RecorderCallback {
                samples: samples.clone(),
                overflowed: overflowed.clone(),
                max_buffered_samples: max_buffered_samples.clone(),
            })
            .open_stream()
            .map_err(|error| format!("打开音频流失败: {error:?}"))?;

        let sample_rate_hz = u32::try_from(stream.get_sample_rate())
            .map_err(|_| "设备返回了无效采样率".to_string())?;
        let channels = match stream.get_channel_count() {
            ChannelCount::Mono => 1,
            other => return Err(format!("设备协商出了不支持的声道数: {other:?}")),
        };
        if !(ale_core::remote::MIN_SAMPLE_RATE_HZ..=ale_core::remote::MAX_SAMPLE_RATE_HZ)
            .contains(&sample_rate_hz)
        {
            return Err(format!("设备采样率不受支持: {sample_rate_hz} Hz"));
        }
        max_buffered_samples.store(
            sample_rate_hz as usize * channels as usize * MAX_BUFFER_SECONDS,
            Ordering::Release,
        );
        stream
            .start()
            .map_err(|error| format!("启动录音失败: {error:?}"))?;

        Ok(Self {
            stream,
            samples,
            overflowed,
            sample_rate_hz,
            channels,
        })
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn drain_pcm16(&self, max_bytes: usize) -> Result<Vec<u8>, String> {
        if self.overflowed.load(Ordering::Acquire) {
            return Err("录音缓冲溢出，已停止录音且未丢弃任何音频".to_string());
        }
        let bytes_per_frame = usize::from(self.channels) * 2;
        let max_samples = max_bytes / bytes_per_frame * usize::from(self.channels);
        let mut samples = self
            .samples
            .lock()
            .map_err(|_| "读取录音缓冲失败".to_string())?;
        let take = samples.len().min(max_samples);
        let mut pcm = Vec::with_capacity(take * 2);
        for sample in samples.drain(..take) {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            pcm.extend_from_slice(&value.to_le_bytes());
        }
        Ok(pcm)
    }

    pub fn finish_pcm_chunks(self, max_bytes: usize) -> Result<Vec<Vec<u8>>, String> {
        let Self {
            stream,
            samples,
            overflowed,
            channels,
            ..
        } = self;
        drop(stream);
        if overflowed.load(Ordering::Acquire) {
            return Err("录音缓冲溢出，已停止录音且未丢弃任何音频".to_string());
        }
        let bytes_per_frame = usize::from(channels) * 2;
        let max_samples = max_bytes / bytes_per_frame * usize::from(channels);
        let mut samples = samples.lock().map_err(|_| "读取录音缓冲失败".to_string())?;
        let mut chunks = Vec::new();
        while !samples.is_empty() {
            let take = samples.len().min(max_samples);
            let mut pcm = Vec::with_capacity(take * 2);
            for sample in samples.drain(..take) {
                let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                pcm.extend_from_slice(&value.to_le_bytes());
            }
            chunks.push(pcm);
        }
        Ok(chunks)
    }

    pub fn stop(self) {
        drop(self);
    }
}

struct RecorderCallback {
    samples: Arc<Mutex<VecDeque<f32>>>,
    overflowed: Arc<AtomicBool>,
    max_buffered_samples: Arc<AtomicUsize>,
}

impl AudioInputCallback for RecorderCallback {
    type FrameType = (f32, Mono);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn AudioInputStreamSafe,
        frames: &[f32],
    ) -> DataCallbackResult {
        let Ok(mut buffer) = self.samples.try_lock() else {
            self.overflowed.store(true, Ordering::Release);
            return DataCallbackResult::Stop;
        };
        let limit = self.max_buffered_samples.load(Ordering::Acquire);
        if buffer.len().saturating_add(frames.len()) > limit {
            self.overflowed.store(true, Ordering::Release);
            return DataCallbackResult::Stop;
        }
        buffer.extend(frames.iter().copied());
        DataCallbackResult::Continue
    }
}
