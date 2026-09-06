using System.Diagnostics;
using System.Runtime.InteropServices;
using Reshot.Capture;
using Reshot.Core.Diagnostics;

namespace Reshot.Recording;

internal interface IVideoFrameWriter : IDisposable
{
    void WriteFrame(byte[] buffer);
}

/// <summary>
/// Records a screen rectangle to an MP4 (ARCHITECTURE §9). A
/// <see cref="WgcMonitorStream"/> supplies live frames; a video thread pulls the crop at
/// the target fps into a <b>video-only</b> temp MP4, while an audio thread writes each
/// source (system, microphone) to its <b>own</b> raw PCM file.
///
/// Nothing is mixed during capture, <see cref="Finish"/> decides which tracks make it
/// into the final file, which is what lets the user pick tracks after recording. The
/// audio clock starts when the first video frame arrives (and the buffers are flushed
/// then) so the tracks line up.
/// </summary>
public sealed class VideoRecorder : IDisposable
{
    private sealed class Mp4VideoEncoderAdapter : IVideoFrameWriter
    {
        private readonly Mp4VideoEncoder _encoder;
        public Mp4VideoEncoderAdapter(Mp4VideoEncoder encoder) => _encoder = encoder;
        public void WriteFrame(byte[] buffer) => _encoder.WriteFrame(buffer);
        public void Dispose() => _encoder.Dispose();
    }

    private const int AudioMaxChunkFrames = 4800; // ~100 ms at 48 kHz

    private WgcMonitorStream? _stream;
    private IVideoFrameWriter? _encoder;
    private readonly AudioCaptureMixer? _systemAudio;
    private readonly AudioCaptureMixer? _micAudio;
    private FileStream? _systemPcm;
    private FileStream? _micPcm;
    private readonly string? _tempVideoPath;
    private readonly string? _systemPcmPath;
    private readonly string? _micPcmPath;
    private Thread? _videoThread;
    private Thread? _audioThread;
    private readonly ManualResetEventSlim _started = new(false);
    private readonly int _cropLeft, _cropTop, _cropWidth, _cropHeight, _fps;
    private readonly byte[]? _mask;
    private readonly int _maskWidth;
    private readonly int _maskHeight;
    private readonly int _maskStride;
    private volatile bool _stop;
    private int _stopInitiated;
    private int _timerPeriodRaised;
    private bool _disposed;
    private bool _workerTerminated;

    internal TimeSpan WorkerCooperativeTimeout { get; set; } = TimeSpan.FromSeconds(1);
    internal TimeSpan WorkerDrainTimeout { get; set; } = TimeSpan.FromSeconds(5);
    internal bool IsWorkerTerminated => _workerTerminated;

    private readonly Action<uint> _timeBegin;
    private readonly Action<uint> _timeEnd;

    internal bool IsTimerPeriodRaised => Volatile.Read(ref _timerPeriodRaised) == 1;

    private void EnsureTimerResolutionRaised()
    {
        if (Interlocked.CompareExchange(ref _timerPeriodRaised, 1, 0) == 0)
        {
            _timeBegin(1);
        }
    }

    private void EnsureTimerResolutionRestored()
    {
        if (Interlocked.CompareExchange(ref _timerPeriodRaised, 0, 1) == 1)
        {
            _timeEnd(1);
        }
    }

    /// <summary>Final output path (only written by <see cref="Finish"/>).</summary>
    public string Path { get; }
    public int Width => _cropWidth;
    public int Height => _cropHeight;

    /// <summary>True if a system-audio track was actually captured.</summary>
    public bool HasSystemTrack => _systemAudio is not null;

    /// <summary>True if a microphone track was actually captured.</summary>
    public bool HasMicTrack => _micAudio is not null;

    public VideoRecorder(
        IScreenCaptureService capture,
        int rectLeft, int rectTop, int rectWidth, int rectHeight,
        int fps, int bitrate, string path,
        AudioSources sources,
        byte[]? shapeMask = null, int maskStride = 0)
        : this(capture, rectLeft, rectTop, rectWidth, rectHeight, fps, bitrate, path, sources, shapeMask, maskStride, null, null, null)
    {
    }

    internal VideoRecorder(
        IScreenCaptureService capture,
        int rectLeft, int rectTop, int rectWidth, int rectHeight,
        int fps, int bitrate, string path,
        AudioSources sources,
        byte[]? shapeMask, int maskStride,
        Action<uint>? timeBegin, Action<uint>? timeEnd)
        : this(capture, rectLeft, rectTop, rectWidth, rectHeight, fps, bitrate, path, sources, shapeMask, maskStride, timeBegin, timeEnd, null)
    {
    }

    internal VideoRecorder(
        IScreenCaptureService capture,
        int rectLeft, int rectTop, int rectWidth, int rectHeight,
        int fps, int bitrate, string path,
        AudioSources sources,
        byte[]? shapeMask, int maskStride,
        Action<uint>? timeBegin, Action<uint>? timeEnd,
        Func<string, int, int, int, int, IVideoFrameWriter>? encoderFactory)
    {
        _timeBegin = timeBegin ?? (ms => TimeBeginPeriod(ms));
        _timeEnd = timeEnd ?? (ms => TimeEndPeriod(ms));
        Path = path;
        _fps = fps;
        _mask = shapeMask;
        _maskStride = maskStride > 0 ? maskStride : rectWidth;
        _maskWidth = rectWidth;
        _maskHeight = rectHeight;

        EnsureTimerResolutionRaised();

        try
        {
            _stream = capture.StartMonitorStream(rectLeft, rectTop);

            // Recording crop relative to the captured monitor, clamped to it, even dims.
            var cl = Math.Clamp(rectLeft - _stream.MonitorLeft, 0, Math.Max(0, _stream.Width - 2));
            var ct = Math.Clamp(rectTop - _stream.MonitorTop, 0, Math.Max(0, _stream.Height - 2));
            _cropLeft = cl;
            _cropTop = ct;
            _cropWidth = Math.Max(2, Math.Min(rectWidth, _stream.Width - cl) & ~1);
            _cropHeight = Math.Max(2, Math.Min(rectHeight, _stream.Height - ct) & ~1);

            // Each source is captured on its own, so tracks can be kept or dropped later.
            // Audio is best-effort: a source that won't open just isn't offered at save time.
            if (sources.SystemFull || sources.IncludePids.Length > 0)
            {
                _systemAudio = TryOpen(new AudioSources
                {
                    SystemFull = sources.SystemFull,
                    IncludePids = sources.IncludePids,
                });
            }
            if (sources.Mic)
                _micAudio = TryOpen(new AudioSources { Mic = true, MicDevice = sources.MicDevice });

            var stamp = Guid.NewGuid().ToString("N")[..8];
            var tempDir = System.IO.Path.GetTempPath();
            _tempVideoPath = System.IO.Path.Combine(tempDir, $"reshot_{stamp}_video.mp4");
            if (_systemAudio is not null)
            {
                _systemPcmPath = System.IO.Path.Combine(tempDir, $"reshot_{stamp}_sys.pcm");
                _systemPcm = File.Create(_systemPcmPath);
            }
            if (_micAudio is not null)
            {
                _micPcmPath = System.IO.Path.Combine(tempDir, $"reshot_{stamp}_mic.pcm");
                _micPcm = File.Create(_micPcmPath);
            }

            // Video-only: the audio track is added by the muxer once the tracks are chosen.
            _encoder = encoderFactory is not null
                ? encoderFactory(_tempVideoPath, _cropWidth, _cropHeight, fps, bitrate)
                : new Mp4VideoEncoderAdapter(new Mp4VideoEncoder(_tempVideoPath, _cropWidth, _cropHeight, fps, bitrate, null));

            _videoThread = new Thread(VideoLoop) { IsBackground = true, Name = "reshot-video" };
            _videoThread.Start();
            if (_systemAudio is not null || _micAudio is not null)
            {
                _audioThread = new Thread(AudioLoop) { IsBackground = true, Name = "reshot-audio" };
                _audioThread.Start();
            }
        }
        catch
        {
            EnsureTimerResolutionRestored();
            _encoder?.Dispose();
            _systemAudio?.Dispose();
            _micAudio?.Dispose();
            _systemPcm?.Dispose();
            _micPcm?.Dispose();
            _stream?.Dispose();
            CleanupTemps();
            throw;
        }
    }

    private static AudioCaptureMixer? TryOpen(AudioSources sources)
    {
        var mixer = new AudioCaptureMixer(sources);
        if (mixer.HasAnySource)
            return mixer;
        mixer.Dispose();
        return null;
    }

    private void VideoLoop()
    {
        var buffer = new byte[_cropWidth * _cropHeight * 4];
        try
        {
            // Wait for the first captured frame, then release the audio clock.
            var spin = Stopwatch.StartNew();
            while (!_stop && (_stream is null || !_stream.CopyRegion(_cropLeft, _cropTop, _cropWidth, _cropHeight, buffer)))
            {
                if (spin.Elapsed > TimeSpan.FromSeconds(3))
                {
                    Log.Warn("Recorder: no frames arrived within 3s.");
                    _started.Set();
                    return;
                }
                Thread.Sleep(4);
            }
            _started.Set();
            if (_stop)
                return;

            var clock = Stopwatch.StartNew();
            var interval = 1000.0 / _fps;
            long frame = 0;
            while (!_stop)
            {
                if (_stream is not null && _stream.CopyRegion(_cropLeft, _cropTop, _cropWidth, _cropHeight, buffer))
                {
                    if (_mask is not null)
                        ApplyMask(buffer);
                    _encoder?.WriteFrame(buffer);
                }
                frame++;
                var delayMs = frame * interval - clock.Elapsed.TotalMilliseconds;
                if (delayMs > 1)
                    Thread.Sleep((int)delayMs);
            }
        }
        catch (Exception ex)
        {
            Log.Error("Recorder: video loop failed", ex);
        }
    }

    /// <summary>
    /// Blacks out pixels outside the selected shape(s). MP4/H.264 has no alpha, so
    /// "empty space" becomes black rather than transparent.
    /// Clamped to both buffer extents and mask bounds to prevent out-of-bounds reads
    /// on odd or 1px selections.
    /// </summary>
    internal static unsafe void ApplyMask(
        byte[] bgra,
        int cropWidth,
        int cropHeight,
        byte[]? mask,
        int maskWidth,
        int maskHeight,
        int maskStride)
    {
        if (mask is null || bgra is null || cropWidth <= 0 || cropHeight <= 0)
            return;

        var maxDstUints = bgra.Length / 4;
        if (maxDstUints < cropWidth * cropHeight)
            return;

        var maskLen = mask.Length;
        var validRows = Math.Clamp(maskHeight, 0, cropHeight);
        var validCols = Math.Clamp(maskWidth, 0, cropWidth);

        fixed (byte* pMask = mask)
        fixed (byte* pBgra = bgra)
        {
            var pDst = (uint*)pBgra;

            for (var y = 0; y < validRows; y++)
            {
                var maskOffset = y * maskStride;
                var dstRow = pDst + y * cropWidth;

                // If this row's offset starts past mask length, fill entire row with black.
                if (maskOffset >= maskLen || maskOffset < 0)
                {
                    for (var x = 0; x < cropWidth; x++)
                        dstRow[x] = 0xFF000000;
                    continue;
                }

                var rowValidCols = Math.Min(validCols, maskLen - maskOffset);
                var maskRow = pMask + maskOffset;

                for (var x = 0; x < rowValidCols; x++)
                {
                    if (maskRow[x] < 128)
                        dstRow[x] = 0xFF000000; // Opaque black: B=0, G=0, R=0, A=255
                }

                // Any clamped/padded columns on this row are outside selection
                for (var x = rowValidCols; x < cropWidth; x++)
                {
                    dstRow[x] = 0xFF000000;
                }
            }

            // Any clamped/padded rows beyond maskHeight are outside selection
            for (var y = validRows; y < cropHeight; y++)
            {
                var dstRow = pDst + y * cropWidth;
                for (var x = 0; x < cropWidth; x++)
                {
                    dstRow[x] = 0xFF000000;
                }
            }
        }
    }

    private unsafe void ApplyMask(byte[] bgra)
    {
        ApplyMask(bgra, _cropWidth, _cropHeight, _mask, _maskWidth, _maskHeight, _maskStride);
    }

    private void AudioLoop()
    {
        var pcm = new byte[AudioMaxChunkFrames * AudioCaptureMixer.Channels * 2];
        try
        {
            _started.Wait();
            if (_stop)
                return;

            // Drop audio buffered while the first frame was awaited.
            _systemAudio?.Flush();
            _micAudio?.Flush();

            var clock = Stopwatch.StartNew();
            long written = 0;
            while (!_stop)
            {
                var target = (long)(clock.Elapsed.TotalSeconds * AudioCaptureMixer.SampleRate);
                var need = target - written;
                if (need >= 480)
                {
                    // Both tracks are pulled the same number of frames, so they stay aligned.
                    var chunk = (int)Math.Min(need, AudioMaxChunkFrames);
                    if (_systemAudio is not null && _systemPcm is not null)
                        _systemPcm.Write(pcm, 0, _systemAudio.ReadPcm16(pcm, chunk));
                    if (_micAudio is not null && _micPcm is not null)
                        _micPcm.Write(pcm, 0, _micAudio.ReadPcm16(pcm, chunk));
                    written += chunk;
                }
                Thread.Sleep(8);
            }
        }
        catch (Exception ex)
        {
            Log.Error("Recorder: audio loop failed", ex);
        }
    }

    /// <summary>Stops capture and finalizes the temp video + PCM files (no output yet).</summary>
    public void Stop()
    {
        if (Interlocked.Exchange(ref _stopInitiated, 1) != 0)
            return;

        try
        {
            _stop = true;
            _started.Set();

            // Phase 1: cooperative shutdown wait.
            var cleanlyTerminated = WaitForWorkerTermination(WorkerCooperativeTimeout);
            if (!cleanlyTerminated)
            {
                Log.Warn("Recorder: worker threads did not exit within cooperative timeout; forcing encoder shutdown to unblock pipes.");
                SafeShutdownEncoder();
                cleanlyTerminated = WaitForWorkerTermination(WorkerDrainTimeout);
                if (!cleanlyTerminated)
                {
                    Log.Error("Recorder: worker threads failed to terminate within drain timeout.");
                }
            }
            else
            {
                SafeShutdownEncoder();
            }

            if (cleanlyTerminated)
            {
                _systemAudio?.Dispose();
                _micAudio?.Dispose();
                _systemPcm?.Dispose();
                _micPcm?.Dispose();
                _systemPcm = null;
                _micPcm = null;
                _stream?.Dispose();
                _stream = null;
            }
        }
        finally
        {
            EnsureTimerResolutionRestored();
        }
    }

    private void SafeShutdownEncoder()
    {
        try
        {
            _encoder?.Dispose();
        }
        catch (Exception ex)
        {
            Log.Warn($"Recorder: encoder dispose failed: {ex.Message}");
        }
        finally
        {
            _encoder = null;
        }
    }

    private bool WaitForWorkerTermination(TimeSpan timeout)
    {
        var sw = Stopwatch.StartNew();

        if (_videoThread is not null && _videoThread.IsAlive)
        {
            var remaining = timeout - sw.Elapsed;
            if (remaining <= TimeSpan.Zero || !_videoThread.Join(remaining))
                return false;
        }

        if (_audioThread is not null && _audioThread.IsAlive)
        {
            var remaining = timeout - sw.Elapsed;
            if (remaining <= TimeSpan.Zero || !_audioThread.Join(remaining))
                return false;
        }

        _workerTerminated = true;
        return true;
    }

    [DllImport("winmm.dll", EntryPoint = "timeBeginPeriod")]
    private static extern uint TimeBeginPeriod(uint uMilliseconds);

    [DllImport("winmm.dll", EntryPoint = "timeEndPeriod")]
    private static extern uint TimeEndPeriod(uint uMilliseconds);

    /// <summary>
    /// Writes the final MP4 with the chosen audio tracks (video is copied through, never
    /// re-encoded) and deletes the temp files. Call after <see cref="Stop"/>.
    /// </summary>
    public bool Finish(bool keepSystem, bool keepMic)
    {
        Stop();

        if (!_workerTerminated)
        {
            Log.Error("Recorder: cannot finish recording because worker threads failed to terminate.");
            return false;
        }

        if (_tempVideoPath is null)
            return false;

        var tracks = new List<string>();
        if (keepSystem && _systemPcmPath is not null)
            tracks.Add(_systemPcmPath);
        if (keepMic && _micPcmPath is not null)
            tracks.Add(_micPcmPath);

        var ok = Mp4Muxer.Mux(_tempVideoPath, tracks, Path);
        if (!ok)
        {
            // Muxing failed, keep at least the silent video rather than losing the take.
            try { File.Copy(_tempVideoPath, Path, overwrite: true); ok = File.Exists(Path); }
            catch (Exception ex) { Log.Error("Recorder: fallback copy failed", ex); }
        }
        CleanupTemps();
        return ok;
    }

    private void CleanupTemps()
    {
        foreach (var path in new[] { _tempVideoPath, _systemPcmPath, _micPcmPath })
        {
            if (path is null)
                continue;
            try { if (File.Exists(path)) File.Delete(path); }
            catch (Exception ex) { Log.Warn($"Recorder: could not delete temp '{path}': {ex.Message}"); }
        }
    }

    public void Dispose()
    {
        if (_disposed)
            return;
        _disposed = true;
        try
        {
            Stop();
        }
        finally
        {
            EnsureTimerResolutionRestored();
            _started.Dispose();
        }
    }
}
