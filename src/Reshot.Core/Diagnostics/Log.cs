using System.Diagnostics;
using System.Runtime.CompilerServices;
using System.Text;

namespace Reshot.Core.Diagnostics;

/// <summary>
/// Minimal thread-safe file logger. Reshot spends most of its life asleep, so
/// there is no heavy logging framework and no background worker thread; it holds
/// an append-only file stream open with AutoFlush to guarantee crash durability,
/// zero allocations per line, and direct debugger mirroring.
/// </summary>
public static class Log
{
    private static readonly object Gate = new();
    private static string? _logFile;
    private static StreamWriter? _writer;

    private static readonly string InfoPrefix = " [Info ] ";
    private static readonly string WarnPrefix = " [Warn ] ";
    private static readonly string ErrorPrefix = " [Error] ";

    public enum Level { Info, Warn, Error }

    public static Level MinimumLevel { get; set; } = Level.Info;

    public static bool IsInfoEnabled => IsLevelEnabled(Level.Info);
    public static bool IsWarnEnabled => IsLevelEnabled(Level.Warn);
    public static bool IsErrorEnabled => IsLevelEnabled(Level.Error);

    private static bool IsLevelEnabled(Level level)
    {
        if (level < MinimumLevel)
            return false;
        return _logFile is not null || Debugger.IsAttached;
    }

    static Log()
    {
        AppDomain.CurrentDomain.ProcessExit += (_, _) => Close();
    }

    /// <summary>Must be called once at startup after the AppData dir exists.</summary>
    public static void Init()
    {
        ReshotPaths.EnsureAppDataDir();
        SetLogFile(Path.Combine(ReshotPaths.LogsDir, "reshot.log"));
        Info($"=== Reshot session started (pid {Environment.ProcessId}) ===");
    }

    /// <summary>Points logging at a file (or null to disable file logging).</summary>
    public static void SetLogFile(string? path)
    {
        lock (Gate)
        {
            try
            {
                _writer?.Flush();
                _writer?.Dispose();
            }
            catch { }
            _writer = null;
            _logFile = path;

            if (path is not null)
            {
                OpenWriterLocked();
            }
        }
    }

    public static void Close() => SetLogFile(null);

    public static void Info(string message) => Write(Level.Info, message);
    public static void Info(ref LogInfoInterpolatedStringHandler handler)
    {
        if (handler.IsEnabled)
            Write(Level.Info, handler.GetFormattedText());
    }

    public static void Warn(string message) => Write(Level.Warn, message);
    public static void Warn(ref LogWarnInterpolatedStringHandler handler)
    {
        if (handler.IsEnabled)
            Write(Level.Warn, handler.GetFormattedText());
    }

    public static void Error(string message) => Write(Level.Error, message);
    public static void Error(ref LogErrorInterpolatedStringHandler handler)
    {
        if (handler.IsEnabled)
            Write(Level.Error, handler.GetFormattedText());
    }

    public static void Error(string message, Exception ex) =>
        Write(Level.Error, $"{message}{Environment.NewLine}{ex}");

    public static void Error(ref LogErrorInterpolatedStringHandler handler, Exception ex)
    {
        if (handler.IsEnabled)
            Write(Level.Error, $"{handler.GetFormattedText()}{Environment.NewLine}{ex}");
    }

    private const long MaxLogBytes = 1_000_000;
    private static readonly UTF8Encoding Utf8NoBom = new(encoderShouldEmitUTF8Identifier: false);

    private static void Write(Level level, string message)
    {
        var now = DateTime.Now;

        if (Debugger.IsAttached)
            Debug.WriteLine($"{now:yyyy-MM-dd HH:mm:ss.fff} [{level,-5}] {message}");

        if (_logFile is null)
            return;

        lock (Gate)
        {
            try
            {
                if (_writer is null)
                    OpenWriterLocked();

                if (_writer is not null)
                {
                    // Check rollover BEFORE writing: if the file exceeds the cap, truncate in-place
                    // and reset stream Position so the next write begins at byte 0 instead of leaving
                    // a megabyte of sparse null bytes. The triggering line is retained at the top.
                    if (_writer.BaseStream.Length > MaxLogBytes)
                    {
                        _writer.Flush();
                        _writer.BaseStream.SetLength(0);
                        _writer.BaseStream.Position = 0;
                    }

                    Span<char> ts = stackalloc char[23];
                    now.TryFormat(ts, out _, "yyyy-MM-dd HH:mm:ss.fff");
                    _writer.Write(ts);
                    _writer.Write(level switch
                    {
                        Level.Warn => WarnPrefix,
                        Level.Error => ErrorPrefix,
                        _ => InfoPrefix,
                    });
                    _writer.WriteLine(message);
                }
            }
            catch
            {
                // Logging must never crash the app. Swallow disk/IO errors.
            }
        }
    }

    private static void OpenWriterLocked()
    {
        if (_logFile is null)
            return;
        try
        {
            var dir = Path.GetDirectoryName(_logFile);
            if (!string.IsNullOrEmpty(dir) && !Directory.Exists(dir))
                Directory.CreateDirectory(dir);

            var stream = new FileStream(_logFile, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.ReadWrite);
            if (stream.Length > MaxLogBytes)
            {
                stream.SetLength(0);
                stream.Position = 0;
            }
            else
            {
                stream.Seek(0, SeekOrigin.End);
            }
            _writer = new StreamWriter(stream, Utf8NoBom) { AutoFlush = true };
        }
        catch
        {
            _writer = null;
        }
    }
}

[InterpolatedStringHandler]
public ref struct LogInfoInterpolatedStringHandler
{
    private DefaultInterpolatedStringHandler _inner;
    private readonly bool _enabled;

    public LogInfoInterpolatedStringHandler(int literalLength, int formattedCount, out bool isEnabled)
    {
        _enabled = isEnabled = Log.IsInfoEnabled;
        _inner = isEnabled ? new DefaultInterpolatedStringHandler(literalLength, formattedCount) : default;
    }

    public void AppendLiteral(string value) { if (_enabled) _inner.AppendLiteral(value); }
    public void AppendFormatted<T>(T value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted<T>(T value, string? format) { if (_enabled) _inner.AppendFormatted(value, format); }
    public void AppendFormatted<T>(T value, int alignment) { if (_enabled) _inner.AppendFormatted(value, alignment); }
    public void AppendFormatted<T>(T value, int alignment, string? format) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(ReadOnlySpan<char> value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted(ReadOnlySpan<char> value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(string? value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted(string? value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(object? value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }

    internal string GetFormattedText() => _enabled ? _inner.ToStringAndClear() : string.Empty;
    internal bool IsEnabled => _enabled;
}

[InterpolatedStringHandler]
public ref struct LogWarnInterpolatedStringHandler
{
    private DefaultInterpolatedStringHandler _inner;
    private readonly bool _enabled;

    public LogWarnInterpolatedStringHandler(int literalLength, int formattedCount, out bool isEnabled)
    {
        _enabled = isEnabled = Log.IsWarnEnabled;
        _inner = isEnabled ? new DefaultInterpolatedStringHandler(literalLength, formattedCount) : default;
    }

    public void AppendLiteral(string value) { if (_enabled) _inner.AppendLiteral(value); }
    public void AppendFormatted<T>(T value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted<T>(T value, string? format) { if (_enabled) _inner.AppendFormatted(value, format); }
    public void AppendFormatted<T>(T value, int alignment) { if (_enabled) _inner.AppendFormatted(value, alignment); }
    public void AppendFormatted<T>(T value, int alignment, string? format) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(ReadOnlySpan<char> value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted(ReadOnlySpan<char> value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(string? value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted(string? value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(object? value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }

    internal string GetFormattedText() => _enabled ? _inner.ToStringAndClear() : string.Empty;
    internal bool IsEnabled => _enabled;
}

[InterpolatedStringHandler]
public ref struct LogErrorInterpolatedStringHandler
{
    private DefaultInterpolatedStringHandler _inner;
    private readonly bool _enabled;

    public LogErrorInterpolatedStringHandler(int literalLength, int formattedCount, out bool isEnabled)
    {
        _enabled = isEnabled = Log.IsErrorEnabled;
        _inner = isEnabled ? new DefaultInterpolatedStringHandler(literalLength, formattedCount) : default;
    }

    public void AppendLiteral(string value) { if (_enabled) _inner.AppendLiteral(value); }
    public void AppendFormatted<T>(T value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted<T>(T value, string? format) { if (_enabled) _inner.AppendFormatted(value, format); }
    public void AppendFormatted<T>(T value, int alignment) { if (_enabled) _inner.AppendFormatted(value, alignment); }
    public void AppendFormatted<T>(T value, int alignment, string? format) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(ReadOnlySpan<char> value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted(ReadOnlySpan<char> value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(string? value) { if (_enabled) _inner.AppendFormatted(value); }
    public void AppendFormatted(string? value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }
    public void AppendFormatted(object? value, int alignment = 0, string? format = null) { if (_enabled) _inner.AppendFormatted(value, alignment, format); }

    internal string GetFormattedText() => _enabled ? _inner.ToStringAndClear() : string.Empty;
    internal bool IsEnabled => _enabled;
}
