use anyhow::Result;
use rodio::mixer::Mixer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioPlayback {
    // MixerDeviceSink owns the cpal stream — must be kept alive
    _sink: MixerDeviceSink,
    mixer: Mixer,
    player: Option<Arc<Player>>,
    playing: Arc<AtomicBool>,
}

impl AudioPlayback {
    pub fn new() -> Result<Self> {
        let sink = DeviceSinkBuilder::open_default_sink()?;
        let mixer = sink.mixer().clone();
        Ok(Self {
            _sink: sink,
            mixer,
            player: None,
            playing: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Play an audio file. Returns an Arc<Player> so the caller can wait
    /// for completion via `spawn_blocking(move || player.sleep_until_end())`.
    pub fn play(&mut self, path: &Path) -> Result<Arc<Player>> {
        self.stop();

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let source = Decoder::new(reader)?;

        let player = Arc::new(Player::connect_new(&self.mixer));
        player.append(source);
        self.playing.store(true, Ordering::SeqCst);
        self.player = Some(player.clone());

        Ok(player)
    }

    pub fn stop(&mut self) {
        if let Some(player) = self.player.take() {
            player.stop();
        }
        self.playing.store(false, Ordering::SeqCst);
    }

    pub fn is_playing(&self) -> bool {
        if let Some(ref player) = self.player {
            if player.empty() {
                return false;
            }
        }
        self.playing.load(Ordering::SeqCst)
    }

    pub fn set_done(&self) {
        self.playing.store(false, Ordering::SeqCst);
    }
}
