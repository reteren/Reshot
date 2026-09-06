using Reshot.Recording;
using Xunit;

namespace Reshot.Recording.Tests;

public class FfmpegTuningTests
{
    [Theory]
    [InlineData("libx264", "veryfast", "zerolatency", null)]
    [InlineData("h264_nvenc", "p4", "ll", null)]
    [InlineData("h264_qsv", "veryfast", null, null)]
    [InlineData("h264_amf", null, null, "-quality speed")]
    [InlineData("h264_mf", null, null, "-scenario live_streaming")]
    public void GetDefaultTuning_returns_expected_flags(string encoder, string? expectedPreset, string? expectedTune, string? expectedExtra)
    {
        var (preset, tune, extra) = FfmpegArgs.GetDefaultTuning(encoder);
        Assert.Equal(expectedPreset, preset);
        Assert.Equal(expectedTune, tune);
        Assert.Equal(expectedExtra, extra);
    }

    [Fact]
    public void Video_with_preset_and_tune_builds_optimized_command()
    {
        var args = FfmpegArgs.Video(1920, 1080, 60, 8_000_000, "libx264", @"C:\Videos\recording.mp4", "veryfast", "zerolatency");

        Assert.Equal(
            "-hide_banner -loglevel error -y -f rawvideo -pixel_format bgra -video_size 1920x1080 -framerate 60 -i - -an -c:v libx264 -preset veryfast -tune zerolatency -b:v 8000000 -pix_fmt yuv420p -movflags +faststart \"C:\\Videos\\recording.mp4\"",
            args);
    }

    [Fact]
    public void Video_with_mf_extra_args_builds_optimized_command()
    {
        var (preset, tune, extra) = FfmpegArgs.GetDefaultTuning("h264_mf");
        var args = FfmpegArgs.Video(2560, 1440, 60, 17_694_720, "h264_mf", @"C:\Videos\recording.mp4", preset, tune, extra);

        Assert.Equal(
            "-hide_banner -loglevel error -y -f rawvideo -pixel_format bgra -video_size 2560x1440 -framerate 60 -i - -an -c:v h264_mf -scenario live_streaming -b:v 17694720 -pix_fmt yuv420p -movflags +faststart \"C:\\Videos\\recording.mp4\"",
            args);
    }
}
