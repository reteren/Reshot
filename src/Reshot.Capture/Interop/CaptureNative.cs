using System.Runtime.InteropServices;

namespace Reshot.Capture.Interop;

/// <summary>P/Invoke and COM interop needed to drive Windows.Graphics.Capture from .NET.</summary>
internal static class CaptureNative
{
    // ---- Virtual-desktop metrics ------------------------------------------------
    public const int SM_XVIRTUALSCREEN = 76;
    public const int SM_YVIRTUALSCREEN = 77;
    public const int SM_CXVIRTUALSCREEN = 78;
    public const int SM_CYVIRTUALSCREEN = 79;

    [DllImport("user32.dll")]
    public static extern int GetSystemMetrics(int nIndex);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    // ---- Monitor enumeration ----------------------------------------------------
    private const int MONITORINFOF_PRIMARY = 0x1;

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left, Top, Right, Bottom;
        public int Width => Right - Left;
        public int Height => Bottom - Top;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MONITORINFO
    {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
    }

    private delegate bool MonitorEnumProc(IntPtr hMonitor, IntPtr hdc, ref RECT rect, IntPtr data);

    [DllImport("user32.dll")]
    private static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc callback, IntPtr data);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFO info);

    public readonly record struct MonitorHandle(IntPtr Handle, RECT Bounds, bool IsPrimary);

    /// <summary>Enumerates every monitor with its virtual-desktop rect and HMONITOR.</summary>
    public static List<MonitorHandle> EnumerateMonitors()
    {
        var result = new List<MonitorHandle>();

        // Keep the delegate rooted for the duration of the call.
        bool Callback(IntPtr hMonitor, IntPtr hdc, ref RECT rect, IntPtr data)
        {
            var info = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
            if (GetMonitorInfo(hMonitor, ref info))
            {
                var isPrimary = (info.dwFlags & MONITORINFOF_PRIMARY) != 0;
                result.Add(new MonitorHandle(hMonitor, info.rcMonitor, isPrimary));
            }
            return true;
        }

        EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, Callback, IntPtr.Zero);
        return result;
    }

    /// <summary>
    /// Copies mapped BGRA rows into a managed buffer. When both sides are tightly packed,
    /// pin the destination and copy the entire image in one operation; monitor composition
    /// still uses the row path because each monitor occupies a slot in a wider virtual frame.
    /// </summary>
    public static unsafe void CopyMappedRows(
        IntPtr source,
        int sourceStride,
        byte[] destination,
        int destinationOffset,
        int destinationStride,
        int rowBytes,
        int rowCount)
    {
        if (rowCount <= 0 || rowBytes <= 0)
            return;

        if (sourceStride == rowBytes && destinationStride == rowBytes)
        {
            var byteCount = checked(rowBytes * rowCount);
            fixed (byte* destinationPtr = destination)
            {
                Buffer.MemoryCopy(
                    (void*)source,
                    destinationPtr + destinationOffset,
                    destination.Length - destinationOffset,
                    byteCount);
            }
            return;
        }

        for (var y = 0; y < rowCount; y++)
        {
            Marshal.Copy(
                source + y * sourceStride,
                destination,
                checked(destinationOffset + y * destinationStride),
                rowBytes);
        }
    }

    // ---- WinRT <-> D3D bridge ---------------------------------------------------

    /// <summary>Wraps a DXGI device as a WinRT IDirect3DDevice (returns +1 ref IInspectable).</summary>
    [DllImport("d3d11.dll", EntryPoint = "CreateDirect3D11DeviceFromDXGIDevice", SetLastError = true)]
    public static extern int CreateDirect3D11DeviceFromDXGIDevice(IntPtr dxgiDevice, out IntPtr graphicsDevice);

    /// <summary>IID of ID3D11Texture2D, used to pull the D3D texture out of a capture surface.</summary>
    public static readonly Guid ID3D11Texture2D = new("6f15aaf2-d208-4e89-9ab4-489535d34f9c");

    /// <summary>Activation IID of GraphicsCaptureItem (passed to the interop factory).</summary>
    public static readonly Guid GraphicsCaptureItem = new("79C3F95B-31F7-4EC2-A464-632EF5D30760");

    /// <summary>Interop factory for creating GraphicsCaptureItem from an HWND/HMONITOR.</summary>
    [ComImport]
    [Guid("3628E81B-3CAC-4C60-B7F4-23CE0E0C3356")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IGraphicsCaptureItemInterop
    {
        IntPtr CreateForWindow([In] IntPtr window, [In] ref Guid iid);
        IntPtr CreateForMonitor([In] IntPtr monitor, [In] ref Guid iid);
    }

    /// <summary>Bridges a WinRT IDirect3DSurface back to its underlying DXGI/D3D interface.</summary>
    [ComImport]
    [Guid("A9B3D012-3DF2-4EE3-B8D1-8695F457D3C1")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IDirect3DDxgiInterfaceAccess
    {
        IntPtr GetInterface([In] ref Guid iid);
    }
}
