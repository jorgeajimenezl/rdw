use futures_util::{
    future::{self, Either},
    StreamExt,
};
use gst::prelude::*;
use gst_audio::prelude::*;
use std::{
    collections::HashMap,
    default::Default,
    error::Error,
    str::FromStr,
    sync::{Arc, Mutex},
};

/// The longest a read waits for the source. QEMU waits for the reply: a source that stopped
/// (unplugged, stalled) gets silence instead of holding it up.
const READ_TIMEOUT: gst::ClockTime = gst::ClockTime::from_mseconds(100);

/// The most input held for QEMU to read; older input is dropped rather than read late.
const INPUT_BACKLOG: gst::ClockTime = gst::ClockTime::from_mseconds(200);

/// Log the pipeline's errors and warnings (nothing else reads its bus) and drop every message,
/// so they don't pile up on it.
fn log_bus(pipeline: &gst::Pipeline, what: &'static str) {
    let Some(bus) = pipeline.bus() else {
        return;
    };
    bus.set_sync_handler(move |_, msg| {
        match msg.view() {
            gst::MessageView::Error(e) => {
                log::warn!("Audio {what} error: {} ({:?})", e.error(), e.debug())
            }
            gst::MessageView::Warning(w) => {
                log::warn!("Audio {what} warning: {} ({:?})", w.error(), w.debug())
            }
            _ => {}
        }
        gst::BusSyncReply::Drop
    });
}

#[derive(Debug)]
struct GstAudioOut {
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
}

impl GstAudioOut {
    fn new(caps: &str) -> Result<Self, Box<dyn Error>> {
        let pipeline = &format!(
            "appsrc name=src is-live=1 do-timestamp=0 format=time caps=\"{caps}\" !\
            queue ! decodebin name=decode \
            audioconvert name=convert ! audioresample ! autoaudiosink name=sink"
        );
        let pipeline = gst::parse::launch(pipeline)?;
        let pipeline = pipeline.dynamic_cast::<gst::Pipeline>().unwrap();
        let src = pipeline
            .by_name("src")
            .unwrap()
            .dynamic_cast::<gst_app::AppSrc>()
            .unwrap();
        // decodebin drops its source pad when the stream is disabled (READY) and adds a new
        // one when it plays again. gst-launch would link only the first pad, and every later
        // stream would stop with not-linked: link each one.
        let convert = pipeline.by_name("convert").unwrap().downgrade();
        pipeline
            .by_name("decode")
            .unwrap()
            .connect_pad_added(move |_, pad| {
                let Some(sink) = convert.upgrade().and_then(|c| c.static_pad("sink")) else {
                    return;
                };
                if let Err(e) = pad.link(&sink) {
                    log::warn!("Failed to link the audio decoder: {e:?}");
                }
            });
        log_bus(&pipeline, "output");
        Ok(Self { pipeline, src })
    }
}

/// The host source the guest's microphone records from, `None` for the desktop's default. A
/// handle: the input streams of every [`GstAudio`] made with it follow a change, also while
/// they record.
#[derive(Debug, Clone, Default)]
pub struct AudioInput(Arc<Mutex<InputChoice>>);

#[derive(Debug, Default)]
struct InputChoice {
    device: Option<gst::Device>,
    /// Counts the changes, so a stream can tell it records from an older choice.
    generation: u64,
}

impl AudioInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_device(&self, device: Option<gst::Device>) {
        let mut c = self.0.lock().unwrap();
        c.device = device;
        c.generation += 1;
    }

    pub fn device(&self) -> Option<gst::Device> {
        self.0.lock().unwrap().device.clone()
    }

    fn current(&self) -> (Option<gst::Device>, u64) {
        let c = self.0.lock().unwrap();
        (c.device.clone(), c.generation)
    }
}

#[derive(Debug)]
struct GstAudioIn {
    caps: String,
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    /// What the source delivered beyond the last read.
    queue: Vec<u8>,
    /// One frame of silence in the stream's format.
    silence: Vec<u8>,
    enabled: bool,
    /// The [`AudioInput`] generation the pipeline was built for.
    generation: u64,
    /// The guest's mute and volume (cubic), kept for a pipeline built later.
    mute: bool,
    volume: Option<f64>,
    /// A read waited [`READ_TIMEOUT`] for nothing: the next reads answer at once, with what
    /// came meanwhile and silence, until the source delivers again.
    stalled: bool,
}

impl GstAudioIn {
    fn new(
        caps: &str,
        device: Option<&gst::Device>,
        generation: u64,
    ) -> Result<Self, Box<dyn Error>> {
        let gst_caps = gst::Caps::from_str(caps)?;
        let info = gst_audio::AudioInfo::from_caps(&gst_caps)?;
        let mut silence = vec![0; info.bpf() as usize];
        info.format_info().fill_silence(&mut silence);

        let src = match device {
            Some(d) => d.create_element(Some("src"))?,
            None => gst::ElementFactory::make("autoaudiosrc")
                .name("src")
                .build()?,
        };
        let queue = gst::ElementFactory::make("queue")
            .property_from_str("leaky", "downstream")
            .property("max-size-time", INPUT_BACKLOG.nseconds())
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .build()?;
        let convert = gst::ElementFactory::make("audioconvert").build()?;
        // The guest's volume, here rather than on the source: a source's own volume is the
        // host's for that microphone, and an element keeps its value in NULL.
        let volume = gst::ElementFactory::make("volume").name("volume").build()?;
        let resample = gst::ElementFactory::make("audioresample").build()?;
        let sink = gst_app::AppSink::builder()
            .name("sink")
            .caps(&gst_caps)
            // The queue bounds the backlog by time; this keeps the sink from adding to it.
            .max_buffers(2)
            .drop(true)
            .build();
        let pipeline = gst::Pipeline::new();
        let elements = [
            &src,
            &queue,
            &convert,
            &volume,
            &resample,
            sink.upcast_ref(),
        ];
        pipeline.add_many(elements)?;
        gst::Element::link_many(elements)?;
        log_bus(&pipeline, "input");
        Ok(Self {
            caps: caps.to_owned(),
            pipeline,
            sink,
            queue: Default::default(),
            silence,
            enabled: false,
            generation,
            mute: false,
            volume: None,
            stalled: false,
        })
    }

    fn apply_volume(&self) {
        let Some(volume) = self.pipeline.by_name("volume") else {
            return;
        };
        volume.set_property("mute", self.mute);
        if let Some(v) = self.volume {
            let linear = gst_audio::StreamVolume::convert_volume(
                gst_audio::StreamVolumeFormat::Cubic,
                gst_audio::StreamVolumeFormat::Linear,
                v,
            );
            volume.set_property("volume", linear);
        }
    }

    /// Add a sample's audio to the queue.
    fn push(&mut self, sample: &gst::Sample) -> Result<(), Box<dyn Error>> {
        use std::io::prelude::*;

        let buffer = sample.buffer().ok_or_else(|| "No buffer?".to_string())?;
        let mut cursor = buffer.as_cursor_readable();
        let len = cursor.stream_len()?;
        let cur = self.queue.len();
        let end = cur + len as usize;
        self.queue.resize(end, 0);
        cursor.read_exact(&mut self.queue[cur..end])?;
        Ok(())
    }

    /// Fill the queue up to `size` with silence.
    fn pad(&mut self, size: usize) {
        let missing = size.saturating_sub(self.queue.len());
        let frames = missing.div_ceil(self.silence.len());
        let silence = self.silence.repeat(frames);
        self.queue.extend_from_slice(&silence[..missing]);
    }
}

impl Drop for GstAudioIn {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn device_name(device: Option<&gst::Device>) -> String {
    device.map_or_else(
        || "the default source".to_owned(),
        |d| format!("{:?}", d.display_name()),
    )
}

#[derive(Debug, Default)]
pub struct GstAudio {
    out: HashMap<u64, GstAudioOut>,
    in_: HashMap<u64, GstAudioIn>,
    input: AudioInput,
}

impl GstAudio {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Self::with_input(AudioInput::default())
    }

    /// Input streams record from the source `input` names, and follow its changes.
    pub fn with_input(input: AudioInput) -> Result<Self, Box<dyn Error>> {
        gst::init()?;

        Ok(Self {
            input,
            ..Self::default()
        })
    }

    #[allow(clippy::unused_self)]
    pub fn can_decode(&self, mime: &str) -> Result<bool, Box<dyn Error>> {
        let pipeline = gst::Pipeline::new();
        pipeline.add(&gst::ElementFactory::make("decodebin").build()?)?;
        let can_decode = if let Some(sink) = pipeline.pad_template("sink") {
            use std::str::FromStr;
            let caps = gst::Caps::from_str(mime).unwrap();
            sink.caps().can_intersect(&caps)
        } else {
            false
        };

        Ok(can_decode)
    }

    pub fn init_out(&mut self, id: u64, caps: &str) -> Result<(), Box<dyn Error>> {
        if self.out.contains_key(&id) {
            return Err(format!("id {id} is already setup").into());
        }

        let out = GstAudioOut::new(caps)?;
        self.out.insert(id, out);
        Ok(())
    }

    pub fn fini_out(&mut self, id: u64) {
        self.out.remove(&id);
    }

    fn get_out(&self, id: u64) -> Result<&GstAudioOut, String> {
        self.out
            .get(&id)
            .ok_or_else(|| format!("Stream not found: {id}"))
    }

    pub fn set_enabled_out(&mut self, id: u64, enabled: bool) -> Result<(), Box<dyn Error>> {
        self.get_out(id)?.pipeline.set_state(if enabled {
            gst::State::Playing
        } else {
            gst::State::Ready
        })?;
        Ok(())
    }

    pub fn set_volume_out(
        &mut self,
        id: u64,
        mute: bool,
        vol: Option<f64>,
    ) -> Result<(), Box<dyn Error>> {
        let stream_vol = self
            .get_out(id)?
            .pipeline
            .by_interface(gst_audio::StreamVolume::static_type())
            .ok_or("Pipeline doesn't support volume")?
            .dynamic_cast::<gst_audio::StreamVolume>()
            .unwrap();

        stream_vol.set_mute(mute);
        if let Some(vol) = vol {
            stream_vol.set_volume(gst_audio::StreamVolumeFormat::Cubic, vol);
        }
        Ok(())
    }

    pub fn write_out(&mut self, id: u64, data: Vec<u8>) -> Result<(), Box<dyn Error>> {
        self.get_out(id)?
            .src
            .push_buffer(gst::Buffer::from_slice(data))?;
        Ok(())
    }

    pub fn init_in(&mut self, id: u64, caps: &str) -> Result<(), Box<dyn Error>> {
        if self.in_.contains_key(&id) {
            return Err(format!("id {id} is already setup").into());
        }

        let (device, generation) = self.input.current();
        let in_ = GstAudioIn::new(caps, device.as_ref(), generation)?;
        log::debug!(
            "Audio input {id}: {caps}, from {}",
            device_name(device.as_ref())
        );
        self.in_.insert(id, in_);
        Ok(())
    }

    pub fn fini_in(&mut self, id: u64) {
        self.in_.remove(&id);
    }

    fn get_in_mut(&mut self, id: u64) -> Result<&mut GstAudioIn, String> {
        self.in_
            .get_mut(&id)
            .ok_or_else(|| format!("Stream not found: {id}"))
    }

    /// The stream, its pipeline rebuilt first if the [`AudioInput`] changed since it was made.
    /// A device that can't record, or can't start, falls back to the default source.
    fn follow_input(&mut self, id: u64) -> Result<&mut GstAudioIn, Box<dyn Error>> {
        let (device, generation) = self.input.current();
        let in_ = self
            .in_
            .get_mut(&id)
            .ok_or_else(|| format!("Stream not found: {id}"))?;
        if in_.generation != generation {
            log::info!(
                "Audio input {id}: recording from {}",
                device_name(device.as_ref())
            );
            let build = |device: Option<&gst::Device>| -> Result<GstAudioIn, Box<dyn Error>> {
                let mut new = GstAudioIn::new(&in_.caps, device, generation)?;
                new.enabled = in_.enabled;
                new.mute = in_.mute;
                new.volume = in_.volume;
                new.apply_volume();
                if new.enabled {
                    new.pipeline.set_state(gst::State::Playing)?;
                }
                Ok(new)
            };
            let new = build(device.as_ref()).or_else(|e| {
                log::warn!(
                    "Audio input {id}: can't record from {}: {e}",
                    device_name(device.as_ref())
                );
                build(None)
            })?;
            *in_ = new;
        }
        Ok(in_)
    }

    pub fn set_enabled_in(&mut self, id: u64, enabled: bool) -> Result<(), Box<dyn Error>> {
        log::debug!(
            "Audio input {id}: {}",
            if enabled { "enabled" } else { "disabled" }
        );
        let in_ = self.follow_input(id)?;
        in_.enabled = enabled;
        in_.stalled = false;
        // NULL, not READY: in READY the source keeps what it had captured (pipewiresrc does),
        // and the next recording would start with the end of this one. NULL also lets the
        // host's microphone go between recordings. What the last read left goes too.
        in_.queue.clear();
        in_.pipeline.set_state(if enabled {
            gst::State::Playing
        } else {
            gst::State::Null
        })?;
        Ok(())
    }

    pub fn set_volume_in(
        &mut self,
        id: u64,
        mute: bool,
        vol: Option<f64>,
    ) -> Result<(), Box<dyn Error>> {
        let in_ = self.get_in_mut(id)?;
        in_.mute = mute;
        if vol.is_some() {
            in_.volume = vol;
        }
        in_.apply_volume();
        Ok(())
    }

    pub async fn read_in(&mut self, id: u64, size: u64) -> Result<Vec<u8>, Box<dyn Error>> {
        let size = size as usize;
        let in_ = self.follow_input(id)?;
        // Stalled: take what came since without waiting; the source is back once it delivers.
        if in_.stalled {
            while in_.queue.len() < size {
                let Some(sample) = in_.sink.try_pull_sample(gst::ClockTime::ZERO) else {
                    break;
                };
                in_.push(&sample)?;
                in_.stalled = false;
            }
            if in_.stalled {
                in_.pad(size);
            }
        }
        let mut stream = in_.sink.stream();
        let clock = gst::SystemClock::obtain();
        while in_.queue.len() < size {
            let timeout = clock
                .new_single_shot_id(clock.time() + READ_TIMEOUT)
                .wait_async_future()?;
            match future::select(stream.next(), timeout).await {
                Either::Left((Some(sample), _)) => in_.push(&sample)?,
                Either::Left((None, _)) => return Err("EOS?".into()),
                Either::Right(_) => {
                    log::debug!(
                        "Audio input {id}: no input for {READ_TIMEOUT}, silence until it comes"
                    );
                    in_.stalled = true;
                    in_.pad(size);
                }
            }
        }
        let remainder = in_.queue.split_off(size);
        Ok(std::mem::replace(&mut in_.queue, remainder))
    }
}
