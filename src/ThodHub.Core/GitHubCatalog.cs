using System.Globalization;
using System.Net;
using System.Net.Http.Headers;
using System.Text.Json;
using System.Xml.Linq;

namespace ThodHub.Core;

public sealed class GitHubCatalog : IDisposable
{
    private static readonly XNamespace AtomNs = "http://www.w3.org/2005/Atom";
    private readonly HubStore _store;
    private readonly HttpClient _http;
    public GitHubCatalog(HubStore store)
    {
        _store = store;
        _http = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
        _http.DefaultRequestHeaders.UserAgent.ParseAdd("ThodHUB/0.1.0");
        _http.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
    }
    public Catalog LoadCached(string bundledPath)
    {
        var cache = Path.Combine(_store.CacheRoot, "catalog.json");
        if (File.Exists(cache))
        {
            try { var c = HubJson.Read<Catalog>(File.ReadAllText(cache)); Validate(c); return c; }
            catch (Exception e) when(e is IOException or JsonException or InvalidDataException) { }
        }
        var fallback = HubJson.Read<Catalog>(File.ReadAllText(bundledPath)); Validate(fallback); return fallback;
    }
    public async Task<Catalog> RefreshAsync(Catalog previous, CancellationToken ct = default)
    {
        var repo = _store.Settings.CatalogRepository;
        Contract.Repository(repo);
        var refName = string.Join('/', _store.Settings.CatalogRef.Split('/').Select(Uri.EscapeDataString));
        var catalog = HubJson.Read<Catalog>(await GetTextAsync($"https://raw.githubusercontent.com/{repo}/{refName}/catalog.json", 2_000_000, ct));
        Validate(catalog);
        // Keep installed programs addressable when the registry retires an entry.
        foreach(var old in previous.Apps.Where(a => _store.Settings.Installed.Any(i => i.Id == a.Id) && !catalog.Apps.Any(a2 => a2.Id == a.Id)))
            catalog.Apps.Add(old with { Release = null, ReleaseError = "Программа больше не входит в удалённый каталог." });
        using var slots = new SemaphoreSlim(4);
        await Task.WhenAll(catalog.Apps.Select(async app =>
        {
            await slots.WaitAsync(ct);
            try
            {
                var release = await FindLatestReleaseAsync(app.Repository, ct);
                if(release is null)
                {
                    var old = previous.Apps.FirstOrDefault(x => x.Id == app.Id && x.Repository == app.Repository);
                    app.Release = old?.Release;
                    app.ReleaseError = null;
                    return;
                }
                app.Release = release;
            }
            catch(Exception e) when(e is HttpRequestException or JsonException or InvalidDataException or FormatException or KeyNotFoundException or TaskCanceledException or System.Xml.XmlException)
            {
                if(ct.IsCancellationRequested) throw;
                var old = previous.Apps.FirstOrDefault(x => x.Id == app.Id && x.Repository == app.Repository);
                app.Release = old?.Release;
                app.ReleaseError = "Не удалось проверить выпуск. " + e.Message;
            }
            finally { slots.Release(); }
        }));
        await HubStore.WriteAtomicAsync(Path.Combine(_store.CacheRoot, "catalog.json"), HubJson.Write(catalog));
        return catalog;
    }
    private async Task<PackageRelease?> FindLatestReleaseAsync(string repository, CancellationToken ct)
    {
        foreach(var release in await ReadReleasesAsync(repository, ct))
        {
            var descriptor = await TryReadDescriptorAsync(repository, release.Tag, ct);
            if(descriptor is null || descriptor.Version != release.Version) continue;
            Contract.Release(descriptor, repository);
            return descriptor with { PublishedAt = release.PublishedAt };
        }
        return null;
    }
    private async Task<List<AtomRelease>> ReadReleasesAsync(string repository, CancellationToken ct)
    {
        var releases = new List<AtomRelease>();
        string xml;
        try { xml = await GetTextAsync($"https://github.com/{repository}/releases.atom", 2_000_000, ct); }
        catch(HttpRequestException e) when(e.StatusCode == HttpStatusCode.NotFound) { return releases; }
        var atom = XDocument.Parse(xml);
        if(atom.Root is null) return releases;
        foreach(var entry in atom.Root.Elements(AtomNs + "entry"))
        {
            var link = entry.Elements(AtomNs + "link").FirstOrDefault(e => (string?)e.Attribute("rel") == "alternate")?.Attribute("href")?.Value;
            if(link is null || !IsRepoUrl(link, repository, "releases/tag/")) continue;
            const string marker = "/releases/tag/";
            var index = link.IndexOf(marker, StringComparison.Ordinal);
            if(index < 0) continue;
            var tag = Uri.UnescapeDataString(link[(index + marker.Length)..]);
            var version = VersionFromTag(tag);
            if(version is null) continue;
            var dateText = entry.Element(AtomNs + "published")?.Value ?? entry.Element(AtomNs + "updated")?.Value;
            var publishedAt = DateTimeOffset.TryParse(dateText, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal, out var date) ? date : (DateTimeOffset?)null;
            releases.Add(new AtomRelease(tag, version, link, publishedAt));
        }
        return releases;
    }
    private async Task<PackageRelease?> TryReadDescriptorAsync(string repository, string tag, CancellationToken ct)
    {
        var url = $"https://github.com/{repository}/releases/download/{Uri.EscapeDataString(tag)}/thodhub-release.json";
        try { return HubJson.Read<PackageRelease>(await GetTextAsync(url, 100_000, ct)); }
        catch(HttpRequestException e) when(e.StatusCode == HttpStatusCode.NotFound) { return null; }
    }
    private static string? VersionFromTag(string tag)
    {
        var version = tag.StartsWith("hub-v", StringComparison.OrdinalIgnoreCase) ? tag[5..] : tag.StartsWith('v') ? tag[1..] : tag;
        try { Contract.Version(version); return version; }
        catch(InvalidDataException) { return null; }
    }
    private async Task<string> GetTextAsync(string url, int limit, CancellationToken ct)
    {
        using var response = await _http.GetAsync(url, HttpCompletionOption.ResponseHeadersRead, ct);
        response.EnsureSuccessStatusCode();
        if(response.Content.Headers.ContentLength > limit) throw new InvalidDataException("Ответ сервера слишком большой.");
        await using var stream = await response.Content.ReadAsStreamAsync(ct);
        using var buffer = new MemoryStream();
        var chunk = new byte[16384]; int n;
        while((n = await stream.ReadAsync(chunk, ct)) > 0)
        {
            if(buffer.Length + n > limit) throw new InvalidDataException("Ответ сервера слишком большой.");
            buffer.Write(chunk, 0, n);
        }
        return System.Text.Encoding.UTF8.GetString(buffer.ToArray());
    }
    public async Task<HubReleaseInfo?> CheckHubUpdateAsync(string currentVersion, CancellationToken ct = default)
    {
        const string repository = "ThodProd/ThodHUB";
        Contract.Version(currentVersion);
        HubReleaseInfo? best = null;
        foreach(var release in await ReadReleasesAsync(repository, ct))
        {
            if(!Contract.IsNewer(release.Version, currentVersion)) continue;
            if(best is not null && !Contract.IsNewer(release.Version, best.Version)) continue;
            var descriptor = await TryReadDescriptorAsync(repository, release.Tag, ct);
            if(descriptor is null || descriptor.Version != release.Version) continue;
            Contract.Release(descriptor, repository);
            if(!IsRepoUrl(descriptor.PackageUrl, repository, "releases/download/")) continue;
            best = new HubReleaseInfo
            {
                Version = release.Version,
                Notes = descriptor.Notes,
                Size = descriptor.Size,
                DownloadUrl = descriptor.PackageUrl,
                ReleaseUrl = release.ReleaseUrl,
                PublishedAt = release.PublishedAt
            };
        }
        return best;
    }
    private static bool IsRepoUrl(string value, string repository, string path)
        => Uri.TryCreate(value, UriKind.Absolute, out var uri) && uri.Scheme == "https" && uri.Host == "github.com" && uri.IsDefaultPort && uri.UserInfo == "" && uri.Query == "" && uri.Fragment == "" && uri.AbsolutePath.StartsWith($"/{repository}/{path}", StringComparison.Ordinal);
    private sealed record AtomRelease(string Tag, string Version, string ReleaseUrl, DateTimeOffset? PublishedAt);
    public async Task<string> DownloadAsync(CatalogApp app, IProgress<double>? progress = null, CancellationToken ct = default)
    {
        var r = app.Release ?? throw new InvalidOperationException("Нет готового выпуска.");
        Contract.Release(r, app.Repository);
        var target = Path.Combine(_store.CacheRoot, Guid.NewGuid().ToString("N") + ".zip");
        try
        {
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
            timeout.CancelAfter(TimeSpan.FromMinutes(15));
            using var response = await _http.GetAsync(r.PackageUrl, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            response.EnsureSuccessStatusCode();
            if(response.Content.Headers.ContentLength is long length && length != r.Size) throw new InvalidDataException("Размер выпуска изменился.");
            await using var input = await response.Content.ReadAsStreamAsync(timeout.Token);
            await using var output = new FileStream(target, FileMode.CreateNew, FileAccess.Write, FileShare.None, 65536, true);
            var buffer = new byte[65536]; long received = 0; int n;var lastProgress=Environment.TickCount64;
            while((n = await input.ReadAsync(buffer, timeout.Token)) != 0)
            {
                received += n;
                if(received > r.Size) throw new InvalidDataException("Пакет превышает заявленный размер.");
                await output.WriteAsync(buffer.AsMemory(0, n), timeout.Token);
                if(received==r.Size||Environment.TickCount64-lastProgress>=100){progress?.Report((double)received / r.Size);lastProgress=Environment.TickCount64;}
            }
            if(received != r.Size) throw new InvalidDataException("Загрузка прервалась до конца файла.");
            return target;
        }
        catch { if(File.Exists(target)) File.Delete(target); throw; }
    }
    public static void Validate(Catalog catalog)
    {
        if(catalog.SchemaVersion != 1 || catalog.Apps.Count > 200) throw new InvalidDataException("Неподдерживаемый каталог.");
        var ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach(var app in catalog.Apps)
        {
            Contract.Id(app.Id); Contract.Repository(app.Repository);
            if(app.Mode is not ("embedded" or "standalone")) throw new InvalidDataException("Некорректный режим программы.");
            if(!ids.Add(app.Id)) throw new InvalidDataException("Повторяющийся идентификатор программы.");
            if(app.Name.Length is 0 or > 100 || app.Description.Length > 1000) throw new InvalidDataException("Некорректное описание программы.");
            if(app.Release is not null) Contract.Release(app.Release, app.Repository);
            if(app.ArtworkUrl is not null && (!Uri.TryCreate(app.ArtworkUrl, UriKind.Absolute, out var u) || u.Scheme != "https" || u.Host != "raw.githubusercontent.com")) throw new InvalidDataException("Изображения каталога должны храниться на GitHub.");
            if(app.IconUrl is not null && (!Uri.TryCreate(app.IconUrl, UriKind.Absolute, out var i) || i.Scheme != "https" || i.Host != "raw.githubusercontent.com" || !i.IsDefaultPort || i.UserInfo!="")) throw new InvalidDataException("Иконки каталога должны храниться на GitHub.");
        }
    }
    public void Dispose() => _http.Dispose();
}
