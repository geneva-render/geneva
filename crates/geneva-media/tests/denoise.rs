//! The speech denoiser: as many frames out as in, the sound where it
//! was, and how long a minute takes.

#![cfg(feature = "denoise")]

use std::time::Instant;

use geneva_media::denoise::Denoiser;

/// A voiced burst: harmonics of 120 Hz up to 3 kHz, the shape of a
/// vowel more than of a tone, `from` to `to` seconds, in silence.
fn burst(rate: u32, seconds: f64, from: f64, to: f64) -> Vec<f32> {
    let n = (f64::from(rate) * seconds) as usize;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let mut v = 0.0;
        if t >= from && t < to {
            for k in 1..=25 {
                let f = 120.0 * f64::from(k);
                v += (2.0 * std::f64::consts::PI * f * t).sin() / f64::from(k);
            }
            v *= 0.3;
        }
        out.push(v as f32);
        out.push(v as f32 * 0.8);
    }
    out
}

fn run(rate: u32, input: &[f32]) -> Vec<f32> {
    let mut d = Denoiser::new(rate).expect("model");
    let mut out = Vec::new();
    for chunk in input.chunks(rate as usize * 2) {
        out.extend(d.push(chunk).expect("push"));
    }
    out.extend(d.finish().expect("finish"));
    out
}

/// Where the sound starts: the first 10 ms window with a tenth of the
/// loudest window's energy.
fn onset(rate: u32, s: &[f32]) -> f64 {
    let w = rate as usize / 100 * 2;
    let energies: Vec<f64> = s
        .chunks(w)
        .map(|c| c.iter().map(|v| f64::from(*v) * f64::from(*v)).sum())
        .collect();
    let top = energies.iter().copied().fold(0.0, f64::max);
    energies
        .iter()
        .position(|e| *e > top / 10.0)
        .map_or(f64::NAN, |i| i as f64 / 100.0)
}

#[test]
fn the_count_and_the_timing_survive() {
    let rate = 48_000;
    let input = burst(rate, 3.0, 1.0, 1.4);
    let out = run(rate, &input);
    assert_eq!(out.len(), input.len(), "frames out");
    let (a, b) = (onset(rate, &input), onset(rate, &out));
    assert!((a - b).abs() <= 0.02, "onset moved from {a} to {b} s");
    // The sound starts where it started, to a few samples.
    let first = |s: &[f32]| s.iter().position(|v| v.abs() > 0.02).unwrap_or(0) / 2;
    let (i, o) = (first(&input), first(&out));
    assert!(
        (i as i64 - o as i64).abs() <= i64::from(rate) / 200,
        "first loud sample moved from {i} to {o}"
    );
    // Silence stays silence, and the burst is still there.
    let quiet: f64 = out[..(rate as usize - rate as usize / 100) * 2]
        .iter()
        .map(|v| f64::from(v.abs()))
        .sum::<f64>()
        / f64::from(rate * 2);
    assert!(quiet < 1e-4, "{quiet}");
    let loud: f64 = out[rate as usize * 2..rate as usize * 3]
        .iter()
        .map(|v| f64::from(v.abs()))
        .sum();
    assert!(loud > 100.0, "{loud}");
}

#[test]
fn another_rate_comes_back_at_the_same_length() {
    let rate = 44_100;
    let input = burst(rate, 2.0, 0.5, 0.9);
    let out = run(rate, &input);
    let diff = (out.len() as i64 - input.len() as i64).abs();
    assert!(
        diff <= 64,
        "{} frames out for {} in",
        out.len() / 2,
        input.len() / 2
    );
    let (a, b) = (onset(rate, &input), onset(rate, &out));
    assert!((a - b).abs() <= 0.02, "onset moved from {a} to {b} s");
}

#[test]
fn a_minute_is_timed() {
    let rate = 48_000;
    let mut input = burst(rate, 60.0, 0.0, 60.0);
    // Some noise to work on.
    let mut x = 7u32;
    for v in &mut input {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *v += (x >> 8) as f32 / 16_777_216.0 * 0.02 - 0.01;
    }
    let t = Instant::now();
    let out = run(rate, &input);
    let secs = t.elapsed().as_secs_f64();
    eprintln!(
        "denoise: 60 s of 48 kHz stereo in {secs:.1} s, real-time factor {:.2}",
        secs / 60.0
    );
    assert_eq!(out.len(), input.len());
}
