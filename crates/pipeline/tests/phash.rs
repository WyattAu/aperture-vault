//! Perceptual hashing against real decodable images: the same image
//! hashes identically, a brightness/size perturbation hashes close
//! (Hamming ≤ 10 — the near-duplicate threshold), and structurally
//! distinct images hash far apart.

// Fixtures are allowed to assert: an `expect` here is the test stating
// what it assumes, the honest form, and the workspace lint denies it
// everywhere a caller would write it.
#![allow(clippy::expect_used)]

use aperture_pipeline::phash::{hamming, hash_rgba, near_duplicate};
use image::DynamicImage;

fn rgba_from(draw: impl Fn(u32, u32) -> [u8; 3], width: u32, height: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8({
        let mut buffer = image::RgbImage::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            *pixel = image::Rgb(draw(x, y));
        }
        buffer
    });
    img.to_rgba8().into_raw()
}

/// A structured scene: gradients plus sinusoidal texture — enough
/// spectrum that the hash bits are signal, not float noise. (A pure
/// linear gradient has a degenerate DCT: two coefficients carry
/// everything and the median threshold flips on rounding.)
/// A structured scene over NORMALIZED coordinates — the same scene at
/// any resolution, so a resize is truly a resize of the same content.
/// A 1/f spectral scene: sinusoids across a frequency grid with
/// amplitude falling as 1/(kx+ky+1) and deterministic phases — the
/// amplitude decay real photographs have, which keeps the DCT
/// coefficients spread and the median stable. Resolution-independent by
/// construction (defined over normalized coordinates).
fn one_f(xn: f64, yn: f64) -> f64 {
    let mut sum = 0.0;
    let terms = [
        (1.0, 0.0, 40.0),
        (0.0, 1.0, 32.0),
        (2.0, 1.0, 18.0),
        (1.0, 3.0, 12.0),
        (4.0, 0.0, 9.0),
        (0.0, 5.0, 7.0),
        (3.0, 4.0, 5.0),
        (6.0, 2.0, 4.0),
        (2.0, 7.0, 3.0),
        (8.0, 1.0, 2.0),
        (5.0, 6.0, 2.0),
        (11.0, 3.0, 1.5),
    ];
    for (kx, ky, amplitude) in terms {
        let phase = (kx * 7.0 + ky * 13.0) * core::f64::consts::FRAC_PI_4;
        sum += amplitude * (2.0 * core::f64::consts::PI * (kx * xn + ky * yn) + phase).sin();
    }
    sum
}

fn scene(x: u32, y: u32, width: u32, height: u32) -> [u8; 3] {
    let xn = f64::from(x) / f64::from(width.saturating_sub(1).max(1));
    let yn = f64::from(y) / f64::from(height.saturating_sub(1).max(1));
    let wave = one_f(xn, yn);
    let clamp = |v: f64| v.clamp(0.0, 255.0) as u8;
    [
        clamp(128.0 + wave),
        clamp(128.0 + yn * 100.0),
        clamp(255.0 - xn * 255.0),
    ]
}

#[test]
fn hashing_is_deterministic() {
    let a = rgba_from(|x, y| scene(x, y, 256, 256), 256, 256);
    assert_eq!(hash_rgba(&a, 256, 256), hash_rgba(&a, 256, 256));
    assert_eq!(hash_rgba(&a, 256, 256).len(), 16, "64 bits as 16 hex");
}

#[test]
fn a_perturbation_is_a_near_duplicate() {
    let base = rgba_from(|x, y| scene(x, y, 256, 256), 256, 256);
    // Slightly brighter: the classic "same photo, different exposure".
    let brighter = rgba_from(
        |x, y| {
            let [r, g, b] = scene(x, y, 256, 256);
            [
                (f64::from(r) / 2.0 + 64.0).min(255.0) as u8,
                (f64::from(g) / 2.0 + 64.0).min(255.0) as u8,
                (f64::from(b) / 2.0 + 32.0).min(255.0) as u8,
            ]
        },
        256,
        256,
    );
    let h1 = hash_rgba(&base, 256, 256);
    let h2 = hash_rgba(&brighter, 256, 256);
    assert!(
        near_duplicate(&h1, &h2),
        "a brightness nudge must stay within the threshold: hamming {}",
        hamming(&h1, &h2)
    );
}

#[test]
fn distinct_structures_are_not_near_duplicates() {
    let gradient = rgba_from(|x, y| scene(x, y, 256, 256), 256, 256);
    let checkerboard = rgba_from(
        |x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            }
        },
        256,
        256,
    );
    let h1 = hash_rgba(&gradient, 256, 256);
    let h2 = hash_rgba(&checkerboard, 256, 256);
    assert!(
        hamming(&h1, &h2) > 10,
        "a gradient and a checkerboard are different photos"
    );
}

#[test]
fn a_resized_copy_stays_a_near_duplicate() {
    // The re-encode case: same photo, smaller file.
    let base = rgba_from(|x, y| scene(x, y, 256, 256), 256, 256);
    let small = rgba_from(|x, y| scene(x, y, 128, 128), 128, 128);
    let h1 = hash_rgba(&base, 256, 256);
    let h2 = hash_rgba(&small, 128, 128);
    assert!(
        near_duplicate(&h1, &h2),
        "a resized copy must stay within the threshold: hamming {}",
        hamming(&h1, &h2)
    );
}

#[test]
fn hamming_helpers_are_exact() {
    assert_eq!(hamming("0000", "0000"), 0);
    assert_eq!(hamming("0001", "0000"), 1);
    assert_eq!(hamming("ffff", "0000"), 16);
    assert!(near_duplicate("0123456789abcdef", "0123456789abcdef"));
    assert!(!near_duplicate("0123456789abcdef", "fedcba9876543210"));
}
