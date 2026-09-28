using System;
using System.Collections.Generic;
using System.IO;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;

namespace CuaTestHarness.Wpf;

/// <summary>
/// Opt-in jev-use task window (RFC #4268), shown instead of the main window
/// when CUA_WPF_TASK_STATE=&lt;path&gt; is set. It carries the same labeled
/// controls as the AppKit harness task mode (Increment, Reset, I agree,
/// Small/Medium/Large, Note, Save note, Exit) in a small window where every
/// control is on screen, and atomically rewrites an app-owned JSON state file
/// on every change. The jev-use task oracle reads that file; it never depends
/// on Cua Driver output. Ordinary launches never create this window.
/// </summary>
public sealed class TaskWindow : Window
{
    public const string StateEnv = "CUA_WPF_TASK_STATE";
    private const string StateSchema = "cua.wpf_task_state_v1";

    private readonly string _statePath;
    private readonly TextBlock _counterLabel = new() { Text = "counter=0" };
    private readonly TextBox _note = new() { Width = 240 };
    private int _counter;
    private bool _agreed;
    private string _size = "none";
    private string? _savedNote;
    private int _sequence;

    public TaskWindow(string statePath)
    {
        _statePath = statePath;
        Title = "CuaTestHarness WPF Tasks";
        AutomationProperties.SetAutomationId(this, "wnd-tasks");
        Width = 480;
        Height = 360;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;

        var root = new StackPanel { Margin = new Thickness(16) };
        root.Children.Add(_counterLabel);

        var counterRow = Row();
        counterRow.Children.Add(MakeButton("Increment", "btn-increment", () => { _counter++; }));
        counterRow.Children.Add(MakeButton("Reset", "btn-reset", () => { _counter = 0; }));
        root.Children.Add(counterRow);

        var agree = new CheckBox { Content = "I agree", Margin = new Thickness(0, 8, 0, 0) };
        AutomationProperties.SetAutomationId(agree, "chk-agree");
        agree.Checked += (_, _) => { _agreed = true; Publish(); };
        agree.Unchecked += (_, _) => { _agreed = false; Publish(); };
        root.Children.Add(agree);

        var sizeRow = Row();
        foreach (var title in new[] { "Small", "Medium", "Large" })
        {
            var radio = new RadioButton { Content = title, GroupName = "size", Margin = new Thickness(0, 0, 12, 0) };
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

        root.Children.Add(MakeButton("Exit", "btn-exit", () => Application.Current.Shutdown(0)));
        Content = root;
        Publish();
    }

    private static StackPanel Row() =>
        new() { Orientation = Orientation.Horizontal, Margin = new Thickness(0, 8, 0, 0) };

    private Button MakeButton(string label, string automationId, Action onClick)
    {
        var button = new Button
        {
            Content = label,
            MinWidth = 90,
            Margin = new Thickness(0, 0, 8, 0),
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        AutomationProperties.SetAutomationId(button, automationId);
        button.Click += (_, _) => { onClick(); Publish(); };
        return button;
    }

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
        try
        {
            var temporaryPath = $"{_statePath}.{Environment.ProcessId}.tmp";
            File.WriteAllText(temporaryPath, System.Text.Json.JsonSerializer.Serialize(state));
            File.Move(temporaryPath, _statePath, true);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"WPF task state publish failed: {ex.Message}");
        }
    }
}
