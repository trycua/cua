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
        Close();
        System.Environment.Exit(0);
    }

    private void OnInputChanged(object sender, TextChangedEventArgs e)
    {
        LblInputMirror.Text = $"mirror={TxtInput.Text}";
    }
}
