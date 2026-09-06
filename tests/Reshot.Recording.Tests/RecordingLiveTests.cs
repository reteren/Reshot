using Reshot.Capture;
using Reshot.Recording;
using Xunit;

namespace Reshot.Recording.Tests;

public class RecordingLiveTests
{
    [Fact]
    public void Can_record_short_clip()
    {
        Assert.True(Ffmpeg.IsAvailable, "Ffmpeg must be available");
        var tempOutput = Path.Combine(Path.GetTempPath(), $"reshot_test_{Guid.NewGuid():N}.mp4");
        var capture = new ScreenCaptureService();
        var recorder = new VideoRecorder(
            capture, 0, 0, 1280, 720, 30, 4_000_000, tempOutput,
            new AudioSources { SystemFull = false });

        Thread.Sleep(2000);
        var ok = recorder.Finish(keepSystem: false, keepMic: false);
        Assert.True(ok, "Finish should succeed");
        Assert.True(File.Exists(tempOutput), "Output file should exist");
        Assert.True(new FileInfo(tempOutput).Length > 0, "Output file should not be empty");
        File.Delete(tempOutput);
    }
}
