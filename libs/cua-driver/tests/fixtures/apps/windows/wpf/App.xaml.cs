using System;
using System.Windows;

namespace CuaTestHarness.Wpf;

public partial class App : Application
{
    // Ordinary launches show MainWindow exactly as before (it was the
    // StartupUri). CUA_WPF_TASK_STATE selects the opt-in jev-use task window;
    // CUA_WPF_TASK_DENSITY adds its distractor controls (#4312).
    private void OnStartup(object sender, StartupEventArgs e)
    {
        // Keep the native child-HWND keyboard fixture in the existing Windows
        // harness binary and runner, without changing ordinary WPF launches.
        if (Array.IndexOf(e.Args, "--winforms-keyboard") >= 0)
        {
            using var form = new WinFormsKeyboardWindow();
            System.Windows.Forms.Application.Run(form);
            Shutdown();
            return;
        }

        var taskState = Environment.GetEnvironmentVariable(TaskWindow.StateEnv);
        Window window = string.IsNullOrWhiteSpace(taskState)
            ? new MainWindow()
            : new TaskWindow(taskState, TaskWindow.DensityFromEnvironment());
        MainWindow = window;
        window.Show();
    }
}
