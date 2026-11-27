//! 语音输入: microphone capture (`cpal`) and transcription into the chat input box.
//! Audio is recorded on a dedicated OS thread holding the `!Send` input stream,
//! resampled to 16 kHz mono 16-bit PCM WAV (`wisp_core::asr`), and sent off the
//! main thread through `wisp_net::send_audio`.

use crate::app::{Place, Workspace};
use crate::i18n::t;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gpui::*;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wisp_core::asr;

/// Shortest clip worth sending for transcription, in milliseconds.
const MIN_CLIP_MS: u64 = 250;

/// Recorded audio returned when `Recorder::stop` finishes.
struct Captured {
    samples: Vec<f32>,
    channels: usize,
    sample_rate: u32,
}

/// Active microphone stream running on a dedicated OS thread.
struct Recorder {
    stop_tx: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<Result<Captured, String>>>,
}

impl Recorder {
    fn start(max_seconds: usize) -> Result<Recorder, String> {
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || capture_loop(max_seconds, init_tx, stop_rx));
        match init_rx.recv() {
            Ok(Ok(())) => Ok(Recorder { stop_tx: Some(stop_tx), handle: Some(handle) }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err("无法启动录音线程。".into())
            }
        }
    }

    fn stop(mut self) -> Result<Captured, String> {
        if let Some(tx) = self.stop_tx.take() { let _ = tx.send(()); }
        match self.handle.take() {
            Some(handle) => handle.join().unwrap_or_else(|_| Err("录音线程异常结束。".into())),
            None => Err("录音已经结束。".into()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() { let _ = tx.send(()); }
        if let Some(handle) = self.handle.take() { let _ = handle.join(); }
    }
}

fn capture_loop(max_seconds: usize, init_tx: mpsc::SyncSender<Result<(), String>>, stop_rx: mpsc::Receiver<()>) -> Result<Captured, String> {
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        let err = "没有找到可用的麦克风。".to_string();
        let _ = init_tx.send(Err(err.clone()));
        return Err(err);
    };
    let supported = match device.default_input_config() {
        Ok(config) => config,
        Err(err) => {
            let msg = format!("无法打开麦克风：{err}");
            let _ = init_tx.send(Err(msg.clone()));
            return Err(msg);
        }
    };
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate;
    let cap = if max_seconds > 0 { (max_seconds + 2) * sample_rate as usize * channels } else { 0 };
    let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
    let error = Arc::new(Mutex::new(Option::<String>::None));

    let err_slot = Arc::clone(&error);
    let err_fn = move |err: cpal::Error| {
        let mut guard = err_slot.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() { *guard = Some(format!("录音中断：{err}")) }
    };

    fn build_typed<T: cpal::SizedSample>(device: &cpal::Device, config: cpal::StreamConfig, buffer: Arc<Mutex<Vec<f32>>>, cap: usize, err_fn: impl FnMut(cpal::Error) + Send + 'static) -> Result<cpal::Stream, cpal::Error>
    where
        f32: cpal::FromSample<T>,
    {
        device.build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut guard = buffer.lock().unwrap_or_else(|p| p.into_inner());
                let room = if cap == 0 { data.len() } else { cap.saturating_sub(guard.len()) };
                guard.extend(data.iter().take(room).map(|s| cpal::Sample::to_sample::<f32>(*s)));
            },
            err_fn,
            None,
        )
    }

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_typed::<f32>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::I16 => build_typed::<i16>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::U16 => build_typed::<u16>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::I32 => build_typed::<i32>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::U32 => build_typed::<u32>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::I8 => build_typed::<i8>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::U8 => build_typed::<u8>(&device, config, Arc::clone(&buffer), cap, err_fn),
        cpal::SampleFormat::F64 => build_typed::<f64>(&device, config, Arc::clone(&buffer), cap, err_fn),
        other => {
            let msg = format!("不支持的麦克风采样格式：{other:?}");
            let _ = init_tx.send(Err(msg.clone()));
            return Err(msg);
        }
    };
    let stream = match stream {
        Ok(stream) => stream,
        Err(err) => {
            let msg = format!("无法启动麦克风录音：{err}");
            let _ = init_tx.send(Err(msg.clone()));
            return Err(msg);
        }
    };
    if let Err(err) = stream.play() {
        let msg = format!("无法启动麦克风录音：{err}");
        let _ = init_tx.send(Err(msg.clone()));
        return Err(msg);
    }
    let _ = init_tx.send(Ok(()));

    if max_seconds > 0 {
        let _ = stop_rx.recv_timeout(Duration::from_secs(max_seconds as u64));
    } else {
        let _ = stop_rx.recv();
    }
    drop(stream);

    if let Some(err) = error.lock().unwrap_or_else(|p| p.into_inner()).take() {
        let samples = std::mem::take(&mut *buffer.lock().unwrap_or_else(|p| p.into_inner()));
        if samples.is_empty() { return Err(err) }
        return Ok(Captured { samples, channels, sample_rate });
    }
    let samples = std::mem::take(&mut *buffer.lock().unwrap_or_else(|p| p.into_inner()));
    Ok(Captured { samples, channels, sample_rate })
}

enum VoiceState {
    Idle,
    Recording { place: Place, library: Option<String>, recorder: Recorder, started: Instant, max_seconds: usize, run: u64 },
    Transcribing { place: Place, run: u64 },
}

/// Active voice input state for the workspace (at most one recording or transcription at a time).
pub struct Voice {
    state: VoiceState,
    runs: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Voice { state: VoiceState::Idle, runs: 0 }
    }
}

impl Voice {
    pub(crate) fn is_recording(&self) -> bool {
        matches!(self.state, VoiceState::Recording { .. })
    }

    pub(crate) fn recording_at(&self, target: Place) -> Option<Duration> {
        match &self.state {
            VoiceState::Recording { place, started, .. } if *place == target => Some(started.elapsed()),
            _ => None,
        }
    }

    pub(crate) fn transcribing_at(&self, target: Place) -> bool {
        matches!(&self.state, VoiceState::Transcribing { place, .. } if *place == target)
    }

    pub(crate) fn busy_elsewhere(&self, target: Place) -> bool {
        match &self.state {
            VoiceState::Idle => false,
            VoiceState::Recording { place, .. } | VoiceState::Transcribing { place, .. } => *place != target,
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.state = VoiceState::Idle;
    }

    pub(crate) fn cancel_chat(&mut self) {
        if matches!(self.state, VoiceState::Recording { place: Place::Chat(_), .. } | VoiceState::Transcribing { place: Place::Chat(_), .. }) {
            self.state = VoiceState::Idle;
        }
    }
}

impl Workspace {
    /// Start recording at `place`, or stop and transcribe if `place` is already recording.
    pub(crate) fn voice_toggle(&mut self, place: Place, cx: &mut Context<Self>) {
        match &self.voice.state {
            VoiceState::Recording { place: active, .. } if *active == place => {
                self.voice_finish(cx);
                return;
            }
            VoiceState::Recording { .. } | VoiceState::Transcribing { .. } => return,
            VoiceState::Idle => {}
        }
        let ready = match asr::ready(&self.settings, "") {
            Ok(ready) => ready,
            Err(err) => return self.fail(crate::i18n::reasons(&err), cx),
        };
        let max_seconds = ready.engine.seconds;
        let recorder = match Recorder::start(max_seconds) {
            Ok(recorder) => recorder,
            Err(err) => return self.fail(err, cx),
        };
        let library = match place {
            Place::Chat(_) => self.lib.as_ref().map(|lib| lib.info.id.clone()),
            Place::Home => None,
        };
        self.voice.runs += 1;
        let run = self.voice.runs;
        self.voice.state = VoiceState::Recording { place, library, recorder, started: Instant::now(), max_seconds, run };
        cx.notify();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let keep = this.update(cx, |this, cx| {
                    let VoiceState::Recording { run: active, started, max_seconds, .. } = &this.voice.state else { return false };
                    if *active != run { return false }
                    if *max_seconds > 0 && started.elapsed() >= Duration::from_secs(*max_seconds as u64) {
                        this.voice_finish(cx);
                        return false;
                    }
                    cx.notify();
                    true
                });
                if !matches!(keep, Ok(true)) { break }
            }
        })
        .detach();
    }

    pub(crate) fn voice_cancel(&mut self, cx: &mut Context<Self>) {
        if matches!(self.voice.state, VoiceState::Idle) { return }
        self.voice.cancel();
        cx.notify();
    }

    fn voice_finish(&mut self, cx: &mut Context<Self>) {
        let VoiceState::Recording { place, library, recorder, run, .. } = std::mem::replace(&mut self.voice.state, VoiceState::Idle) else { return };
        let captured = match recorder.stop() {
            Ok(captured) => captured,
            Err(err) => {
                self.fail(err, cx);
                cx.notify();
                return;
            }
        };
        let pcm = asr::resample_mono_i16(&captured.samples, captured.channels, captured.sample_rate, asr::SAMPLE_RATE);
        if asr::duration_ms(pcm.len(), asr::SAMPLE_RATE) < MIN_CLIP_MS {
            self.say(t("说话时间太短，没有识别到内容。"), cx);
            cx.notify();
            return;
        }
        let ready = match asr::ready(&self.settings, "") {
            Ok(ready) => ready,
            Err(err) => {
                self.fail(crate::i18n::reasons(&err), cx);
                cx.notify();
                return;
            }
        };
        self.voice.state = VoiceState::Transcribing { place, run };
        cx.notify();

        let work = cx.background_executor().spawn(async move {
            let wav = asr::wav(&pcm, asr::SAMPLE_RATE);
            let upload = asr::request(&ready, "speech.wav", &wav)?;
            wisp_net::send_audio(&upload, ready.engine)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if !matches!(this.voice.state, VoiceState::Transcribing { run: active, .. } if active == run) { return }
                this.voice.state = VoiceState::Idle;
                match result {
                    Ok(transcript) if transcript.text.trim().is_empty() => this.say(t("没有识别到说话内容。"), cx),
                    Ok(transcript) => this.insert_voice_text(place, library.as_deref(), &transcript.text, cx),
                    Err(err) => this.fail(crate::i18n::reasons(&err), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn insert_voice_text(&mut self, place: Place, library: Option<&str>, text: &str, cx: &mut Context<Self>) {
        match place {
            Place::Home => {
                self.home.input.update(cx, |input, cx| input.insert(text, cx));
                self.home.focus = true;
            }
            Place::Chat(talk) => {
                let Some(lib) = self.lib.as_mut().filter(|lib| Some(lib.info.id.as_str()) == library) else { return };
                lib.chat(talk).input.update(cx, |input, cx| input.insert(text, cx));
                lib.focus_box = Some(talk);
            }
        }
    }
}
