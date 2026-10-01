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
        // Try to bootstrap the system-wide WindowsAppSDK runtime.
        // We publish self-contained (<WindowsAppSDKSelfContained>true</...>),
        // so TryInitialize returns false on systems without the user-mode
        // bootstrap installed — that's expected and the app still runs
        // from its local copy of the SDK DLLs. Only call Shutdown() if
        // Init actually succeeded — pairing the calls keeps the
        // bootstrapper's internal refcount consistent.
        var initialized = Bootstrap.TryInitialize(0x00010006, out var hresult);
        if (!initialized)
        {
            Console.Error.WriteLine(
                $"WindowsAppSDK bootstrap not initialized (0x{hresult:X8}); " +
                "continuing with self-contained runtime.");
        }
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
            if (initialized)
            {
                Bootstrap.Shutdown();
            }
        }
    }
}
