using System.Runtime.InteropServices;
using NAudio.Wave;
using Reshot.Core.Diagnostics;

namespace Reshot.Recording;

/// <summary>
/// Captures audio from specific processes via the Windows Process Loopback API
/// (Win10 20H1+): ActivateAudioInterfaceAsync on the "VAD\Process_Loopback"
/// virtual device with a target PID, in include or exclude tree mode. Exposed as
/// an NAudio <see cref="IWaveIn"/> (IEEE float 48 kHz stereo) so it plugs into the
/// mixer next to the microphone.
/// </summary>
public sealed class ProcessLoopbackCapture : IWaveIn
{
    private const string VirtualDevice = "VAD\\Process_Loopback";
    private const uint AUDCLNT_STREAMFLAGS_LOOPBACK = 0x00020000;
    private const uint AUDCLNT_STREAMFLAGS_EVENTCALLBACK = 0x00040000;

    private readonly int _pid;
    private readonly bool _excludeMode;
    private IAudioClient? _client;
    private IAudioCaptureClient? _capture;
    private IntPtr _event;
    private Thread? _thread;
    private volatile bool _stop;

    public WaveFormat WaveFormat { get; set; } = WaveFormat.CreateIeeeFloatWaveFormat(48000, 2);
    public event EventHandler<WaveInEventArgs>? DataAvailable;
    public event EventHandler<StoppedEventArgs>? RecordingStopped;

    public ProcessLoopbackCapture(int processId, bool excludeMode)
    {
        _pid = processId;
        _excludeMode = excludeMode;
    }

    public void StartRecording()
    {
        _event = CreateEventW(IntPtr.Zero, false, false, null);
        _thread = new Thread(Run) { IsBackground = true, Name = "reshot-proc-loopback" };
        _thread.SetApartmentState(ApartmentState.MTA);
        _thread.Start();
    }

    private void Run()
    {
        try
        {
            _client = Activate();
            var format = BuildFloatFormat();
            var fmtPtr = Marshal.AllocHGlobal(Marshal.SizeOf<WaveFormatExS>());
            Marshal.StructureToPtr(format, fmtPtr, false);
            try
            {
                const long hns = 20 * 10_000; // 20 ms buffer
                var hr = _client!.Initialize(0, AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK, hns, 0, fmtPtr, IntPtr.Zero);
                if (hr != 0)
                    throw Marshal.GetExceptionForHR(hr) ?? new COMException("IAudioClient.Initialize failed", hr);
            }
            finally
            {
                Marshal.FreeHGlobal(fmtPtr);
            }

            _client.SetEventHandle(_event);
            var iid = typeof(IAudioCaptureClient).GUID;
            _client.GetService(ref iid, out var svc);
            _capture = (IAudioCaptureClient)svc;

            _client.Start();
            var frameBytes = WaveFormat.BlockAlign;
            byte[] recordBuffer = Array.Empty<byte>();
            while (!_stop)
            {
                if (WaitForSingleObject(_event, 200) != 0)
                    continue;
                while (_capture.GetNextPacketSize(out var packet) == 0 && packet > 0)
                {
                    var r = _capture.GetBuffer(out var data, out var frames, out var flags, out _, out _);
                    if (r != 0)
                        break;
                    var bytes = (int)frames * frameBytes;
                    if (recordBuffer.Length < bytes)
                        recordBuffer = new byte[bytes];

                    if ((flags & 0x2) == 0 && data != IntPtr.Zero) // not AUDCLNT_BUFFERFLAGS_SILENT
                        Marshal.Copy(data, recordBuffer, 0, bytes);
                    else
                        Array.Clear(recordBuffer, 0, bytes);

                    _capture.ReleaseBuffer(frames);
                    if (bytes > 0)
                        DataAvailable?.Invoke(this, new WaveInEventArgs(recordBuffer, bytes));
                }
            }
            _client.Stop();
        }
        catch (Exception ex)
        {
            Log.Warn($"Process loopback (pid {_pid}) failed: {ex.Message}");
            RecordingStopped?.Invoke(this, new StoppedEventArgs(ex));
        }
    }

    private IAudioClient Activate()
    {
        var paramsBlob = new AudioClientActivationParams
        {
            ActivationType = 1, // ProcessLoopback
            TargetProcessId = _pid,
            ProcessLoopbackMode = _excludeMode ? 1 : 0,
        };
        var blobPtr = Marshal.AllocHGlobal(Marshal.SizeOf<AudioClientActivationParams>());
        Marshal.StructureToPtr(paramsBlob, blobPtr, false);

        var propPtr = Marshal.AllocHGlobal(Marshal.SizeOf<PropVariantBlob>());
        Marshal.StructureToPtr(new PropVariantBlob
        {
            vt = 0x41, // VT_BLOB
            cbSize = Marshal.SizeOf<AudioClientActivationParams>(),
            pBlobData = blobPtr,
        }, propPtr, false);

        try
        {
            var handler = new CompletionHandler();
            var iid = typeof(IAudioClient).GUID;
            ActivateAudioInterfaceAsync(VirtualDevice, ref iid, propPtr, handler, out _);
            if (!handler.Wait(3000))
                throw new TimeoutException("Process-loopback activation timed out.");

            handler.Operation!.GetActivateResult(out var hr, out var iface);
            if (hr != 0)
                throw Marshal.GetExceptionForHR(hr) ?? new COMException("Process-loopback activation failed", hr);
            return (IAudioClient)iface;
        }
        finally
        {
            Marshal.FreeHGlobal(propPtr);
            Marshal.FreeHGlobal(blobPtr);
        }
    }

    private static WaveFormatExS BuildFloatFormat() => new()
    {
        wFormatTag = 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels = 2,
        nSamplesPerSec = 48000,
        wBitsPerSample = 32,
        nBlockAlign = 8,
        nAvgBytesPerSec = 48000 * 8,
        cbSize = 0,
    };

    public void StopRecording()
    {
        _stop = true;
        _thread?.Join(1000);
    }

    public void Dispose()
    {
        StopRecording();
        if (_capture is not null) Marshal.ReleaseComObject(_capture);
        if (_client is not null) Marshal.ReleaseComObject(_client);
        if (_event != IntPtr.Zero) CloseHandle(_event);
        _capture = null;
        _client = null;
        _event = IntPtr.Zero;
    }

    // ---- Native / COM ----------------------------------------------------------

    [DllImport("Mmdevapi.dll", ExactSpelling = true, PreserveSig = false)]
    private static extern void ActivateAudioInterfaceAsync(
        [MarshalAs(UnmanagedType.LPWStr)] string deviceInterfacePath,
        [In] ref Guid riid,
        [In] IntPtr activationParams,
        [In] IActivateAudioInterfaceCompletionHandler completionHandler,
        out IActivateAudioInterfaceAsyncOperation operation);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr CreateEventW(IntPtr attrs, bool manualReset, bool initialState, string? name);
    [DllImport("kernel32.dll")] private static extern uint WaitForSingleObject(IntPtr handle, uint ms);
    [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);

    private sealed class CompletionHandler : IActivateAudioInterfaceCompletionHandler
    {
        private readonly System.Threading.ManualResetEventSlim _done = new(false);
        public IActivateAudioInterfaceAsyncOperation? Operation { get; private set; }
        public void ActivateCompleted(IActivateAudioInterfaceAsyncOperation operation)
        {
            Operation = operation;
            _done.Set();
        }
        public bool Wait(int ms) => _done.Wait(ms);
    }

    [ComImport, Guid("41D949AB-9862-444A-80F6-C261334DA5EB"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IActivateAudioInterfaceCompletionHandler
    {
        void ActivateCompleted(IActivateAudioInterfaceAsyncOperation operation);
    }

    [ComImport, Guid("72A22D78-CDE4-431D-B8CC-843A71199B6D"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IActivateAudioInterfaceAsyncOperation
    {
        void GetActivateResult(out int activateResult, [MarshalAs(UnmanagedType.IUnknown)] out object activatedInterface);
    }

    [ComImport, Guid("1CB9AD4C-DBFA-4C32-B178-C2F568A703B2"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IAudioClient
    {
        [PreserveSig] int Initialize(int shareMode, uint streamFlags, long bufDuration, long periodicity, IntPtr format, IntPtr sessionGuid);
        [PreserveSig] int GetBufferSize(out uint frames);
        [PreserveSig] int GetStreamLatency(out long latency);
        [PreserveSig] int GetCurrentPadding(out uint padding);
        [PreserveSig] int IsFormatSupported(int shareMode, IntPtr format, out IntPtr closest);
        [PreserveSig] int GetMixFormat(out IntPtr format);
        [PreserveSig] int GetDevicePeriod(out long defaultPeriod, out long minPeriod);
        [PreserveSig] int Start();
        [PreserveSig] int Stop();
        [PreserveSig] int Reset();
        [PreserveSig] int SetEventHandle(IntPtr handle);
        [PreserveSig] int GetService(ref Guid iid, [MarshalAs(UnmanagedType.IUnknown)] out object service);
    }

    [ComImport, Guid("C8ADBD64-E71E-48A0-A4DE-185C395CD317"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IAudioCaptureClient
    {
        [PreserveSig] int GetBuffer(out IntPtr data, out uint frames, out uint flags, out long devicePos, out long qpcPos);
        [PreserveSig] int ReleaseBuffer(uint frames);
        [PreserveSig] int GetNextPacketSize(out uint frames);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct AudioClientActivationParams
    {
        public int ActivationType;
        public int TargetProcessId;
        public int ProcessLoopbackMode;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct PropVariantBlob
    {
        public ushort vt;
        public ushort r1, r2, r3;
        public int cbSize;
        public IntPtr pBlobData;
    }

    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    private struct WaveFormatExS
    {
        public ushort wFormatTag;
        public ushort nChannels;
        public uint nSamplesPerSec;
        public uint nAvgBytesPerSec;
        public ushort nBlockAlign;
        public ushort wBitsPerSample;
        public ushort cbSize;
    }
}
