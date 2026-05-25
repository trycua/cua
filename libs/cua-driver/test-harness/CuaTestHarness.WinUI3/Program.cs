using System;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.ApplicationModel.DynamicDependency;
using WinRT;

namespace CuaTestHarness.WinUI3;

public static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        // Bootstrap the WindowsAppSDK for unpackaged scenarios.
        Bootstrap.TryInitialize(0x00010006, out _);
        try
        {
            ComWrappersSupport.InitializeComWrappers();
            Application.Start(p =>
            {
                var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
                System.Threading.SynchronizationContext.SetSynchronizationContext(context);
                _ = new App();
            });
            return 0;
        }
        finally
        {
            Bootstrap.Shutdown();
        }
    }
}
