using System;
using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace CuaTestHarness.WinUI3;

public sealed partial class MainWindow : Window
{
    private int _counter;
    private int _clickCount;
    private DateTime _lastClickTime = DateTime.MinValue;

    // Keep a strong reference to the subclass delegate so the GC doesn't
    // collect it while Win32 still holds the function pointer.
    private SUBCLASSPROC? _subclassProc;

    public MainWindow()
    {
        InitializeComponent();
        Title = "CuaTestHarness WinUI3";
        InstallScrollMessageHook();
    }

    private void OnIncrementClick(object sender, RoutedEventArgs e)
    {
        _counter++;
        LblCounter.Text = $"counter={_counter}";
    }

    private void OnResetClick(object sender, RoutedEventArgs e)
    {
        _counter = 0;
        LblCounter.Text = "counter=0";
    }

    private void OnOpenFlyoutClick(object sender, RoutedEventArgs e)
    {
        // Button.Flyout is wired declaratively — XAML auto-opens on click. This
        // handler intentionally left empty so the flyout's default behavior
        // (open, raise UIA Window-opened event) drives the test.
    }

    private void OnOpenPopupClick(object sender, RoutedEventArgs e)
    {
        PopXaml.IsOpen = true;
    }

    private void OnClosePopupClick(object sender, RoutedEventArgs e)
    {
        PopXaml.IsOpen = false;
    }

    private void OnExitClick(object sender, RoutedEventArgs e)
    {
        // Application.Current.Exit() runs the WinUI/WindowsAppSDK
        // shutdown path so Program.Main's finally block (which calls
        // Bootstrap.Shutdown) actually executes. Environment.Exit(0)
        // terminates the process immediately and skips that cleanup.
        Application.Current.Exit();
    }

    private void OnInputChanged(object sender, TextChangedEventArgs e)
    {
        LblInputMirror.Text = $"mirror={TxtInput.Text}";
    }

    private void OnSliderChanged(object sender, Microsoft.UI.Xaml.Controls.Primitives.RangeBaseValueChangedEventArgs e)
    {
        if (LblSliderValue is null) return;
        LblSliderValue.Text = $"slider_value={(int)e.NewValue}";
    }

    private void UpdateChkState()
    {
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

    // ── click target ────────────────────────────────────────────────────────
    // A WinUI3 Button raises Click on every primary tap, so an injected
    // double-click fires Click twice; the timing check below promotes the
    // second one to `double_click` (DoubleTapped is also handled as a backup).
    private void OnTargetClick(object sender, RoutedEventArgs e)
    {
        _clickCount++;
        var now = DateTime.Now;
        var isDouble = (now - _lastClickTime).TotalMilliseconds <= 600;
        _lastClickTime = now;
        LblLastAction.Text = isDouble ? "last_action=double_click" : "last_action=left_click";
        LblClickCount.Text = $"clicks={_clickCount}";
    }

    private void OnTargetDoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        LblLastAction.Text = "last_action=double_click";
        LblClickCount.Text = $"clicks={_clickCount}";
    }

    private void OnTargetRightTapped(object sender, RightTappedRoutedEventArgs e)
    {
        LblLastAction.Text = "last_action=right_click";
    }

    // ── scroll target ───────────────────────────────────────────────────────
    private void OnScrollViewChanged(object sender, ScrollViewerViewChangedEventArgs e)
    {
        if (LblScrollOffset is null || ScrollTall is null) return;
        LblScrollOffset.Text = $"scroll_offset={(int)ScrollTall.VerticalOffset}";
    }

    // WM_VSCROLL hook. The driver's `scroll` tool posts WM_VSCROLL/WM_HSCROLL
    // to the target's top-level HWND. WinUI3 routes input through composition,
    // not the Win32 message queue, so the ScrollViewer never sees the posted
    // message on its own. Subclass the HWND and translate the SB_* codes into
    // ScrollViewer.ChangeView (same approach the WPF harness uses via
    // HwndSource.AddHook).
    private const uint WM_VSCROLL = 0x0115;
    private const int SB_LINEUP   = 0;
    private const int SB_LINEDOWN = 1;
    private const int SB_PAGEUP   = 2;
    private const int SB_PAGEDOWN = 3;

    private delegate IntPtr SUBCLASSPROC(
        IntPtr hWnd, uint uMsg, IntPtr wParam, IntPtr lParam,
        uint uIdSubclass, IntPtr dwRefData);

    [DllImport("Comctl32.dll", SetLastError = true)]
    private static extern bool SetWindowSubclass(
        IntPtr hWnd, SUBCLASSPROC pfnSubclass, uint uIdSubclass, IntPtr dwRefData);

    [DllImport("Comctl32.dll")]
    private static extern IntPtr DefSubclassProc(
        IntPtr hWnd, uint uMsg, IntPtr wParam, IntPtr lParam);

    private void InstallScrollMessageHook()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        _subclassProc = SubclassWndProc;
        SetWindowSubclass(hwnd, _subclassProc, 1, IntPtr.Zero);
    }

    private IntPtr SubclassWndProc(
        IntPtr hWnd, uint uMsg, IntPtr wParam, IntPtr lParam,
        uint uIdSubclass, IntPtr dwRefData)
    {
        if (uMsg == WM_VSCROLL && ScrollTall != null)
        {
            int code = (int)((long)wParam & 0xFFFF);
            const double line = 16.0;   // ~one TextBlock row
            const double page = 100.0;  // ScrollTall viewport height
            double delta = code switch
            {
                SB_LINEDOWN => line,
                SB_LINEUP   => -line,
                SB_PAGEDOWN => page,
                SB_PAGEUP   => -page,
                _ => 0.0,
            };
            if (delta != 0.0)
            {
                // The subclass proc runs on the UI (HWND-owner) thread, so
                // ChangeView can be called directly. disableAnimation:true so
                // VerticalOffset updates synchronously for the verifier.
                ScrollTall.ChangeView(null, ScrollTall.VerticalOffset + delta, null, true);
            }
        }
        return DefSubclassProc(hWnd, uMsg, wParam, lParam);
    }
}
