using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;

namespace CuaTestHarness.Wpf;

public partial class MainWindow : Window
{
    public static readonly RoutedUICommand AccelCmd =
        new("Accel", "AccelCmd", typeof(MainWindow));

    private int _counter;
    private int _accelCount;
    private int _clickCount;
    private bool _targetPointerSeen;
    private readonly ScenariosManifest _manifest;
    private readonly string? _fixtureStatePath;

    public MainWindow()
    {
        _manifest = ScenariosManifest.Load();
        _fixtureStatePath = Environment.GetEnvironmentVariable("CUA_E2E_FIXTURE_STATE_PATH");
        InitializeComponent();

        Title = _manifest.Wpf.MainWindow.Title;
        AutomationProperties.SetAutomationId(this, _manifest.Wpf.MainWindow.AutomationId);

        CommandBindings.Add(new CommandBinding(AccelCmd, (s, e) =>
        {
            _accelCount++;
            LblAccelCount.Text = $"accel_fired={_accelCount}";
        }));

        Loaded += OnLoaded;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        HwndHostSlot.Content = new NativeButtonHost("Native Win32 Child");

        // Hook WM_VSCROLL on the main HWND and route it into ScrollTall.
        // WPF's input system is purely routed-event based; it does not
        // translate WM_VSCROLL into a ScrollViewer scroll. cua-driver's
        // `scroll` tool delivers SB_LINEDOWN/SB_PAGEDOWN via
        // PostMessage(WM_VSCROLL), so without this hook the scroll never
        // takes effect on a WPF host. The hook lets us assert the
        // PostMessage actually arrived and was actionable.
        var source = HwndSource.FromHwnd(new WindowInteropHelper(this).Handle);
        source?.AddHook(OnWindowMessage);
        PublishFixtureState();
    }

    private const int WM_VSCROLL    = 0x0115;
    private const int SB_LINEUP     = 0;
    private const int SB_LINEDOWN   = 1;
    private const int SB_PAGEUP     = 2;
    private const int SB_PAGEDOWN   = 3;

    private IntPtr OnWindowMessage(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (msg != WM_VSCROLL || ScrollTall == null) return IntPtr.Zero;
        int code = (int)((long)wParam & 0xFFFF);
        const double LINE = 16.0;  // ~one TextBlock line
        const double PAGE = 96.0;  // ScrollTall.Height
        double delta = code switch
        {
            SB_LINEDOWN => LINE,
            SB_LINEUP   => -LINE,
            SB_PAGEDOWN => PAGE,
            SB_PAGEUP   => -PAGE,
            _ => 0
        };
        if (delta != 0)
        {
            ScrollTall.ScrollToVerticalOffset(ScrollTall.VerticalOffset + delta);
            handled = true;
        }
        return IntPtr.Zero;
    }

    private void OnIncrementClick(object sender, RoutedEventArgs e)
    {
        _counter++;
        LblCounter.Text = $"counter={_counter}";
        PublishFixtureState();
    }

    private void OnResetClick(object sender, RoutedEventArgs e)
    {
        _counter = 0;
        LblCounter.Text = "counter=0";
        PublishFixtureState();
    }

    private void OnOpenMessageBoxClick(object sender, RoutedEventArgs e)
    {
        var title = _manifest.Get("message_box").ExpectedDialogTitle ?? "Harness MessageBox";
        MessageBox.Show(this,
            "Harness modal dialog. Click OK or Cancel.",
            title,
            MessageBoxButton.OKCancel,
            MessageBoxImage.Information);
    }

    private void OnOpenOwnedClick(object sender, RoutedEventArgs e)
    {
        var owned = new OwnedPopupWindow
        {
            Owner = this,
            Title = _manifest.Ctrl("owned_popup", "owned_window_title"),
        };
        AutomationProperties.SetAutomationId(owned, _manifest.Ctrl("owned_popup", "owned_window_aid"));
        owned.Show();
    }

    private void OnOpenLayeredClick(object sender, RoutedEventArgs e)
    {
        var layered = new LayeredPopupWindow
        {
            Owner = this,
            Title = _manifest.Ctrl("layered_popup", "layered_window_title"),
        };
        layered.Show();
    }

    private void OnSaveClick(object sender, RoutedEventArgs e)
    {
        // Intentionally a no-op except for logging — the test asserts on
        // the post-click UIA tree (button stays present, focus didn't shift).
        Title = $"{_manifest.Wpf.MainWindow.Title} [last_action=save]";
    }

    private void OnCancelClick(object sender, RoutedEventArgs e)
    {
        Title = $"{_manifest.Wpf.MainWindow.Title} [last_action=cancel]";
    }

    private void OnExitClick(object sender, RoutedEventArgs e)
    {
        Application.Current.Shutdown(0);
    }

    private void OnInputChanged(object sender, TextChangedEventArgs e)
    {
        LblInputMirror.Text = $"mirror={TxtInput.Text}";
    }

    private void OnTargetLeftDown(object sender, MouseButtonEventArgs e)
    {
        _targetPointerSeen = true;
        _clickCount++;
        if (e.ClickCount >= 2)
        {
            LblLastAction.Text = "last_action=double_click";
        }
        else
        {
            LblLastAction.Text = "last_action=left_click";
        }
        LblClickCount.Text = $"clicks={_clickCount}";
        PublishFixtureState();
        // Don't mark handled — let the Button's own logic still run.
    }

    private void OnTargetClick(object sender, RoutedEventArgs e)
    {
        // A real pointer click already passed through PreviewMouseLeftButtonDown.
        // UIA Invoke raises Click directly, so count that path here. This gives
        // the PX-background row observable fixture state for both its foreground
        // geometry probe and its occluded delivery action.
        if (_targetPointerSeen)
        {
            _targetPointerSeen = false;
        }
        else
        {
            _clickCount++;
            LblLastAction.Text = "last_action=left_click";
            LblClickCount.Text = $"clicks={_clickCount}";
        }
        PublishFixtureState();
    }

    private void OnTargetDoubleClick(object sender, MouseButtonEventArgs e)
    {
        // Belt + suspenders: Button raises MouseDoubleClick separately from
        // the second MouseLeftButtonDown. Capturing both means even
        // back-end implementations that fire only one path still register.
        LblLastAction.Text = "last_action=double_click";
        LblClickCount.Text = $"clicks={_clickCount}";
        PublishFixtureState();
    }

    private void OnTargetRightDown(object sender, MouseButtonEventArgs e)
    {
        LblLastAction.Text = "last_action=right_click";
        PublishFixtureState();
    }

    private void PublishFixtureState()
    {
        if (string.IsNullOrWhiteSpace(_fixtureStatePath)) return;

        var state = new Dictionary<string, object>
        {
            ["page-marker"] = new { text = "WPF_HARNESS_MARKER_v1" },
            ["lbl-counter"] = new { text = LblCounter?.Text ?? "counter=0" },
            ["lbl-last-action"] = new { text = LblLastAction?.Text ?? "last_action=none" },
            ["lbl-click-count"] = new { text = LblClickCount?.Text ?? "clicks=0" },
        };
        try
        {
            var temporaryPath = $"{_fixtureStatePath}.{Environment.ProcessId}.tmp";
            File.WriteAllText(temporaryPath, JsonSerializer.Serialize(state));
            File.Move(temporaryPath, _fixtureStatePath, true);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"WPF fixture state publish failed: {ex.Message}");
        }
    }

    private void OnScrollChanged(object sender, ScrollChangedEventArgs e)
    {
        if (sender is ScrollViewer sv)
        {
            LblScrollOffset.Text = $"scroll_offset={(int)sv.VerticalOffset}";
        }
    }

    private void OnSliderChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        LblSliderValue.Text = $"slider_value={(int)e.NewValue}";
    }

    private void UpdateChkState()
    {
        // XAML init fires Checked on the IsChecked="True" RadioButton before
        // sibling named controls are wired up — guard against the partial
        // construction.
        if (ChkAgreed is null || RdoLow is null || LblChkState is null) return;
        var prio = RdoLow.IsChecked == true ? "Low"
                 : RdoMed?.IsChecked == true ? "Medium"
                 : RdoHigh?.IsChecked == true ? "High"
                 : "?";
        LblChkState.Text = $"agreed={ChkAgreed.IsChecked == true}, prio={prio}";
    }
    private void OnChkChanged(object sender, RoutedEventArgs e) => UpdateChkState();
    private void OnRadioChanged(object sender, RoutedEventArgs e) => UpdateChkState();

    private void OnComboChanged(object sender, SelectionChangedEventArgs e)
    {
        if (LblComboValue is null) return;
        if (CboColor?.SelectedItem is ComboBoxItem item)
        {
            LblComboValue.Text = $"color={item.Content}";
        }
    }

    private void OnListChanged(object sender, SelectionChangedEventArgs e)
    {
        if (LblListValue is null) return;
        if (LstItems?.SelectedItem is ListBoxItem item)
        {
            LblListValue.Text = $"selected={item.Content}";
        }
    }

    private void OnMenuFileNew(object sender, RoutedEventArgs e)  => LblMenuAction.Text = "menu_action=file_new";
    private void OnMenuFileOpen(object sender, RoutedEventArgs e) => LblMenuAction.Text = "menu_action=file_open";
    private void OnMenuEditCopy(object sender, RoutedEventArgs e) => LblMenuAction.Text = "menu_action=edit_copy";

    private void OnCtxAction(object sender, RoutedEventArgs e)
    {
        if (sender is MenuItem mi)
        {
            var label = mi.Header?.ToString()?.Replace("_", "").ToLowerInvariant() ?? "?";
            LblMenuAction.Text = $"menu_action=ctx_{label}";
        }
    }
}
