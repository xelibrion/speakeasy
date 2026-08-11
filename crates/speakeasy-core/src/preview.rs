//! What a preview tick should do with the audio it is holding.
//!
//! Split from the engine because this is the only genuinely subtle rule in the
//! feature — when text may be committed and then never revised — and it is
//! worth deciding with no threads, no ports and no clock involved.

use crate::ports::SpeechSegment;

/// How much silence must follow speech before it counts as finished.
const SILENCE_TAIL_S: f32 = 0.5;

/// Below this there is not enough audio to be worth a decode.
const MIN_SLICE_S: f32 = 0.3;

/// Speech this long without a pause is committed anyway, so a slice never
/// approaches whisper's 30-second padded window — past which it processes a
/// second window and every decode costs roughly double.
const FORCE_COMMIT_S: f32 = 25.0;

/// Kept after the end of a span, so a commit does not clip the last consonant.
const COMMIT_PAD_S: f32 = 0.2;

/// What to do with the audio a tick is holding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tick {
    /// Not enough audio, or no speech in it. Decode nothing.
    Skip,
    /// Speech is still in progress. Decode all of it and show it as the live
    /// tail, committing nothing.
    Live,
    /// The utterance has finished. Decode up to `through_seconds`, commit that
    /// text permanently, and advance past it.
    Commit { through_seconds: f32 },
}

/// Decide what a tick holding `slice_seconds` of audio containing `spans`
/// should do.
pub fn decide(slice_seconds: f32, spans: &[SpeechSegment]) -> Tick {
    if slice_seconds < MIN_SLICE_S {
        return Tick::Skip;
    }

    let Some(last) = spans.last() else {
        return Tick::Skip;
    };

    let settled = last.end + SILENCE_TAIL_S <= slice_seconds;
    if settled || slice_seconds >= FORCE_COMMIT_S {
        return Tick::Commit {
            through_seconds: (last.end + COMMIT_PAD_S).min(slice_seconds),
        };
    }

    Tick::Live
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f32, end: f32) -> SpeechSegment {
        SpeechSegment { start, end }
    }

    #[test]
    fn too_little_audio_is_not_worth_a_decode() {
        assert_eq!(decide(0.1, &[span(0.0, 0.1)]), Tick::Skip);
    }

    /// The silence gate. Without it whisper is handed a quiet buffer and
    /// invents words to fill it.
    #[test]
    fn silence_decodes_nothing() {
        assert_eq!(decide(3.0, &[]), Tick::Skip);
    }

    #[test]
    fn speech_still_running_shows_a_live_tail() {
        // Speech ends 0.1s before the buffer does — the user is mid-word.
        assert_eq!(decide(2.0, &[span(0.0, 1.9)]), Tick::Live);
    }

    #[test]
    fn speech_followed_by_silence_is_committed() {
        // Speech ended a full second ago.
        assert_eq!(
            decide(3.0, &[span(0.0, 2.0)]),
            Tick::Commit {
                through_seconds: 2.2
            }
        );
    }

    /// A long unbroken monologue must still commit, or the slice grows past
    /// whisper's 30-second window and every decode doubles in cost.
    #[test]
    fn unbroken_speech_is_committed_before_the_thirty_second_cliff() {
        assert!(matches!(
            decide(25.5, &[span(0.0, 25.4)]),
            Tick::Commit { .. }
        ));
    }

    /// Padding must never reach past the audio we actually hold.
    #[test]
    fn a_commit_never_reaches_past_the_audio() {
        let Tick::Commit { through_seconds } = decide(25.5, &[span(0.0, 25.45)]) else {
            panic!("expected a commit");
        };
        assert!(through_seconds <= 25.5);
    }
}
