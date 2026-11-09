using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace CuaTestHarness.Wpf;

public sealed class ScenariosManifest
{
    [JsonPropertyName("version")] public int Version { get; set; }
    [JsonPropertyName("wpf")] public PlatformBlock Wpf { get; set; } = new();

    public static ScenariosManifest Load()
    {
        string path = Path.Combine(AppContext.BaseDirectory, "scenarios.json");
        string json = File.ReadAllText(path);
        var manifest = JsonSerializer.Deserialize<ScenariosManifest>(json, new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true,
        }) ?? throw new InvalidOperationException("scenarios.json failed to parse");
        return manifest;
    }

    public ScenarioBlock Get(string id)
    {
        foreach (var s in Wpf.Scenarios)
        {
            if (string.Equals(s.Id, id, StringComparison.OrdinalIgnoreCase)) return s;
        }
        throw new KeyNotFoundException($"scenario '{id}' not found in scenarios.json");
    }

    public string Ctrl(string scenarioId, string controlKey)
    {
        var s = Get(scenarioId);
        if (s.Controls.TryGetValue(controlKey, out var v)) return v;
        throw new KeyNotFoundException($"control '{controlKey}' not found under scenario '{scenarioId}'");
    }
}

public sealed class PlatformBlock
{
    [JsonPropertyName("process_name")] public string ProcessName { get; set; } = "";
    [JsonPropertyName("main_window")]  public MainWindowBlock MainWindow { get; set; } = new();
    [JsonPropertyName("scenarios")]    public List<ScenarioBlock> Scenarios { get; set; } = new();
}

public sealed class MainWindowBlock
{
    [JsonPropertyName("title")]         public string Title { get; set; } = "";
    [JsonPropertyName("automation_id")] public string AutomationId { get; set; } = "";
}

public sealed class ScenarioBlock
{
    [JsonPropertyName("id")]                    public string Id { get; set; } = "";
    [JsonPropertyName("kind")]                  public string Kind { get; set; } = "";
    [JsonPropertyName("description")]           public string Description { get; set; } = "";
    [JsonPropertyName("controls")]              public Dictionary<string, string> Controls { get; set; } = new();
    [JsonPropertyName("expected_text_marker")]  public string? ExpectedTextMarker { get; set; }
    [JsonPropertyName("expected_dialog_title")] public string? ExpectedDialogTitle { get; set; }
    [JsonPropertyName("expected_chord")]        public string? ExpectedChord { get; set; }
}
