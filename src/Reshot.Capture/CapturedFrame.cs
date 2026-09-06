
namespace Reshot.Capture;

/// <summary>
/// One frozen snapshot of the whole virtual desktop, as tightly-packed BGRA32
/// pixels (stride == Width*4). Coordinates are in virtual-desktop space so the
/// overlay can map screen points straight into the buffer. UI-agnostic on purpose
/// (Reshot.Core / Reshot.Capture never depend on WPF).
/// </summary>
public sealed class CapturedFrame : IDisposable
{
    private byte[]? _pixelsBgra;
    private bool _ownsPixels;

    /// <summary>Creates an externally-backed frame; capture services use a private lease to own pooled buffers.</summary>
    public CapturedFrame()
    {
    }

    /// <summary>
    /// Raised on a timer thread once the pooled frame buffer has been idle long enough to be
    /// dropped. The host subscribes to compact the large-object heap at a moment it knows is
    /// safe; the pool itself cannot tell a recording or an export from an idle tray.
    /// </summary>
    public static event Action? PoolWentIdle;

    internal static void RaisePoolWentIdle() => PoolWentIdle?.Invoke();

    /// <summary>
    /// The captured pixels. Access after <see cref="Dispose"/> throws instead of exposing a
    /// returned buffer that may already contain a later screenshot.
    /// </summary>
    public required byte[] PixelsBgra
    {
        get => Volatile.Read(ref _pixelsBgra)
            ?? throw new ObjectDisposedException(nameof(CapturedFrame));
        init => _pixelsBgra = value ?? throw new ArgumentNullException(nameof(value));
    }
    public required int Width { get; init; }
    public required int Height { get; init; }

    /// <summary>Bytes per row. Always <c>Width * 4</c>, rows are compacted on capture.</summary>
    public int Stride => Width * 4;

    /// <summary>X of the virtual desktop's top-left in screen coordinates (can be negative).</summary>
    public required int VirtualLeft { get; init; }

    /// <summary>Y of the virtual desktop's top-left in screen coordinates (can be negative).</summary>
    public required int VirtualTop { get; init; }

    /// <summary>The monitors that make up this frame, in screen coordinates.</summary>
    public required IReadOnlyList<CapturedMonitor> Monitors { get; init; }

    internal static FrameBufferLease RentBuffer(int length) =>
        new(FrameBufferPool.Rent(length));

    public void Dispose()
    {
        var pixels = Interlocked.Exchange(ref _pixelsBgra, null);
        if (pixels is null || !_ownsPixels)
            return;

        _ownsPixels = false;
        FrameBufferPool.Return(pixels);
        GC.SuppressFinalize(this);
    }

    ~CapturedFrame()
    {
        var pixels = Interlocked.Exchange(ref _pixelsBgra, null);
        if (pixels is not null && _ownsPixels)
        {
            _ownsPixels = false;
            FrameBufferPool.Return(pixels);
        }
    }

    /// <summary>Owns a rented exact-sized frame buffer until a frame takes it.</summary>
    internal sealed class FrameBufferLease : IDisposable
    {
        private byte[]? _buffer;

        internal FrameBufferLease(byte[] buffer) => _buffer = buffer;

        internal byte[] Buffer => _buffer
            ?? throw new ObjectDisposedException(nameof(FrameBufferLease));

        internal CapturedFrame CreateFrame(
            int width,
            int height,
            int virtualLeft,
            int virtualTop,
            IReadOnlyList<CapturedMonitor> monitors)
        {
            var buffer = Buffer;
            var frame = new CapturedFrame
            {
                PixelsBgra = buffer,
                Width = width,
                Height = height,
                VirtualLeft = virtualLeft,
                VirtualTop = virtualTop,
                Monitors = monitors,
            };
            frame._ownsPixels = true;
            _buffer = null;
            return frame;
        }

        public void Dispose()
        {
            var buffer = Interlocked.Exchange(ref _buffer, null);
            if (buffer is not null)
                FrameBufferPool.Return(buffer);
        }
    }
}

/// <summary>
/// Exact-length pool for frame buffers. ArrayPool can return a larger array, which would
/// violate the public PixelsBgra.Length contract used by the WPF and Skia consumers. A single
/// one-shot timer drops the retained buffer after idle; it is never a periodic process wakeup.
/// </summary>
internal static class FrameBufferPool
{
    private static readonly object Gate = new();
    private static readonly TimeSpan Retention = TimeSpan.FromSeconds(30);
    private static byte[]? _retained;
    private static int _retainedLength;
    private static int _lastRequestedLength;
    private static int _activeRentals;
    private static Timer? _evictionTimer;

    public static byte[] Rent(int length)
    {
        byte[] buffer;
        lock (Gate)
        {
            _lastRequestedLength = length;
            _activeRentals++;
            if (_retained is not null && _retainedLength == length)
            {
                buffer = _retained;
                _retained = null;
                _retainedLength = 0;
                DisposeEvictionTimerLocked();
            }
            else
            {
                buffer = new byte[length];
                _retained = null;
                _retainedLength = 0;
                DisposeEvictionTimerLocked();
            }
        }

        // A failed/aborted capture may have written only part of the frame. Clear before
        // handing the buffer out so unwritten virtual-desktop gaps remain transparent black.
        Array.Clear(buffer);
        return buffer;
    }

    public static void Return(byte[] buffer)
    {
        lock (Gate)
        {
            _activeRentals--;
            // Keep exactly one buffer and reject late returns from an older monitor layout.
            if (_retained is not null || buffer.Length != _lastRequestedLength)
                return;

            _retained = buffer;
            _retainedLength = buffer.Length;
            _evictionTimer = new Timer(
                static _ => EvictAfterIdle(),
                state: null,
                dueTime: Retention,
                period: Timeout.InfiniteTimeSpan);
        }
    }

    private static void EvictAfterIdle()
    {
        lock (Gate)
        {
            if (_retained is null)
            {
                DisposeEvictionTimerLocked();
                return;
            }

            // A concurrent capture may still own a second, non-retained rental. Keep the
            // one-shot timer alive until every reader has released its frame.
            if (_activeRentals != 0)
            {
                _evictionTimer = new Timer(
                    static _ => EvictAfterIdle(),
                    state: null,
                    dueTime: Retention,
                    period: Timeout.InfiniteTimeSpan);
                return;
            }

            _retained = null;
            _retainedLength = 0;
            DisposeEvictionTimerLocked();
        }

        // Dropping the root alone leaves an otherwise idle LOH segment committed until the
        // runtime's next collection, so someone still has to compact. Not us: this timer
        // knows nothing about whether a recording or an export is running, and a blocking
        // compacting gen-2 in the middle of one is a visible stutter. The app layer owns
        // session state, so it decides when collecting is safe.
        CapturedFrame.RaisePoolWentIdle();
    }

    private static void DisposeEvictionTimerLocked()
    {
        _evictionTimer?.Dispose();
        _evictionTimer = null;
    }
}

/// <summary>A single monitor's placement within the virtual desktop.</summary>
public sealed record CapturedMonitor(int Left, int Top, int Width, int Height, bool IsPrimary)
{
    public int Right => Left + Width;
    public int Bottom => Top + Height;
}
