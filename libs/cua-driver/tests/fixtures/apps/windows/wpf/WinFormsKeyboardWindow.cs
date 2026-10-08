using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;
using System.Windows.Forms;

namespace CuaTestHarness.Wpf;

// Native EDIT children: unlike windowless WPF controls, these can receive
// posted keys directly without changing the application's keyboard focus.
public sealed class WinFormsKeyboardWindow : Form
{
    private readonly string _statePath = Environment.GetEnvironmentVariable("CUA_E2E_FIXTURE_STATE_PATH")
        ?? throw new InvalidOperationException("keyboard fixture requires a state path");
    private readonly TextBox _target = new() { Name = "key-target", AccessibleName = "Key target", Top = 20, Left = 20, Width = 260 };
    private readonly TextBox _decoy = new() { Name = "key-decoy", AccessibleName = "Key decoy", Top = 60, Left = 20, Width = 260 };
    // A LinkLabel link is a real windowless UIA child, not a fabricated provider.
    private readonly LinkLabel _link = new() { Name = "key-link-container", AccessibleName = "Link container", Text = "Key link", Top = 100, Left = 20, Width = 260 };
    private int _linkClicks;
    private int _activations;
    private int _targetDown;
    private int _targetUp;
    private int _decoyDown;
    private readonly Dictionary<string, int> _targetDownByKey = new();
    private readonly Dictionary<string, int> _targetUpByKey = new();
    private readonly Dictionary<string, int> _decoyDownByKey = new();
    private string _lastKey = "";
    private string _lastModifiers = "";

    public WinFormsKeyboardWindow()
    {
        Text = "CuaTestHarness WinForms Keyboard";
        Width = 340;
        Height = 180;
        StartPosition = FormStartPosition.CenterScreen;
        Controls.AddRange(new Control[] { _target, _decoy, _link });
        _link.LinkClicked += (_, _) => { _linkClicks++; Publish(); };
        Activated += (_, _) => { _activations++; Publish(); };
        Shown += (_, _) => { _decoy.Focus(); Publish(); };
        _target.KeyDown += (_, e) =>
        {
            _targetDown++;
            Count(_targetDownByKey, e.KeyCode);
            _lastKey = e.KeyCode.ToString();
            _lastModifiers = e.Modifiers.ToString();
            Publish();
        };
        _target.KeyUp += (_, e) => { _targetUp++; Count(_targetUpByKey, e.KeyCode); Publish(); };
        _decoy.KeyDown += (_, e) => { _decoyDown++; Count(_decoyDownByKey, e.KeyCode); Publish(); };
    }

    private static void Count(Dictionary<string, int> receipts, Keys key)
    {
        var name = key.ToString();
        receipts[name] = receipts.TryGetValue(name, out var count) ? count + 1 : 1;
    }

    private void Publish()
    {
        var state = new
        {
            activations = _activations,
            target_down = _targetDown,
            target_up = _targetUp,
            decoy_down = _decoyDown,
            target_down_by_key = _targetDownByKey,
            target_up_by_key = _targetUpByKey,
            decoy_down_by_key = _decoyDownByKey,
            link_clicks = _linkClicks,
            last_key = _lastKey,
            last_modifiers = _lastModifiers,
            target_focused = _target.Focused,
            decoy_focused = _decoy.Focused,
        };
        var temporaryPath = $"{_statePath}.{Environment.ProcessId}.tmp";
        File.WriteAllText(temporaryPath, JsonSerializer.Serialize(state));
        File.Move(temporaryPath, _statePath, true);
    }
}
