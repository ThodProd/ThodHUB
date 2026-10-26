using System.IO.Compression;
using System.Security.Cryptography;

namespace ThodHub.Core;

public sealed class PackageInstaller(HubStore store)
{
    private readonly SemaphoreSlim _gate = new(1, 1);
    public async Task<ModuleManifest> InstallAsync(string archivePath, CatalogApp app, CancellationToken ct = default)
    {
        Contract.Id(app.Id);
        var release = app.Release ?? throw new InvalidOperationException("Совместимый выпуск пока не опубликован.");
        Contract.Release(release, app.Repository);
        await _gate.WaitAsync(ct);
        var staging = Path.Combine(store.AppsRoot, ".staging-" + Guid.NewGuid().ToString("N"));
        try
        {
            await using var input = File.OpenRead(archivePath);
            if (input.Length != release.Size) throw new InvalidDataException("Размер скачанного файла не совпадает с выпуском.");
            var hash = Convert.ToHexString(await SHA256.HashDataAsync(input, ct));
            if (!hash.Equals(release.Sha256, StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("Пакет повреждён: SHA-256 не совпадает.");
            input.Position = 0;
            using var zip = new ZipArchive(input, ZipArchiveMode.Read);
            if (zip.Entries.Count > 20000) throw new InvalidDataException("Слишком много файлов в пакете.");
            Directory.CreateDirectory(staging);
            long expanded = 0;
            var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var entry in zip.Entries)
            {
                ct.ThrowIfCancellationRequested();
                // ZIP writers on Windows may expose backslash separators. Normalize before validation,
                // never after it: '..\\outside' must still be rejected.
                var normalized = entry.FullName.Replace('\\','/');
                var name = normalized.TrimEnd('/');
                string target;
                try { target=Contract.Inside(staging,name); }
                catch(InvalidDataException e) { throw new InvalidDataException("Недопустимый путь в ZIP: "+System.Text.Json.JsonSerializer.Serialize(entry.FullName)+". "+e.Message,e); }
                if(!seen.Add(target)) throw new InvalidDataException("Повторяющийся путь в архиве.");
                var unixKind = (entry.ExternalAttributes >> 16) & 0xF000;
                if (unixKind == 0xA000 || (entry.ExternalAttributes & (int)FileAttributes.ReparsePoint) != 0) throw new InvalidDataException("Ссылки внутри пакета запрещены.");
                expanded = checked(expanded + entry.Length);
                if (expanded > 2 * Contract.MaxPackageBytes) throw new InvalidDataException("Распакованный пакет слишком большой.");
                if(normalized.EndsWith('/')) { Directory.CreateDirectory(target); continue; }
                Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                await using var source = entry.Open();
                await using var output = new FileStream(target, FileMode.CreateNew, FileAccess.Write);
                await source.CopyToAsync(output, ct);
            }
            var manifestPath = Path.Combine(staging, "manifest.json");
            if (!File.Exists(manifestPath) || new FileInfo(manifestPath).Length > 65536) throw new InvalidDataException("В корне пакета нужен manifest.json до 64 КБ.");
            var m = HubJson.Read<ModuleManifest>(await File.ReadAllTextAsync(manifestPath, ct));
            Contract.Manifest(m);
            if (m.Id != app.Id || m.Version != release.Version || m.Mode != app.Mode) throw new InvalidDataException("Пакет не соответствует карточке каталога.");
            if ((m.Mode == "embedded" && !File.Exists(Contract.Inside(staging, m.Ui!))) ||
                (m.Launch is not null && !File.Exists(Contract.Inside(staging,m.Launch.Executable))) ||
                (m.Cleanup is not null && !File.Exists(Contract.Inside(staging,m.Cleanup.Executable))) ||
                (m.Worker is not null && !File.Exists(Contract.Inside(staging, m.Worker.Executable)))) throw new InvalidDataException("В пакете отсутствует точка входа.");
            var dest = store.AppVersion(app.Id, release.Version);
            Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
            var replaced=Path.Combine(store.AppsRoot,".replace-"+Guid.NewGuid().ToString("N"));
            if(Directory.Exists(dest))
            {
                ManagedFiles.AssertUnlocked(store.AppsRoot,dest);
                Directory.Move(dest,replaced);
            }
            try
            {
                Directory.Move(staging, dest);
                await store.ChangeAsync(s =>
                {
                    var old = s.Installed.SingleOrDefault(x => x.Id == app.Id);
                    s.Installed.RemoveAll(x => x.Id == app.Id);
                    s.Installed.Add(new() { Id = app.Id, Version = release.Version, PreviousVersion = old?.Version==release.Version?old.PreviousVersion:old?.Version });
                    if(!s.SidebarOrder.Contains(app.Id)) s.SidebarOrder.Add(app.Id);
                });
            }
            catch
            {
                if(Directory.Exists(dest))ManagedFiles.DeleteTree(store.AppsRoot,dest);
                if(Directory.Exists(replaced))Directory.Move(replaced,dest);
                throw;
            }
            if(Directory.Exists(replaced))ManagedFiles.DeleteTree(store.AppsRoot,replaced);
            return m;
        }
        finally
        {
            if(Directory.Exists(staging)) Directory.Delete(staging, true);
            _gate.Release();
        }
    }
    public async Task RollbackAsync(string id)
    {
        await _gate.WaitAsync();
        try
        {
            var old = store.Settings.Installed.Single(x => x.Id == id);
            if(old.PreviousVersion is null) throw new InvalidOperationException("Нет предыдущей версии.");
            var m = HubJson.Read<ModuleManifest>(await File.ReadAllTextAsync(Path.Combine(store.AppVersion(id, old.PreviousVersion), "manifest.json")));
            Contract.Manifest(m);
            if(m.Id != id || m.Version != old.PreviousVersion) throw new InvalidDataException("Повреждена предыдущая версия.");
            await store.ChangeAsync(s => { s.Installed.RemoveAll(x => x.Id == id); s.Installed.Add(old with { Version = old.PreviousVersion, PreviousVersion = old.Version }); });
        }
        finally { _gate.Release(); }
    }
    public async Task UninstallAsync(string id, bool deleteData = false)
    {
        Contract.Id(id);
        await _gate.WaitAsync();
        try
        {
            if(!store.Settings.Installed.Any(x => x.Id == id)) throw new InvalidOperationException("Программа уже удалена.");
            var appPath=ManagedFiles.Check(store.AppsRoot,Path.Combine(store.AppsRoot,id));
            var dataPath=ManagedFiles.Check(store.DataRoot,Path.Combine(store.DataRoot,id));
            try
            {
                ManagedFiles.AssertUnlocked(store.AppsRoot,appPath);
                if(deleteData) ManagedFiles.AssertUnlocked(store.DataRoot,dataPath);
            }
            catch(IOException e) { throw new IOException("Закройте программу и её файлы перед удалением. " + e.Message,e); }
            var trash=ManagedFiles.Check(store.AppsRoot,Path.Combine(store.AppsRoot,".remove-"+Guid.NewGuid().ToString("N")));
            var dataTrash=ManagedFiles.Check(store.DataRoot,Path.Combine(store.DataRoot,".remove-"+Guid.NewGuid().ToString("N")));
            try
            {
                if(Directory.Exists(appPath)) Directory.Move(appPath,trash);
                if(deleteData && Directory.Exists(dataPath)) Directory.Move(dataPath,dataTrash);
                await store.ChangeAsync(s => { s.Installed.RemoveAll(x => x.Id == id); s.SidebarOrder.RemoveAll(x => x == id); });
            }
            catch
            {
                if(Directory.Exists(trash)) Directory.Move(trash,appPath);
                if(Directory.Exists(dataTrash)) Directory.Move(dataTrash,dataPath);
                throw;
            }
            ManagedFiles.DeleteTree(store.AppsRoot,trash);
            if(deleteData) ManagedFiles.DeleteTree(store.DataRoot,dataTrash);
        }
        finally { _gate.Release(); }
    }
}
