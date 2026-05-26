using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CuaTestHarness.WinUI3;

public sealed partial class MainWindow : Window
{
    private int _counter;

    public MainWindow()
    {
        InitializeComponent();
        Title = "CuaTestHarness WinUI3";
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
}
