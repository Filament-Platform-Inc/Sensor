//! Speech to text, via whisper.cpp.
//!
//! The model is loaded once and kept resident. Loading costs seconds, which is
//! the whole reason this project runs as a daemon rather than a script.

use anyhow::{Context, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Transcriber {
    // The state owns ~230 MB of compute buffers. Creating it per utterance
    // costs most of a second, so it is built once and reused. It borrows from
    // ctx, so both live here together and the struct is self-referential in
    // spirit -- hence the state is created eagerly in load().
    state: whisper_rs::WhisperState,
    _ctx: Box<WhisperContext>,
}

impl Transcriber {
    /// Load a ggml model. Expensive — do this once at startup, never per utterance.
    pub fn load(model: &Path) -> Result<Self> {
        let path = model.to_str().context("model path is not valid UTF-8")?;
        let ctx = Box::new(
            WhisperContext::new_with_params(path, WhisperContextParameters::default())
                .with_context(|| format!("loading whisper model from {}", model.display()))?,
        );
        let state = ctx.create_state().context("creating whisper state")?;
        Ok(Self { state, _ctx: ctx })
    }

    /// Transcribe 16 kHz mono f32 samples.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        // Greedy sampling: dictation wants speed, and beam search buys accuracy
        // we do not need for short utterances.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(num_threads());
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // Whisper hallucinates confident text from silence; this suppresses the
        // worst of it on empty or near-empty input.
        params.set_no_context(true);

        self.state
            .full(params, samples)
            .context("running whisper")?;

        let n = self.state.full_n_segments();
        let mut out = String::new();
        for i in 0..n {
            if let Some(seg) = self.state.get_segment(i) {
                out.push_str(&seg.to_str_lossy().context("decoding segment text")?);
            }
        }
        Ok(out.trim().to_string())
    }
}

fn num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
}
