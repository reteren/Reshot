//! Deterministic ffmpeg grammar. The command line is passed directly to CreateProcessW.

pub struct FfmpegArgs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264Tuning {
    pub preset: Option<&'static str>,
    pub tune: Option<&'static str>,
    pub extra_args: Option<&'static str>,
}

impl FfmpegArgs {
    pub fn default_tuning(encoder: &str) -> H264Tuning {
        match encoder {
            "libx264" => H264Tuning {
                preset: Some("veryfast"),
                tune: Some("zerolatency"),
                extra_args: None,
            },
            "h264_nvenc" => H264Tuning {
                preset: Some("p4"),
                tune: Some("ll"),
                extra_args: None,
            },
            "h264_qsv" => H264Tuning {
                preset: Some("veryfast"),
                tune: None,
                extra_args: None,
            },
            "h264_amf" => H264Tuning {
                preset: None,
                tune: None,
                extra_args: Some("-quality speed"),
            },
            "h264_mf" => H264Tuning {
                preset: None,
                tune: None,
                extra_args: Some("-scenario live_streaming"),
            },
            _ => H264Tuning {
                preset: None,
                tune: None,
                extra_args: None,
            },
        }
    }

    fn tuning(tuning: H264Tuning) -> String {
        let mut result = String::new();
        if let Some(preset) = tuning.preset.filter(|x| !x.trim().is_empty()) {
            result.push_str(&format!(" -preset {preset}"));
        }
        if let Some(tune) = tuning.tune.filter(|x| !x.trim().is_empty()) {
            result.push_str(&format!(" -tune {tune}"));
        }
        if let Some(extra) = tuning.extra_args.filter(|x| !x.trim().is_empty()) {
            result.push_str(&format!(" {}", extra.trim()));
        }
        result
    }

    pub fn video(
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        encoder: &str,
        output_path: &str,
        tuning: H264Tuning,
    ) -> String {
        format!(
            "-hide_banner -loglevel error -y -f rawvideo -pixel_format bgra -video_size {width}x{height} -framerate {fps} -i - -an -c:v {encoder}{} -b:v {bitrate} -pix_fmt yuv420p -movflags +faststart \"{output_path}\"",
            Self::tuning(tuning)
        )
    }

    pub fn audio(sample_rate: u32, channels: u16, bitrate: u32, output_path: &str) -> String {
        format!(
            "-hide_banner -loglevel error -y -f s16le -ar {sample_rate} -ac {channels} -i - -c:a aac -b:a {bitrate} -movflags +faststart \"{output_path}\""
        )
    }

    pub fn h264_probe(width: u32, height: u32, encoder: &str, tuning: H264Tuning) -> String {
        format!(
            "-hide_banner -loglevel error -f lavfi -i color=black:s={width}x{height}:r=10:d=0.2 -an -c:v {encoder}{} -pix_fmt yuv420p -f null -",
            Self::tuning(tuning)
        )
    }

    pub fn mux(
        video_only_mp4: &str,
        pcm_paths: &[&str],
        output_path: &str,
        sample_rate: u32,
        channels: u16,
        audio_bitrate: u32,
    ) -> String {
        let mut args = format!("-hide_banner -loglevel error -y -i \"{video_only_mp4}\"");
        if pcm_paths.is_empty() {
            args.push_str(&format!(" -c copy -movflags +faststart \"{output_path}\""));
            return args;
        }
        for path in pcm_paths {
            args.push_str(&format!(
                " -f s16le -ar {sample_rate} -ac {channels} -i \"{path}\""
            ));
        }
        if pcm_paths.len() == 1 {
            args.push_str(&format!(" -map 0:v:0 -map 1:a:0 -c:v copy -c:a aac -b:a {audio_bitrate} -shortest -movflags +faststart \"{output_path}\""));
            return args;
        }
        args.push_str(" -filter_complex \"");
        for index in 1..=pcm_paths.len() {
            args.push_str(&format!("[{index}:a]"));
        }
        args.push_str(&format!(
            "amix=inputs={}:duration=longest:normalize=0[aout]\"",
            pcm_paths.len()
        ));
        args.push_str(&format!(" -map 0:v:0 -map \"[aout]\" -c:v copy -c:a aac -b:a {audio_bitrate} -movflags +faststart \"{output_path}\""));
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: H264Tuning = H264Tuning {
        preset: None,
        tune: None,
        extra_args: None,
    };

    #[test]
    fn video_builds_documented_command() {
        assert_eq!(
            FfmpegArgs::video(
                1920,
                1080,
                60,
                8_000_000,
                "libx264",
                r"C:\Videos\recording.mp4",
                NONE
            ),
            r#"-hide_banner -loglevel error -y -f rawvideo -pixel_format bgra -video_size 1920x1080 -framerate 60 -i - -an -c:v libx264 -b:v 8000000 -pix_fmt yuv420p -movflags +faststart "C:\Videos\recording.mp4""#
        );
    }

    #[test]
    fn audio_builds_documented_command() {
        assert_eq!(
            FfmpegArgs::audio(48_000, 2, 192_000, r"C:\Audio\track.m4a"),
            r#"-hide_banner -loglevel error -y -f s16le -ar 48000 -ac 2 -i - -c:a aac -b:a 192000 -movflags +faststart "C:\Audio\track.m4a""#
        );
    }

    #[test]
    fn h264_probe_builds_documented_command_and_supports_64_square() {
        assert_eq!(
            FfmpegArgs::h264_probe(1920, 1080, "h264_nvenc", NONE),
            "-hide_banner -loglevel error -f lavfi -i color=black:s=1920x1080:r=10:d=0.2 -an -c:v h264_nvenc -pix_fmt yuv420p -f null -"
        );
        assert_eq!(
            FfmpegArgs::h264_probe(64, 64, "h264_nvenc", NONE),
            "-hide_banner -loglevel error -f lavfi -i color=black:s=64x64:r=10:d=0.2 -an -c:v h264_nvenc -pix_fmt yuv420p -f null -"
        );
    }

    #[test]
    fn probe_numeric_arguments_are_locale_independent() {
        let args = FfmpegArgs::h264_probe(1920, 1080, "h264_qsv", NONE);
        assert!(args.contains("color=black:s=1920x1080:r=10:d=0.2"));
        assert!(!args.contains("1.920"));
        assert!(!args.contains("1.080"));
    }

    #[test]
    fn mux_with_no_tracks_copies_video() {
        assert_eq!(
            FfmpegArgs::mux(
                r"C:\Temp\video-only.mp4",
                &[],
                r"C:\Videos\final.mp4",
                48_000,
                2,
                192_000
            ),
            r#"-hide_banner -loglevel error -y -i "C:\Temp\video-only.mp4" -c copy -movflags +faststart "C:\Videos\final.mp4""#
        );
    }

    #[test]
    fn mux_with_one_track_maps_audio_and_uses_shortest() {
        assert_eq!(
            FfmpegArgs::mux(
                r"C:\Temp\video-only.mp4",
                &[r"C:\Temp\system.pcm"],
                r"C:\Videos\final.mp4",
                48_000,
                2,
                192_000
            ),
            r#"-hide_banner -loglevel error -y -i "C:\Temp\video-only.mp4" -f s16le -ar 48000 -ac 2 -i "C:\Temp\system.pcm" -map 0:v:0 -map 1:a:0 -c:v copy -c:a aac -b:a 192000 -shortest -movflags +faststart "C:\Videos\final.mp4""#
        );
    }

    #[test]
    fn mux_with_two_tracks_mixes_audio() {
        assert_eq!(
            FfmpegArgs::mux(
                r"C:\Temp\video-only.mp4",
                &[r"C:\Temp\system.pcm", r"C:\Temp\microphone.pcm"],
                r"C:\Videos\final.mp4",
                48_000,
                2,
                192_000
            ),
            r#"-hide_banner -loglevel error -y -i "C:\Temp\video-only.mp4" -f s16le -ar 48000 -ac 2 -i "C:\Temp\system.pcm" -f s16le -ar 48000 -ac 2 -i "C:\Temp\microphone.pcm" -filter_complex "[1:a][2:a]amix=inputs=2:duration=longest:normalize=0[aout]" -map 0:v:0 -map "[aout]" -c:v copy -c:a aac -b:a 192000 -movflags +faststart "C:\Videos\final.mp4""#
        );
    }

    #[test]
    fn mux_with_three_tracks_generalizes_audio_mix() {
        assert_eq!(
            FfmpegArgs::mux(
                r"C:\Temp\video-only.mp4",
                &[
                    r"C:\Temp\system.pcm",
                    r"C:\Temp\microphone.pcm",
                    r"C:\Temp\application.pcm"
                ],
                r"C:\Videos\final.mp4",
                44_100,
                1,
                128_000
            ),
            r#"-hide_banner -loglevel error -y -i "C:\Temp\video-only.mp4" -f s16le -ar 44100 -ac 1 -i "C:\Temp\system.pcm" -f s16le -ar 44100 -ac 1 -i "C:\Temp\microphone.pcm" -f s16le -ar 44100 -ac 1 -i "C:\Temp\application.pcm" -filter_complex "[1:a][2:a][3:a]amix=inputs=3:duration=longest:normalize=0[aout]" -map 0:v:0 -map "[aout]" -c:v copy -c:a aac -b:a 128000 -movflags +faststart "C:\Videos\final.mp4""#
        );
    }

    #[test]
    fn paths_with_spaces_remain_quoted() {
        let args = FfmpegArgs::video(
            1280,
            720,
            30,
            4_000_000,
            "h264_nvenc",
            r"C:\My Videos\recording one.mp4",
            NONE,
        );
        assert!(args.ends_with(r#""C:\My Videos\recording one.mp4""#));
        assert!(!args.contains(r"-movflags +faststart C:\My Videos"));
    }

    #[test]
    fn mux_numeric_arguments_are_locale_independent() {
        let args = FfmpegArgs::mux(
            r"C:\Temp\video-only.mp4",
            &[r"C:\Temp\system.pcm", r"C:\Temp\microphone.pcm"],
            r"C:\Videos\final.mp4",
            44_100,
            2,
            128_000,
        );
        assert!(args.contains("-ar 44100 -ac 2"));
        assert!(args.contains("amix=inputs=2"));
        assert!(args.contains("-b:a 128000"));
        assert!(!args.contains("44.100"));
        assert!(!args.contains("128.000"));
    }

    #[test]
    fn default_tuning_matches_each_encoder() {
        for (name, preset, tune, extra) in [
            ("libx264", Some("veryfast"), Some("zerolatency"), None),
            ("h264_nvenc", Some("p4"), Some("ll"), None),
            ("h264_qsv", Some("veryfast"), None, None),
            ("h264_amf", None, None, Some("-quality speed")),
            ("h264_mf", None, None, Some("-scenario live_streaming")),
        ] {
            assert_eq!(
                FfmpegArgs::default_tuning(name),
                H264Tuning {
                    preset,
                    tune,
                    extra_args: extra
                }
            );
        }
    }

    #[test]
    fn video_tuning_includes_preset_tune_and_mf_scenario() {
        assert_eq!(
            FfmpegArgs::video(
                1920,
                1080,
                60,
                8_000_000,
                "libx264",
                r"C:\Videos\recording.mp4",
                FfmpegArgs::default_tuning("libx264")
            ),
            r#"-hide_banner -loglevel error -y -f rawvideo -pixel_format bgra -video_size 1920x1080 -framerate 60 -i - -an -c:v libx264 -preset veryfast -tune zerolatency -b:v 8000000 -pix_fmt yuv420p -movflags +faststart "C:\Videos\recording.mp4""#
        );
        let mf = FfmpegArgs::video(
            2560,
            1440,
            60,
            17_694_720,
            "h264_mf",
            r"C:\Videos\recording.mp4",
            FfmpegArgs::default_tuning("h264_mf"),
        );
        assert!(mf.contains("-c:v h264_mf -scenario live_streaming -b:v 17694720"));
    }

    #[test]
    fn probes_include_mf_amf_nvenc_tuning() {
        assert!(
            FfmpegArgs::h264_probe(2560, 1440, "h264_mf", FfmpegArgs::default_tuning("h264_mf"))
                .contains("-c:v h264_mf -scenario live_streaming")
        );
        assert!(
            FfmpegArgs::h264_probe(
                2560,
                1440,
                "h264_amf",
                FfmpegArgs::default_tuning("h264_amf")
            )
            .contains("-c:v h264_amf -quality speed")
        );
        assert!(
            FfmpegArgs::h264_probe(
                1920,
                1080,
                "h264_nvenc",
                FfmpegArgs::default_tuning("h264_nvenc")
            )
            .contains("-c:v h264_nvenc -preset p4 -tune ll")
        );
    }
}
