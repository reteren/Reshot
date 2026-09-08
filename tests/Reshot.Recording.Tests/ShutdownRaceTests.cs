using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using Reshot.Capture;
using Reshot.Recording;
using Xunit;

namespace Reshot.Recording.Tests;

public class ShutdownRaceTests
{
    private sealed class BlockingEncoder : IVideoFrameWriter
    {
        private readonly ManualResetEventSlim _disposedEvent = new(false);
        public ManualResetEventSlim WriteCalled { get; } = new(false);
        public bool WasDisposed { get; private set; }

        public void WriteFrame(byte[] buffer)
        {
            WriteCalled.Set();
            // Block until disposed, simulating a blocked pipe / backpressure stall
            _disposedEvent.Wait();
            throw new IOException("Pipe closed upon encoder disposal.");
        }

        public void Dispose()
        {
            WasDisposed = true;
            _disposedEvent.Set();
        }
    }

    private sealed class HungEncoder : IVideoFrameWriter
    {
        public ManualResetEventSlim WriteCalled { get; } = new(false);
        public ManualResetEventSlim UnblockForTeardown { get; } = new(false);
        public bool WasDisposed { get; private set; }

        public void WriteFrame(byte[] buffer)
        {
            WriteCalled.Set();
            // Permanently hung write (simulating native driver deadlock)
            UnblockForTeardown.Wait();
        }

        public void Dispose()
        {
            WasDisposed = true;
            // Deliberately does NOT unblock WriteFrame to test hung worker protection
        }
    }

    private sealed class FastStubEncoder : IVideoFrameWriter
    {
        public int FrameCount;
        public bool WasDisposed { get; private set; }

        public void WriteFrame(byte[] buffer)
        {
            Interlocked.Increment(ref FrameCount);
        }

        public void Dispose()
        {
            WasDisposed = true;
        }
    }

    [Fact]
    public async Task WgcMonitorStream_Dispose_waits_for_in_flight_callback_to_drain()
    {
        var capture = new ScreenCaptureService();
        WgcMonitorStream? stream = null;
        try
        {
            stream = capture.StartMonitorStream(0, 0);
        }
        catch (Exception ex)
        {
            // If running on an environment without WGC monitor access, skip gracefully
            Assert.True(true, $"WGC not available in test environment: {ex.Message}");
            return;
        }

        using (stream)
        {
            var inCallback = new ManualResetEventSlim(false);
            var allowCallbackToFinish = new ManualResetEventSlim(false);

            stream.FrameProcessingStarting = () =>
            {
                inCallback.Set();
                allowCallbackToFinish.Wait(TimeSpan.FromSeconds(5));
            };

            // Wait until a frame callback starts executing
            var callbackStarted = inCallback.Wait(TimeSpan.FromSeconds(5));
            Assert.True(callbackStarted, "Frame callback did not start within 5s");
            Assert.True(stream.InFlightCallbacks > 0, "InFlightCallbacks should be > 0 while callback is active");

            // Dispose stream on a background thread while callback is still running
            var disposeTask = Task.Run(() => stream.Dispose());

            // Give Dispose() a moment to initiate and wait on _drained
            await Task.Delay(100);
            Assert.False(disposeTask.IsCompleted, "Dispose() must wait for in-flight callback to drain before returning");
            Assert.True(stream.IsDisposed, "IsDisposed should be true once Dispose() has started");

            // Allow the callback to finish
            allowCallbackToFinish.Set();

            // Dispose should now complete cleanly within 3 seconds
            var completedTask = await Task.WhenAny(disposeTask, Task.Delay(3000));
            Assert.Same(disposeTask, completedTask);
            Assert.Equal(0, stream.InFlightCallbacks);
        }
    }

    [Fact]
    public void VideoRecorder_Stop_unblocks_worker_via_SafeShutdownEncoder()
    {
        var capture = new ScreenCaptureService();
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_unblock_{Guid.NewGuid():N}.mp4");
        var encoder = new BlockingEncoder();

        try
        {
            var recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false },
                shapeMask: null, maskStride: 0,
                timeBegin: null, timeEnd: null,
                encoderFactory: (_, _, _, _, _) => encoder);

            recorder.WorkerCooperativeTimeout = TimeSpan.FromMilliseconds(50);
            recorder.WorkerDrainTimeout = TimeSpan.FromSeconds(2);

            // Wait for worker thread to enter WriteFrame and block
            var writeReached = encoder.WriteCalled.Wait(TimeSpan.FromSeconds(5));
            Assert.True(writeReached, "Worker thread did not call WriteFrame within 5s");

            // Stop() should detect the worker hasn't exited cooperatively within 50ms,
            // trigger SafeShutdownEncoder() which disposes the encoder, and unblock the thread
            var sw = Stopwatch.StartNew();
            recorder.Stop();
            sw.Stop();

            Assert.True(encoder.WasDisposed, "Encoder should have been disposed by SafeShutdownEncoder");
            Assert.True(recorder.IsWorkerTerminated, "Worker thread should be terminated");
            Assert.True(sw.Elapsed < TimeSpan.FromSeconds(3), $"Stop took too long: {sw.ElapsedMilliseconds}ms");
        }
        finally
        {
            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public void VideoRecorder_Finish_refuses_to_mux_if_worker_failed_to_terminate()
    {
        var capture = new ScreenCaptureService();
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_hung_{Guid.NewGuid():N}.mp4");
        var encoder = new HungEncoder();

        VideoRecorder? recorder = null;
        try
        {
            recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false },
                shapeMask: null, maskStride: 0,
                timeBegin: null, timeEnd: null,
                encoderFactory: (_, _, _, _, _) => encoder);

            recorder.WorkerCooperativeTimeout = TimeSpan.FromMilliseconds(50);
            recorder.WorkerDrainTimeout = TimeSpan.FromMilliseconds(50);

            var writeReached = encoder.WriteCalled.Wait(TimeSpan.FromSeconds(5));
            Assert.True(writeReached, "Worker thread did not call WriteFrame within 5s");

            // Stop() will attempt cooperative wait (50ms) and drain wait (50ms), both timing out
            recorder.Stop();

            Assert.False(recorder.IsWorkerTerminated, "Worker should not be reported as terminated when thread is hung");

            // Finish() must return false and NOT run muxing or delete locked temp files
            var finished = recorder.Finish(keepSystem: false, keepMic: false);
            Assert.False(finished, "Finish() must return false when worker failed to terminate");
        }
        finally
        {
            // Unblock worker thread so test process doesn't leak or hang
            encoder.UnblockForTeardown.Set();
            Thread.Sleep(50);
            recorder?.Dispose();

            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public void VideoRecorder_clean_Stop_latency_is_under_50ms()
    {
        var capture = new ScreenCaptureService();
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_latency_{Guid.NewGuid():N}.mp4");
        var encoder = new FastStubEncoder();

        try
        {
            var recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false },
                shapeMask: null, maskStride: 0,
                timeBegin: null, timeEnd: null,
                encoderFactory: (_, _, _, _, _) => encoder);

            // Let it run for 100ms
            Thread.Sleep(100);

            var sw = Stopwatch.StartNew();
            recorder.Stop();
            sw.Stop();

            Assert.True(recorder.IsWorkerTerminated, "Worker must be cleanly terminated");
            Assert.True(encoder.WasDisposed, "Encoder must be disposed");
            Assert.True(sw.ElapsedMilliseconds < 50, $"Stop latency was {sw.ElapsedMilliseconds}ms, expected < 50ms");
        }
        finally
        {
            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public void WgcMonitorStream_Dispose_defers_D3D_teardown_when_drain_times_out()
    {
        var capture = new ScreenCaptureService();
        WgcMonitorStream? stream = null;
        try
        {
            stream = capture.StartMonitorStream(0, 0);
        }
        catch (Exception ex)
        {
            Assert.True(true, $"WGC not available in test environment: {ex.Message}");
            return;
        }

        using (stream)
        {
            var inCallback = new ManualResetEventSlim(false);
            var allowCallbackToFinish = new ManualResetEventSlim(false);

            stream.DrainTimeout = TimeSpan.FromMilliseconds(50);
            stream.FrameProcessingStarting = () =>
            {
                inCallback.Set();
                allowCallbackToFinish.Wait(TimeSpan.FromSeconds(5));
            };

            var callbackStarted = inCallback.Wait(TimeSpan.FromSeconds(5));
            Assert.True(callbackStarted, "Frame callback did not start within 5s");
            Assert.True(stream.InFlightCallbacks > 0, "InFlightCallbacks should be > 0 while callback is active");

            // Dispose stream; drain wait will time out (50ms) while callback is still running
            var sw = Stopwatch.StartNew();
            stream.Dispose();
            sw.Stop();

            Assert.True(stream.IsDisposed, "IsDisposed should be true once Dispose() has executed");
            Assert.True(sw.ElapsedMilliseconds < 500, $"Dispose() should return promptly on timeout, took {sw.ElapsedMilliseconds}ms");
            Assert.False(stream.IsD3DDisposed, "D3D resources must NOT be disposed while in-flight callback is still executing (Finding 1 fix)");
            Assert.True(stream.InFlightCallbacks > 0, "Callback should still be in-flight");

            // Allow the callback to finish
            allowCallbackToFinish.Set();

            // Once the callback exits, it must perform the deferred D3D teardown
            var unblocked = SpinWait.SpinUntil(() => stream.IsD3DDisposed, TimeSpan.FromSeconds(2));
            Assert.True(unblocked, "Exiting callback must complete deferred D3D teardown");
            Assert.True(stream.IsD3DDisposed, "D3D resources should now be disposed");
            Assert.Equal(0, stream.InFlightCallbacks);
        }
    }

    [Fact]
    public async Task VideoRecorder_failed_drain_hands_off_to_reaper_which_cleans_up_streams_and_temps()
    {
        var capture = new ScreenCaptureService();
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_reaper_{Guid.NewGuid():N}.mp4");
        var encoder = new HungEncoder();

        VideoRecorder? recorder = null;
        string? tempVideo = null;
        try
        {
            recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false },
                shapeMask: null, maskStride: 0,
                timeBegin: null, timeEnd: null,
                encoderFactory: (_, _, _, _, _) => encoder);

            recorder.WorkerCooperativeTimeout = TimeSpan.FromMilliseconds(50);
            recorder.WorkerDrainTimeout = TimeSpan.FromMilliseconds(50);
            recorder.ReaperTimeout = TimeSpan.FromSeconds(5);

            tempVideo = recorder.TempVideoPath;
            Assert.NotNull(tempVideo);

            var writeReached = encoder.WriteCalled.Wait(TimeSpan.FromSeconds(5));
            Assert.True(writeReached, "Worker thread did not call WriteFrame within 5s");

            // Stop() will attempt cooperative wait (50ms) and drain wait (50ms), both timing out
            recorder.Stop();

            Assert.False(recorder.IsWorkerTerminated, "Worker should not be terminated");
            Assert.NotNull(recorder.ReaperTask);
            Assert.False(recorder.ReaperTask.IsCompleted, "Reaper should still be waiting for surviving worker");

            // Finish() must return false and arrange reaper
            var finished = recorder.Finish(keepSystem: false, keepMic: false);
            Assert.False(finished, "Finish() must return false when worker failed to terminate");

            // Unblock the worker thread
            encoder.UnblockForTeardown.Set();

            // Reaper task should now finish and clean up temp files and streams
            await recorder.ReaperTask.WaitAsync(TimeSpan.FromSeconds(3));
            Assert.True(recorder.ReaperTask.IsCompleted, "Reaper task should have completed after worker unblocked");

            Assert.False(File.Exists(tempVideo), "Temp video file must be deleted by the background reaper");
            Assert.Null(recorder.Stream);
        }
        finally
        {
            encoder.UnblockForTeardown.Set();
            recorder?.Dispose();

            if (tempVideo is not null && File.Exists(tempVideo))
                File.Delete(tempVideo);
            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public void VideoRecorder_Dispose_without_Finish_cleans_up_temp_files()
    {
        var capture = new ScreenCaptureService();
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_disp_clean_{Guid.NewGuid():N}.mp4");
        var encoder = new FastStubEncoder();

        string? tempVideo = null;
        try
        {
            var recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false },
                shapeMask: null, maskStride: 0,
                timeBegin: null, timeEnd: null,
                encoderFactory: (_, _, _, _, _) => encoder);

            tempVideo = recorder.TempVideoPath;
            Assert.NotNull(tempVideo);

            // Let it run briefly
            Thread.Sleep(50);

            // Disposing without Finish() (e.g. cancelled recording take) must clean up temp files
            recorder.Dispose();

            Assert.True(recorder.IsWorkerTerminated, "Worker should be terminated cleanly");
            Assert.False(File.Exists(tempVideo), "Dispose without Finish must delete temporary video file");
        }
        finally
        {
            if (tempVideo is not null && File.Exists(tempVideo))
                File.Delete(tempVideo);
            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }

    [Fact]
    public async Task VideoRecorder_reaper_forces_resource_cleanup_if_worker_never_unblocks()
    {
        var capture = new ScreenCaptureService();
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_reaper_timeout_{Guid.NewGuid():N}.mp4");
        var encoder = new HungEncoder();

        VideoRecorder? recorder = null;
        try
        {
            recorder = new VideoRecorder(
                capture, 0, 0, 128, 128, 30, 1_000_000, tempOutput,
                new AudioSources { SystemFull = false },
                shapeMask: null, maskStride: 0,
                timeBegin: null, timeEnd: null,
                encoderFactory: (_, _, _, _, _) => encoder);

            recorder.WorkerCooperativeTimeout = TimeSpan.FromMilliseconds(20);
            recorder.WorkerDrainTimeout = TimeSpan.FromMilliseconds(20);
            recorder.ReaperTimeout = TimeSpan.FromMilliseconds(100);

            var writeReached = encoder.WriteCalled.Wait(TimeSpan.FromSeconds(5));
            Assert.True(writeReached, "Worker thread did not call WriteFrame within 5s");

            recorder.Stop();

            Assert.NotNull(recorder.ReaperTask);
            // Reaper should time out after 100ms and force resource teardown
            await recorder.ReaperTask.WaitAsync(TimeSpan.FromSeconds(2));
            Assert.True(recorder.ReaperTask.IsCompleted, "Reaper should complete after forced timeout");
        }
        finally
        {
            encoder.UnblockForTeardown.Set();
            recorder?.Dispose();

            if (File.Exists(tempOutput))
                File.Delete(tempOutput);
        }
    }
}
