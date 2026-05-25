using System;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Input;
using System.Windows.Interop;

namespace CuaTestHarness.Wpf;

public partial class MainWindow : Window
{
    public static readonly RoutedUICommand AccelCmd =
        new("Accel", "AccelCmd", typeof(MainWindow));

    private int _counter;
    private int _accelCount;
    private readonly ScenariosManifest _manifest;

    public MainWindow()
    {
        _manifest = ScenariosManifest.Load();
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
}
