using System;
using System.IO;
using System.Text.Json;
using System.Threading.Tasks;
using System.Windows;
using Microsoft.Web.WebView2.Core;

namespace CuaTestHarness.WebView;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        Loaded += OnLoaded;
    }

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        try
        {
            // Read the CDP port from CUA_WEBVIEW_CDP_PORT (default 9222).
            // cua-driver's `page` tool routes JS execution through CDP when
            // `--remote-debugging-port` is exposed; this is the analogue of
            // launching Chrome with --remote-debugging-port for the same
            // path. Setting it on WebView2 is essential for testing the
            // page tool against this host.
            // Validate the CDP port — Chromium accepts --remote-debugging-port=0
            // (means "pick an ephemeral port") but the harness tests expect a
            // fixed port and don't read DevToolsActivePort. Reject 0 + any
            // non-numeric input here so failures are diagnosed at startup
            // rather than as opaque "CDP discovery timed out" later.
            var portStr = Environment.GetEnvironmentVariable("CUA_WEBVIEW_CDP_PORT") ?? "9222";
            if (!ushort.TryParse(portStr, out var cdpPort) || cdpPort == 0)
            {
                throw new InvalidOperationException(
                    $"Invalid CUA_WEBVIEW_CDP_PORT: '{portStr}'. Expected an integer in 1-65535.");
            }
            // WebView2 requires every process sharing a user-data directory to
            // use identical environment options. Each fixture gets a different
            // CDP port, so isolate its browser environment by process and port.
            var userData = Path.Combine(
                Path.GetTempPath(),
                "CuaTestHarness.WebView.UserData",
                $"{Environment.ProcessId}-{cdpPort}");
            Directory.CreateDirectory(userData);
            var opts = new CoreWebView2EnvironmentOptions
            {
                AdditionalBrowserArguments = $"--remote-debugging-port={cdpPort}",
            };
            var env = await CoreWebView2Environment.CreateAsync(userDataFolder: userData, options: opts);
            await Wv.EnsureCoreWebView2Async(env);

            // The Rust E2E harness owns the loopback receiver. Publish DOM state
            // through the shared fixture script so click delivery is judged
            // independently of cua-driver's UIA or CDP read-back channels.
            var journalUrl = Environment.GetEnvironmentVariable("CUA_E2E_FIXTURE_JOURNAL_URL");
            if (!string.IsNullOrWhiteSpace(journalUrl))
            {
                var encodedJournalUrl = JsonSerializer.Serialize(journalUrl);
                await Wv.CoreWebView2.AddScriptToExecuteOnDocumentCreatedAsync(
                    $"window.__CUA_E2E_FIXTURE_JOURNAL_URL = {encodedJournalUrl};");
            }

            var htmlPath = Path.Combine(AppContext.BaseDirectory, "web", "index.html");
            if (!File.Exists(htmlPath))
            {
                throw new FileNotFoundException(
                    "Harness web entry point not found. The csproj copies " +
                    "../shared-web/index.html to bin/.../web/index.html via the " +
                    "<None Include=...> + Link rule; check that the rule fired.",
                    htmlPath);
            }
            var fileUri  = new Uri(htmlPath).AbsoluteUri;
            var navigation = new TaskCompletionSource<CoreWebView2NavigationCompletedEventArgs>(
                TaskCreationOptions.RunContinuationsAsynchronously);
            void OnNavigationCompleted(
                object? navigationSender,
                CoreWebView2NavigationCompletedEventArgs navigationArgs)
            {
                Wv.NavigationCompleted -= OnNavigationCompleted;
                navigation.TrySetResult(navigationArgs);
            }
            Wv.NavigationCompleted += OnNavigationCompleted;
            Wv.Source = new Uri(fileUri);
            var navigationResult = await navigation.Task;
            if (!navigationResult.IsSuccess)
            {
                throw new InvalidOperationException(
                    $"Web fixture navigation failed: {navigationResult.WebErrorStatus}");
            }
            LblPageUrl.Text = fileUri;
            Title = $"CuaTestHarness WebView [ready cdp={cdpPort}]";
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"WebView2 init failed: {ex}");
            Environment.Exit(1);
        }
    }

    private void OnExitClick(object sender, RoutedEventArgs e) => Application.Current.Shutdown(0);
}
