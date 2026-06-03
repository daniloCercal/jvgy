//! Trivial energy-based voice activity detection over PCM s16le. Used to skip
//! near-silent chunks so we don't spend STT/tokens on dead air (cost control).
//! Not a speech classifier — just an RMS gate.

/// Root-mean-square amplitude of an s16le buffer, normalized to `[0.0, 1.0]`.
pub fn rms_s16le(pcm: &[u8]) -> f64 {
    if pcm.len() < 2 {
        return 0.0;
    }
    let mut sum_sq = 0.0f64;
    let mut n = 0u64;
    for frame in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([frame[0], frame[1]]) as f64;
        sum_sq += sample * sample;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    let rms = (sum_sq / n as f64).sqrt();
    rms / 32768.0
}

/// True if the chunk's energy exceeds `threshold` (default ~0.012 works well for
/// voice vs. ambient). Tune via `VAD_THRESHOLD`.
pub fn is_voiced(pcm: &[u8], threshold: f64) -> bool {
    rms_s16le(pcm) >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_not_voiced() {
        let silence = vec![0u8; 3200];
        assert_eq!(rms_s16le(&silence), 0.0);
        assert!(!is_voiced(&silence, 0.012));
    }

    #[test]
    fn loud_signal_is_voiced() {
        // Alternating +/- 8000 amplitude.
        let mut pcm = Vec::new();
        for i in 0..800 {
            let s: i16 = if i % 2 == 0 { 8000 } else { -8000 };
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        assert!(is_voiced(&pcm, 0.012));
    }
}
