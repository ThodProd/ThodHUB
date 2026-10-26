using System.Text.Json;

namespace ThodHub.Core;

public sealed class HubStore
{
    private readonly SemaphoreSlim _gate = new(1, 1);
    public string Root { get; }
    public string AppsRoot => Path.Combine(Root, "Apps");
    public string DataRoot => Path.Combine(Root, "Data");
    public string CacheRoot => Path.Combine(Root, "Cache");
    public HubSettings Settings { get; private set; }
    public HubStore(string root)
    {
        Root = Path.GetFullPath(root);
        foreach (var p in new[] { Root, AppsRoot, DataRoot, CacheRoot }) Directory.CreateDirectory(p);
        var file = Path.Combine(Root, "hub.json");
        Settings = File.Exists(file) ? HubJson.Read<HubSettings>(File.ReadAllText(file)) : new();
        foreach (var app in Settings.Installed) { Contract.Id(app.Id); Contract.Version(app.Version); if(app.PreviousVersion is not null) Contract.Version(app.PreviousVersion); }
    }
    public string AppVersion(string id, string version) { Contract.Id(id); Contract.Version(version); return Path.Combine(AppsRoot, id, version); }
    public string DataDirectory(string id) { Contract.Id(id); var path = Path.Combine(DataRoot, id); Directory.CreateDirectory(path); return path; }
    public ModuleManifest Manifest(string id)
    {
        var app = Settings.Installed.SingleOrDefault(x => x.Id == id) ?? throw new InvalidOperationException("Программа не установлена.");
        var m = HubJson.Read<ModuleManifest>(File.ReadAllText(Path.Combine(AppVersion(id, app.Version), "manifest.json")));
        Contract.Manifest(m);
        if(m.Id != id || m.Version != app.Version) throw new InvalidDataException("Манифест установленной программы не совпадает с учётом хаба.");
        return m;
    }
    public async Task ChangeAsync(Action<HubSettings> change)
    {
        await _gate.WaitAsync();
        try
        {
            var next = HubJson.Read<HubSettings>(HubJson.Write(Settings));
            change(next);
            await WriteAtomicAsync(Path.Combine(Root, "hub.json"), HubJson.Write(next));
            Settings = next;
        }
        finally { _gate.Release(); }
    }
    public static async Task WriteAtomicAsync(string path, string content)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var tmp = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try { await File.WriteAllTextAsync(tmp, content); File.Move(tmp, path, true); }
        finally { if(File.Exists(tmp)) File.Delete(tmp); }
    }
    public async Task SaveModuleSettingsAsync(string id, JsonElement settings)
    {
        var json = settings.GetRawText();
        if(json.Length > 8 * 1024 * 1024) throw new InvalidDataException("Настройки превышают 8 МБ.");
        await WriteAtomicAsync(Path.Combine(DataDirectory(id), "settings.json"), json);
    }
    public JsonElement? LoadModuleSettings(string id)
    {
        var file = Path.Combine(DataDirectory(id), "settings.json");
        return File.Exists(file) ? JsonSerializer.Deserialize<JsonElement>(File.ReadAllText(file)) : null;
    }
}
