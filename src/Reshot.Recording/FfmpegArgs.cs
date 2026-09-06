using System.Globalization;
using System.Text;

namespace Reshot.Recording;

/// <summary>
/// Builds ffmpeg command lines without touching process or filesystem state.
/// Keeping the grammar here makes the recording paths deterministic and keeps
/// quoting changes reviewable in one place.
/// </summary>
public static class FfmpegArgs
{
    public static string Video(
        int width,
        int height,
        int fps,
        int bitrate,
        string encoder,
        string outputPath,
        string? preset = null,
        string? tune = null,
        string? extraArgs = null)
    {
        var extra = new StringBuilder();
        if (!string.IsNullOrWhiteSpace(preset))
            extra.Append($" -preset {preset}");
        if (!string.IsNullOrWhiteSpace(tune))
            extra.Append($" -tune {tune}");
        if (!string.IsNullOrWhiteSpace(extraArgs))
            extra.Append($" {extraArgs.Trim()}");

        return $"-hide_banner -loglevel error -y -f rawvideo -pixel_format bgra -video_size {I(width)}x{I(height)} -framerate {I(fps)} -i - -an -c:v {encoder}{extra} -b:v {I(bitrate)} -pix_fmt yuv420p -movflags +faststart \"{outputPath}\"";
    }

    /// <summary>
    /// Returns recommended (preset, tune, extraArgs) flags tailored to the specific H.264 encoder.
    /// Real-time screen capture requires low latency and fast encoding to prevent pipe backpressure.
    /// </summary>
    public static (string? Preset, string? Tune, string? ExtraArgs) GetDefaultTuning(string encoder) =>
        encoder switch
        {
            "libx264" => ("veryfast", "zerolatency", null),
            "h264_nvenc" => ("p4", "ll", null),
            "h264_qsv" => ("veryfast", null, null),
            "h264_amf" => (null, null, "-quality speed"),
            "h264_mf" => (null, null, "-scenario live_streaming"),
            _ => (null, null, null)
        };

    public static string Audio(int sampleRate, int channels, int bitrate, string outputPath)
    {
        return $"-hide_banner -loglevel error -y -f s16le -ar {I(sampleRate)} -ac {I(channels)} -i - -c:a aac -b:a {I(bitrate)} -movflags +faststart \"{outputPath}\"";
    }

    /// <summary>
    /// Builds a no-output live capability trial. The dimensions match the real
    /// recording because hardware availability alone does not imply size support.
    /// Includes optional tuning flags so drivers that reject live flags fail during probing.
    /// </summary>
    public static string H264Probe(
        int width,
        int height,
        string encoder,
        string? preset = null,
        string? tune = null,
        string? extraArgs = null)
    {
        var extra = new StringBuilder();
        if (!string.IsNullOrWhiteSpace(preset))
            extra.Append($" -preset {preset}");
        if (!string.IsNullOrWhiteSpace(tune))
            extra.Append($" -tune {tune}");
        if (!string.IsNullOrWhiteSpace(extraArgs))
            extra.Append($" {extraArgs.Trim()}");

        return $"-hide_banner -loglevel error -f lavfi -i color=black:s={I(width)}x{I(height)}:r=10:d=0.2 -an -c:v {encoder}{extra} -pix_fmt yuv420p -f null -";
    }

    public static string Mux(
        string videoOnlyMp4,
        IReadOnlyList<string> pcmPaths,
        string outputPath,
        int sampleRate,
        int channels,
        int audioBitrate)
    {
        var command = new StringBuilder(
            $"-hide_banner -loglevel error -y -i \"{videoOnlyMp4}\"");

        if (pcmPaths.Count == 0)
        {
            command.Append($" -c copy -movflags +faststart \"{outputPath}\"");
            return command.ToString();
        }

        foreach (var pcmPath in pcmPaths)
        {
            command.Append($" -f s16le -ar {I(sampleRate)} -ac {I(channels)} -i \"{pcmPath}\"");
        }

        if (pcmPaths.Count == 1)
        {
            command.Append($" -map 0:v:0 -map 1:a:0 -c:v copy -c:a aac -b:a {I(audioBitrate)} -shortest -movflags +faststart \"{outputPath}\"");
            return command.ToString();
        }

        command.Append(" -filter_complex \"");
        for (var index = 1; index <= pcmPaths.Count; index++)
            command.Append($"[{I(index)}:a]");
        command.Append($"amix=inputs={I(pcmPaths.Count)}:duration=longest:normalize=0[aout]\"");
        command.Append($" -map 0:v:0 -map \"[aout]\" -c:v copy -c:a aac -b:a {I(audioBitrate)} -movflags +faststart \"{outputPath}\"");
        return command.ToString();
    }

    private static string I(int value) => value.ToString(CultureInfo.InvariantCulture);
}
