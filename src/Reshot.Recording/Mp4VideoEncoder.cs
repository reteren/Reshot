using System.Diagnostics;
using Reshot.Core.Diagnostics;

namespace Reshot.Recording;

/// <summary>
/// Streams top-down BGRA frames to ffmpeg for H.264 encoding. ffmpeg owns the
/// BGRA-to-YUV conversion so the capture thread no longer spends CPU time walking
/// every pixel before the selected hardware encoder can receive the frame.
/// </summary>
public sealed class Mp4VideoEncoder : IDisposable
{
    private const int FinalizeTimeoutMs = 30_000;

    private readonly string _outputPath;
    private readonly string _videoPath;
    private readonly string? _audioPcmPath;
    private readonly AudioConfig? _audioConfig;
    private readonly Process _process;
    private readonly Stream _videoInput;
    private readonly FileStream? _audioPcm;
    private readonly int _frameBytes;
    private long _frameIndex;
    private long _audioFramesWritten;
    private bool _disposed;

    /// <summary>Optional PCM audio track config (16-bit); null = video only.</summary>
    public sealed record AudioConfig(int SampleRate, int Channels, int Bitrate);

    public bool HasAudio => _audioConfig is not null;

    public Mp4VideoEncoder(
        string path,
        int width,
        int height,
        int fps,
        int bitrate,
        AudioConfig? audio = null)
    {
        // H.264 4:2:0 requires even dimensions; VideoRecorder already crops the
        // matching frame buffer, while this keeps direct callers equally safe.
        var encodedWidth = width & ~1;
        var encodedHeight = height & ~1;
        if (encodedWidth <= 0 || encodedHeight <= 0)
            throw new ArgumentOutOfRangeException(nameof(width), "Video dimensions must contain at least one even 2x2 block.");
        if (fps <= 0)
            throw new ArgumentOutOfRangeException(nameof(fps));

        _outputPath = path;
        _audioConfig = audio;
        _frameBytes = checked(encodedWidth * encodedHeight * 4);

        if (audio is null)
        {
            _videoPath = path;
        }
        else
        {
            var stamp = Guid.NewGuid().ToString("N");
            _videoPath = Path.Combine(Path.GetTempPath(), $"reshot_{stamp}_video.mp4");
            _audioPcmPath = Path.Combine(Path.GetTempPath(), $"reshot_{stamp}_audio.pcm");
            _audioPcm = File.Create(_audioPcmPath);
        }

        try
        {
            var encoder = Ffmpeg.SelectH264Encoder(encodedWidth, encodedHeight);
            var (preset, tune, extraArgs) = FfmpegArgs.GetDefaultTuning(encoder);
            _process = Ffmpeg.Start(
                FfmpegArgs.Video(encodedWidth, encodedHeight, fps, bitrate, encoder, _videoPath, preset, tune, extraArgs),
                redirectStdin: true);
            _videoInput = _process.StandardInput.BaseStream;
            Log.Info($"Encoder: ffmpeg/{encoder} {encodedWidth}x{encodedHeight} @ {fps}fps, {bitrate / 1000}kbps"
                     + (audio is not null ? $" + deferred AAC {audio.SampleRate}Hz/{audio.Channels}ch" : "")
                     + $" → {path}");
        }
        catch
        {
            _audioPcm?.Dispose();
            DeleteIfPresent(_audioPcmPath);
            if (audio is not null)
                DeleteIfPresent(_videoPath);
            throw;
        }
    }

    /// <summary>
    /// Writes one top-down BGRA frame whose tightly packed stride is width*4.
    /// Rawvideo is implicitly constant-frame-rate, so timestamps remain the
    /// caller's responsibility and no timing work occurs on this hot path.
    /// </summary>
    public void WriteFrame(ReadOnlySpan<byte> bgra)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (bgra.Length != _frameBytes)
            throw new ArgumentException($"Expected exactly {_frameBytes} BGRA bytes, got {bgra.Length}.", nameof(bgra));

        _videoInput.Write(bgra);
        _frameIndex++;
    }

    /// <summary>
    /// Writes 16-bit interleaved PCM for the optional audio track. A temporary PCM
    /// file is intentional: ffmpeg has only one stdin, so audio is muxed after the
    /// video process finishes without changing the public encoder contract.
    /// </summary>
    public void WriteAudio(byte[] pcm, int byteCount)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (_audioPcm is null || byteCount <= 0)
            return;
        if ((uint)byteCount > (uint)pcm.Length)
            throw new ArgumentOutOfRangeException(nameof(byteCount));

        _audioPcm.Write(pcm, 0, byteCount);
        _audioFramesWritten += byteCount / (_audioConfig!.Channels * 2);
    }

    public void Dispose()
    {
        if (_disposed)
            return;
        _disposed = true;

        Exception? closeError = null;
        try
        {
            _videoInput.Close();
        }
        catch (Exception ex)
        {
            closeError = ex;
            Log.Error("Encoder: failed to close ffmpeg input", ex);
        }

        _audioPcm?.Dispose();

        var videoOk = Ffmpeg.WaitForSuccessfulExit(_process, FinalizeTimeoutMs, out var exitCode);
        _process.Dispose();
        var videoHasContent = HasContent(_videoPath);
        if (!videoOk || closeError is not null || !videoHasContent)
        {
            var reason = closeError is not null
                ? "input pipe failed"
                : exitCode is null
                    ? "timed out"
                    : exitCode != 0
                        ? $"exited with code {exitCode}"
                        : "produced no output";
            Log.Error($"Encoder: ffmpeg {reason} after {_frameIndex} frames; discarding incomplete output '{_videoPath}'.");
            DeleteIfPresent(_videoPath);
            DeleteIfPresent(_audioPcmPath);
            return;
        }

        if (_audioConfig is not null)
        {
            IReadOnlyList<string> tracks = _audioFramesWritten > 0
                ? new[] { _audioPcmPath! }
                : Array.Empty<string>();
            var muxed = Ffmpeg.Run(
                FfmpegArgs.Mux(
                    _videoPath,
                    tracks,
                    _outputPath,
                    _audioConfig.SampleRate,
                    _audioConfig.Channels,
                    _audioConfig.Bitrate),
                out var stderrTail);

            if (!muxed || !HasContent(_outputPath))
            {
                Log.Error($"Encoder: ffmpeg audio mux failed for '{_outputPath}': {stderrTail}");
                DeleteIfPresent(_outputPath);
            }
            else
            {
                Log.Info($"Encoder: finalized {_frameIndex} frames and {_audioFramesWritten} audio frames.");
            }

            DeleteIfPresent(_videoPath);
            DeleteIfPresent(_audioPcmPath);
            return;
        }

        Log.Info($"Encoder: finalized {_frameIndex} frames.");
    }

    private static bool HasContent(string path)
    {
        try
        {
            return File.Exists(path) && new FileInfo(path).Length > 0;
        }
        catch
        {
            return false;
        }
    }

    private static void DeleteIfPresent(string? path)
    {
        if (path is null)
            return;
        try
        {
            if (File.Exists(path))
                File.Delete(path);
        }
        catch (Exception ex)
        {
            Log.Warn($"Encoder: could not delete incomplete file '{path}': {ex.Message}");
        }
    }
}
