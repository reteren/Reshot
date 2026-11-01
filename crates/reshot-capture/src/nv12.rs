use std::mem::ManuallyDrop;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoContext1,
    ID3D11VideoProcessor, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use crate::native::{D3d, copy_rows};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Crop {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

pub(crate) struct Nv12Converter {
    source_width: u32,
    source_height: u32,
    crop: Crop,
    input: ID3D11Texture2D,
    output: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    processor: ID3D11VideoProcessor,
    input_view: ID3D11VideoProcessorInputView,
    output_view: ID3D11VideoProcessorOutputView,
    video_context: ID3D11VideoContext,
}

impl Nv12Converter {
    pub(crate) fn new(
        d3d: &D3d,
        source_width: u32,
        source_height: u32,
        crop: Crop,
    ) -> Result<Self> {
        if crop.width < 2
            || crop.height < 2
            || !crop.width.is_multiple_of(2)
            || !crop.height.is_multiple_of(2)
        {
            bail!("NV12 requires even dimensions of at least 2x2");
        }
        if crop
            .left
            .checked_add(crop.width)
            .is_none_or(|x| x > source_width)
            || crop
                .top
                .checked_add(crop.height)
                .is_none_or(|y| y > source_height)
        {
            bail!("NV12 crop exceeds monitor");
        }
        let video_device = d3d
            .device
            .cast::<windows::Win32::Graphics::Direct3D11::ID3D11VideoDevice>()?;
        let video_context = d3d.context.cast::<ID3D11VideoContext>()?;
        let video_context1 = d3d.context.cast::<ID3D11VideoContext1>()?;
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: source_width,
            InputHeight: source_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: crop.width,
            OutputHeight: crop.height,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        // SAFETY: The descriptor is fully initialized and remains live for the call.
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&desc) }?;
        // SAFETY: The enumerator belongs to this D3D device.
        let input_caps =
            unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM) }?;
        let output_caps = unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12) }?;
        if input_caps & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 == 0
            || output_caps & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0
        {
            bail!("video processor does not support BGRA to NV12");
        }
        // SAFETY: Rate conversion index zero is the first enumerated processor mode.
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }?;

        let input_desc = D3D11_TEXTURE2D_DESC {
            Width: source_width,
            Height: source_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let output_desc = D3D11_TEXTURE2D_DESC {
            Width: crop.width,
            Height: crop.height,
            Format: DXGI_FORMAT_NV12,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..input_desc
        };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..output_desc
        };
        let mut input = None;
        let mut output = None;
        let mut staging = None;
        // SAFETY: These texture descriptions use supported formats and initialized output slots.
        unsafe {
            d3d.device
                .CreateTexture2D(&input_desc, None, Some(&mut input))?;
            d3d.device
                .CreateTexture2D(&output_desc, None, Some(&mut output))?;
            d3d.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        }
        let input = input.context("NV12 input texture is null")?;
        let output = output.context("NV12 output texture is null")?;
        let staging = staging.context("NV12 staging texture is null")?;
        let input_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut input_view = None;
        let mut output_view = None;
        // SAFETY: The views refer to textures and the enumerator created on this device.
        unsafe {
            video_device.CreateVideoProcessorInputView(
                &input,
                &enumerator,
                &input_view_desc,
                Some(&mut input_view),
            )?;
            video_device.CreateVideoProcessorOutputView(
                &output,
                &enumerator,
                &output_view_desc,
                Some(&mut output_view),
            )?;
        }
        let input_view = input_view.context("NV12 input view is null")?;
        let output_view = output_view.context("NV12 output view is null")?;
        let source_rect = RECT {
            left: crop.left as i32,
            top: crop.top as i32,
            right: (crop.left + crop.width) as i32,
            bottom: (crop.top + crop.height) as i32,
        };
        let destination_rect = RECT {
            left: 0,
            top: 0,
            right: crop.width as i32,
            bottom: crop.height as i32,
        };
        // SAFETY: All processor state refers to live objects and valid crop rectangles.
        unsafe {
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            video_context.VideoProcessorSetStreamSourceRect(
                &processor,
                0,
                true,
                Some(&source_rect),
            );
            video_context.VideoProcessorSetStreamDestRect(
                &processor,
                0,
                true,
                Some(&destination_rect),
            );
            video_context.VideoProcessorSetOutputTargetRect(
                &processor,
                true,
                Some(&destination_rect),
            );
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video_context1.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            video_context1.VideoProcessorSetOutputColorSpace1(
                &processor,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            );
        }
        Ok(Self {
            source_width,
            source_height,
            crop,
            input,
            output,
            staging,
            processor,
            input_view,
            output_view,
            video_context,
        })
    }

    pub(crate) fn convert(&self, d3d: &D3d, texture: &ID3D11Texture2D) -> Result<Vec<u8>> {
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `texture` is a live WGC frame texture.
        unsafe { texture.GetDesc(&mut source_desc) };
        if source_desc.Width != self.source_width
            || source_desc.Height != self.source_height
            || source_desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
        {
            bail!("WGC frame dimensions or format changed");
        }
        // SAFETY: The source and input textures have identical dimensions and BGRA format.
        unsafe { d3d.context.CopyResource(&self.input, texture) };
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
            ..Default::default()
        };
        // SAFETY: The input and output views remain live during the processor operation.
        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &self.output_view,
                0,
                std::slice::from_ref(&stream),
            )
        };
        // SAFETY: This field owns a cloned COM reference that the C call did not take ownership of.
        unsafe { ManuallyDrop::drop(&mut stream.pInputSurface) };
        result?;
        // SAFETY: The output and staging textures have identical NV12 dimensions and format.
        unsafe { d3d.context.CopyResource(&self.staging, &self.output) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: The staging texture was created for CPU reads.
        unsafe {
            d3d.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }?;
        let width = self.crop.width as usize;
        let height = self.crop.height as usize;
        let mut pixels = vec![0; width * height * 3 / 2];
        let pitch = mapped.RowPitch as usize;
        let copied = (|| {
            if pitch < width {
                bail!("NV12 staging pitch shorter than width");
            }
            // SAFETY: A mapped NV12 texture has `height` Y rows followed by `height / 2` UV rows,
            // both with `RowPitch` bytes, and the destination has exactly 1.5 bytes per pixel.
            unsafe {
                copy_rows(
                    mapped.pData.cast(),
                    pitch,
                    &mut pixels,
                    0,
                    width,
                    width,
                    height,
                )?;
                copy_rows(
                    (mapped.pData as *const u8).add(pitch * height),
                    pitch,
                    &mut pixels,
                    width * height,
                    width,
                    width,
                    height / 2,
                )?;
            }
            Ok(())
        })();
        // SAFETY: The staging texture is mapped exactly once above.
        unsafe { d3d.context.Unmap(&self.staging, 0) };
        copied?;
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    use windows::Win32::Graphics::Direct3D11::D3D11_SUBRESOURCE_DATA;

    #[test]
    #[ignore]
    fn gpu_red_crop_is_bt709_limited() -> Result<()> {
        let d3d = D3d::create()?;
        let (source_width, source_height) = (64u32, 64u32);
        let mut pixels = vec![0u8; source_width as usize * source_height as usize * 4];
        for row in pixels.chunks_exact_mut(source_width as usize * 4) {
            for pixel in row[16 * 4..48 * 4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: source_width,
            Height: source_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: source_width * 4,
            SysMemSlicePitch: source_width * source_height * 4,
        };
        let mut texture = None;
        // SAFETY: The source pixel buffer and texture descriptor remain live for creation.
        unsafe {
            d3d.device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
        }?;
        let texture = texture.context("test texture is null")?;
        let converter = Nv12Converter::new(
            &d3d,
            source_width,
            source_height,
            Crop {
                left: 16,
                top: 16,
                width: 32,
                height: 32,
            },
        )?;
        let output = converter.convert(&d3d, &texture)?;
        let center_y = output[16 * 32 + 16];
        let uv_offset = 32 * 32 + 8 * 32 + 16;
        let (u, v) = (output[uv_offset], output[uv_offset + 1]);
        println!("GPU red: Y={center_y}, U={u}, V={v}");
        assert!((center_y as i32 - 63).abs() <= 3);
        assert!((u as i32 - 102).abs() <= 4);
        assert!((v as i32 - 240).abs() <= 4);
        Ok(())
    }

    /// A flat colour must come out flat: a driver that dithers its NV12 output adds noise the
    /// encoder then spends bits and CPU on.
    #[test]
    #[ignore]
    fn gpu_flat_colours_stay_flat() -> Result<()> {
        let d3d = D3d::create()?;
        let (w, h) = (256u32, 128u32);
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            // Left half a mid grey, right half a soft blue, as UI backgrounds are.
            let x = i % w as usize;
            px.copy_from_slice(if x < 128 {
                &[0x80, 0x80, 0x80, 255]
            } else {
                &[0xC8, 0x8A, 0x5A, 255]
            });
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: w * 4,
            SysMemSlicePitch: w * h * 4,
        };
        let mut texture = None;
        // SAFETY: The pixels and descriptor stay live during texture creation.
        unsafe {
            d3d.device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
        }?;
        let texture = texture.context("flat texture is null")?;
        let converter = Nv12Converter::new(
            &d3d,
            w,
            h,
            Crop {
                left: 0,
                top: 0,
                width: w,
                height: h,
            },
        )?;
        let out = converter.convert(&d3d, &texture)?;
        let distinct = |x0: usize, x1: usize| {
            let mut v: Vec<u8> = (0..h as usize)
                .flat_map(|y| out[y * w as usize + x0..y * w as usize + x1].to_vec())
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (grey, blue) = (distinct(8, 120), distinct(136, 248));
        println!("Y values in the grey half: {grey:?}; in the blue half: {blue:?}");
        assert_eq!(grey.len(), 1, "grey half is dithered: {grey:?}");
        assert_eq!(blue.len(), 1, "blue half is dithered: {blue:?}");
        Ok(())
    }

    #[test]
    #[ignore]
    fn gpu_color_bars_match_ffmpeg_bt709() -> Result<()> {
        let d3d = D3d::create()?;
        let (source_width, source_height) = (224u32, 128u32);
        let crop = Crop {
            left: 16,
            top: 16,
            width: 192,
            height: 96,
        };
        let colors = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [0, 255, 255, 255],
            [255, 255, 0, 255],
            [255, 0, 255, 255],
        ];
        let mut source = vec![32u8; source_width as usize * source_height as usize * 4];
        let mut bgra = Vec::with_capacity(crop.width as usize * crop.height as usize * 4);
        for y in 0..crop.height as usize {
            for x in 0..crop.width as usize {
                let color = colors[(x / 32 + 6 * (y / 32)) % colors.len()];
                let index =
                    ((crop.top as usize + y) * source_width as usize + crop.left as usize + x) * 4;
                source[index..index + 4].copy_from_slice(&color);
                bgra.extend_from_slice(&color);
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: source_width,
            Height: source_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: source.as_ptr().cast(),
            SysMemPitch: source_width * 4,
            SysMemSlicePitch: source_width * source_height * 4,
        };
        let mut texture = None;
        // SAFETY: The source pixels and descriptor remain live during texture creation.
        unsafe {
            d3d.device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
        }?;
        let texture = texture.context("color-bar texture is null")?;
        let converter = Nv12Converter::new(&d3d, source_width, source_height, crop)?;
        let nv12 = converter.convert(&d3d, &texture)?;

        let directory =
            std::env::temp_dir().join(format!("reshot-nv12-verification-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let bgra_path = directory.join("bars.bgra");
        let nv12_path = directory.join("bars.nv12");
        let reference_path = directory.join("reference.yuv");
        let actual_path = directory.join("actual.yuv");
        fs::write(&bgra_path, bgra)?;
        fs::write(&nv12_path, nv12)?;
        let reference = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pixel_format",
                "bgra",
                "-video_size",
                "192x96",
                "-i",
            ])
            .arg(&bgra_path)
            .args([
                "-vf",
                "scale=out_color_matrix=bt709:out_range=tv",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
            ])
            .arg(&reference_path)
            .status()?;
        let actual = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pixel_format",
                "nv12",
                "-video_size",
                "192x96",
                "-i",
            ])
            .arg(&nv12_path)
            .args(["-pix_fmt", "yuv420p", "-f", "rawvideo"])
            .arg(&actual_path)
            .status()?;
        assert!(reference.success() && actual.success());
        let reference = fs::read(&reference_path)?;
        let actual = fs::read(&actual_path)?;
        assert_eq!(reference.len(), actual.len());
        let squared_error: f64 = reference
            .iter()
            .zip(actual.iter())
            .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
            .sum();
        let mse = squared_error / reference.len() as f64;
        let psnr = 10.0 * (255.0 * 255.0 / mse).log10();
        println!("GPU NV12 vs ffmpeg BT.709 YUV PSNR: {psnr:.2} dB");
        assert!(
            psnr > 40.0,
            "GPU output differs from ffmpeg BT.709 reference"
        );
        for path in [bgra_path, nv12_path, reference_path, actual_path] {
            fs::remove_file(path)?;
        }
        fs::remove_dir(directory)?;
        Ok(())
    }
}
