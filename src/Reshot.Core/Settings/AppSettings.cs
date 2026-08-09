namespace Reshot.Core.Settings;

/// <summary>
/// Strongly-typed mirror of <c>settings.json</c> (SPEC §13). Serialized with
/// camelCase, so C# <c>Dim.Opacity</c> ↔ JSON <c>dim.opacity</c>. Every property
/// carries its documented default, so a fresh object already equals the shipped
/// defaults; <see cref="SettingsService"/> only fills in machine-specific paths.
/// </summary>
public sealed class AppSettings
{
    /// <summary>
    /// Main global hotkey. Rebindable in Settings. Default is Print Screen
    /// ("PrtScn"; aliases include Prnt, Print, PrintScreen).
    /// </summary>
    public string Hotkey { get; set; } = "PrtScn";

    /// <summary>Optional global hotkey for instant audio recording (last-used settings). Empty = off.</summary>
    public string AudioHotkey { get; set; } = string.Empty;

    public DimSettings Dim { get; set; } = new();
    public PathSettings Paths { get; set; } = new();

    /// <summary>Start Reshot with Windows.</summary>
    public bool Autostart { get; set; } = true;

    /// <summary>
    /// Start it with administrator rights, which is what lets the overlay come up over an
    /// application that is itself elevated. Registers a scheduled task instead of the
    /// <c>Run</c> key, so the rights are approved once rather than at every logon.
    /// Ignored while <see cref="Autostart"/> is off.
    /// </summary>
    public bool AutostartElevated { get; set; } = false;

    public FormatSettings Format { get; set; } = new();
    public FilenameSettings Filename { get; set; } = new();
    public VideoSettings Video { get; set; } = new();
    public AudioSettings Audio { get; set; } = new();
    public UpdateSettings Update { get; set; } = new();
    public RadialSettings Radial { get; set; } = new();
}

/// <summary>Behaviour of the hold-to-open radial quick menu.</summary>
public sealed class RadialSettings
{
    /// <summary>
    /// Keep the wheel open after the hotkey is released and pick with a left click,
    /// instead of committing whatever the cursor points at on release. On by default:
    /// the gesture is faster once learned, but it has to be known before it can be used,
    /// and a menu that vanishes when you let go teaches nobody that. The settings window
    /// offers the gesture as the opt-in, so the checkbox there is this key negated.
    /// </summary>
    public bool ClickToChoose { get; set; } = true;
}

/// <summary>Standalone audio-recording tool settings (remembers the last-used sources).</summary>
public sealed class AudioSettings
{
    /// <summary>Record system (loopback) audio.</summary>
    public bool System { get; set; } = true;

    /// <summary>Record the microphone.</summary>
    public bool Mic { get; set; } = false;

    /// <summary>Microphone device id, or "default". Shared with the video recorder.</summary>
    public string MicDevice { get; set; } = "default";
}

/// <summary>Dimming of the non-selected area of the overlay.</summary>
public sealed class DimSettings
{
    /// <summary>Opacity of the dim fill (0..1). Default 0.5.</summary>
    public double Opacity { get; set; } = 0.5;

    /// <summary>Dim fill color, hex. Default black.</summary>
    public string Color { get; set; } = "#000000";
}

/// <summary>Output folders. Empty means "use the per-user default folder".</summary>
public sealed class PathSettings
{
    /// <summary>Screenshot folder. Default resolves to <c>Pictures\reshot</c>.</summary>
    public string Screenshots { get; set; } = string.Empty;

    /// <summary>Video folder. Default resolves to <c>Videos\reshot</c>.</summary>
    public string Videos { get; set; } = string.Empty;

    /// <summary>Audio-recording folder. Default resolves to <c>Music\reshot</c>.</summary>
    public string Records { get; set; } = string.Empty;
}

public sealed class FormatSettings
{
    /// <summary>Default image format: png / jpg / webp.</summary>
    public string Image { get; set; } = "png";

    /// <summary>Quality (1..100) for lossy formats (jpg / webp). Ignored for png.</summary>
    public int Quality { get; set; } = 90;
}

public sealed class FilenameSettings
{
    /// <summary>Filename template with {date} and {time} placeholders.</summary>
    public string Template { get; set; } = "Reshot_{date}_{time}";
}

public sealed class VideoSettings
{
    /// <summary>Recording frame rate: 60 / 30 / 25.</summary>
    public int Fps { get; set; } = 60;

    public VideoAudioSettings Audio { get; set; } = new();
    public VideoCornersSettings Corners { get; set; } = new();
}

public sealed class VideoAudioSettings
{
    public bool Mic { get; set; } = true;
    public bool System { get; set; } = true;

    /// <summary>Show the track-selection dialog when saving a recording.</summary>
    public bool AskOnSave { get; set; } = true;

    /// <summary>Microphone device id, or "default".</summary>
    public string MicDevice { get; set; } = "default";
}

public sealed class VideoCornersSettings
{
    public bool Enabled { get; set; } = true;
    public string Color { get; set; } = "#3C9898";
    public double Opacity { get; set; } = 0.7;
}

public sealed class UpdateSettings
{
    /// <summary>Auto-update via Velopack + GitHub Releases.</summary>
    public bool Auto { get; set; } = true;
}
