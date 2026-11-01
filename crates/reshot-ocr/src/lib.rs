//! Offline Windows.Media.Ocr recognition and RU/EN line-by-line AUTO merge.

use anyhow::{Context, Result, bail};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::System::WinRT::IBufferByteAccess;
use windows::core::{HSTRING, Interface};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrLanguage {
    Auto,
    Ru,
    En,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OcrRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub rect: OcrRect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub words: Vec<OcrWord>,
}

#[derive(Clone)]
struct EngineLine {
    line: OcrLine,
    top: f32,
    bottom: f32,
    latin: usize,
    cyrillic: usize,
}

/// Return the OCR lines in reading order. Unavailable engines and invalid images yield none.
pub fn recognize(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    mode: OcrLanguage,
) -> Vec<OcrLine> {
    match try_recognize(bgra, width, height, stride, mode) {
        Ok(lines) => lines,
        Err(error) => {
            log::warn!("OCR failed: {error:#}");
            Vec::new()
        }
    }
}

pub fn is_available() -> bool {
    OcrEngine::AvailableRecognizerLanguages().is_ok_and(|langs| langs.Size().is_ok_and(|n| n > 0))
}

pub fn available_language_tags() -> Vec<String> {
    OcrEngine::AvailableRecognizerLanguages()
        .ok()
        .into_iter()
        .flat_map(|langs| langs.into_iter())
        .filter_map(|language| language.LanguageTag().ok().map(|tag| tag.to_string()))
        .collect()
}

pub fn supports_language(tag: &str) -> bool {
    Language::CreateLanguage(&HSTRING::from(tag))
        .is_ok_and(|lang| OcrEngine::IsLanguageSupported(&lang).unwrap_or(false))
}

/// Fallible variant for callers that need diagnostics rather than an empty result.
pub fn try_recognize(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    mode: OcrLanguage,
) -> Result<Vec<OcrLine>> {
    if width == 0 || height == 0 || !is_available() {
        return Ok(Vec::new());
    }
    let row_bytes = (width as usize)
        .checked_mul(4)
        .context("OCR width overflow")?;
    if stride < row_bytes {
        bail!("OCR stride shorter than BGRA row");
    }
    let required = stride
        .checked_mul(height as usize - 1)
        .and_then(|n| n.checked_add(row_bytes))
        .context("OCR image size overflow")?;
    if bgra.len() < required {
        bail!("OCR BGRA buffer is too short");
    }
    let max = OcrEngine::MaxImageDimension()?;
    if width > max || height > max {
        bail!("OCR image exceeds Windows maximum dimension {max}");
    }
    let scale = upscale_factor(width, height, max);
    let scaled_width = width * scale;
    let scaled_height = height * scale;
    let pixel_bytes = (scaled_width as usize)
        .checked_mul(scaled_height as usize)
        .and_then(|n| n.checked_mul(4))
        .context("OCR buffer too large")?;
    let buffer = Buffer::Create(pixel_bytes.try_into().context("OCR buffer too large")?)?;
    let access: IBufferByteAccess = buffer.cast()?;
    let ptr = unsafe { access.Buffer()? };
    if ptr.is_null() {
        bail!("Windows OCR buffer pointer is null");
    }
    // The bitmap copies from this buffer, so fill it directly instead of copying a second image.
    let pixels = unsafe { std::slice::from_raw_parts_mut(ptr, pixel_bytes) };
    if scale > 1 {
        upscale_bgra_into(bgra, width, height, stride, scale, pixels);
    } else {
        compact_bgra_into(bgra, width, height, stride, pixels);
    }
    buffer.SetLength(pixel_bytes as u32)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        scaled_width as i32,
        scaled_height as i32,
        BitmapAlphaMode::Ignore,
    )?;
    let ru = if mode != OcrLanguage::En {
        run_engine("ru", &bitmap, scale)?
    } else {
        Vec::new()
    };
    let en = if mode != OcrLanguage::Ru {
        run_engine("en", &bitmap, scale)?
    } else {
        Vec::new()
    };
    let lines = match mode {
        OcrLanguage::Auto => merge(ru, en),
        OcrLanguage::Ru => ru.into_iter().map(|line| line.line).collect(),
        OcrLanguage::En => en.into_iter().map(|line| line.line).collect(),
    };
    Ok(lines)
}

fn create_engine(tag: &str) -> Option<OcrEngine> {
    if let Ok(language) = Language::CreateLanguage(&HSTRING::from(tag))
        && OcrEngine::IsLanguageSupported(&language).unwrap_or(false)
        && let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language)
    {
        return Some(engine);
    }
    if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
        return Some(engine);
    }
    let available = OcrEngine::AvailableRecognizerLanguages().ok()?;
    if available.Size().ok()? > 0 {
        OcrEngine::TryCreateFromLanguage(&available.GetAt(0).ok()?).ok()
    } else {
        None
    }
}

fn run_engine(tag: &str, bitmap: &SoftwareBitmap, scale: u32) -> Result<Vec<EngineLine>> {
    let Some(engine) = create_engine(tag) else {
        return Ok(Vec::new());
    };
    let result = engine.RecognizeAsync(bitmap)?.get()?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let mut words = Vec::new();
        for word in line.Words()? {
            let rect = word.BoundingRect()?;
            let divisor = scale as f32;
            words.push(OcrWord {
                text: word.Text()?.to_string(),
                rect: OcrRect {
                    left: rect.X / divisor,
                    top: rect.Y / divisor,
                    right: (rect.X + rect.Width) / divisor,
                    bottom: (rect.Y + rect.Height) / divisor,
                },
            });
        }
        if words.is_empty() {
            continue;
        }
        let top = words
            .iter()
            .map(|word| word.rect.top)
            .fold(f32::INFINITY, f32::min);
        let bottom = words
            .iter()
            .map(|word| word.rect.bottom)
            .fold(f32::NEG_INFINITY, f32::max);
        let text = line.Text()?.to_string();
        lines.push(EngineLine {
            line: OcrLine { words },
            top,
            bottom,
            latin: text.chars().filter(|c| c.is_ascii_alphabetic()).count(),
            cyrillic: text
                .chars()
                .filter(|&c| ('\u{0400}'..='\u{04ff}').contains(&c))
                .count(),
        });
    }
    Ok(lines)
}

fn merge(ru: Vec<EngineLine>, en: Vec<EngineLine>) -> Vec<OcrLine> {
    if ru.is_empty() {
        return en.into_iter().map(|line| line.line).collect();
    }
    if en.is_empty() {
        return ru.into_iter().map(|line| line.line).collect();
    }
    ru.into_iter()
        .map(|r| {
            // RU reports a line's real script reliably. Latin-dominant lines use EN to avoid
            // Cyrillic look-alike substitutions; Cyrillic and mixed lines stay with RU.
            if r.latin > r.cyrillic
                && let Some(match_line) = best_overlap(&r, &en)
            {
                return match_line.line.clone();
            }
            r.line
        })
        .collect()
}

fn best_overlap<'a>(target: &EngineLine, candidates: &'a [EngineLine]) -> Option<&'a EngineLine> {
    let mut best = None;
    let mut best_overlap = 0.0f32;
    for candidate in candidates {
        let overlap = target.bottom.min(candidate.bottom) - target.top.max(candidate.top);
        if overlap > best_overlap {
            best_overlap = overlap;
            best = Some(candidate);
        }
    }
    if best_overlap >= 0.4 * (target.bottom - target.top) {
        best
    } else {
        None
    }
}

fn compact_bgra_into(source: &[u8], width: u32, height: u32, stride: usize, out: &mut [u8]) {
    let row_bytes = width as usize * 4;
    for y in 0..height as usize {
        out[y * row_bytes..(y + 1) * row_bytes]
            .copy_from_slice(&source[y * stride..y * stride + row_bytes]);
    }
}

#[cfg(test)]
fn compact_bgra(source: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
    let mut out = vec![0; width as usize * height as usize * 4];
    compact_bgra_into(source, width, height, stride, &mut out);
    out
}

/// How much to enlarge the image before recognition. Windows OCR finds lines by their
/// height: UI text 9-10 px tall at 100 % display scaling is mostly not detected at all at its
/// own size (measured on 2400x1300 crops of Segoe UI: 3-38 % of the words found at 1x,
/// 76-100 % at 2x, 95-100 % at 3x), while every word it does detect is read correctly. The
/// factor is the largest that keeps both sides within the engine's limit (10000 px) and the
/// pixel budget, so a large selection is enlarged as well, not only a small one.
fn upscale_factor(width: u32, height: u32, max: u32) -> u32 {
    (1..=MAX_UPSCALE)
        .rev()
        .find(|&s| {
            width * s <= max
                && height * s <= max
                && u64::from(width * s) * u64::from(height * s) <= MAX_UPSCALED_PIXELS
        })
        .unwrap_or(1)
}

const MAX_UPSCALE: u32 = 3;
/// Above this the two engine passes take seconds; a full 4480x1440 desktop at 2x is 25.8 MP.
const MAX_UPSCALED_PIXELS: u64 = 30_000_000;

/// Enlarges by an integer factor with a Mitchell-Netravali cubic, which keeps small UI text
/// legible without the hard steps of nearest-neighbour. Separable (rows, then columns) with
/// the weights computed once per phase, and split into bands of rows across the cores: the
/// direct 16-tap version took over a second for a large selection, before the engines ran.
fn upscale_bgra_into(
    source: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    scale: u32,
    out: &mut [u8],
) {
    let (w, h, s) = (width as usize, height as usize, scale as usize);
    let (out_w, out_h) = (w * s, h * s);
    // Output pixel `o` maps to source `(o + 0.5) / s - 0.5`: four taps from `o / s - 1`
    // (or one earlier for phases left of the source pixel's centre), same weights per phase.
    let taps: Vec<(isize, [f32; 4])> = (0..s)
        .map(|phase| {
            let src = (phase as f32 + 0.5) / s as f32 - 0.5;
            let first = src.floor() as isize - 1;
            let mut wts = [0.0f32; 4];
            for (k, wt) in wts.iter_mut().enumerate() {
                *wt = cubic(src - (first + k as isize) as f32);
            }
            let sum: f32 = wts.iter().sum();
            wts.iter_mut().for_each(|wt| *wt /= sum);
            (first, wts)
        })
        .collect();
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;

    // One source row widened to `out_w`, as floats (B, G, R per pixel; alpha is ignored).
    let widen = |y: usize, row: &mut [f32]| {
        let src = &source[y * stride..];
        for ox in 0..out_w {
            let (first, wts) = &taps[ox % s];
            let base = (ox / s) as isize + first;
            let mut acc = [0.0f32; 3];
            for (k, wt) in wts.iter().enumerate() {
                let px = &src[clamp(base + k as isize, w) * 4..];
                for c in 0..3 {
                    acc[c] += px[c] as f32 * wt;
                }
            }
            row[ox * 3..ox * 3 + 3].copy_from_slice(&acc);
        }
    };

    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let band = out_h.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (b, chunk) in out.chunks_mut(band * out_w * 4).enumerate() {
            let (widen, taps) = (&widen, &taps);
            scope.spawn(move || {
                // The four widened source rows the current output row reads, by source row.
                let mut cache: [(usize, Vec<f32>); 4] =
                    std::array::from_fn(|_| (usize::MAX, vec![0.0; out_w * 3]));
                for (i, dst) in chunk.chunks_mut(out_w * 4).enumerate() {
                    let oy = b * band + i;
                    let (first, wts) = &taps[oy % s];
                    let base = (oy / s) as isize + first;
                    for k in 0..4 {
                        let y = clamp(base + k as isize, h);
                        let slot = &mut cache[y % 4];
                        if slot.0 != y {
                            widen(y, &mut slot.1);
                            slot.0 = y;
                        }
                    }
                    let rows: [&[f32]; 4] = std::array::from_fn(|k| {
                        cache[clamp(base + k as isize, h) % 4].1.as_slice()
                    });
                    for ox in 0..out_w {
                        for c in 0..3 {
                            let v = rows[0][ox * 3 + c] * wts[0]
                                + rows[1][ox * 3 + c] * wts[1]
                                + rows[2][ox * 3 + c] * wts[2]
                                + rows[3][ox * 3 + c] * wts[3];
                            dst[ox * 4 + c] = v.round().clamp(0.0, 255.0) as u8;
                        }
                        dst[ox * 4 + 3] = 255;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
fn upscale_bgra(source: &[u8], width: u32, height: u32, stride: usize, scale: u32) -> Vec<u8> {
    let mut out = vec![0; width as usize * height as usize * scale as usize * scale as usize * 4];
    upscale_bgra_into(source, width, height, stride, scale, &mut out);
    out
}

fn cubic(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        (7.0 * x * x * x - 12.0 * x * x + 16.0 / 3.0) / 6.0
    } else if x < 2.0 {
        (-7.0 / 3.0 * x * x * x + 12.0 * x * x - 20.0 * x + 32.0 / 3.0) / 6.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn bench_ocr_buffer_preparation() {
        use std::hint::black_box;
        use std::time::Instant;
        let (width, height, scale) = (1200u32, 800u32, 3u32);
        let source = vec![127u8; width as usize * height as usize * 4];
        let len = source.len() * scale as usize * scale as usize;
        let mut direct = vec![0; len];
        let start = Instant::now();
        for _ in 0..20 {
            let intermediate = upscale_bgra(&source, width, height, width as usize * 4, scale);
            direct.copy_from_slice(&intermediate);
            black_box(&direct);
        }
        println!(
            "OCR prepare via intermediate: {:.2} ms",
            start.elapsed().as_secs_f64() * 50.0
        );
        let start = Instant::now();
        for _ in 0..20 {
            upscale_bgra_into(
                &source,
                width,
                height,
                width as usize * 4,
                scale,
                &mut direct,
            );
            black_box(&direct);
        }
        println!(
            "OCR prepare direct: {:.2} ms",
            start.elapsed().as_secs_f64() * 50.0
        );
    }

    fn line(text: &str, top: f32, bottom: f32, latin: usize, cyrillic: usize) -> EngineLine {
        EngineLine {
            line: OcrLine {
                words: vec![OcrWord {
                    text: text.into(),
                    rect: OcrRect {
                        left: 0.0,
                        top,
                        right: 10.0,
                        bottom,
                    },
                }],
            },
            top,
            bottom,
            latin,
            cyrillic,
        }
    }

    #[test]
    fn auto_selects_english_only_for_latin_dominant_ru_lines() {
        let ru = vec![
            line("ип", 0.0, 10.0, 3, 1),
            line("Привет", 20.0, 30.0, 0, 6),
            line("mixed", 40.0, 50.0, 3, 3),
        ];
        let en = vec![
            line("un", 1.0, 11.0, 2, 0),
            line("wrong", 21.0, 31.0, 5, 0),
            line("other", 41.0, 51.0, 5, 0),
        ];
        let result = merge(ru, en);
        assert_eq!(
            result
                .iter()
                .map(|line| line.words[0].text.as_str())
                .collect::<Vec<_>>(),
            ["un", "Привет", "mixed"]
        );
    }

    #[test]
    fn auto_requires_real_vertical_overlap_and_keeps_ru_line_count() {
        let ru = vec![line("Latin", 0.0, 10.0, 5, 0)];
        let en = vec![
            line("wrong", 8.0, 18.0, 5, 0),
            line("extra", 20.0, 30.0, 5, 0),
        ];
        let result = merge(ru, en);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].words[0].text, "Latin");
    }

    #[test]
    fn empty_language_pass_falls_back_to_other_pass() {
        assert_eq!(
            merge(vec![], vec![line("English", 0.0, 10.0, 7, 0)])[0].words[0].text,
            "English"
        );
        assert_eq!(
            merge(vec![line("Русский", 0.0, 10.0, 0, 7)], vec![])[0].words[0].text,
            "Русский"
        );
    }

    #[test]
    fn padded_rows_compact_and_upscale_without_crossing_stride() {
        let source = [
            10, 20, 30, 255, 99, 99, 99, 99, 40, 50, 60, 255, 88, 88, 88, 88,
        ];
        assert_eq!(
            compact_bgra(&source, 1, 2, 8),
            [10, 20, 30, 255, 40, 50, 60, 255]
        );
        let scaled = upscale_bgra(&source, 1, 2, 8, 2);
        assert_eq!(scaled.len(), 2 * 4 * 4);
        let mut different_padding = source;
        different_padding[4..8].fill(0);
        different_padding[12..16].fill(255);
        assert_eq!(scaled, upscale_bgra(&different_padding, 1, 2, 8, 2));
    }

    #[test]
    fn upscaling_keeps_flat_areas_and_edges_in_place() {
        // Left half black, right half white, 8x3 with a padded stride.
        let (w, h, stride) = (8usize, 3usize, 40usize);
        let mut src = vec![0u8; stride * h];
        for y in 0..h {
            for x in 4..w {
                src[y * stride + x * 4..y * stride + x * 4 + 3].fill(255);
            }
        }
        for scale in [2u32, 3] {
            let s = scale as usize;
            let out = upscale_bgra(&src, w as u32, h as u32, stride, scale);
            assert_eq!(out.len(), w * s * h * s * 4);
            let at = |x: usize, y: usize| out[(y * w * s + x) * 4];
            for y in 0..h * s {
                assert_eq!(at(0, y), 0, "flat black stays black at x{scale}");
                assert_eq!(at(w * s - 1, y), 255, "flat white stays white at x{scale}");
                // The edge sits between source pixels 3 and 4, at output x = 4 * s.
                assert!(
                    at(4 * s - 1, y) < 128 && at(4 * s, y) > 127,
                    "edge stays put at x{scale}"
                );
            }
        }
    }

    #[test]
    fn the_factor_is_the_largest_that_fits_the_engine() {
        assert_eq!(upscale_factor(800, 600, 10_000), 3);
        assert_eq!(
            upscale_factor(2400, 1300, 10_000),
            3,
            "a large selection is enlarged too"
        );
        assert_eq!(upscale_factor(4480, 1440, 10_000), 2);
        assert_eq!(
            upscale_factor(5760, 1080, 10_000),
            1,
            "past the engine's limit"
        );
        assert_eq!(
            upscale_factor(3000, 3000, 10_000),
            1,
            "past the pixel budget"
        );
    }
}
