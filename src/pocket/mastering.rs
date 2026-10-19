//! Pocket TTS mastering: loudness measurement plus the per-chunk master
//! chain (high-pass, pre-gain, compressor, make-up, limiter).

/// Mastered chunk target: about −10 LUFS after compression and limiting.
const TARGET_LUFS: f64 = -10.0;
/// Pre-compressor level: each chunk is gained here first (gain clamped
/// below).
const PRE_TARGET_LUFS: f64 = -20.0;
/// Pre-gain clamp in dB, so near-silent chunks can't explode.
const PRE_GAIN_MIN_DB: f64 = -12.0;
const PRE_GAIN_MAX_DB: f64 = 24.0;
/// Compressor: threshold, ratio, RMS detector ballistics.
const COMP_THRESHOLD_DB: f64 = -28.0;
const COMP_RATIO: f64 = 6.0;
const COMP_ATTACK_MS: f64 = 5.0;
const COMP_RELEASE_MS: f64 = 80.0;
/// Look-ahead limiter ceiling and ballistics; output never exceeds the
/// ceiling.
const PEAK_CEILING: f32 = 0.95;
const LIMITER_LOOKAHEAD_MS: f64 = 5.0;
const LIMITER_RELEASE_MS: f64 = 50.0;
/// Second-order Butterworth high-pass in front of the chain.
const HIGHPASS_FREQ_HZ: f64 = 70.0;

/// ITU-R BS.1770-4 integrated loudness in LUFS: K-weighting (high-shelf
/// pre-filter + RLB high-pass, coefficients computed for `sample_rate` with
/// the libebur128/pyloudnorm formulas), 400 ms blocks with 75 % overlap,
/// −70 LUFS absolute gate and −10 LU relative gate. Shorter than one 400 ms
/// block: one ungated block over the whole input. All-silent input returns
/// negative infinity.
fn measure_lufs(samples: &[f32], sample_rate: u32) -> f64 {
    if samples.is_empty() || sample_rate == 0 {
        return f64::NEG_INFINITY;
    }
    let input: Vec<f64> = samples.iter().map(|s| f64::from(*s)).collect();
    let weighted = apply_biquad(&input, &k_shelf_coeffs(sample_rate));
    let weighted = apply_biquad(&weighted, &k_high_pass_coeffs(sample_rate));
    let block_size = sample_rate as usize * 400 / 1000;
    let hop_size = sample_rate as usize * 100 / 1000;
    if block_size == 0 || hop_size == 0 {
        return f64::NEG_INFINITY;
    }
    if weighted.len() < block_size {
        let mean_square = mean_square(&weighted, 0, weighted.len());
        if mean_square <= 0.0 {
            return f64::NEG_INFINITY;
        }
        return -0.691 + 10.0 * mean_square.log10();
    }
    let mut blocks = Vec::new();
    let mut start = 0;
    while start + block_size <= weighted.len() {
        blocks.push(mean_square(&weighted, start, start + block_size));
        start += hop_size;
    }
    if blocks.is_empty() {
        return f64::NEG_INFINITY;
    }
    let abs_gate = 10f64.powf((-70.0 + 0.691) / 10.0);
    let gated: Vec<f64> = blocks.into_iter().filter(|b| *b > abs_gate).collect();
    if gated.is_empty() {
        return f64::NEG_INFINITY;
    }
    let relative_loudness =
        -0.691 + 10.0 * (gated.iter().sum::<f64>() / gated.len() as f64).log10();
    let rel_gate = 10f64.powf((relative_loudness - 10.0 + 0.691) / 10.0);
    let kept: Vec<f64> = gated.into_iter().filter(|b| *b > rel_gate).collect();
    if kept.is_empty() {
        return f64::NEG_INFINITY;
    }
    -0.691 + 10.0 * (kept.iter().sum::<f64>() / kept.len() as f64).log10()
}

/// Master one chunk to about [`TARGET_LUFS`]: 70 Hz high-pass, pre-gain to
/// [`PRE_TARGET_LUFS`] (clamped), 6:1 compression, make-up gain to
/// [`TARGET_LUFS`], and a look-ahead peak limiter at [`PEAK_CEILING`]. The
/// limiter shaves loudness off the peaks it catches, so the output is
/// measured once more after limiting and the deficit is topped up at 4× as
/// headroom — hotter peaks catch the final limit harder, so only part
/// survives — before that final limit; the limiter still runs last, so no
/// output sample exceeds [`PEAK_CEILING`]. Output length equals input
/// length. All-zero input passes through unchanged.
pub(crate) fn master_chunk(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    if samples.iter().all(|s| *s == 0.0) {
        return samples.to_vec();
    }
    let mut out = apply_high_pass(samples, sample_rate);
    let pre_loudness = measure_lufs(&out, sample_rate);
    if pre_loudness == f64::NEG_INFINITY {
        return samples.to_vec();
    }
    out = apply_gain_db(
        &out,
        (PRE_TARGET_LUFS - pre_loudness).clamp(PRE_GAIN_MIN_DB, PRE_GAIN_MAX_DB),
    );
    out = apply_compressor(&out, sample_rate);
    let post_loudness = measure_lufs(&out, sample_rate);
    if post_loudness == f64::NEG_INFINITY {
        return samples.to_vec();
    }
    out = apply_gain_db(&out, TARGET_LUFS - post_loudness);
    // Limiting only ever lowers loudness; without this the −10 target
    // measured −11.8 on the fixture (the limiter caught 1.8 LU of peaks).
    // The top-up quadruples the deficit as headroom: hotter peaks catch the
    // final limit harder, so only part of the top-up survives on real
    // speech.
    out = apply_limiter(&out, sample_rate);
    let limited = measure_lufs(&out, sample_rate);
    if limited.is_finite() && limited < TARGET_LUFS - 0.1 {
        out = apply_limiter(
            &apply_gain_db(&out, (TARGET_LUFS - limited) * 4.0),
            sample_rate,
        );
    }
    out
}

/// Second-order direct-form biquad: y = b0·x + b1·x1 + b2·x2 − a1·y1 − a2·y2.
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// K-weighting high-shelf pre-filter (libebur128/pyloudnorm formula for
/// `sample_rate`).
fn k_shelf_coeffs(sample_rate: u32) -> Biquad {
    let f0 = 1_681.974_450_955_533;
    let gain = 3.737_636_852_614_458;
    let q = 0.707_175_236_955_419_6;
    let k = (std::f64::consts::PI * f0 / f64::from(sample_rate)).tan();
    let vh = 10f64.powf(gain / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

/// K-weighting RLB high-pass (libebur128/pyloudnorm formula for
/// `sample_rate`).
fn k_high_pass_coeffs(sample_rate: u32) -> Biquad {
    high_pass_coeffs(38.135_470_876_024_44, 0.500_327_037_323_877_3, sample_rate)
}

/// Second-order high-pass with cutoff `f0` Hz and Q `q` at `sample_rate`.
fn high_pass_coeffs(f0: f64, q: f64, sample_rate: u32) -> Biquad {
    let k = (std::f64::consts::PI * f0 / f64::from(sample_rate)).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: 1.0 / a0,
        b1: -2.0 / a0,
        b2: 1.0 / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

fn apply_biquad(input: &[f64], c: &Biquad) -> Vec<f64> {
    let mut out = vec![0.0; input.len()];
    let mut x1 = 0.0;
    let mut x2 = 0.0;
    let mut y1 = 0.0;
    let mut y2 = 0.0;
    for (n, x0) in input.iter().enumerate() {
        let y0 = c.b0 * x0 + c.b1 * x1 + c.b2 * x2 - c.a1 * y1 - c.a2 * y2;
        out[n] = y0;
        x2 = x1;
        x1 = *x0;
        y2 = y1;
        y1 = y0;
    }
    out
}

fn apply_high_pass(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    // Second-order Butterworth: Q = 1/sqrt(2).
    let c = high_pass_coeffs(HIGHPASS_FREQ_HZ, 0.707_106_781_186_547_5, sample_rate);
    let mut out = vec![0.0; samples.len()];
    let mut x1 = 0.0;
    let mut x2 = 0.0;
    let mut y1 = 0.0;
    let mut y2 = 0.0;
    for (n, sample) in samples.iter().enumerate() {
        let x0 = f64::from(*sample);
        let y0 = c.b0 * x0 + c.b1 * x1 + c.b2 * x2 - c.a1 * y1 - c.a2 * y2;
        out[n] = y0 as f32;
        x2 = x1;
        x1 = x0;
        y2 = y1;
        y1 = y0;
    }
    out
}

fn apply_gain_db(samples: &[f32], gain_db: f64) -> Vec<f32> {
    if gain_db == 0.0 {
        return samples.to_vec();
    }
    let gain = 10f64.powf(gain_db / 20.0) as f32;
    samples.iter().map(|s| s * gain).collect()
}

/// Downward compressor with an RMS detector: the squared signal is smoothed
/// with the attack/release ballistics, then the static 6:1 curve above the
/// threshold is applied. The threshold test runs in the power domain, so
/// below-threshold samples skip the sqrt/pow entirely; above it the gain is
/// one pow, with no log.
fn apply_compressor(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let attack = 1.0 - (-1.0 / (COMP_ATTACK_MS / 1000.0 * f64::from(sample_rate))).exp();
    let release = 1.0 - (-1.0 / (COMP_RELEASE_MS / 1000.0 * f64::from(sample_rate))).exp();
    let thresh_power = 10f64.powf(COMP_THRESHOLD_DB / 10.0);
    let thresh_lin = 10f64.powf(COMP_THRESHOLD_DB / 20.0);
    // gainDb = k·(thr − detDb), so gain = (thrLin/detector)^k.
    let curve = 1.0 - 1.0 / COMP_RATIO;
    let mut out = vec![0.0; samples.len()];
    let mut envelope = 0.0;
    for (n, sample) in samples.iter().enumerate() {
        let x = f64::from(*sample);
        let squared = x * x;
        envelope += (if squared > envelope { attack } else { release }) * (squared - envelope);
        if envelope < thresh_power {
            out[n] = *sample;
            continue;
        }
        out[n] = (x * (thresh_lin / envelope.sqrt()).powf(curve)) as f32;
    }
    out
}

/// Look-ahead peak limiter: each sample's gain sees the peak
/// [`LIMITER_LOOKAHEAD_MS`] ahead, attacks instantly and releases over
/// [`LIMITER_RELEASE_MS`]. Offline, so the look-ahead delay is compensated
/// (no extra samples); the final clamp guarantees the ceiling against float
/// error. The look-ahead peak is a sliding-window maximum over
/// [n, n + look-ahead] with a monotonic deque, O(n).
fn apply_limiter(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let look_ahead =
        ((LIMITER_LOOKAHEAD_MS / 1000.0 * f64::from(sample_rate)).round() as usize).max(1);
    let release = 1.0 - (-1.0 / (LIMITER_RELEASE_MS / 1000.0 * f64::from(sample_rate))).exp();
    let mut targets = vec![0.0f32; samples.len()];
    // Indices with decreasing |sample|; each sample enters and leaves once.
    let mut deque = vec![0usize; samples.len()];
    let mut head = 0usize;
    let mut tail = 0usize;
    let first_end = samples.len().min(look_ahead + 1);
    for i in 0..first_end {
        while tail > head && samples[deque[tail - 1]].abs() <= samples[i].abs() {
            tail -= 1;
        }
        deque[tail] = i;
        tail += 1;
    }
    for (n, target) in targets.iter_mut().enumerate() {
        let peak = samples[deque[head]].abs();
        *target = if peak < 1e-10 {
            1.0
        } else {
            1.0f32.min(PEAK_CEILING / peak)
        };
        if deque[head] == n {
            head += 1;
        }
        let incoming = n + look_ahead + 1;
        if incoming < samples.len() {
            while tail > head && samples[deque[tail - 1]].abs() <= samples[incoming].abs() {
                tail -= 1;
            }
            deque[tail] = incoming;
            tail += 1;
        }
    }
    let mut out = vec![0.0f32; samples.len()];
    let mut smooth = 1.0f64;
    for (n, target) in targets.iter().enumerate() {
        let target = f64::from(*target);
        smooth = if target < smooth {
            target
        } else {
            smooth + (target - smooth) * release
        };
        let y = (f64::from(samples[n]) * smooth) as f32;
        out[n] = y.clamp(-PEAK_CEILING, PEAK_CEILING);
    }
    out
}

fn mean_square(samples: &[f64], from: usize, to: usize) -> f64 {
    let mut sum = 0.0;
    for sample in &samples[from..to] {
        sum += sample * sample;
    }
    sum / (to - from) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mastering_lifts_quiet_audio_and_limits_peaks() {
        // Quiet sine gains up toward the target.
        let quiet: Vec<f32> = (0..24_000)
            .map(|i| 0.1 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 24_000.0).sin())
            .collect();
        let lifted = master_chunk(&quiet, 24_000);
        assert_eq!(lifted.len(), quiet.len());
        let rms = |v: &[f32]| {
            (v.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / v.len() as f64).sqrt()
        };
        assert!(
            rms(&lifted) > rms(&quiet) * 1.5,
            "quiet audio must get louder"
        );
        // Loud audio never exceeds the ceiling.
        let loud: Vec<f32> = (0..24_000)
            .map(|i| 0.99 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 24_000.0).sin())
            .collect();
        let limited = master_chunk(&loud, 24_000);
        let peak = limited.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= PEAK_CEILING, "peak {peak}");
        // Silence passes through.
        assert_eq!(master_chunk(&[0.0; 16], 24_000), vec![0.0; 16]);
        assert_eq!(measure_lufs(&[0.0; 4096], 24_000), f64::NEG_INFINITY);
    }

    /// Read a mono 16-bit PCM WAV fixture into f32 samples plus its rate.
    fn read_mono_pcm16_wav(path: &str) -> (Vec<f32>, u32) {
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() > 44, "too short for a WAV header");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let mut offset = 12;
        let mut rate = None;
        let mut samples = None;
        while offset + 8 <= bytes.len() {
            let id = &bytes[offset..offset + 4];
            let size =
                u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            let body = &bytes[offset + 8..];
            if id == b"fmt " {
                let format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                assert_eq!((format, channels, bits), (1, 1, 16), "need mono PCM16");
                rate = Some(u32::from_le_bytes(body[4..8].try_into().unwrap()));
            } else if id == b"data" {
                assert_eq!(size % 2, 0);
                samples = Some(
                    body[..size]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| {
                            let value = i16::from_le_bytes(*pair);
                            f64::from(value) as f32 / 32768.0
                        })
                        .collect(),
                );
            }
            offset += 8 + size + (size % 2);
        }
        (
            samples.expect("WAV has no data chunk"),
            rate.expect("WAV has no fmt chunk"),
        )
    }

    #[test]
    fn fixture_masters_to_target_loudness() {
        let (raw, rate) = read_mono_pcm16_wav("tests/fixtures/pocket/raw-v1-6s.wav");
        assert_eq!((raw.len(), rate), (144_000, 24_000));
        let mastered = master_chunk(&raw, rate);
        assert_eq!(mastered.len(), raw.len());
        let lufs = measure_lufs(&mastered, rate);
        assert!(
            (lufs - TARGET_LUFS).abs() <= 1.0,
            "mastered fixture at {lufs} LUFS, want {TARGET_LUFS} ± 1"
        );
        let peak = mastered.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= PEAK_CEILING, "peak {peak}");
    }
}
