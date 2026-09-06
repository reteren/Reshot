using System;
using System.IO;
using System.Threading;
using Reshot.Capture;
using Reshot.Recording;
using Xunit;

namespace Reshot.Recording.Tests;

public class SafetyAndTimerTests
{
    private sealed class ThrowingCaptureService : IScreenCaptureService
    {
        public CapturedFrame SnapshotAllMonitors() => throw new NotSupportedException();
        public WgcMonitorStream StartMonitorStream(int screenX, int screenY) =>
            throw new InvalidOperationException("Simulated capture startup failure for timer test");
    }

    [Fact]
    public void VideoRecorder_constructor_exception_restores_timer_period()
    {
        int beginCount = 0;
        int endCount = 0;

        var capture = new ThrowingCaptureService();
        Assert.Throws<InvalidOperationException>(() => new VideoRecorder(
            capture, 0, 0, 100, 100, 30, 1_000_000, "out.mp4",
            new AudioSources(), null, 0,
            timeBegin: _ => Interlocked.Increment(ref beginCount),
            timeEnd: _ => Interlocked.Increment(ref endCount)));

        Assert.Equal(1, beginCount);
        Assert.Equal(1, endCount);
    }

    [Fact]
    public void VideoRecorder_double_stop_and_dispose_is_idempotent()
    {
        int beginCount = 0;
        int endCount = 0;

        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_timer_{Guid.NewGuid():N}.mp4");
        try
        {
            var capture = new ScreenCaptureService();
            var recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false }, null, 0,
                timeBegin: _ => Interlocked.Increment(ref beginCount),
                timeEnd: _ => Interlocked.Increment(ref endCount));

            Assert.Equal(1, beginCount);
            Assert.Equal(0, endCount);

            recorder.Stop();
            Assert.Equal(1, endCount);

            // Second stop must be idempotent (no extra TimeEndPeriod)
            recorder.Stop();
            Assert.Equal(1, endCount);

            // Dispose after stop must also not call TimeEndPeriod again
            recorder.Dispose();
            Assert.Equal(1, endCount);
        }
        finally
        {
            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public void VideoRecorder_dispose_without_prior_stop_restores_timer()
    {
        int beginCount = 0;
        int endCount = 0;

        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_timer_disp_{Guid.NewGuid():N}.mp4");
        try
        {
            var capture = new ScreenCaptureService();
            var recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false }, null, 0,
                timeBegin: _ => Interlocked.Increment(ref beginCount),
                timeEnd: _ => Interlocked.Increment(ref endCount));

            Assert.Equal(1, beginCount);
            Assert.Equal(0, endCount);

            recorder.Dispose();
            Assert.Equal(1, endCount);

            // Double dispose must not call TimeEndPeriod again
            recorder.Dispose();
            Assert.Equal(1, endCount);
        }
        finally
        {
            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public void ApplyMask_1x1_on_2x2_crop_does_not_overrun_and_blacks_out_padding()
    {
        // 1x1 selection clamped to 2x2 crop
        // Buffer: 2x2 = 4 pixels (16 bytes), initially all white (0xFFFFFFFF)
        var buffer = new byte[2 * 2 * 4];
        Array.Fill<byte>(buffer, 0xFF);

        // Mask: 1x1, value 255 (selected pixel)
        var mask = new byte[] { 255 };

        VideoRecorder.ApplyMask(buffer, cropWidth: 2, cropHeight: 2, mask, maskWidth: 1, maskHeight: 1, maskStride: 1);

        unsafe
        {
            fixed (byte* p = buffer)
            {
                var uints = (uint*)p;
                // Pixel (0,0): kept white
                Assert.Equal(0xFFFFFFFF, uints[0]);
                // Pixel (1,0): padded column -> blacked out
                Assert.Equal(0xFF000000, uints[1]);
                // Pixel (0,1): padded row -> blacked out
                Assert.Equal(0xFF000000, uints[2]);
                // Pixel (1,1): padded row & col -> blacked out
                Assert.Equal(0xFF000000, uints[3]);
            }
        }
    }

    [Fact]
    public void ApplyMask_1xN_on_2xN_crop_does_not_overrun_and_blacks_out_extra_col()
    {
        // 1x10 selection clamped to 2x10
        int n = 10;
        var buffer = new byte[2 * n * 4];
        Array.Fill<byte>(buffer, 0xFF);

        var mask = new byte[n];
        Array.Fill<byte>(mask, 255);

        VideoRecorder.ApplyMask(buffer, cropWidth: 2, cropHeight: n, mask, maskWidth: 1, maskHeight: n, maskStride: 1);

        unsafe
        {
            fixed (byte* p = buffer)
            {
                var uints = (uint*)p;
                for (int y = 0; y < n; y++)
                {
                    Assert.Equal(0xFFFFFFFF, uints[y * 2 + 0]); // selected column
                    Assert.Equal(0xFF000000, uints[y * 2 + 1]); // padded column
                }
            }
        }
    }

    [Fact]
    public void ApplyMask_Nx1_on_Nx2_crop_does_not_overrun_and_blacks_out_extra_row()
    {
        // 10x1 selection clamped to 10x2
        int n = 10;
        var buffer = new byte[n * 2 * 4];
        Array.Fill<byte>(buffer, 0xFF);

        var mask = new byte[n];
        Array.Fill<byte>(mask, 255);

        VideoRecorder.ApplyMask(buffer, cropWidth: n, cropHeight: 2, mask, maskWidth: n, maskHeight: 1, maskStride: n);

        unsafe
        {
            fixed (byte* p = buffer)
            {
                var uints = (uint*)p;
                for (int x = 0; x < n; x++)
                {
                    Assert.Equal(0xFFFFFFFF, uints[0 * n + x]); // selected row
                    Assert.Equal(0xFF000000, uints[1 * n + x]); // padded row
                }
            }
        }
    }

    [Fact]
    public void ApplyMask_odd_dimensions_handled_safely()
    {
        // 3x3 selection with 4x4 crop
        var buffer = new byte[4 * 4 * 4];
        Array.Fill<byte>(buffer, 0xFF);

        var mask = new byte[3 * 3];
        Array.Fill<byte>(mask, 255);

        VideoRecorder.ApplyMask(buffer, cropWidth: 4, cropHeight: 4, mask, maskWidth: 3, maskHeight: 3, maskStride: 3);

        unsafe
        {
            fixed (byte* p = buffer)
            {
                var uints = (uint*)p;
                for (int y = 0; y < 4; y++)
                {
                    for (int x = 0; x < 4; x++)
                    {
                        if (x < 3 && y < 3)
                            Assert.Equal(0xFFFFFFFF, uints[y * 4 + x]);
                        else
                            Assert.Equal(0xFF000000, uints[y * 4 + x]);
                    }
                }
            }
        }
    }

    [Fact]
    public void ApplyMask_threshold_blacks_out_unselected_pixels()
    {
        var buffer = new byte[2 * 2 * 4];
        Array.Fill<byte>(buffer, 0xFF);

        // Mask: (0,0) is 127 (< 128 -> black), (1,0) is 128 (>= 128 -> keep)
        var mask = new byte[] { 127, 128, 0, 255 };

        VideoRecorder.ApplyMask(buffer, cropWidth: 2, cropHeight: 2, mask, maskWidth: 2, maskHeight: 2, maskStride: 2);

        unsafe
        {
            fixed (byte* p = buffer)
            {
                var uints = (uint*)p;
                Assert.Equal(0xFF000000, uints[0]);
                Assert.Equal(0xFFFFFFFF, uints[1]);
                Assert.Equal(0xFF000000, uints[2]);
                Assert.Equal(0xFFFFFFFF, uints[3]);
            }
        }
    }

    [Fact]
    public void ApplyMask_null_or_undersized_buffer_is_noop()
    {
        var buffer = new byte[8]; // too small for 2x2
        var mask = new byte[] { 255, 255, 255, 255 };

        // Should return early without throwing
        VideoRecorder.ApplyMask(buffer, 2, 2, mask, 2, 2, 2);
        VideoRecorder.ApplyMask(buffer, 2, 2, null, 2, 2, 2);
    }

    [Fact]
    public void ReadPcm16_with_undersized_dest_does_not_overflow()
    {
        using var mixer = new AudioCaptureMixer(new AudioSources());
        var dest = new byte[10]; // only room for 5 samples (10 bytes)

        // Asking for 100 frames = 200 samples (400 bytes).
        // Mixer should only write 10 bytes and return 10 without heap corruption.
        int written = mixer.ReadPcm16(dest, 100);

        Assert.Equal(10, written);
    }

    [Fact]
    public void ReadPcm16_with_zero_or_odd_length_dest_does_not_overflow()
    {
        using var mixer = new AudioCaptureMixer(new AudioSources());
        var empty = Array.Empty<byte>();
        Assert.Equal(0, mixer.ReadPcm16(empty, 100));

        var odd = new byte[1]; // length 1 -> 0 samples
        Assert.Equal(0, mixer.ReadPcm16(odd, 100));
    }

    [Fact]
    public void ReadPcm16_null_dest_throws_argument_null_exception()
    {
        using var mixer = new AudioCaptureMixer(new AudioSources());
        Assert.Throws<ArgumentNullException>(() => mixer.ReadPcm16(null!, 100));
    }

    [Fact]
    public void ReadPcm16_zero_frames_returns_zero()
    {
        using var mixer = new AudioCaptureMixer(new AudioSources());
        var dest = new byte[100];
        Assert.Equal(0, mixer.ReadPcm16(dest, 0));
        Assert.Equal(0, mixer.ReadPcm16(dest, -5));
    }

    [Fact]
    public void H264Probe_includes_mf_live_streaming_scenario()
    {
        var (preset, tune, extra) = FfmpegArgs.GetDefaultTuning("h264_mf");
        var args = FfmpegArgs.H264Probe(2560, 1440, "h264_mf", preset, tune, extra);

        Assert.Contains("-c:v h264_mf -scenario live_streaming", args);
    }

    [Fact]
    public void H264Probe_includes_amf_quality_speed()
    {
        var (preset, tune, extra) = FfmpegArgs.GetDefaultTuning("h264_amf");
        var args = FfmpegArgs.H264Probe(2560, 1440, "h264_amf", preset, tune, extra);

        Assert.Contains("-c:v h264_amf -quality speed", args);
    }

    [Fact]
    public void H264Probe_includes_nvenc_tuning()
    {
        var (preset, tune, extra) = FfmpegArgs.GetDefaultTuning("h264_nvenc");
        var args = FfmpegArgs.H264Probe(1920, 1080, "h264_nvenc", preset, tune, extra);

        Assert.Contains("-c:v h264_nvenc -preset p4 -tune ll", args);
    }
}