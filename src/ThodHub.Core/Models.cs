using System.Text.Json;
using System.Text.RegularExpressions;

namespace ThodHub.Core;

public static class HubJson
{
    public static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    public static T Read<T>(string text) => JsonSerializer.Deserialize<T>(text.TrimStart('\uFEFF'), Options) ?? throw new InvalidDataException("Пустой JSON.");
    public static string Write<T>(T value) => JsonSerializer.Serialize(value, Options);
}

public sealed record Catalog
{
    public int SchemaVersion { get; init; } = 1;
    public List<CatalogApp> Apps { get; init; } = [];
}
public sealed record CatalogApp
{
    public required string Id { get; init; }
    public required string Name { get; init; }
    public required string Description { get; init; }
    public string Kind { get; init; } = "Программа";
    public required string Repository { get; init; }
    public string Category { get; init; } = "Утилиты";
    public string Icon { get; init; } = "app";
    public string Mode { get; init; } = "embedded";
    public string? ArtworkUrl { get; init; }
    public string? IconUrl { get; init; }
    public PackageRelease? Release { get; set; }
    public string? ReleaseError { get; set; }
}
public sealed record PackageRelease
{
    public required string Version { get; init; }
    public required string PackageUrl { get; init; }
    public required string Sha256 { get; init; }
    public long Size { get; init; }
    public string Notes { get; init; } = "";
    public string Platform { get; init; } = "win-x64";
    public int HubApi { get; init; } = 1;
}
public sealed record ModuleManifest
{
    public int SchemaVersion { get; init; } = 1;
    public int HubApi { get; init; } = 1;
    public required string Id { get; init; }
    public required string Version { get; init; }
    public string Platform { get; init; } = "win-x64";
    public string Mode { get; init; } = "embedded";
    public string? Ui { get; init; }
    public WorkerDefinition? Launch { get; init; }
    public bool RequiresElevation { get; init; }
    public WorkerDefinition? Worker { get; init; }
    public WorkerDefinition? Cleanup { get; init; }
    public WorkerDefinition? Integration { get; init; }
    public List<ModuleAction> Actions { get; init; } = [];
    public List<string> Permissions { get; init; } = [];
}
public sealed record WorkerDefinition
{
    public required string Executable { get; init; }
    public List<string> Arguments { get; init; } = [];
}
public sealed record ModuleAction
{
    public bool WaitForExit { get; init; }
    public required string Id { get; init; }
    public required string Label { get; init; }
    public List<string> Arguments { get; init; } = [];
}
public sealed record InstalledApp
{
    public string? IntegrationError { get; init; }
    public required string Id { get; init; }
    public required string Version { get; init; }
    public string? PreviousVersion { get; init; }
    public DateTimeOffset InstalledAt { get; init; } = DateTimeOffset.UtcNow;
}
public sealed record HubSettings
{
    public string Theme { get; set; } = "system";
    public bool CheckOnStartup { get; set; } = true;
    public string CatalogRepository { get; set; } = "ThodProd/ThodHUB-Registry";
    public string CatalogRef { get; set; } = "main";
    public string BackupRepository { get; set; } = "ThodProd/ThodHUB-Backups";
    public List<string> SidebarOrder { get; set; } = [];
    public List<InstalledApp> Installed { get; set; } = [];
}

public static partial class Contract
{
    public const int ApiVersion = 1;
    public const long MaxPackageBytes = 1024L * 1024 * 1024;
    [GeneratedRegex("^[a-z][a-z0-9-]{1,63}$", RegexOptions.CultureInvariant)]
    private static partial Regex IdRegex();
    [GeneratedRegex("^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$", RegexOptions.CultureInvariant)]
    private static partial Regex VersionRegex();
    [GeneratedRegex("^[a-zA-Z0-9][a-zA-Z0-9-]{0,38}/[a-zA-Z0-9_.-]{1,100}$", RegexOptions.CultureInvariant)]
    private static partial Regex RepoRegex();
    public static void Id(string id) { if (!IdRegex().IsMatch(id)) throw new InvalidDataException("Некорректный идентификатор программы."); }
    public static void Version(string version) { if (!VersionRegex().IsMatch(version) || !System.Version.TryParse(version, out _)) throw new InvalidDataException("Версия должна иметь формат 1.2.3."); }
    public static void Repository(string repository) { if (!RepoRegex().IsMatch(repository)) throw new InvalidDataException("Репозиторий должен иметь формат owner/name."); }
    public static bool IsNewer(string candidate, string installed)
    {
        Version(candidate); Version(installed);
        return System.Version.Parse(candidate) > System.Version.Parse(installed);
    }
    public static string Inside(string root, string relative)
    {
        if (string.IsNullOrWhiteSpace(relative) || Path.IsPathRooted(relative) || relative.Contains(':') || relative.Contains('\\'))
            throw new InvalidDataException("Недопустимый путь внутри пакета.");
        var parts = relative.Split('/');
        if (parts.Any(p => p is "." or ".." || p.EndsWith(' ') || p.EndsWith('.') || p.Any(c => c < 32) || p.IndexOfAny(['<','>','|','?','*']) >= 0))
            throw new InvalidDataException("Недопустимый путь внутри пакета.");
        var reserved = new[] { "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9" };
        if (parts.Any(p => reserved.Contains(p.Split('.')[0], StringComparer.OrdinalIgnoreCase))) throw new InvalidDataException("Зарезервированное имя файла Windows.");
        var fullRoot = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        var target = Path.GetFullPath(Path.Combine(root, relative));
        if (!target.StartsWith(fullRoot, StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Путь выходит за пределы пакета.");
        return target;
    }
    public static void Manifest(ModuleManifest m)
    {
        Id(m.Id); Version(m.Version);
        if (m.SchemaVersion != 1 || m.HubApi != ApiVersion || m.Platform != "win-x64") throw new InvalidDataException("Пакет несовместим с этим хабом.");
        if(m.Mode is not ("embedded" or "standalone")) throw new InvalidDataException("Неизвестный режим программы.");
        if(m.Mode == "embedded")
        {
            if(m.Ui is null) throw new InvalidDataException("Не задан интерфейс программы.");
            Inside(Path.GetTempPath(), m.Ui);
            if (!m.Ui.EndsWith(".html", StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Точка входа UI должна быть HTML.");
            if(m.RequiresElevation) throw new InvalidDataException("Встроенный модуль не может требовать повышения прав хаба.");
        }
        else
        {
            if(m.Launch is null) throw new InvalidDataException("Не задан запуск самостоятельной программы.");
            Inside(Path.GetTempPath(), m.Launch.Executable);
            if(!m.Launch.Executable.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Самостоятельная программа должна содержать exe.");
            if(m.Worker is not null) throw new InvalidDataException("Самостоятельная программа не должна объявлять worker.");
        }
        if (m.Worker is not null)
        {
            Inside(Path.GetTempPath(), m.Worker.Executable);
            if (!m.Worker.Executable.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Пакет должен содержать готовый Windows exe.");
        }
        foreach(var hook in new[]{m.Cleanup,m.Integration}.OfType<WorkerDefinition>())
        {
            Inside(Path.GetTempPath(),hook.Executable);
            if(!hook.Executable.EndsWith(".exe",StringComparison.OrdinalIgnoreCase)||hook.Arguments.Count>16||hook.Arguments.Any(a=>a.Length>2048||a.Contains('\0'))) throw new InvalidDataException("Интеграция должна выполняться exe из пакета с корректными аргументами.");
        }
        if(m.Integration is not null && m.Cleanup is null)throw new InvalidDataException("Интеграция Windows требует cleanup для удаления.");
        if(m.Actions.Count>8 || (m.Mode!="standalone" && m.Actions.Count>0) || m.Actions.Select(a=>a.Id).Distinct().Count()!=m.Actions.Count)
            throw new InvalidDataException("Некорректные действия программы.");
        foreach(var action in m.Actions)
        {
            Id(action.Id);
            if(string.IsNullOrWhiteSpace(action.Label)||action.Label.Length>80||action.Arguments.Count>16||action.Arguments.Any(a=>a.Length>2048||a.Contains('\0')))
                throw new InvalidDataException("Некорректное действие программы.");
        }
        string[] allowed = ["settings", "clipboard.write", "dialog.folder", "dialog.save", "files.export", "document.pdf", "document.print", "worker"];
        if (m.Permissions.Any(x => !allowed.Contains(x))) throw new InvalidDataException("Неизвестное разрешение пакета.");
        if (m.Worker is not null && !m.Permissions.Contains("worker")) throw new InvalidDataException("Не объявлено разрешение worker.");
    }
    public static void Release(PackageRelease r, string repository)
    {
        Version(r.Version); Repository(repository);
        if (r.Platform != "win-x64" || r.HubApi != ApiVersion) throw new InvalidDataException("Выпуск несовместим с хабом.");
        if (r.Size <= 0 || r.Size > MaxPackageBytes) throw new InvalidDataException("Некорректный размер пакета.");
        if (r.Sha256.Length != 64 || r.Sha256.Any(c => !Uri.IsHexDigit(c))) throw new InvalidDataException("Не задана SHA-256 пакета.");
        if (!Uri.TryCreate(r.PackageUrl, UriKind.Absolute, out var u) || u.Scheme != "https" || u.Host != "github.com" || !u.IsDefaultPort || u.UserInfo != "" || !u.AbsolutePath.StartsWith($"/{repository}/releases/download/", StringComparison.Ordinal) || u.Query != "" || u.Fragment != "")
            throw new InvalidDataException("Пакет должен быть выпуском указанного GitHub-репозитория.");
    }
}
