using System;
using Microsoft.UI.Xaml;

namespace CuaTestHarness.WinUI3;

public partial class App : Application
{
    private Window? _window;

    public App() { InitializeComponent(); }

    // Ordinary launches show MainWindow exactly as before.
    // CUA_WINUI3_TASK_STATE selects the opt-in jev-use task window;
    // CUA_WINUI3_TASK_DENSITY adds its distractor controls (#4312).
    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var taskState = Environment.GetEnvironmentVariable(TaskWindow.StateEnv);
        _window = string.IsNullOrWhiteSpace(taskState)
            ? new MainWindow()
            : new TaskWindow(taskState, TaskWindow.DensityFromEnvironment());
        _window.Activate();
    }
}
