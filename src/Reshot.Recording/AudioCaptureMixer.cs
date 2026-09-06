using NAudio.CoreAudioApi;
using NAudio.Wave;
using NAudio.Wave.SampleProviders;
using Reshot.Core.Diagnostics;

namespace Reshot.Recording;

/// <summary>
/// Captures system audio (WASAPI loopback) and/or the microphone (WASAPI capture)
/// and mixes them into one 48 kHz stereo stream (ARCHITECTURE §9). Each source is
/// buffered, converted to the mix format, and summed; the recorder pulls 16-bit
/// PCM at its own cadence. ReadFully means the pull always returns the asked-for
/// amount (silence when a source is idle), so the track stays continuous.
/// </summary>
public sealed class AudioCaptureMixer : IDisposable
{
    public const int SampleRate = 48000;
    public const int Channels = 2;

    private readonly List<IWaveIn> _captures = new();
    private readonly List<BufferedWaveProvider> _buffers = new();
    private readonly MixingSampleProvider _mixer;
    private float[] _scratch = Array.Empty<float>();
    private bool _disposed;

    public bool HasAnySource => _captures.Count > 0;

    public AudioCaptureMixer(AudioSources sources)
    {
        _mixer = new MixingSampleProvider(WaveFormat.CreateIeeeFloatWaveFormat(SampleRate, Channels))
        {
            ReadFully = true,
        };

        if (sources.SystemFull)
        {
            TryAddSource(() => new WasapiLoopbackCapture(), "system");
        }
        else
        {
            foreach (var pid in sources.IncludePids)
            {
                var target = pid;
                TryAddSource(() => new ProcessLoopbackCapture(target, excludeMode: false), $"process {target}");
            }
        }

        if (sources.Mic)
            TryAddSource(() => CreateMic(sources.MicDevice), "microphone");
    }

    private static IWaveIn CreateMic(string? deviceId)
    {
        if (!string.IsNullOrWhiteSpace(deviceId) && deviceId != "default")
        {
            using var enumerator = new MMDeviceEnumerator();
            foreach (var device in enumerator.EnumerateAudioEndPoints(DataFlow.Capture, DeviceState.Active))
            {
                if (device.ID == deviceId)
                    return new WasapiCapture(device);
            }
        }
        return new WasapiCapture(); // default capture device
    }

    private void TryAddSource(Func<IWaveIn> factory, string name)
    {
        try
        {
            var capture = factory();
            var buffer = new BufferedWaveProvider(capture.WaveFormat)
            {
                BufferDuration = TimeSpan.FromSeconds(10),
                DiscardOnBufferOverflow = true,
            };
            capture.DataAvailable += (_, e) => buffer.AddSamples(e.Buffer, 0, e.BytesRecorded);

            ISampleProvider provider = buffer.ToSampleProvider();
            if (provider.WaveFormat.Channels == 1)
                provider = new MonoToStereoSampleProvider(provider);
            else if (provider.WaveFormat.Channels > 2)
                provider = new MultiplexingSampleProvider(new[] { provider }, 2);
            if (provider.WaveFormat.SampleRate != SampleRate)
                provider = new WdlResamplingSampleProvider(provider, SampleRate);

            _mixer.AddMixerInput(provider);
            capture.StartRecording();
            _captures.Add(capture);
            _buffers.Add(buffer);
            Log.Info($"Audio: capturing {name} ({capture.WaveFormat.SampleRate}Hz/{capture.WaveFormat.Channels}ch).");
        }
        catch (Exception ex)
        {
            Log.Warn($"Audio: {name} unavailable: {ex.Message}");
        }
    }

    /// <summary>Drops any buffered audio so playback re-aligns to "now" (called at recording start).</summary>
    public void Flush()
    {
        foreach (var buffer in _buffers)
            buffer.ClearBuffer();
    }

    /// <summary>Reads <paramref name="frames"/> mixed frames as 16-bit interleaved PCM; returns bytes written.</summary>
    public unsafe int ReadPcm16(byte[] dest, int frames)
    {
        ArgumentNullException.ThrowIfNull(dest);
        if (frames <= 0)
            return 0;

        var samples = frames * Channels;
        if (_scratch.Length < samples)
            _scratch = new float[samples];

        var read = _mixer.Read(_scratch, 0, samples);
        var maxSamples = Math.Min(read, dest.Length / 2);

        fixed (float* pScratch = _scratch)
        fixed (byte* pDest = dest)
        {
            var pOut = (short*)pDest;
            for (var i = 0; i < maxSamples; i++)
            {
                pOut[i] = (short)(Math.Clamp(pScratch[i], -1f, 1f) * short.MaxValue);
            }
        }
        return maxSamples * 2;
    }

    public void Dispose()
    {
        if (_disposed)
            return;
        _disposed = true;
        foreach (var capture in _captures)
        {
            try { capture.StopRecording(); } catch { /* ignore */ }
            capture.Dispose();
        }
    }
}
