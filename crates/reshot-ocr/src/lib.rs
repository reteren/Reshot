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
    let scale = if width.max(height) < 1200 && width * 2 <= max && height * 2 <= max {
        2
    } else {
        1
    };
    let pixels = if scale == 2 {
        upscale_bgra(bgra, width, height, stride)
    } else {
        compact_bgra(bgra, width, height, stride)
    };
    let scaled_width = width * scale;
    let scaled_height = height * scale;
    let buffer = Buffer::Create(pixels.len().try_into().context("OCR buffer too large")?)?;
    let access: IBufferByteAccess = buffer.cast()?;
    let ptr = unsafe { access.Buffer()? };
    if ptr.is_null() {
        bail!("Windows OCR buffer pointer is null");
    }
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, pixels.len());
    }
    buffer.SetLength(pixels.len() as u32)?;
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

fn compact_bgra(source: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
    let row_bytes = width as usize * 4;
    let mut out = vec![0; row_bytes * height as usize];
    for y in 0..height as usize {
        out[y * row_bytes..(y + 1) * row_bytes]
            .copy_from_slice(&source[y * stride..y * stride + row_bytes]);
    }
    out
}

// Skia's high-quality upsampling is cubic. Mitchell-Netravali keeps small UI text legible
// without the hard edges of nearest-neighbour scaling. Both engines share this one bitmap.
fn upscale_bgra(source: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
    let out_width = width as usize * 2;
    let out_height = height as usize * 2;
    let mut out = vec![0; out_width * out_height * 4];
    for y in 0..out_height {
        let sy = (y as f32 + 0.5) / 2.0 - 0.5;
        let iy = sy.floor() as i32;
        for x in 0..out_width {
            let sx = (x as f32 + 0.5) / 2.0 - 0.5;
            let ix = sx.floor() as i32;
            let mut sum = [0.0f32; 4];
            let mut weight_sum = 0.0f32;
            for dy in -1..=2 {
                let source_y = (iy + dy).clamp(0, height as i32 - 1) as usize;
                let wy = cubic(sy - (iy + dy) as f32);
                for dx in -1..=2 {
                    let source_x = (ix + dx).clamp(0, width as i32 - 1) as usize;
                    let weight = wy * cubic(sx - (ix + dx) as f32);
                    let offset = source_y * stride + source_x * 4;
                    for channel in 0..4 {
                        sum[channel] += source[offset + channel] as f32 * weight;
                    }
                    weight_sum += weight;
                }
            }
            let offset = (y * out_width + x) * 4;
            for channel in 0..4 {
                out[offset + channel] = (sum[channel] / weight_sum).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
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
        let scaled = upscale_bgra(&source, 1, 2, 8);
        assert_eq!(scaled.len(), 2 * 4 * 4);
        let mut different_padding = source;
        different_padding[4..8].fill(0);
        different_padding[12..16].fill(255);
        assert_eq!(scaled, upscale_bgra(&different_padding, 1, 2, 8));
    }
}
