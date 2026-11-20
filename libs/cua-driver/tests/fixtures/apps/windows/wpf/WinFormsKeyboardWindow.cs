// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;
using System.Windows.Forms;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;

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
    private int _focusChanges;
    private int _targetDown;
    private int _targetUp;
    private int _decoyDown;
    private readonly Dictionary<string, int> _targetDownByKey = new();
    private readonly Dictionary<string, int> _targetUpByKey = new();
    private readonly Dictionary<string, int> _decoyDownByKey = new();
    private readonly Dictionary<string, string> _targetModifiersByKey = new();
    // With --with-webview the same controls share a frame with a Chromium
    // surface, so a native control must keep its own exact-HWND route.
    private readonly WebView2? _webView;
    private bool _webViewReady;
    private string _lastKey = "";
    private string _lastModifiers = "";

    public WinFormsKeyboardWindow(bool hostWebView)
    {
        Text = "CuaTestHarness WinForms Keyboard";
        Width = 340;
        Height = hostWebView ? 340 : 180;
        StartPosition = FormStartPosition.CenterScreen;
        Controls.AddRange(new Control[] { _target, _decoy, _link });
        if (hostWebView)
        {
            _webView = new WebView2 { Name = "key-webview", Top = 140, Left = 20, Width = 260, Height = 140 };
            Controls.Add(_webView);
        }
        _link.LinkClicked += (_, _) => { _linkClicks++; Publish(); };
        Activated += (_, _) => { _activations++; Publish(); };
        Shown += async (_, _) =>
        {
            if (_webView is not null)
            {
                await StartWebView(_webView);
            }
            _decoy.Focus();
            Publish();
        };
        _target.KeyDown += (_, e) =>
        {
            _targetDown++;
            Count(_targetDownByKey, e.KeyCode);
            _lastKey = e.KeyCode.ToString();
            _lastModifiers = e.Modifiers.ToString();
            _targetModifiersByKey[_lastKey] = _lastModifiers;
            Publish();
        };
        _target.KeyUp += (_, e) => { _targetUp++; Count(_targetUpByKey, e.KeyCode); Publish(); };
        _decoy.KeyDown += (_, e) => { _decoyDown++; Count(_decoyDownByKey, e.KeyCode); Publish(); };
        _target.TextChanged += (_, _) => Publish();
        _decoy.TextChanged += (_, _) => Publish();
        _target.GotFocus += (_, _) => { _focusChanges++; Publish(); };
        _target.LostFocus += (_, _) => { _focusChanges++; Publish(); };
        _decoy.GotFocus += (_, _) => { _focusChanges++; Publish(); };
        _decoy.LostFocus += (_, _) => { _focusChanges++; Publish(); };
    }

    private async System.Threading.Tasks.Task StartWebView(WebView2 webView)
    {
        // Isolate the browser environment so parallel fixtures never share it.
        var userData = Path.Combine(
            Path.GetTempPath(),
            "CuaTestHarness.WinFormsKeyboard.UserData",
            Environment.ProcessId.ToString());
        Directory.CreateDirectory(userData);
        var env = await CoreWebView2Environment.CreateAsync(userDataFolder: userData);
        await webView.EnsureCoreWebView2Async(env);
        webView.NavigateToString("<p>WebView surface</p>");
        _webViewReady = true;
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
            focus_changes = _focusChanges,
            target_down = _targetDown,
            target_up = _targetUp,
            decoy_down = _decoyDown,
            target_down_by_key = _targetDownByKey,
            target_up_by_key = _targetUpByKey,
            decoy_down_by_key = _decoyDownByKey,
            target_modifiers_by_key = _targetModifiersByKey,
            link_clicks = _linkClicks,
            last_key = _lastKey,
            last_modifiers = _lastModifiers,
            target_focused = _target.Focused,
            decoy_focused = _decoy.Focused,
            target_text = _target.Text,
            decoy_text = _decoy.Text,
            webview_ready = _webViewReady,
        };
        var temporaryPath = $"{_statePath}.{Environment.ProcessId}.tmp";
        File.WriteAllText(temporaryPath, JsonSerializer.Serialize(state));
        // Windows replacement can race with the Rust state reader. Keep the
        // complete JSON atomic; retry only the observed access/sharing errors,
        // and still fail on a persistent error instead of losing the receipt.
        for (var attempt = 0; ; attempt++)
        {
            try
            {
                File.Move(temporaryPath, _statePath, true);
                return;
            }
            catch (Exception ex) when (
                attempt < 49 &&
                (ex is IOException || ex is UnauthorizedAccessException) &&
                (ex.HResult & 0xFFFF) is 5 or 32)
            {
                System.Threading.Thread.Sleep(5);
            }
        }
    }
}
