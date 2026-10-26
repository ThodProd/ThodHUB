using System.Net.Http.Headers;
using System.Text.Json;

namespace ThodHub.Core;

public sealed class GitHubCatalog : IDisposable
{
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
        var url = $"https://api.github.com/repos/{repo}/contents/catalog.json?ref={Uri.EscapeDataString(_store.Settings.CatalogRef)}";
        var content = HubJson.Read<JsonElement>(await GetTextAsync(url, 2_000_000, ct));
        var raw = Convert.FromBase64String(content.GetProperty("content").GetString()!.Replace("\n", ""));
        var catalog = HubJson.Read<Catalog>(System.Text.Encoding.UTF8.GetString(raw));
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
                var releases = HubJson.Read<JsonElement>(await GetTextAsync($"https://api.github.com/repos/{app.Repository}/releases?per_page=10", 2_000_000, ct));
                var release = releases.EnumerateArray().FirstOrDefault(x => !x.GetProperty("draft").GetBoolean() && !x.GetProperty("prerelease").GetBoolean() && x.GetProperty("assets").EnumerateArray().Any(a => a.GetProperty("name").GetString() == "thodhub-release.json"));
                if(release.ValueKind == JsonValueKind.Undefined) { app.Release = null;app.ReleaseError="В последних стабильных выпусках нет файла thodhub-release.json. Автору нужно опубликовать пакет для хаба.";return; }
                var asset = release.GetProperty("assets").EnumerateArray().First(a => a.GetProperty("name").GetString() == "thodhub-release.json");
                var assetUrl = asset.GetProperty("browser_download_url").GetString()!;
                var expectedPrefix = $"https://github.com/{app.Repository}/releases/download/";
                if(!assetUrl.StartsWith(expectedPrefix, StringComparison.Ordinal)) throw new InvalidDataException("Некорректный адрес описания выпуска.");
                var descriptor = HubJson.Read<PackageRelease>(await GetTextAsync(assetUrl, 100_000, ct));
                Contract.Release(descriptor, app.Repository);
                app.Release = descriptor;
            }
            catch(Exception e) when(e is HttpRequestException or JsonException or InvalidDataException or FormatException or KeyNotFoundException or TaskCanceledException)
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
