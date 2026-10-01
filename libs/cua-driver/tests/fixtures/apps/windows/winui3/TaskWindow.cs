using System;
using System.Collections.Generic;
using System.IO;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace CuaTestHarness.WinUI3;

/// <summary>
/// Opt-in jev-use task window (RFC #4268), shown instead of the main window
/// when CUA_WINUI3_TASK_STATE=&lt;path&gt; is set. It carries the same labeled
/// controls as the AppKit, WPF, and GTK3 harness task modes (Increment, Reset,
/// I agree, Small/Medium/Large, Note, Save note, Exit) in a small window where
/// every control is on screen, and atomically rewrites an app-owned JSON state
/// file on every change. The jev-use task oracle reads that file; it never
/// depends on Cua Driver output. Ordinary launches never create this window.
/// CUA_WINUI3_TASK_DENSITY=12 or 24 (task mode only) also adds benign
/// distractor controls in a panel before the task controls, for measuring
/// jev-use accuracy at larger candidate sets (#4312).
/// </summary>
public sealed class TaskWindow : Window
{
    public const string StateEnv = "CUA_WINUI3_TASK_STATE";
    private const string StateSchema = "cua.winui3_task_state_v1";
    public const string DensityEnv = "CUA_WINUI3_TASK_DENSITY";

    // Opt-in distractor controls (#4312). The labels are benign (no
    // risky-action phrase) and match the AppKit, GTK3, and WPF harnesses,
    // so candidate IDs agree across platforms. Some are unrelated to every
    // task; some are close to a task control ("Save draft", "Increase font
    // size", "Note title", "Large icons"). Density 12 uses the first entries
    // of each list; density 24 uses all of them.
    private static readonly string[] DistractorButtons =
    {
        "New folder", "Refresh", "Undo", "Redo", "Zoom in", "Zoom out", "Save draft",
        "Increase font size", "Copy link", "Duplicate", "Rename", "Print preview", "Export PDF",
        "Import", "Bold", "Italic", "Underline", "Align left", "Align center", "Align right",
        "Insert table", "Insert image", "Spell check", "Word count", "Show sidebar", "Help",
    };
    private static readonly string[] DistractorCheckboxes =
    {
        "Show ruler", "Show previews", "Word wrap", "Auto-save", "Line numbers", "Dark mode",
        "Show hidden files", "Sync on startup", "Compact layout", "Show status bar",
        "Remember window size", "Check spelling as you type",
    };
    private static readonly string[][] DistractorRadioGroups =
    {
        new[] { "Light", "Dark", "System" },
        new[] { "List", "Grid", "Columns" },
        new[] { "Name", "Date", "Kind" },
        new[] { "Small icons", "Medium icons", "Large icons" },
    };
    private static readonly string[] DistractorFields = { "Search", "Note title" };
    // density -> (buttons, checkboxes, radio groups, text fields)
    private static readonly Dictionary<int, (int, int, int, int)> DensityCounts = new() { [12] = (8, 3, 1, 1), [24] = (26, 12, 4, 2) };

    /// <summary>
    /// The opt-in distractor density: null, 12, or 24. Anything else is a
    /// launch error, so a measurement never runs at an unintended density.
    /// </summary>
    public static int? DensityFromEnvironment()
    {
        var raw = Environment.GetEnvironmentVariable(DensityEnv)?.Trim();
        if (string.IsNullOrEmpty(raw)) return null;
        if (int.TryParse(raw, out var density) && DensityCounts.ContainsKey(density)) return density;
        Console.Error.WriteLine($"{DensityEnv} must be 12 or 24, not {raw}");
        Environment.Exit(2);
        return null;
    }


    private readonly string _statePath;
    private readonly TextBlock _counterLabel = new() { Text = "counter=0" };
    private readonly TextBox _note = new() { Width = 240 };
    private int _counter;
    private bool _agreed;
    private string _size = "none";
    private string? _savedNote;
    private int _sequence;
    private readonly int? _density;
    private int _distractorActions;

    public TaskWindow(string statePath, int? density = null)
    {
        _statePath = statePath;
        _density = density;
        Title = "CuaTestHarness WinUI3 Tasks";

        var root = new StackPanel { Padding = new Thickness(16), Spacing = 8 };
        root.Children.Add(_counterLabel);

        var counterRow = Row();
        counterRow.Children.Add(MakeButton("Increment", "btn-increment", () => { _counter++; }));
        counterRow.Children.Add(MakeButton("Reset", "btn-reset", () => { _counter = 0; }));
        root.Children.Add(counterRow);

        var agree = new CheckBox { Content = "I agree" };
        AutomationProperties.SetAutomationId(agree, "chk-agree");
        agree.Checked += (_, _) => { _agreed = true; Publish(); };
        agree.Unchecked += (_, _) => { _agreed = false; Publish(); };
        root.Children.Add(agree);

        var sizeRow = Row();
        foreach (var title in new[] { "Small", "Medium", "Large" })
        {
            var radio = new RadioButton { Content = title, GroupName = "size", MinWidth = 0 };
            AutomationProperties.SetAutomationId(radio, $"rad-size-{title.ToLowerInvariant()}");
            radio.Checked += (_, _) => { _size = title.ToLowerInvariant(); Publish(); };
            sizeRow.Children.Add(radio);
        }
        root.Children.Add(sizeRow);

        var noteRow = Row();
        // A UIA Name and no placeholder: an unnamed Edit falls back to its
        // value as its label, which jev-use treats as unlabeled.
        AutomationProperties.SetName(_note, "Note");
        AutomationProperties.SetAutomationId(_note, "txt-note");
        noteRow.Children.Add(_note);
        noteRow.Children.Add(MakeButton("Save note", "btn-save-note", () => { _savedNote = _note.Text; }));
        root.Children.Add(noteRow);

        root.Children.Add(MakeButton("Exit", "btn-exit", () => Application.Current.Exit()));
        if (density is int value)
        {
            // Before the task controls, as a sidebar precedes the content in a
            // typical document app's depth-first order.
            var layout = new StackPanel { Orientation = Orientation.Horizontal };
            layout.Children.Add(DistractorPanel(value));
            layout.Children.Add(root);
            Content = layout;
        }
        else
        {
            Content = root;
        }
        PlaceWindow();
        Publish();
    }

    private static StackPanel Row() => new() { Orientation = Orientation.Horizontal, Spacing = 8 };

    /// <summary>
    /// Benign distractor controls for density mode (#4312): rows of compact
    /// buttons, labeled text fields, checkboxes, and radio groups, sized so the
    /// whole window fits a 1024x768 display. Each radio row is its own group,
    /// and no option starts selected.
    /// </summary>
    private StackPanel DistractorPanel(int density)
    {
        var (buttons, checkboxes, groups, fields) = DensityCounts[density];
        var panel = new StackPanel { Padding = new Thickness(16, 16, 0, 16), Spacing = 4, Width = 560 };
        StackPanel? row = null;
        for (var index = 0; index < buttons; index++)
        {
            if (index % 4 == 0) panel.Children.Add(row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 });
            var button = new Button
            {
                Content = DistractorButtons[index], FontSize = 12, MinWidth = 0, Padding = new Thickness(8, 3, 8, 4),
            };
            button.Click += (_, _) => OnDistractor();
            row!.Children.Add(button);
        }
        var fieldRow = Row();
        for (var index = 0; index < fields; index++)
        {
            // A UIA Name and no placeholder, like the Note field.
            var field = new TextBox { Width = 180, FontSize = 12 };
            AutomationProperties.SetName(field, DistractorFields[index]);
            field.TextChanged += (_, _) => OnDistractor();
            fieldRow.Children.Add(field);
        }
        panel.Children.Add(fieldRow);
        for (var index = 0; index < checkboxes; index++)
        {
            if (index % 3 == 0) panel.Children.Add(row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 });
            var check = new CheckBox { Content = DistractorCheckboxes[index], FontSize = 12, MinWidth = 0 };
            check.Checked += (_, _) => OnDistractor();
            check.Unchecked += (_, _) => OnDistractor();
            row!.Children.Add(check);
        }
        for (var group = 0; group < groups; group++)
        {
            var radioRow = Row();
            foreach (var title in DistractorRadioGroups[group])
            {
                var radio = new RadioButton { Content = title, GroupName = $"distractor-{group}", FontSize = 12, MinWidth = 0 };
                radio.Checked += (_, _) => OnDistractor();
                radioRow.Children.Add(radio);
            }
            panel.Children.Add(radioRow);
        }
        return panel;
    }

    private void OnDistractor()
    {
        _distractorActions++;
        Publish();
    }

    private Button MakeButton(string label, string automationId, Action onClick)
    {
        var button = new Button { Content = label, MinWidth = 90 };
        AutomationProperties.SetAutomationId(button, automationId);
        button.Click += (_, _) => { onClick(); Publish(); };
        return button;
    }

    // A small window, centered on its display, so every control is on screen.
    // AppWindow sizes are physical pixels, so scale by the window's DPI.
    private void PlaceWindow()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        var scale = GetDpiForWindow(hwnd) / 96.0;
        if (scale <= 0) scale = 1.0;
        // The distractor panel widens and heightens the window (#4312).
        var width = (int)((_density is null ? 520 : 1000) * scale);
        var height = (int)((_density is null ? 400 : 640) * scale);
        var work = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        AppWindow.MoveAndResize(new Windows.Graphics.RectInt32(
            work.X + Math.Max(0, (work.Width - width) / 2),
            work.Y + Math.Max(0, (work.Height - height) / 2),
            width,
            height));
    }

    [System.Runtime.InteropServices.DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hwnd);

    private void Publish()
    {
        _counterLabel.Text = $"counter={_counter}";
        _sequence++;
        var state = new SortedDictionary<string, object?>(StringComparer.Ordinal)
        {
            ["schema"] = StateSchema,
            ["pid"] = Environment.ProcessId,
            ["seq"] = _sequence,
            ["counter"] = _counter,
            ["agreed"] = _agreed,
            ["size"] = _size,
            ["note_saved"] = _savedNote,
        };
        if (_density is int density)
        {
            state["density"] = density;
            state["distractor_actions"] = _distractorActions;
        }
        try
        {
            var temporaryPath = $"{_statePath}.{Environment.ProcessId}.tmp";
            File.WriteAllText(temporaryPath, System.Text.Json.JsonSerializer.Serialize(state));
            File.Move(temporaryPath, _statePath, true);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"WinUI3 task state publish failed: {ex.Message}");
        }
    }
}
