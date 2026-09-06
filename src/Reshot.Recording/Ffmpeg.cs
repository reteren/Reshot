using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text;
using Reshot.Core;
using Reshot.Core.Diagnostics;

namespace Reshot.Recording;

/// <summary>
/// Locates and launches the private ffmpeg runtime used by the recording layer.
/// Resolution is cached because PATH probing belongs at process startup, not on
/// a capture thread where filesystem work could disturb frame pacing.
/// </summary>
public static class Ffmpeg
{
    private const int EncoderProbeTimeoutMs = 3_000;
    private const int StderrTailLength = 2 * 1024;
    private static readonly string[] HardwareH264Encoders =
        ["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"];
    private static readonly string? CachedExecutablePath = ResolveExecutablePath();
    private static readonly ConcurrentDictionary<(int Width, int Height), Lazy<string>> CachedH264Encoders = new();

    public static string? ExecutablePath => CachedExecutablePath;

    public static bool IsAvailable => ExecutablePath is not null;

    public static string MissingMessage { get; } =
        $"ffmpeg.exe was not found next to Reshot, in its ffmpeg subdirectory, under '{Path.Combine(ReshotPaths.AppDataDir, "ffmpeg")}', or on PATH.";

    /// <summary>
    /// Starts a long-running ffmpeg process. Stderr is consumed asynchronously so
    /// diagnostics can never fill the pipe and stall a producer writing raw media.
    /// </summary>
    public static Process Start(string arguments, bool redirectStdin)
    {
        var executable = ExecutablePath ?? throw new FileNotFoundException(MissingMessage);
        var process = new Process
        {
            StartInfo = CreateStartInfo(executable, arguments, redirectStdin, redirectStdout: false),
        };

        process.ErrorDataReceived += (_, e) =>
        {
            if (!string.IsNullOrWhiteSpace(e.Data))
                Log.Warn($"ffmpeg: {e.Data}");
        };

        try
        {
            if (!process.Start())
                throw new InvalidOperationException("ffmpeg failed to start.");
            process.BeginErrorReadLine();
            return process;
        }
        catch
        {
            process.Dispose();
            throw;
        }
    }

    /// <summary>
    /// Runs a one-shot ffmpeg command and retains only the useful end of stderr.
    /// Reading while the child runs is essential: waiting first can deadlock on a
    /// full redirected pipe even for an otherwise successful command.
    /// </summary>
    public static bool Run(string arguments, out string stderrTail)
    {
        var executable = ExecutablePath ?? throw new FileNotFoundException(MissingMessage);
        using var process = new Process
        {
            StartInfo = CreateStartInfo(executable, arguments, redirectStdin: false, redirectStdout: false),
        };

        if (!process.Start())
            throw new InvalidOperationException("ffmpeg failed to start.");

        var tail = new StringBuilder(StderrTailLength);
        var buffer = new char[512];
        int read;
        while ((read = process.StandardError.Read(buffer, 0, buffer.Length)) > 0)
            AppendTail(tail, buffer, read);

        process.WaitForExit();
        stderrTail = tail.ToString().Trim();
        return process.ExitCode == 0;
    }

    /// <summary>
    /// Chooses the first H.264 encoder that can really initialize for this frame
    /// size. Encoder listings are insufficient because ffmpeg exposes compiled-in
    /// backends even when their matching GPU or minimum dimensions are absent.
    /// </summary>
    public static string SelectH264Encoder(int width, int height)
    {
        if (width <= 0)
            throw new ArgumentOutOfRangeException(nameof(width));
        if (height <= 0)
            throw new ArgumentOutOfRangeException(nameof(height));

        var selected = CachedH264Encoders.GetOrAdd(
            (width, height),
            static size => new Lazy<string>(
                () => ProbeH264Encoder(size.Width, size.Height),
                LazyThreadSafetyMode.ExecutionAndPublication));
        return selected.Value;
    }

    private static ProcessStartInfo CreateStartInfo(
        string executable,
        string arguments,
        bool redirectStdin,
        bool redirectStdout)
    {
        return new ProcessStartInfo
        {
            FileName = executable,
            Arguments = arguments,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardInput = redirectStdin,
            RedirectStandardOutput = redirectStdout,
            RedirectStandardError = true,
        };
    }

    private static string? ResolveExecutablePath()
    {
        var candidates = new[]
        {
            Path.Combine(AppContext.BaseDirectory, "ffmpeg.exe"),
            Path.Combine(AppContext.BaseDirectory, "ffmpeg", "ffmpeg.exe"),
            Path.Combine(ReshotPaths.AppDataDir, "ffmpeg", "ffmpeg.exe"),
        };

        foreach (var candidate in candidates)
        {
            if (File.Exists(candidate))
                return candidate;
        }

        var path = Environment.GetEnvironmentVariable("PATH");
        if (string.IsNullOrWhiteSpace(path))
            return null;

        foreach (var directory in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            try
            {
                var candidate = Path.Combine(directory.Trim().Trim('"'), "ffmpeg.exe");
                if (File.Exists(candidate))
                    return Path.GetFullPath(candidate);
            }
            catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException)
            {
                // One malformed PATH entry must not hide a valid ffmpeg later in PATH.
            }
        }

        return null;
    }

    private static string ProbeH264Encoder(int width, int height)
    {
        if (ExecutablePath is null)
        {
            Log.Warn($"ffmpeg: H.264 capability probe unavailable for {width}x{height}; {MissingMessage}");
            Log.Info($"ffmpeg: selected H.264 encoder libx264 for {width}x{height} without a capability probe.");
            return "libx264";
        }

        foreach (var encoder in HardwareH264Encoders)
        {
            if (TryProbeH264Encoder(encoder, width, height, out var failure))
            {
                Log.Info($"ffmpeg: selected H.264 encoder {encoder} for {width}x{height} after a live capability probe.");
                return encoder;
            }

            Log.Warn($"ffmpeg: skipped H.264 encoder {encoder} for {width}x{height}; capability probe {failure}.");
        }

        // The shipped GPL build always contains libx264. It is deliberately not
        // probed so unsupported hardware costs at most three short child processes.
        Log.Info($"ffmpeg: selected H.264 encoder libx264 for {width}x{height} after hardware capability probes failed.");
        return "libx264";
    }

    private static bool TryProbeH264Encoder(
        string encoder,
        int width,
        int height,
        out string failure)
    {
        var executable = ExecutablePath!;
        var (preset, tune, extraArgs) = FfmpegArgs.GetDefaultTuning(encoder);
        using var process = new Process
        {
            StartInfo = CreateStartInfo(
                executable,
                FfmpegArgs.H264Probe(width, height, encoder, preset, tune, extraArgs),
                redirectStdin: false,
                redirectStdout: false),
        };

        try
        {
            if (!process.Start())
            {
                failure = "could not start ffmpeg";
                return false;
            }

            var stderrRead = process.StandardError.ReadToEndAsync();
            var succeeded = WaitForSuccessfulExit(process, EncoderProbeTimeoutMs, out var exitCode);
            if (!succeeded && exitCode is null)
            {
                // Do not turn a bounded probe into an unbounded wait on stderr if
                // even terminating a wedged driver process did not close its pipe.
                failure = $"timed out after {EncoderProbeTimeoutMs} ms";
                return false;
            }

            string stderr;
            try
            {
                stderr = stderrRead.GetAwaiter().GetResult();
            }
            catch (Exception ex)
            {
                stderr = $"stderr unavailable: {ex.Message}";
            }

            if (succeeded)
            {
                failure = string.Empty;
                return true;
            }

            var reason = $"exited with code {exitCode}";
            var tail = Tail(stderr);
            failure = string.IsNullOrWhiteSpace(tail) ? reason : $"{reason}: {tail}";
            return false;
        }
        catch (Exception ex)
        {
            failure = $"threw {ex.GetType().Name}: {ex.Message}";
            return false;
        }
    }

    private static string Tail(string value) =>
        (value.Length <= StderrTailLength ? value : value[^StderrTailLength..]).Trim();

    private static void AppendTail(StringBuilder tail, char[] buffer, int count)
    {
        if (count >= StderrTailLength)
        {
            tail.Clear();
            tail.Append(buffer, count - StderrTailLength, StderrTailLength);
            return;
        }

        var overflow = tail.Length + count - StderrTailLength;
        if (overflow > 0)
            tail.Remove(0, overflow);
        tail.Append(buffer, 0, count);
    }

    internal static bool WaitForSuccessfulExit(Process process, int timeoutMilliseconds, out int? exitCode)
    {
        exitCode = null;
        if (!process.WaitForExit(timeoutMilliseconds))
        {
            TryKill(process);
            return false;
        }

        // Flush the asynchronous stderr callback after the native process exits.
        process.WaitForExit();
        exitCode = process.ExitCode;
        return process.ExitCode == 0;
    }

    private static void TryKill(Process process)
    {
        try
        {
            if (!process.HasExited)
                process.Kill(entireProcessTree: true);
            process.WaitForExit(5_000);
        }
        catch (Exception ex)
        {
            Log.Warn($"ffmpeg: could not terminate process {process.Id}: {ex.Message}");
        }
    }
}
