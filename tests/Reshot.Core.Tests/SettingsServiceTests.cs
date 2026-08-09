using Reshot.Core;
using Reshot.Core.Settings;
using Xunit;

namespace Reshot.Core.Tests;

public class SettingsServiceTests : IDisposable
{
    private readonly string _tempFile =
        Path.Combine(Path.GetTempPath(), $"reshot_test_{Guid.NewGuid():N}.json");

    [Fact]
    public void Load_missing_file_creates_defaults()
    {
        var service = new SettingsService(_tempFile);
        var settings = service.Load();

        Assert.True(File.Exists(_tempFile));
        Assert.Equal("PrtScn", settings.Hotkey);
        Assert.Equal(0.5, settings.Dim.Opacity);
        Assert.Equal("#000000", settings.Dim.Color);
        Assert.True(settings.Autostart);
        Assert.Equal(60, settings.Video.Fps);
        Assert.True(settings.Video.Audio.Mic);
        Assert.True(settings.Radial.ClickToChoose); // clicking is the default, not the gesture
    }

    [Fact]
    public void Load_resolves_empty_paths_to_defaults()
    {
        var service = new SettingsService(_tempFile);
        var settings = service.Load();

        Assert.Equal(ReshotPaths.DefaultScreenshotsDir, settings.Paths.Screenshots);
        Assert.Equal(ReshotPaths.DefaultVideosDir, settings.Paths.Videos);
    }

    [Fact]
    public void Save_then_load_roundtrips_custom_values()
    {
        var service = new SettingsService(_tempFile);
        service.Load();
        service.Current.Hotkey = "Ctrl+Shift+4";
        service.Current.Dim.Opacity = 0.75;
        service.Current.Autostart = false;
        service.Current.Radial.ClickToChoose = false; // must differ from the default to prove anything
        service.Save();

        var reloaded = new SettingsService(_tempFile);
        var settings = reloaded.Load();

        Assert.Equal("Ctrl+Shift+4", settings.Hotkey);
        Assert.Equal(0.75, settings.Dim.Opacity);
        Assert.False(settings.Autostart);
        Assert.False(settings.Radial.ClickToChoose);
    }

    [Fact]
    public void Load_corrupt_file_falls_back_to_defaults()
    {
        File.WriteAllText(_tempFile, "{ this is not valid json ");

        var service = new SettingsService(_tempFile);
        var settings = service.Load();

        Assert.Equal("PrtScn", settings.Hotkey);
    }

    [Fact]
    public void Serialized_json_uses_camelCase_keys()
    {
        var service = new SettingsService(_tempFile);
        service.Load();
        service.Save();

        var json = File.ReadAllText(_tempFile);
        Assert.Contains("\"hotkey\"", json);
        Assert.Contains("\"askOnSave\"", json);
        Assert.DoesNotContain("\"Hotkey\"", json);

        // The settings window addresses this key by name (src/reshot-tauri/src/main.ts).
        // Renaming the C# property would leave that checkbox writing a key nothing reads,
        // which fails silently in both directions.
        Assert.Contains("\"radial\"", json);
        Assert.Contains("\"clickToChoose\"", json);
    }

    public void Dispose()
    {
        if (File.Exists(_tempFile))
            File.Delete(_tempFile);
    }
}
