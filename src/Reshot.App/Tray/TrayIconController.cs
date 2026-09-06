using System.Runtime.InteropServices;
using System.Windows.Interop;
using Reshot.App.Interop;
using Reshot.Core.Diagnostics;

namespace Reshot.App.Tray;

/// <summary>
/// Icon styles for tray balloon notifications, mapping to Win32 NIIF_* flags.
/// </summary>
public enum BalloonIcon
{
    None = 0,
    Info = 1,
    Warning = 2,
    Error = 3
}

/// <summary>
/// The tray presence (SPEC §14): a native Win32 Shell_NotifyIconW on a hidden top-level window
/// with a context menu (Capture / Settings / Pause hotkey / Quit) and balloon feedback.
/// Owns an HICON loaded via CreateIconFromResourceEx from the embedded reshot.ico resource.
/// Listens for TaskbarCreated to restore the tray icon when Explorer restarts.
/// Raises intent events; it holds no app logic itself.
/// </summary>
public sealed class TrayIconController : IDisposable
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NOTIFYICONDATA
    {
        public uint cbSize;
        public IntPtr hWnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
        public string szTip;
        public uint dwState;
        public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
        public string szInfo;
        public uint uTimeoutOrVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
        public string szInfoTitle;
        public uint dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    private const uint NIM_ADD = 0x00000000;
    private const uint NIM_MODIFY = 0x00000001;
    private const uint NIM_DELETE = 0x00000002;

    private const uint NIF_MESSAGE = 0x00000001;
    private const uint NIF_ICON = 0x00000002;
    private const uint NIF_TIP = 0x00000004;
    private const uint NIF_INFO = 0x00000010;

    private const uint WM_USER = 0x0400;
    private const uint WM_TRAYCALLBACK = WM_USER + 101;

    private const int WM_LBUTTONDBLCLK = 0x0203;
    private const int WM_RBUTTONUP = 0x0205;
    private const int WM_CONTEXTMENU = 0x007B;

    private const int IDI_APPLICATION = 32512;
    private const uint TrayIconId = 1;

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool Shell_NotifyIcon(uint dwMessage, ref NOTIFYICONDATA lpData);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint RegisterWindowMessage(string lpString);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr CreateIconFromResourceEx(
        byte[] pbIconBits,
        uint cbIconBits,
        [MarshalAs(UnmanagedType.Bool)] bool fIcon,
        uint dwVersion,
        int cxDesired,
        int cyDesired,
        uint uFlags);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyIcon(IntPtr hIcon);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr LoadIcon(IntPtr hInstance, IntPtr lpIconName);

    private readonly HwndSource _source;
    private readonly IntPtr _hwnd;
    private readonly uint _taskbarCreatedMsg;
    private IntPtr _hIcon;
    private bool _ownsIcon;
    private bool _iconAdded;
    private bool _disposed;

    public event EventHandler? CaptureRequested;

    /// <summary>
    /// Right-click on the icon. The menu itself is a styled WPF window
    /// (<see cref="TrayMenuWindow"/>) owned by the App layer, not a
    /// <c>ContextMenuStrip</c>, so it can match the settings dialog.
    /// </summary>
    public event EventHandler? MenuRequested;

    /// <summary>Current "Pause hotkey" state, used to seed the menu when it opens.</summary>
    public bool IsPaused { get; private set; }

    public TrayIconController()
    {
        // Hidden top-level window (WS_POPUP, 0x0, no parent).
        // Message-only windows (HWND_MESSAGE) do not receive broadcast messages like TaskbarCreated.
        var parameters = new HwndSourceParameters("Reshot.TrayHost")
        {
            WindowStyle = unchecked((int)0x80000000), // WS_POPUP
            Width = 0,
            Height = 0,
            ParentWindow = IntPtr.Zero,
        };

        _source = new HwndSource(parameters);
        _source.AddHook(WndProc);
        _hwnd = _source.Handle;

        _taskbarCreatedMsg = RegisterWindowMessage("TaskbarCreated");

        _hIcon = LoadAppIcon(out _ownsIcon);
        if (_hIcon == IntPtr.Zero)
        {
            _hIcon = LoadIcon(IntPtr.Zero, (IntPtr)IDI_APPLICATION);
            _ownsIcon = false;
        }

        AddTrayIcon();

        AppDomain.CurrentDomain.ProcessExit += OnProcessExit;

        Log.Info("Tray: native icon created.");
    }

    /// <summary>Records the current pause state so the next menu opens in sync.</summary>
    public void SetPaused(bool paused) => IsPaused = paused;

    public void ShowBalloon(string title, string text, BalloonIcon icon = BalloonIcon.Info)
    {
        if (_disposed || _hwnd == IntPtr.Zero)
            return;

        var nid = new NOTIFYICONDATA
        {
            cbSize = (uint)Marshal.SizeOf<NOTIFYICONDATA>(),
            hWnd = _hwnd,
            uID = TrayIconId,
            uFlags = NIF_INFO,
            szInfoTitle = Truncate(title, 63),
            szInfo = Truncate(text, 255),
            dwInfoFlags = (uint)icon,
        };

        Shell_NotifyIcon(NIM_MODIFY, ref nid);
    }

    private void AddTrayIcon()
    {
        if (_hwnd == IntPtr.Zero || _hIcon == IntPtr.Zero)
            return;

        var nid = new NOTIFYICONDATA
        {
            cbSize = (uint)Marshal.SizeOf<NOTIFYICONDATA>(),
            hWnd = _hwnd,
            uID = TrayIconId,
            uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage = WM_TRAYCALLBACK,
            hIcon = _hIcon,
            szTip = "Reshot",
        };

        if (Shell_NotifyIcon(NIM_ADD, ref nid))
        {
            _iconAdded = true;
        }
        else
        {
            // If NIM_ADD fails (e.g. leftover icon from crashed instance), attempt NIM_MODIFY
            _iconAdded = Shell_NotifyIcon(NIM_MODIFY, ref nid);
            if (!_iconAdded)
            {
                Log.Warn("Tray: Shell_NotifyIcon(NIM_ADD) failed.");
            }
        }
    }

    private IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (msg == (int)_taskbarCreatedMsg && _taskbarCreatedMsg != 0)
        {
            Log.Info("Tray: taskbar recreated, restoring tray icon.");
            AddTrayIcon();
            handled = true;
            return IntPtr.Zero;
        }

        if (msg == WM_TRAYCALLBACK)
        {
            var mouseMsg = lParam.ToInt32();
            switch (mouseMsg)
            {
                case WM_RBUTTONUP:
                case WM_CONTEXTMENU:
                    NativeMethods.SetForegroundWindow(_hwnd);
                    MenuRequested?.Invoke(this, EventArgs.Empty);
                    handled = true;
                    break;

                case WM_LBUTTONDBLCLK:
                    CaptureRequested?.Invoke(this, EventArgs.Empty);
                    handled = true;
                    break;
            }
        }

        return IntPtr.Zero;
    }

    /// <summary>Loads the designed app icon from the embedded resource, or null on failure.</summary>
    private static IntPtr LoadAppIcon(out bool ownsHandle)
    {
        ownsHandle = false;
        try
        {
            var uri = new Uri("pack://application:,,,/Assets/reshot.ico");
            var info = System.Windows.Application.GetResourceStream(uri);
            if (info?.Stream is null)
                return IntPtr.Zero;

            using var stream = info.Stream;
            using var ms = new System.IO.MemoryStream();
            stream.CopyTo(ms);
            var bytes = ms.ToArray();

            if (bytes.Length < 6)
                return IntPtr.Zero;

            var idReserved = BitConverter.ToUInt16(bytes, 0);
            var idType = BitConverter.ToUInt16(bytes, 2);
            var idCount = BitConverter.ToUInt16(bytes, 4);

            if (idReserved != 0 || idType != 1 || idCount == 0)
                return IntPtr.Zero;

            // Target 32x32 frame (crisp in the tray and scales down cleanly at any DPI).
            const int targetSize = 32;
            int bestIndex = 0;
            int bestDiff = int.MaxValue;

            for (int i = 0; i < idCount; i++)
            {
                int entryOffset = 6 + i * 16;
                if (entryOffset + 16 > bytes.Length)
                    break;

                int w = bytes[entryOffset];
                if (w == 0) w = 256;

                int diff = Math.Abs(w - targetSize);
                if (diff < bestDiff)
                {
                    bestDiff = diff;
                    bestIndex = i;
                }
            }

            int chosenOffset = 6 + bestIndex * 16;
            uint imgSize = BitConverter.ToUInt32(bytes, chosenOffset + 8);
            uint imgOffset = BitConverter.ToUInt32(bytes, chosenOffset + 12);

            if (imgOffset + imgSize > bytes.Length)
                return IntPtr.Zero;

            var imgData = new byte[imgSize];
            Array.Copy(bytes, imgOffset, imgData, 0, imgSize);

            var hIcon = CreateIconFromResourceEx(imgData, (uint)imgData.Length, true, 0x00030000, 32, 32, 0);
            if (hIcon != IntPtr.Zero)
            {
                ownsHandle = true;
                return hIcon;
            }
        }
        catch (Exception ex)
        {
            Log.Warn($"Tray: could not load app icon: {ex.Message}");
        }

        return IntPtr.Zero;
    }

    private static string Truncate(string str, int maxLen)
    {
        if (string.IsNullOrEmpty(str)) return string.Empty;
        return str.Length <= maxLen ? str : str.Substring(0, maxLen);
    }

    ~TrayIconController()
    {
        Dispose(false);
    }

    private void OnProcessExit(object? sender, EventArgs e) => Dispose();

    public void Dispose()
    {
        Dispose(true);
        GC.SuppressFinalize(this);
    }

    private void Dispose(bool disposing)
    {
        if (_disposed)
            return;

        _disposed = true;

        if (_iconAdded && _hwnd != IntPtr.Zero)
        {
            var nid = new NOTIFYICONDATA
            {
                cbSize = (uint)Marshal.SizeOf<NOTIFYICONDATA>(),
                hWnd = _hwnd,
                uID = TrayIconId,
            };
            Shell_NotifyIcon(NIM_DELETE, ref nid);
            _iconAdded = false;
        }

        if (_hIcon != IntPtr.Zero && _ownsIcon)
        {
            DestroyIcon(_hIcon);
            _hIcon = IntPtr.Zero;
        }

        if (disposing)
        {
            AppDomain.CurrentDomain.ProcessExit -= OnProcessExit;
            if (_source.CheckAccess())
            {
                _source.RemoveHook(WndProc);
                _source.Dispose();
            }
        }
    }
}
