using Microsoft.UI.Xaml;

namespace CuaTestHarness.WinUI3;

public partial class App : Application
{
    private Window? _window;

    public App() { InitializeComponent(); }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activate();
    }
}
