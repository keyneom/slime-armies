use web_sys::{AudioContext, OscillatorType};

/// Audio system for playing WASM-4 style synthesized sounds
pub struct Audio {
    ctx: Option<AudioContext>,
    pub muted: bool,
    pub volume: f32,
}

/// Sound channel types (matching WASM-4)
#[derive(Clone, Copy)]
pub enum Channel {
    Pulse1,   // Square wave with 12.5% duty
    Pulse2,   // Square wave with 25% duty
    Triangle, // Triangle wave
    Noise,    // White noise (approximated)
}

/// ADSR envelope
#[derive(Clone, Copy)]
pub struct Envelope {
    pub attack: f32,  // seconds
    pub decay: f32,   // seconds
    pub sustain: f32, // level 0-1
    pub release: f32, // seconds
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.1,
        }
    }
}

impl Audio {
    pub fn new() -> Self {
        // Create audio context - may fail on some browsers before user interaction
        let ctx = AudioContext::new().ok();

        if ctx.is_some() {
            web_sys::console::log_1(&"Audio context created".into());
        } else {
            web_sys::console::log_1(&"Failed to create audio context".into());
        }

        Self {
            ctx,
            muted: false,
            volume: 0.8,
        }
    }

    /// Try to resume/create audio context (needed after user interaction)
    pub fn ensure_context(&mut self) {
        if self.ctx.is_none() {
            self.ctx = AudioContext::new().ok();
        }

        // Resume if suspended (browser autoplay policy)
        if let Some(ctx) = &self.ctx {
            let _ = ctx.resume();
        }
    }

    /// Toggle mute on/off
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
    }

    /// Set volume (0.0 to 1.0)
    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
    }

    /// Play a tone with frequency sweep and ADSR envelope
    pub fn play_tone(
        &self,
        freq_start: f32,
        freq_end: f32,
        channel: Channel,
        envelope: Envelope,
        volume_mult: f32,
    ) {
        if self.muted {
            return;
        }

        let ctx = match &self.ctx {
            Some(c) => c,
            None => return,
        };

        let current_time = ctx.current_time();
        let total_duration = envelope.attack + envelope.decay + envelope.release;

        // Create oscillator
        let oscillator = match ctx.create_oscillator() {
            Ok(osc) => osc,
            Err(_) => return,
        };

        // Set oscillator type based on channel
        let osc_type = match channel {
            Channel::Pulse1 | Channel::Pulse2 => OscillatorType::Square,
            Channel::Triangle => OscillatorType::Triangle,
            Channel::Noise => OscillatorType::Sawtooth, // Approximate noise with sawtooth
        };
        oscillator.set_type(osc_type);

        // Set frequency with sweep
        let freq = oscillator.frequency();
        freq.set_value_at_time(freq_start, current_time).ok();
        if (freq_end - freq_start).abs() > 1.0 {
            freq.linear_ramp_to_value_at_time(freq_end, current_time + total_duration as f64).ok();
        }

        // Create gain node for envelope
        let gain = match ctx.create_gain() {
            Ok(g) => g,
            Err(_) => return,
        };

        let final_volume = self.volume * volume_mult;
        let gain_param = gain.gain();

        // ADSR envelope
        gain_param.set_value_at_time(0.0, current_time).ok();

        // Attack
        gain_param.linear_ramp_to_value_at_time(
            final_volume,
            current_time + envelope.attack as f64
        ).ok();

        // Decay to sustain
        gain_param.linear_ramp_to_value_at_time(
            final_volume * envelope.sustain,
            current_time + envelope.attack as f64 + envelope.decay as f64
        ).ok();

        // Release
        gain_param.linear_ramp_to_value_at_time(
            0.0,
            current_time + total_duration as f64
        ).ok();

        // Connect nodes
        oscillator.connect_with_audio_node(&gain).ok();
        gain.connect_with_audio_node(&ctx.destination()).ok();

        // Start and stop
        oscillator.start().ok();
        oscillator.stop_with_when(current_time + total_duration as f64 + 0.1).ok();
    }

    // ============ Game-specific sound effects ============

    /// Player attack sound
    pub fn play_attack(&self) {
        self.play_tone(
            150.0, 600.0,
            Channel::Noise,
            Envelope { attack: 0.01, decay: 0.0, sustain: 1.0, release: 0.0 },
            1.0
        );
    }

    /// Player block sound
    pub fn play_block(&self) {
        self.play_tone(
            100.0, 280.0,
            Channel::Pulse1,
            Envelope { attack: 0.02, decay: 0.0, sustain: 1.0, release: 0.1 },
            1.0
        );
    }

    /// Successful block deflection
    pub fn play_deflect(&self) {
        self.play_tone(
            250.0, 200.0,
            Channel::Noise,
            Envelope { attack: 0.03, decay: 0.0, sustain: 1.0, release: 0.0 },
            0.6
        );
    }

    /// Phase/dodge sound
    pub fn play_phase(&self) {
        self.play_tone(
            240.0, 20.0,
            Channel::Noise,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.1 },
            1.0
        );
        self.play_tone(
            440.0, 440.0,
            Channel::Triangle,
            Envelope { attack: 0.05, decay: 0.0, sustain: 1.0, release: 0.1 },
            0.5
        );
    }

    /// Enemy killed sound
    pub fn play_enemy_kill(&self) {
        self.play_tone(
            300.0, 100.0,
            Channel::Pulse1,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.1 },
            0.7
        );
    }

    /// Player hit/damage sound
    pub fn play_hit(&self) {
        self.play_tone(
            200.0, 60.0,
            Channel::Noise,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.2 },
            1.0
        );
    }

    /// Player death sound
    pub fn play_death(&self) {
        self.play_tone(
            180.0, 50.0,
            Channel::Noise,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.3 },
            1.0
        );
    }

    /// Projectile hit/explosion
    pub fn play_explosion(&self) {
        self.play_tone(
            170.0, 40.0,
            Channel::Noise,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.25 },
            1.0
        );
    }

    /// Enemy nearby warning (pitch based on distance)
    pub fn play_enemy_near(&self, distance: f32, max_distance: f32) {
        let volume = ((max_distance - distance) / max_distance).clamp(0.0, 1.0);
        if volume < 0.1 {
            return;
        }
        self.play_tone(
            60.0, 60.0,
            Channel::Pulse2,
            Envelope { attack: 0.01, decay: 0.0, sustain: 1.0, release: 0.05 },
            volume * 0.5
        );
    }

    /// Menu selection sound
    pub fn play_menu_select(&self) {
        self.play_tone(
            440.0, 440.0,
            Channel::Triangle,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.02 },
            0.5
        );
    }

    /// Volume change confirmation beep
    pub fn play_volume_change(&self, increasing: bool) {
        let (freq_start, freq_end) = if increasing {
            (40.0, 200.0)
        } else {
            (200.0, 40.0)
        };
        self.play_tone(
            freq_start, freq_end,
            Channel::Pulse1,
            Envelope { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.01 },
            0.5
        );
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}
