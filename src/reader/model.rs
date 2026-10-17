//! Reading panel model: pure state driven by [`PlayerEvent`]s, no window.
//!
//! Time is always passed in as `now: [`Instant`]` so tests are deterministic.
//! Pausing freezes the elapsed clock (the highlighted word must not advance
//! while paused); a new chunk restarts it at zero.

use std::time::{Duration, Instant};

use crate::player::{PlayerEvent, PlayerState};
use crate::word_timing::word_index;

/// Reading panel state; see the module docs.
pub struct ReaderModel {
    chunks: Vec<String>,
    current: Option<usize>,
    state: PlayerState,
    /// Speech length of the current chunk.
    duration: Duration,
    /// Elapsed time banked before the current run (a pause freezes it here).
    frozen: Duration,
    /// When `Playing`, the instant `frozen` started accruing from.
    anchor: Option<Instant>,
    reading_id: u64,
    dismissed_reading: Option<u64>,
}

impl ReaderModel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: &PlayerEvent, now: Instant) {
        match event {
            PlayerEvent::Playlist(texts) => {
                self.chunks = texts.clone();
                self.reading_id = self.reading_id.wrapping_add(1);
                self.current = None;
                self.reset_timing();
            }
            PlayerEvent::Chunk {
                index, duration, ..
            } => {
                self.current = Some(*index);
                self.duration = *duration;
                self.frozen = Duration::ZERO;
                // While paused the audio isn't advancing, so the clock stays
                // frozen at 0 until `Playing`. (A chunk arriving while
                // `Loading` is mid-reading audio still playing, so it runs.)
                self.anchor = if self.state == PlayerState::Paused {
                    None
                } else {
                    Some(now)
                };
            }
            PlayerEvent::State(state) => match state {
                PlayerState::Paused => {
                    self.frozen = self.elapsed(now);
                    self.anchor = None;
                    self.state = PlayerState::Paused;
                }
                PlayerState::Playing => {
                    self.frozen = self.elapsed(now);
                    self.anchor = Some(now);
                    self.state = PlayerState::Playing;
                }
                PlayerState::Loading => {
                    self.state = PlayerState::Loading;
                }
                PlayerState::Idle => {
                    self.state = PlayerState::Idle;
                    self.current = None;
                    self.reset_timing();
                }
            },
            PlayerEvent::Finished | PlayerEvent::Error(_) => {
                self.state = PlayerState::Idle;
                self.current = None;
                self.reset_timing();
            }
            PlayerEvent::Latency { .. } => {}
        }
    }

    fn reset_timing(&mut self) {
        self.duration = Duration::ZERO;
        self.frozen = Duration::ZERO;
        self.anchor = None;
    }

    /// Chunk texts of the current (or most recent) reading.
    #[must_use]
    pub fn chunks(&self) -> &[String] {
        &self.chunks
    }

    /// Index of the chunk now playing, or `None` when idle.
    #[must_use]
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    #[must_use]
    pub fn state(&self) -> PlayerState {
        self.state
    }

    /// Time spent in the current chunk.
    #[must_use]
    pub fn elapsed(&self, now: Instant) -> Duration {
        match self.anchor {
            Some(anchor) => self
                .frozen
                .saturating_add(now.saturating_duration_since(anchor)),
            None => self.frozen,
        }
    }

    /// Estimated word index within the current chunk.
    #[must_use]
    pub fn current_word(&self, now: Instant) -> Option<usize> {
        let text = self.chunks.get(self.current?)?;
        word_index(
            text,
            self.elapsed(now).as_secs_f64(),
            self.duration.as_secs_f64(),
        )
    }

    /// `(current + 1) / count`, or 0 when there is no current chunk.
    #[must_use]
    pub fn progress(&self) -> f32 {
        match (self.current, self.chunks.len()) {
            (Some(current), count) if count > 0 => (current + 1) as f32 / count as f32,
            _ => 0.0,
        }
    }

    #[must_use]
    pub fn progress_label(&self) -> String {
        if self.chunks.is_empty() {
            return "Nothing to read yet.".to_string();
        }
        if let Some(current) = self.current {
            return format!("Sentence {} of {}", current + 1, self.chunks.len());
        }
        if self.is_active() {
            "Starting…".to_string()
        } else {
            "Not reading".to_string()
        }
    }

    /// There is a chunk to go back to.
    #[must_use]
    pub fn can_previous(&self) -> bool {
        self.is_active() && matches!(self.current, Some(index) if index > 0)
    }

    /// There is a chunk to skip to.
    #[must_use]
    pub fn can_next(&self) -> bool {
        self.is_active()
            && matches!(self.current, Some(index) if index.saturating_add(1) < self.chunks.len())
    }

    /// A reading is in progress (`Loading`, `Playing` or `Paused`).
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            PlayerState::Loading | PlayerState::Playing | PlayerState::Paused
        )
    }

    /// The highlight is moving and the panel should repaint.
    #[must_use]
    pub fn needs_animation(&self) -> bool {
        self.state == PlayerState::Playing && self.current.is_some()
    }

    /// Bumped on every `Playlist`; distinguishes readings for dismissal.
    #[must_use]
    pub fn reading_id(&self) -> u64 {
        self.reading_id
    }

    /// Close the panel for this reading.
    pub fn dismiss(&mut self) {
        self.dismissed_reading = Some(self.reading_id);
    }

    /// Show the panel again ("Show Reading Panel" menu item).
    pub fn undismiss(&mut self) {
        self.dismissed_reading = None;
    }

    /// Whether the panel should be visible given the `show_reader` setting.
    #[must_use]
    pub fn should_show(&self, show_setting: bool) -> bool {
        show_setting && self.is_active() && self.dismissed_reading != Some(self.reading_id)
    }
}

impl Default for ReaderModel {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            current: None,
            state: PlayerState::Idle,
            duration: Duration::ZERO,
            frozen: Duration::ZERO,
            anchor: None,
            reading_id: 0,
            dismissed_reading: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(texts: &[&str]) -> PlayerEvent {
        PlayerEvent::Playlist(texts.iter().map(ToString::to_string).collect())
    }

    fn chunk_event(index: usize, text: &str, duration: Duration) -> PlayerEvent {
        PlayerEvent::Chunk {
            index,
            count: 2,
            text: text.to_string(),
            duration,
        }
    }

    fn playing_model(start: Instant) -> ReaderModel {
        let mut model = ReaderModel::new();
        model.apply(&playlist(&["One.", "Two."]), start);
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        model.apply(&chunk_event(0, "One.", Duration::from_secs(10)), start);
        model
    }

    #[test]
    fn playlist_resets_and_bumps_id() {
        let mut model = ReaderModel::new();
        assert_eq!(model.reading_id(), 0);
        let start = Instant::now();
        model.apply(&playlist(&["One.", "Two."]), start);
        assert_eq!(model.reading_id(), 1);
        assert_eq!(model.chunks(), ["One.", "Two."].as_slice());
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        model.apply(&chunk_event(0, "One.", Duration::from_secs(10)), start);
        assert_eq!(model.current(), Some(0));
        let later = start + Duration::from_secs(1);
        model.apply(&playlist(&["Alpha."]), later);
        assert_eq!(model.reading_id(), 2);
        assert_eq!(model.chunks(), ["Alpha."].as_slice());
        assert_eq!(model.current(), None);
        assert_eq!(model.elapsed(later), Duration::ZERO);
    }

    #[test]
    fn elapsed_advances_while_playing() {
        let start = Instant::now();
        let model = playing_model(start);
        assert_eq!(
            model.elapsed(start + Duration::from_secs(3)),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn pause_does_not_advance_elapsed() {
        let start = Instant::now();
        let mut model = playing_model(start);
        model.apply(
            &PlayerEvent::State(PlayerState::Paused),
            start + Duration::from_secs(1),
        );
        // A 2 s pause leaves the clock where it froze.
        assert_eq!(
            model.elapsed(start + Duration::from_secs(3)),
            Duration::from_secs(1)
        );
        model.apply(
            &PlayerEvent::State(PlayerState::Playing),
            start + Duration::from_secs(3),
        );
        assert_eq!(
            model.elapsed(start + Duration::from_secs(4)),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn chunk_while_paused_stays_at_zero_until_playing() {
        let start = Instant::now();
        let mut model = playing_model(start);
        model.apply(
            &PlayerEvent::State(PlayerState::Paused),
            start + Duration::from_secs(1),
        );
        model.apply(
            &chunk_event(1, "Two.", Duration::from_secs(10)),
            start + Duration::from_secs(1),
        );
        assert_eq!(
            model.elapsed(start + Duration::from_secs(5)),
            Duration::ZERO
        );
        model.apply(
            &PlayerEvent::State(PlayerState::Playing),
            start + Duration::from_secs(5),
        );
        assert_eq!(
            model.elapsed(start + Duration::from_secs(7)),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn terminal_events_keep_chunks_and_clear_current() {
        for terminal in [
            PlayerEvent::State(PlayerState::Idle),
            PlayerEvent::Finished,
            PlayerEvent::Error("boom".to_string()),
        ] {
            let start = Instant::now();
            let mut model = playing_model(start);
            model.apply(&terminal, start + Duration::from_secs(1));
            assert_eq!(model.state(), PlayerState::Idle, "{terminal:?}");
            assert_eq!(model.current(), None, "{terminal:?}");
            assert_eq!(model.chunks(), ["One.", "Two."].as_slice(), "{terminal:?}");
            assert!(!model.is_active(), "{terminal:?}");
        }
    }

    #[test]
    fn progress_labels() {
        let start = Instant::now();
        let mut model = ReaderModel::new();
        assert_eq!(model.progress_label(), "Nothing to read yet.");
        model.apply(&playlist(&["One.", "Two."]), start);
        // Idle with chunks (no state yet).
        assert_eq!(model.progress_label(), "Not reading");
        model.apply(&PlayerEvent::State(PlayerState::Loading), start);
        assert_eq!(model.progress_label(), "Starting…");
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        assert_eq!(model.progress_label(), "Starting…");
        model.apply(&chunk_event(0, "One.", Duration::from_secs(10)), start);
        assert_eq!(model.progress_label(), "Sentence 1 of 2");
        model.apply(&chunk_event(1, "Two.", Duration::from_secs(10)), start);
        assert_eq!(model.progress_label(), "Sentence 2 of 2");
        model.apply(&PlayerEvent::Finished, start);
        assert_eq!(model.progress_label(), "Not reading");
    }

    #[test]
    fn previous_and_next_at_both_ends() {
        let start = Instant::now();
        let mut model = ReaderModel::new();
        assert!(!model.can_previous());
        assert!(!model.can_next());
        model.apply(&playlist(&["One.", "Two.", "Three."]), start);
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        // Active but no current chunk yet.
        assert!(!model.can_previous());
        assert!(!model.can_next());
        let ten = Duration::from_secs(10);
        model.apply(
            &PlayerEvent::Chunk {
                index: 0,
                count: 3,
                text: "One.".to_string(),
                duration: ten,
            },
            start,
        );
        assert!(!model.can_previous());
        assert!(model.can_next());
        model.apply(
            &PlayerEvent::Chunk {
                index: 2,
                count: 3,
                text: "Three.".to_string(),
                duration: ten,
            },
            start,
        );
        assert!(model.can_previous());
        assert!(!model.can_next());
        // Idle: neither, even with chunks.
        model.apply(&PlayerEvent::State(PlayerState::Idle), start);
        assert!(!model.can_previous());
        assert!(!model.can_next());
    }

    #[test]
    fn dismiss_hides_until_next_playlist() {
        let start = Instant::now();
        let mut model = playing_model(start);
        assert!(model.should_show(true));
        model.dismiss();
        assert!(!model.should_show(true));
        model.apply(&playlist(&["Alpha."]), start + Duration::from_secs(1));
        model.apply(
            &PlayerEvent::State(PlayerState::Playing),
            start + Duration::from_secs(1),
        );
        assert!(model.should_show(true));
    }

    #[test]
    fn undismiss_shows_again() {
        let start = Instant::now();
        let mut model = playing_model(start);
        model.dismiss();
        assert!(!model.should_show(true));
        model.undismiss();
        assert!(model.should_show(true));
    }

    #[test]
    fn hidden_when_setting_off_or_inactive() {
        let start = Instant::now();
        let model = ReaderModel::new();
        assert!(!model.should_show(false));
        let active = playing_model(start);
        assert!(!active.should_show(false));
        assert!(active.should_show(true));
    }

    #[test]
    fn current_word_follows_elapsed() {
        let start = Instant::now();
        let mut model = ReaderModel::new();
        assert_eq!(model.current_word(start), None);
        model.apply(&playlist(&["aaa bbb ccc"]), start);
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        assert_eq!(model.current_word(start), None);
        model.apply(
            &PlayerEvent::Chunk {
                index: 0,
                count: 1,
                text: "aaa bbb ccc".to_string(),
                duration: Duration::from_secs(12),
            },
            start,
        );
        assert_eq!(model.current_word(start + Duration::from_secs(2)), Some(0));
        assert_eq!(model.current_word(start + Duration::from_secs(5)), Some(1));
    }

    #[test]
    fn progress_and_animation() {
        let start = Instant::now();
        let mut model = ReaderModel::new();
        assert_eq!(model.progress(), 0.0);
        assert!(!model.needs_animation());
        model.apply(&playlist(&["One.", "Two."]), start);
        model.apply(&PlayerEvent::State(PlayerState::Playing), start);
        assert_eq!(model.progress(), 0.0);
        assert!(!model.needs_animation());
        model.apply(&chunk_event(0, "One.", Duration::from_secs(10)), start);
        assert_eq!(model.progress(), 0.5);
        assert!(model.needs_animation());
        model.apply(
            &PlayerEvent::State(PlayerState::Paused),
            start + Duration::from_secs(1),
        );
        assert!(!model.needs_animation());
    }
}
