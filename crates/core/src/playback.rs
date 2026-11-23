//! The player of 朗读 and 播客, as far as it can be decided without a sound device: the speeds offered,
//! where a seek lands and how it is told, where a kept podcast goes on from, and `Stretch`, which plays
//! speech faster or slower without raising or lowering the voice.
//!
//! Positions are samples of the sound as it was made (`speech::SAMPLE_RATE`), whatever the speed: the
//! clock and the bar tell where in the podcast one is, not how long is left to hear.

use crate::speech::SAMPLE_RATE;

/// The speeds offered, slowest first.
pub const SPEEDS: [f32; 5] = [0.75, 1., 1.25, 1.5, 2.];

/// The speed a setting stands for: the nearest one offered (0, an older settings file, is 1×).
pub fn speed(setting: f32) -> f32 {
    if !setting.is_finite() || setting <= 0. { return 1. }
    SPEEDS.into_iter().min_by(|a, b| (a - setting).abs().total_cmp(&(b - setting).abs())).unwrap_or(1.)
}

/// A speed as its button shows it: `1×`, `1.25×`, `0.75×`.
pub fn speed_label(speed: f32) -> String {
    let text = format!("{speed:.2}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
}

/// A position as a clock: `0:42`, `12:05`, `1:02:09`.
pub fn clock(samples: usize) -> String {
    let seconds = samples / SAMPLE_RATE as usize;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

/// How far along `length` the position `at` is, 0 to 1, as the bar fills.
pub fn fraction(at: usize, length: usize) -> f32 {
    if length == 0 { return 0. }
    (at as f64 / length as f64).clamp(0., 1.) as f32
}

/// The position a point `x` on a bar from `left`, `width` wide, stands for in `length`: the pointer may
/// go past either end while it is held.
pub fn seek_to(x: f32, left: f32, width: f32, length: usize) -> usize {
    if width <= 0. { return 0 }
    let at = ((x - left) / width).clamp(0., 1.);
    (at as f64 * length as f64).round() as usize
}

/// What heard a little before stopping is said again: a podcast taken up again goes back this far.
const AGAIN: usize = SAMPLE_RATE as usize * 2;
/// Stopped this near the end, it was heard: taken up again, it starts over.
const NEAR_END: usize = SAMPLE_RATE as usize * 3;

/// Where a kept podcast of `length` goes on from when it was last left at `place`: a moment before it,
/// so the sentence broken off is heard whole; from the start when it was heard to its end (or the place
/// is of some other, longer sound).
pub fn resume_at(place: usize, length: usize) -> usize {
    if place + NEAR_END >= length { return 0 }
    place.saturating_sub(AGAIN)
}

/// The length of a piece of sound `Stretch` works on: 40 ms, longer than the lowest voice's period.
pub const FRAME: usize = SAMPLE_RATE as usize / 25;
/// What it gives out at each step: half a frame, the frames overlapping by half.
pub const HOP: usize = FRAME / 2;
/// How far from its place in the sound a frame may be taken so it follows on from the one before.
pub const SEEK: usize = SAMPLE_RATE as usize / 100;

/// Speech played faster or slower at the same pitch (WSOLA): the sound is cut into overlapping frames
/// that are laid down a fixed `HOP` apart, taken from the sound `speed` times as far apart, each moved by
/// up to `SEEK` to where its wave best follows on from the frame before, so the voice has no echo or
/// rattle. Fed the sound it reads from; gives out `HOP` samples a step.
#[derive(Clone, Debug)]
pub struct Stretch {
    /// Where in the sound the next frame belongs: what is being heard.
    at: f64,
    /// Where the frame laid down last was taken from.
    previous: Option<usize>,
    /// The second half of that frame, which the next one is laid over.
    tail: Vec<f32>,
    window: Vec<f32>,
}

impl Stretch {
    /// Start at sample `at` of the sound.
    pub fn new(at: usize) -> Stretch {
        // A periodic Hann window: two of them half a frame apart add up to one.
        let window = (0..FRAME).map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / FRAME as f32).cos()).collect();
        Stretch { at: at as f64, previous: None, tail: vec![0.; HOP], window }
    }

    /// The sample of the sound being heard.
    pub fn position(&self) -> usize {
        self.at.max(0.) as usize
    }

    /// The next `HOP` samples at `speed`, from the sound `input` holds from its sample `base` on; `None`
    /// while the sound needed has not come yet, or after its end once `done` (all of it came).
    pub fn step(&mut self, speed: f32, input: &[f32], base: usize, done: bool) -> Option<Vec<f32>> {
        let end = base + input.len();
        let at = self.at.max(0.) as usize;
        if done && at >= end { return None }
        let natural = self.previous.map(|previous| previous + HOP);
        let reach = (at + SEEK + FRAME).max(natural.map_or(0, |natural| natural + FRAME));
        if !done && reach > end { return None }
        // Past what came, once all of it came, is silence.
        let get = |i: usize| if i >= base && i < end { input[i - base] } else { 0. };
        let from = match natural {
            None => at,
            Some(natural) => {
                // The frame whose wave is most like what would have followed the last one: compared on every
                // fourth sample at every second offset, which is near enough for a voice and cheap.
                let low = at.saturating_sub(SEEK).max(base);
                let mut best = (f32::MIN, at);
                for from in (low..=at + SEEK).step_by(2) {
                    let likeness: f32 = (0..FRAME).step_by(4).map(|n| get(from + n) * get(natural + n)).sum();
                    if likeness > best.0 { best = (likeness, from) }
                }
                best.1
            }
        };
        let frame: Vec<f32> = (0..FRAME).map(|n| get(from + n) * self.window[n]).collect();
        let out = (0..HOP).map(|n| self.tail[n] + frame[n]).collect();
        self.tail.copy_from_slice(&frame[HOP..]);
        self.previous = Some(from);
        self.at += HOP as f64 * f64::from(speed);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_and_clocks_read_as_shown() {
        assert_eq!((speed(0.), speed(1.3), speed(9.), speed(f32::NAN)), (1., 1.25, 2., 1.));
        assert_eq!(SPEEDS.map(speed_label), ["0.75×", "1×", "1.25×", "1.5×", "2×"]);
        let second = SAMPLE_RATE as usize;
        assert_eq!((clock(0), clock(42 * second + 5), clock(725 * second), clock(3729 * second)), ("0:00".into(), "0:42".into(), "12:05".into(), "1:02:09".into()));
    }

    #[test]
    fn a_seek_lands_where_the_bar_was_pressed() {
        assert_eq!(seek_to(150., 100., 200., 1000), 250);
        // Held past either end, it stays at that end.
        assert_eq!((seek_to(50., 100., 200., 1000), seek_to(900., 100., 200., 1000), seek_to(5., 0., 0., 1000)), (0, 1000, 0));
        assert_eq!((fraction(250, 1000), fraction(5, 0), fraction(2000, 1000)), (0.25, 0., 1.));
    }

    #[test]
    fn a_kept_podcast_goes_on_a_moment_before_it_was_left() {
        let second = SAMPLE_RATE as usize;
        assert_eq!(resume_at(60 * second, 300 * second), 58 * second);
        assert_eq!(resume_at(second, 300 * second), 0);
        // Heard to its end, or near it: from the start.
        assert_eq!((resume_at(299 * second, 300 * second), resume_at(400 * second, 300 * second)), (0, 0));
    }

    /// Run a stretch over `input` at `speed` to its end.
    fn stretched(input: &[f32], speed: f32) -> Vec<f32> {
        let mut stretch = Stretch::new(0);
        let mut out = vec![];
        while let Some(step) = stretch.step(speed, input, 0, true) { out.extend(step) }
        out
    }

    /// How often a sound crosses zero going up, per sample: its pitch, for a plain tone.
    fn pitch(sound: &[f32]) -> f32 {
        sound.windows(2).filter(|pair| pair[0] < 0. && pair[1] >= 0.).count() as f32 / sound.len() as f32
    }

    #[test]
    fn faster_or_slower_the_voice_keeps_its_pitch() {
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize * 2).map(|n| (std::f32::consts::TAU * 220. * n as f32 / SAMPLE_RATE as f32).sin() * 0.5).collect();
        for speed in [0.75, 1.25, 1.5, 2.] {
            let out = stretched(&tone, speed);
            let expected = tone.len() as f32 / speed;
            assert!((out.len() as f32 - expected).abs() < expected * 0.03, "{speed}: {} for {expected}", out.len());
            // Away from the ends, the tone is the same tone: no octave up at 2×, no rattle.
            let middle = &out[FRAME * 2..out.len() - FRAME * 2];
            assert!((pitch(middle) / pitch(&tone) - 1.).abs() < 0.02, "{speed}: {} for {}", pitch(middle), pitch(&tone));
            // Laid over one another the frames add up to the tone's own loudness, not more.
            assert!(middle.iter().all(|s| s.abs() <= 0.55), "{speed}");
        }
    }

    #[test]
    fn a_stretch_waits_for_sound_still_coming_and_says_where_it_is() {
        let sound = vec![0.1; FRAME];
        let mut stretch = Stretch::new(0);
        // Not enough to take a frame from yet.
        assert_eq!(stretch.step(1.5, &sound, 0, false), None);
        let more = vec![0.1; FRAME * 4];
        assert_eq!(stretch.step(1.5, &more, 0, false).map(|out| out.len()), Some(HOP));
        assert_eq!(stretch.position(), HOP * 3 / 2);
        // From a later start, reading what is kept from a later sample on.
        let mut later = Stretch::new(1000);
        assert!(later.step(2., &more, 900, true).is_some() && later.position() == 1000 + HOP * 2);
        assert_eq!(Stretch::new(5000).step(1., &more, 900, true), None);
    }
}
