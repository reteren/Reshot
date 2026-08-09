using System.IO;
using System.Windows;
using System.Windows.Forms;
using Reshot.App.Input;
using Reshot.App.Interop;
using Reshot.App.Overlay;
using Reshot.App.Platform;
using Reshot.App.Tray;
using Reshot.Capture;
using Reshot.Core.Diagnostics;
using Reshot.Core.Session;
using Reshot.Core.Settings;

namespace Reshot.App;

/// <summary>
/// Application composition root. Wires together the single-instance guard, tray,
/// global hotkey, settings and autostart. Holds no editing/capture logic, that
/// lands in later phases. Everything here is event-driven so the process sits at
/// ~0% CPU while idle (ARCHITECTURE §7).
/// </summary>
public partial class App : System.Windows.Application
{
    private SingleInstance? _singleInstance;
    private SettingsService? _settingsService;
    private TrayIconController? _tray;
    private HotkeyService? _hotkey;
    private IScreenCaptureService? _capture;
    private OverlayWindow? _overlay;
    private Tray.TrayMenuWindow? _trayMenu;
    private System.Diagnostics.Process? _settingsProcess;
    private Reshot.Recording.VideoRecorder? _recorder;
    private Reshot.Recording.AudioRecorder? _audioRecorder;
    private Recording.RecordingHudWindow? _hud;
    private Recording.RecordingHudWindow? _audioHud;
    private HotkeyService? _audioHotkey;
    private string? _recordingPath;
    private string? _audioRecordingPath;
    private bool _recording;
    private bool _audioRecording;
    private Radial.RadialMenuWindow? _radial;
    private System.Windows.Threading.DispatcherTimer? _holdTimer;
    private DateTime _hotkeyDownAt;
    private bool _holdDetecting;
    private const int HoldThresholdMs = 250;

    /// <summary>
    /// Bumped by every capture attempt. A frame that comes back under an old number was
    /// superseded by a newer press and is dropped: a slow capture must never lock the
    /// hotkey out, and must never open a session showing a screen the user has left.
    /// </summary>
    private int _captureGeneration;
    private readonly SessionStateMachine _session = new();

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // 1. Logging first: a launch that dies on the single-instance check below used to
        // leave no trace at all, so a stale instance holding the lock looked exactly like
        // "the app is broken", with an empty log to debug it with.
        Log.Init();

        // 2. Single instance, a second launch signals the first and exits.
        _singleInstance = new SingleInstance();
        if (!_singleInstance.TryAcquire())
        {
            Log.Info("Startup: another instance already holds the lock; signalling it and exiting.");
            _singleInstance.Dispose();
            _singleInstance = null;
            Shutdown();
            return;
        }
        _singleInstance.SecondInstanceLaunched += OnSecondInstanceLaunched;
        Log.Info("Startup: acquired single-instance lock.");

        // A tray app must survive a bad UI event handler: an unhandled dispatcher
        // exception would otherwise kill the process with nothing in the log.
        DispatcherUnhandledException += (_, args) =>
        {
            Log.Error("Unhandled UI exception", args.Exception);
            _tray?.ShowBalloon("Reshot: error", args.Exception.Message, ToolTipIcon.Error);
            args.Handled = true;
        };

        _settingsService = new SettingsService();
        var settings = _settingsService.Load();

        // 3. Keep autostart in sync with settings. No prompting here: a UAC dialog on every
        // logon is exactly what the scheduled-task lane exists to avoid.
        AutostartManager.Apply(settings.Autostart, settings.AutostartElevated, allowPrompt: false);

        // 4. Tray.
        _tray = new TrayIconController();
        _tray.CaptureRequested += (_, _) => OnCaptureRequested("tray menu");
        _tray.MenuRequested += (_, _) => ShowTrayMenu();

        // 5. Capture service: Desktop Duplication for snapshots where it works, WGC
        // otherwise and for recording. No GPU resources held until a capture is taken.
        _capture = new ScreenCaptureService();
        // Ask early for borderless capture access so recordings don't get the yellow border.
        ScreenCaptureService.RequestBorderlessAccess();

        // 6. Global hotkey.
        _hotkey = new HotkeyService();
        _hotkey.HotkeyPressed += (_, _) => OnHotkeyPressed();
        if (!_hotkey.Register(settings.Hotkey))
        {
            _tray.ShowBalloon(
                "Reshot: hotkey unavailable",
                $"Could not register '{settings.Hotkey}'. It may be in use by another app. " +
                "Edit settings.json to pick another.",
                ToolTipIcon.Warning);
        }

        // 6b. Optional quick audio-record hotkey.
        RegisterAudioHotkey(settings.AudioHotkey);

        // Optional: start a capture immediately (e.g. bound to an external launcher).
        if (e.Args.Contains("--capture", StringComparer.OrdinalIgnoreCase))
        {
            Dispatcher.BeginInvoke(
                new Action(() => OnCaptureRequested("cli --capture")),
                System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        }

        Log.Info("Startup: ready (idle in tray).");
    }

    /// <summary>
    /// Freezes the screen and shows the selection overlay. Re-entrant-safe: a second
    /// trigger while the overlay is open is ignored (the overlay owns the session).
    /// </summary>
    /// <summary>
    /// Main hotkey pressed. A quick tap is an instant screenshot (old behaviour); holding
    /// it past <see cref="HoldThresholdMs"/> opens the radial menu. While recording, the
    /// key stays the Stop control. Distinguishing tap from hold needs key-up, which
    /// RegisterHotKey doesn't give, so we poll GetAsyncKeyState briefly.
    /// </summary>
    private void OnHotkeyPressed()
    {
        // Logged before every guard below, on purpose. Each of those guards is a silent
        // return, so "nothing in the log" used to mean two very different things: the
        // hotkey never arrived, or it arrived and was swallowed. This line separates them.
        Log.Info($"Hotkey pressed over {NativeMethods.DescribeForegroundWindow()} " +
                 $"[overlay={_overlay is not null}, radial={_radial is not null}, " +
                 $"holdDetecting={_holdDetecting}, recording={_recording}, audio={_audioRecording}].");

        ClearStuckHotkeyState();

        // Recording / audio: the hotkey stops it immediately, no menu.
        if (_recording || _audioRecording)
        {
            OnCaptureRequested("hotkey");
            return;
        }
        // The overlay is already up: don't start a second session, but do pull it back in
        // front. A game that took the foreground back is exactly when the user presses
        // again, and answering that with nothing is what makes the app feel dead.
        if (_overlay is not null)
        {
            _overlay.Activate();
            NativeMethods.ForceForegroundWindow(
                new System.Windows.Interop.WindowInteropHelper(_overlay).Handle);
            return;
        }

        // Radial menu open, or WM_HOTKEY auto-repeat while the key is still held → ignore.
        if (_radial is not null || _holdDetecting)
            return;

        uint vk = _hotkey?.Current?.VirtualKey ?? 0;
        if (vk == 0)
        {
            OnCaptureRequested("hotkey");
            return;
        }

        _holdDetecting = true;
        _hotkeyDownAt = DateTime.UtcNow;

        // Nothing is captured until we know this is a tap. Starting the freeze here, in
        // parallel with hold detection, saved the user's key-hold time — and cost far more
        // than it saved: opening a Windows.Graphics.Capture session takes the cursor off
        // its hardware plane, which is one visible blink on every press, and a hold (which
        // never needs a frame at all) paid it too, along with the GPU cost of capturing a
        // running game just to throw the result away.
        _holdTimer?.Stop();
        _holdTimer = new System.Windows.Threading.DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(15),
        };
        _holdTimer.Tick += (_, _) =>
        {
            bool down = (NativeMethods.GetAsyncKeyState((int)vk) & 0x8000) != 0;
            double elapsed = (DateTime.UtcNow - _hotkeyDownAt).TotalMilliseconds;
            if (!down)
            {
                _holdTimer!.Stop();
                _holdDetecting = false;
                OnCaptureRequested("hotkey tap"); // released quickly → screenshot
            }
            else if (elapsed >= HoldThresholdMs)
            {
                _holdTimer!.Stop();
                _holdDetecting = false;
                OpenRadialMenu(vk); // held → radial menu, no frame involved
            }
        };
        _holdTimer.Start();
    }

    /// <summary>
    /// Freezes the screen on a background thread. The WGC frame pool is created
    /// free-threaded, so nothing here needs the UI thread — and it must not have it: device
    /// creation plus the wait for the first frame is the bulk of the hotkey-to-overlay time,
    /// and blocking the dispatcher with it froze the app instead of just delaying it.
    /// </summary>
    private Task<CapturedFrame> CaptureAsync()
    {
        var capture = _capture!;
        return Task.Run(capture.SnapshotAllMonitors);
    }

    /// <summary>
    /// Releases hotkey state that outlived whatever set it. Every guard in
    /// <see cref="OnHotkeyPressed"/> is a silent return, so a flag left set — a hold timer
    /// that stopped without clearing, a menu window that went away without raising Closed —
    /// does not merely lose one press: it kills the hotkey for the rest of the session,
    /// invisibly. Cheap to check here, where it costs one press instead of a restart.
    /// </summary>
    private void ClearStuckHotkeyState()
    {
        if (_holdDetecting && _holdTimer is not { IsEnabled: true })
        {
            Log.Warn("Hotkey: hold detection was left running with a dead timer; clearing.");
            _holdDetecting = false;
        }

        if (_radial is { IsVisible: false })
        {
            Log.Warn("Hotkey: the radial menu is no longer visible but was never cleared; clearing.");
            _radial = null;
        }

        if (_overlay is { IsVisible: false })
        {
            Log.Warn("Hotkey: the overlay is no longer visible but was never cleared; clearing.");
            _overlay = null;
        }
    }

    /// <summary>
    /// Opens the hold-to-open radial menu at the cursor. Under <c>radial.clickToChoose</c>
    /// (the default) the held key is withheld from the menu, which is what makes the wheel
    /// outlive the keypress and wait for a click. With it off, <paramref name="vk"/> is
    /// handed over instead: the menu watches that key and commits the hovered slice when it
    /// comes back up.
    /// </summary>
    private void OpenRadialMenu(uint vk)
    {
        if (_radial is not null || _overlay is not null || _recording || _audioRecording)
            return;

        var clickToChoose = _settingsService?.Current.Radial.ClickToChoose ?? true;
        _radial = new Radial.RadialMenuWindow(clickToChoose ? 0 : vk);
        _radial.Chosen += choice =>
        {
            switch (choice)
            {
                case Radial.RadialChoice.Record: QuickRecordPrimaryMonitor(); break;
                case Radial.RadialChoice.Audio: QuickAudioAllSources(); break;
                case Radial.RadialChoice.Settings: OnSettingsRequested(this, EventArgs.Empty); break;
            }
        };
        _radial.Closed += (_, _) => _radial = null;
        ShowOverGame(_radial, "Radial menu");
    }

    /// <summary>
    /// Shows one of our topmost windows and makes sure it is actually in front.
    ///
    /// Whether a game is involved is decided <b>before</b> the window appears, because
    /// afterwards the foreground is ours and the answer is always no. Against an ordinary
    /// desktop that answer means: show the window and do nothing else. Every unlock, every
    /// input-queue attach, every Alt trick exists for the game case alone, and running
    /// them on the desktop bought nothing but a blinking cursor.
    /// </summary>
    private static void ShowOverGame(System.Windows.Window window, string what)
    {
        var contested = NativeMethods.ForegroundIsFullscreenForeignWindow();
        var over = contested ? NativeMethods.DescribeForegroundWindow() : "desktop";

        // Unlock *after* the frame is frozen, so WGC still saw the game.
        if (contested)
            NativeMethods.UnlockInputFromGame();

        window.Show();

        // Self-guarding: if showing the window already made it the foreground — the normal
        // case — this returns immediately without touching any input state. The invasive
        // fallback is allowed exactly here, once per session, and only if the polite path
        // came up short.
        var hwnd = new System.Windows.Interop.WindowInteropHelper(window).EnsureHandle();
        var won = NativeMethods.ForceForegroundWindow(hwnd, invasive: true);

        Log.Info($"{what} shown over {over}; foreground = " +
                 $"{(won ? "ours" : NativeMethods.DescribeForegroundWindow())}.");

        if (contested)
            KeepWindowInFront(window);
    }

    /// <summary>
    /// Holds a window in front of a game for as long as it is open. Runs <b>only</b> when
    /// a fullscreen foreign window was in front when we opened: against the desktop there
    /// is nothing to hold off, and this loop is not free.
    ///
    /// Two phases, because the two halves cost very different things. Stealing the
    /// foreground attaches our input queue to the game's, which merges cursor state and is
    /// what the user sees as blinking, so it is tried a bounded number of times and then
    /// abandoned. Re-asserting topmost touches no input state at all, so it keeps going at
    /// a slow tick for the whole session — a game that re-asserts its own topmost every
    /// frame would otherwise bury the overlay a second after it appeared.
    /// </summary>
    private static void KeepWindowInFront(System.Windows.Window window)
    {
        const int MaxStealAttempts = 8; // ~0.6s
        var attempts = 0;
        var stealing = true;

        var timer = new System.Windows.Threading.DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(80),
        };
        timer.Tick += (_, _) =>
        {
            if (!window.IsLoaded || !window.IsVisible)
            {
                timer.Stop();
                return;
            }

            var hwnd = new System.Windows.Interop.WindowInteropHelper(window).Handle;
            if (!stealing)
            {
                NativeMethods.AssertTopmost(hwnd);
                return;
            }

            // Free of side effects, unlike the rest of the unlock: a game re-confines the
            // pointer every frame until it accepts that it lost the foreground, and a
            // pinned pointer makes the overlay look dead to the mouse.
            NativeMethods.ReleaseCursorClip();

            // Polite tier only. The invasive one already had its single shot before the
            // loop started, and repeating it against a game is what dropped its frame rate.
            if (NativeMethods.ForceForegroundWindow(hwnd) || ++attempts >= MaxStealAttempts)
            {
                if (attempts >= MaxStealAttempts)
                    Log.Warn($"Foreground: {NativeMethods.DescribeForegroundWindow()} kept it " +
                             $"after {MaxStealAttempts} attempts; holding topmost only.");
                stealing = false;
                timer.Interval = TimeSpan.FromMilliseconds(250);
            }
        };
        timer.Start();
        window.Closed += (_, _) => timer.Stop();
    }

    /// <summary>Radial "quick record": records the whole primary monitor with saved video settings.</summary>
    private void QuickRecordPrimaryMonitor()
    {
        var b = System.Windows.Forms.Screen.PrimaryScreen!.Bounds;
        StartRecording(new System.Windows.Int32Rect(b.X, b.Y, b.Width, b.Height), null);
    }

    /// <summary>Radial "quick audio": records system + mic regardless of the saved toggles.</summary>
    private void QuickAudioAllSources()
    {
        StartAudioRecording(new Reshot.Recording.AudioSources
        {
            SystemFull = true,
            Mic = true,
            MicDevice = _settingsService?.Current.Audio.MicDevice ?? "default",
        });
    }

    private async void OnCaptureRequested(string source)
    {
        // While recording, the main hotkey is the Stop control (SPEC §9).
        if (_recording)
        {
            Log.Info($"Stop recording via {source}.");
            StopRecording();
            return;
        }
        if (_audioRecording)
        {
            Log.Info($"Stop audio recording via {source}.");
            StopAudioRecording();
            return;
        }

        if (_overlay is not null)
        {
            _overlay.Activate();
            return;
        }

        if (_capture is null || _settingsService is null)
            return;

        Log.Info($"Capture requested via {source}; foreground = {NativeMethods.DescribeForegroundWindow()}.");
        _session.TryTransition(SessionState.Capturing);
        var generation = ++_captureGeneration;
        try
        {
            var frame = await CaptureAsync();

            // Pressed again while this frame was in flight: the newer press owns the
            // session, and this frame shows a screen the user has already moved past.
            if (generation != _captureGeneration || _overlay is not null)
            {
                Log.Info("Capture superseded by a newer press; frame dropped.");
                return;
            }

            _overlay = new OverlayWindow(frame, _settingsService.Current);
            _overlay.SelectionActiveChanged += active =>
                _session.TryTransition(active ? SessionState.Editing : SessionState.Selecting);
            _overlay.SessionEnded += (_, produced) =>
            {
                if (produced)
                    _session.TryTransition(SessionState.Exporting);
            };
            _overlay.RecordRequested += StartRecording;
            _overlay.AudioRecordRequested += StartAudioRecording;
            _overlay.Closed += (_, _) =>
            {
                _overlay = null;
                // Don't reset the session if the overlay closed to hand off to recording.
                if (!_recording && !_audioRecording)
                    _session.Reset();
                Log.Info("Overlay closed.");
            };

            ShowOverGame(_overlay, "Overlay");
            _session.TryTransition(SessionState.Selecting);
        }
        catch (Exception ex)
        {
            _overlay = null;
            _session.Reset();
            Log.Error("Capture failed", ex);
            _tray?.ShowBalloon("Reshot: capture failed", ex.Message, ToolTipIcon.Error);
        }
    }

    /// <summary>Starts recording the given screen rect (from the overlay's Record button).</summary>
    private void StartRecording(System.Windows.Int32Rect rect, byte[]? shapeMask) =>
        StartRecording(rect, shapeMask, null);

    /// <summary>
    /// Starts recording <paramref name="rect"/>. <paramref name="sources"/> overrides the
    /// saved audio selection (the overlay's right-click source picker); null = settings.
    /// </summary>
    private void StartRecording(
        System.Windows.Int32Rect rect, byte[]? shapeMask, Reshot.Recording.AudioSources? sources)
    {
        if (_recording || _capture is null || _settingsService is null)
            return;

        // Video recording shells out to ffmpeg.exe beside reshot.exe. Starting without it
        // would raise the REC HUD and write nothing — the user only notices after stopping.
        if (!Reshot.Recording.Ffmpeg.IsAvailable)
        {
            Log.Error(Reshot.Recording.Ffmpeg.MissingMessage);
            _tray?.ShowBalloon(
                "Reshot: recording unavailable",
                "ffmpeg.exe is missing from the installation. Reinstall Reshot, or place ffmpeg.exe next to reshot.exe.",
                ToolTipIcon.Error);
            return;
        }

        try
        {
            var settings = _settingsService.Current;
            var dir = string.IsNullOrWhiteSpace(settings.Paths.Videos)
                ? Reshot.Core.ReshotPaths.DefaultVideosDir
                : settings.Paths.Videos;
            System.IO.Directory.CreateDirectory(dir);
            var fileName = Reshot.Core.Export.FilenameBuilder.Build(settings.Filename.Template, "mp4", DateTime.Now);
            _recordingPath = System.IO.Path.Combine(dir, fileName);

            var fps = settings.Video.Fps <= 0 ? 60 : settings.Video.Fps;
            var bitrate = (int)Math.Clamp((long)rect.Width * rect.Height * fps * 8 / 100, 2_000_000, 40_000_000);

            var audio = sources ?? new Reshot.Recording.AudioSources
            {
                SystemFull = settings.Video.Audio.System,
                Mic = settings.Video.Audio.Mic,
                MicDevice = settings.Audio.MicDevice,
            };

            _recorder = new Reshot.Recording.VideoRecorder(
                _capture, rect.X, rect.Y, rect.Width, rect.Height, fps, bitrate, _recordingPath,
                audio, shapeMask, rect.Width);
            _recording = true;
            _session.TryTransition(SessionState.Recording);

            var primary = System.Windows.Forms.Screen.PrimaryScreen!.Bounds;
            _hud = new Recording.RecordingHudWindow(
                rect, $"{_recorder.Width}×{_recorder.Height}",
                settings.Video.Corners.Enabled, settings.Video.Corners.Color, settings.Video.Corners.Opacity,
                new System.Windows.Int32Rect(primary.X, primary.Y, primary.Width, primary.Height));
            _hud.Show();

            _tray?.ShowBalloon("Reshot: recording", $"Recording… press {_hotkey?.Current} to stop.");
            Log.Info($"Recording started ({_recorder.Width}x{_recorder.Height}) → {_recordingPath}");
        }
        catch (Exception ex)
        {
            Log.Error("Failed to start recording", ex);
            _tray?.ShowBalloon("Reshot: recording failed", ex.Message, ToolTipIcon.Error);
            _recording = false;
            StopRecording();
        }
    }

    /// <summary>Stops recording, finalizes the MP4, and tears down the HUD.</summary>
    private void StopRecording()
    {
        if (_recorder is null && _hud is null && !_recording)
            return;

        _recording = false;
        _hud?.Close();
        _hud = null;
        _session.Reset();

        var recorder = _recorder;
        _recorder = null;
        if (recorder is null)
            return;

        // Capture stops immediately; which audio tracks land in the file is decided next.
        try { recorder.Stop(); }
        catch (Exception ex) { Log.Error("Error stopping recording", ex); }

        var settings = _settingsService?.Current;
        var ask = settings?.Video.Audio.AskOnSave ?? false;
        var hasAudio = recorder.HasSystemTrack || recorder.HasMicTrack;
        Log.Info($"Recording stopped: askOnSave={ask}, system={recorder.HasSystemTrack}, " +
                 $"mic={recorder.HasMicTrack} → {(ask && hasAudio ? "showing track prompt" : "saving directly")}.");

        if (ask && hasAudio)
        {
            var prompt = new Recording.AudioTrackPrompt(
                recorder.HasSystemTrack, recorder.HasMicTrack,
                settings!.Video.Audio.System, settings.Video.Audio.Mic);
            prompt.Decided += (keepSystem, keepMic, never) =>
            {
                if (never && _settingsService is not null)
                {
                    _settingsService.Current.Video.Audio.AskOnSave = false;
                    _settingsService.Save();
                }
                FinishRecording(recorder, keepSystem, keepMic);
            };
            prompt.Show();
            return;
        }

        FinishRecording(recorder, recorder.HasSystemTrack, recorder.HasMicTrack);
    }

    /// <summary>Muxes the chosen audio tracks into the final MP4 and reports the result.</summary>
    private void FinishRecording(Reshot.Recording.VideoRecorder recorder, bool keepSystem, bool keepMic)
    {
        var path = _recordingPath;
        _recordingPath = null;

        try
        {
            var ok = recorder.Finish(keepSystem, keepMic);
            if (ok && path is not null)
            {
                _tray?.ShowBalloon("Reshot: recording saved", path);
                Log.Info($"Recording saved → {path} (system={keepSystem}, mic={keepMic})");
            }
            else
            {
                _tray?.ShowBalloon("Reshot: saving failed", "See reshot.log for details.", ToolTipIcon.Error);
            }
        }
        catch (Exception ex)
        {
            Log.Error("Error finalizing recording", ex);
            _tray?.ShowBalloon("Reshot: saving failed", ex.Message, ToolTipIcon.Error);
        }
        finally
        {
            recorder.Dispose();
        }
    }

    /// <summary>(Re)registers the optional quick audio-record hotkey; empty string unregisters.</summary>
    private void RegisterAudioHotkey(string? hotkey)
    {
        _audioHotkey?.Dispose();
        _audioHotkey = null;
        if (string.IsNullOrWhiteSpace(hotkey))
            return;

        _audioHotkey = new HotkeyService();
        _audioHotkey.HotkeyPressed += (_, _) => OnAudioHotkey();
        if (!_audioHotkey.Register(hotkey))
        {
            _tray?.ShowBalloon(
                "Reshot: audio hotkey unavailable",
                $"Could not register '{hotkey}'. It may be in use by another app.",
                ToolTipIcon.Warning);
        }
    }

    /// <summary>Quick audio-record hotkey: toggles standalone audio recording.</summary>
    private void OnAudioHotkey()
    {
        if (_audioRecording)
        {
            StopAudioRecording();
            return;
        }
        if (_recording || _overlay is not null)
            return; // busy with a capture/recording
        StartAudioRecordingFromSettings();
    }

    /// <summary>
    /// Quick audio hotkey: records everything (system + microphone), matching the overlay's
    /// left-click default. Picking specific sources is the right-click menu's job.
    /// </summary>
    private void StartAudioRecordingFromSettings()
    {
        if (_settingsService is null)
            return;
        StartAudioRecording(new Reshot.Recording.AudioSources
        {
            SystemFull = true,
            Mic = true,
            MicDevice = _settingsService.Current.Audio.MicDevice,
        });
    }

    /// <summary>Starts standalone audio recording with the given sources (and remembers them).</summary>
    private void StartAudioRecording(Reshot.Recording.AudioSources sources)
    {
        if (_audioRecording || _recording || _settingsService is null)
            return;

        // Standalone audio also shells out to ffmpeg.exe. Same early exit as video: no HUD,
        // no temp path, no half-open recorder if the binary is gone.
        if (!Reshot.Recording.Ffmpeg.IsAvailable)
        {
            Log.Error(Reshot.Recording.Ffmpeg.MissingMessage);
            _tray?.ShowBalloon(
                "Reshot: recording unavailable",
                "ffmpeg.exe is missing from the installation. Reinstall Reshot, or place ffmpeg.exe next to reshot.exe.",
                ToolTipIcon.Error);
            return;
        }

        try
        {
            var settings = _settingsService.Current;
            var dir = string.IsNullOrWhiteSpace(settings.Paths.Records)
                ? Reshot.Core.ReshotPaths.DefaultRecordsDir
                : settings.Paths.Records;
            System.IO.Directory.CreateDirectory(dir);
            var fileName = Reshot.Core.Export.FilenameBuilder.Build(settings.Filename.Template, "m4a", DateTime.Now);
            _audioRecordingPath = System.IO.Path.Combine(dir, fileName);

            // Remember system/mic toggles as the "last-used" for the quick hotkey.
            settings.Audio.System = sources.SystemFull;
            settings.Audio.Mic = sources.Mic;
            _settingsService.Save();

            _audioRecorder = new Reshot.Recording.AudioRecorder(_audioRecordingPath, sources);
            _audioRecording = true;

            var primary = System.Windows.Forms.Screen.PrimaryScreen!.Bounds;
            _audioHud = new Recording.RecordingHudWindow(
                new System.Windows.Int32Rect(0, 0, 0, 0), "Audio",
                cornersEnabled: false, "#FF0000", 1.0,
                new System.Windows.Int32Rect(primary.X, primary.Y, primary.Width, primary.Height));
            _audioHud.Show();

            var stopKey = string.IsNullOrWhiteSpace(settings.AudioHotkey) ? settings.Hotkey : $"{settings.Hotkey} / {settings.AudioHotkey}";
            _tray?.ShowBalloon("Reshot: recording audio", $"Recording audio… press {stopKey} to stop.");
            Log.Info($"Audio recording started → {_audioRecordingPath}");
        }
        catch (Exception ex)
        {
            Log.Error("Failed to start audio recording", ex);
            _tray?.ShowBalloon("Reshot: audio recording failed", ex.Message, ToolTipIcon.Error);
            _audioRecording = false;
            StopAudioRecording();
        }
    }

    /// <summary>Stops audio recording, finalizes the M4A, and tears down its HUD.</summary>
    private void StopAudioRecording()
    {
        if (_audioRecorder is null && _audioHud is null && !_audioRecording)
            return;

        _audioRecording = false;
        try { _audioRecorder?.Dispose(); }
        catch (Exception ex) { Log.Error("Error finalizing audio recording", ex); }
        _audioRecorder = null;

        _audioHud?.Close();
        _audioHud = null;
        _session.Reset();

        if (_audioRecordingPath is not null)
        {
            _tray?.ShowBalloon("Reshot: audio saved", _audioRecordingPath);
            Log.Info($"Audio recording saved → {_audioRecordingPath}");
            _audioRecordingPath = null;
        }
    }

    /// <summary>Opens the styled tray menu at the cursor and wires its intents.</summary>
    private void ShowTrayMenu()
    {
        if (_trayMenu is not null)
        {
            _trayMenu.Activate();
            return;
        }

        var menu = new Tray.TrayMenuWindow(_tray?.IsPaused ?? false);
        _trayMenu = menu;
        menu.Closed += (_, _) => _trayMenu = null;

        menu.CaptureRequested += (_, _) => OnCaptureRequested("tray menu");
        menu.SettingsRequested += OnSettingsRequested;
        menu.PauseHotkeyToggled += OnPauseHotkeyToggled;
        menu.QuitRequested += (_, _) => Shutdown();

        menu.ShowAtCursor();
    }

    /// <summary>
    /// Opens the settings UI, which now lives in the Tauri app
    /// (<c>src/reshot-tauri</c>). It edits the same
    /// <c>%AppData%\reshot\settings.json</c> this process owns, merging into it
    /// rather than overwriting, so we simply reload once it exits.
    /// </summary>
    private void OnSettingsRequested(object? sender, EventArgs e)
    {
        if (_settingsService is null)
            return;

        if (_settingsProcess is { HasExited: false })
        {
            NativeMethods.SetForegroundWindow(_settingsProcess.MainWindowHandle);
            return;
        }

        var exe = ResolveSettingsExe();
        if (exe is null)
        {
            Log.Error("Settings: reshot-tauri.exe not found");
            _tray?.ShowBalloon(
                "Reshot: settings unavailable",
                "The settings app (reshot-tauri.exe) was not found. Build it with " +
                "'npm run tauri build' in src/reshot-tauri.",
                ToolTipIcon.Error);
            return;
        }

        try
        {
            Log.Info($"Settings: launching {exe}");
            var process = new System.Diagnostics.Process
            {
                StartInfo = new System.Diagnostics.ProcessStartInfo(exe) { UseShellExecute = true },
                EnableRaisingEvents = true,
            };

            // The settings app writes straight to settings.json, so the only way
            // this process learns about a change is to re-read the file.
            process.Exited += (_, _) => Dispatcher.BeginInvoke(new Action(ReloadSettingsFromDisk));

            process.Start();
            _settingsProcess = process;
        }
        catch (Exception ex)
        {
            _settingsProcess = null;
            Log.Error("Settings app failed to start", ex);
            _tray?.ShowBalloon("Reshot: settings failed", ex.Message, ToolTipIcon.Error);
        }
    }

    /// <summary>Debug build sits in the Tauri target dir; a release install sits next to reshot.exe.</summary>
    private static string? ResolveSettingsExe()
    {
        var baseDir = AppContext.BaseDirectory;
        var candidates = new[]
        {
            Path.Combine(baseDir, "reshot-tauri.exe"),
            Path.Combine(baseDir, "settings", "reshot-tauri.exe"),
            // Running from bin/Debug/net8.0-windows10.0.19041.0 inside the repo.
            // Release first, deliberately: a plain `cargo build` produces a DEV
            // binary that loads the frontend from the vite dev server instead of
            // its own bundle, so target/debug only works while `tauri dev` runs.
            Path.GetFullPath(Path.Combine(
                baseDir, "..", "..", "..", "..", "reshot-tauri",
                "src-tauri", "target", "release", "reshot-tauri.exe")),
            Path.GetFullPath(Path.Combine(
                baseDir, "..", "..", "..", "..", "reshot-tauri",
                "src-tauri", "target", "debug", "reshot-tauri.exe")),
        };

        return candidates.FirstOrDefault(File.Exists);
    }

    /// <summary>
    /// Applies the autostart settings the user just changed, and corrects them if Windows
    /// disagreed. Registering the elevated lane needs consent, and a declined prompt must
    /// leave the checkbox showing what is actually true rather than what was asked for.
    /// </summary>
    private void ApplyAutostart(AppSettings updated)
    {
        if (_settingsService is null)
            return;

        var elevated = AutostartManager.Apply(updated.Autostart, updated.AutostartElevated, allowPrompt: true);
        if (elevated == updated.AutostartElevated)
            return;

        _settingsService.Current.AutostartElevated = elevated;
        _settingsService.Save();

        if (!elevated)
        {
            _tray?.ShowBalloon(
                "Reshot: starting normally",
                "Administrator rights were not granted, so Reshot will start with Windows " +
                "without them. Overlays over elevated applications will not work.",
                ToolTipIcon.Warning);
        }
    }

    /// <summary>Re-reads settings.json after the external settings app edited it.</summary>
    private void ReloadSettingsFromDisk()
    {
        if (_settingsService is null)
            return;

        try
        {
            ApplySettings(_settingsService.Load());
            Log.Info("Settings: reloaded after the settings app exited.");
        }
        catch (Exception ex)
        {
            Log.Error("Settings: reload failed", ex);
        }
    }

    /// <summary>Persists new settings and re-applies the live-affecting ones.</summary>
    private void ApplySettings(AppSettings updated)
    {
        if (_settingsService is null)
            return;

        var oldHotkey = _settingsService.Current.Hotkey;
        var oldAudioHotkey = _settingsService.Current.AudioHotkey;
        _settingsService.Update(updated);
        ApplyAutostart(updated);

        if (!string.Equals(oldAudioHotkey, updated.AudioHotkey, StringComparison.OrdinalIgnoreCase))
            RegisterAudioHotkey(updated.AudioHotkey);

        // Re-register only if the binding changed and the hotkey is currently active
        // (leave it alone while paused, it picks up the new binding on resume).
        if (_hotkey is { IsRegistered: true } &&
            !string.Equals(oldHotkey, updated.Hotkey, StringComparison.OrdinalIgnoreCase))
        {
            _hotkey.Unregister();
            if (!_hotkey.Register(updated.Hotkey))
            {
                _tray?.ShowBalloon(
                    "Reshot: hotkey unavailable",
                    $"Could not register '{updated.Hotkey}'. It may be in use by another app.",
                    ToolTipIcon.Warning);
            }
        }

        Log.Info("Settings updated from the Settings window.");
    }

    private void OnPauseHotkeyToggled(object? sender, bool paused)
    {
        if (_hotkey is null || _settingsService is null)
            return;

        if (paused)
        {
            _hotkey.Unregister();
            _tray?.ShowBalloon("Reshot", "Hotkey paused.");
            Log.Info("Hotkey paused by user.");
        }
        else
        {
            _hotkey.Register(_settingsService.Current.Hotkey);
            _tray?.ShowBalloon("Reshot", "Hotkey resumed.");
            Log.Info("Hotkey resumed by user.");
        }
    }

    private void OnSecondInstanceLaunched(object? sender, EventArgs e)
    {
        // Runs on a thread-pool thread, marshal to the UI thread.
        Dispatcher.Invoke(() =>
        {
            Log.Info("A second instance was launched; surfacing this one.");
            _tray?.ShowBalloon("Reshot", "Reshot is already running here.");
        });
    }

    protected override void OnExit(ExitEventArgs e)
    {
        Log.Info("Shutdown: cleaning up.");
        if (_recording)
            StopRecording();
        if (_audioRecording)
            StopAudioRecording();
        _audioHotkey?.Dispose();
        _hotkey?.Dispose();
        _tray?.Dispose();
        _singleInstance?.Dispose();
        base.OnExit(e);
    }
}
