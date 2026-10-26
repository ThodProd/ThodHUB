using System.IO.Compression;
using System.Security.Cryptography;
using System.Text.Json;
using ThodHub.Core;

if(args.Length==4&&args[0]=="--validate-package")
{
    var temporary=Path.Combine(Path.GetTempPath(),"thodhub-validation-"+Guid.NewGuid().ToString("N"));
    try
    {
        var release=HubJson.Read<PackageRelease>(await File.ReadAllTextAsync(args[2]));
        using var zip=ZipFile.OpenRead(args[1]);using var reader=new StreamReader((zip.GetEntry("manifest.json")??throw new InvalidDataException("manifest.json должен быть в корне ZIP.")).Open());
        var manifest=HubJson.Read<ModuleManifest>(await reader.ReadToEndAsync());
        var app=new CatalogApp{Id=manifest.Id,Name=manifest.Id,Description="Package validation",Repository=args[3],Mode=manifest.Mode,Release=release};
        await new PackageInstaller(new HubStore(temporary)).InstallAsync(args[1],app);
        Console.WriteLine($"VALID: {manifest.Id} {manifest.Version}, {release.Size} bytes, SHA-256 and extraction verified.");return;
    }
    finally{if(Path.GetFullPath(temporary).StartsWith(Path.GetFullPath(Path.GetTempPath()),StringComparison.OrdinalIgnoreCase)&&Path.GetFileName(temporary).StartsWith("thodhub-validation-")&&Directory.Exists(temporary))Directory.Delete(temporary,true);}
}

if(args.Length > 0 && args[0] is "--seed" or "--install-local")
{
    var store = new HubStore(args[1]);
    var catalog = HubJson.Read<Catalog>(await File.ReadAllTextAsync(args[3]));
    var id = args.Length > 4 ? args[4] : "copyall";
    var app = catalog.Apps.Single(x => x.Id == id);
    using var zip = ZipFile.OpenRead(args[2]);
    using var reader = new StreamReader(zip.GetEntry("manifest.json")!.Open());
    var manifest = HubJson.Read<ModuleManifest>(await reader.ReadToEndAsync());
    app.Release = await Describe(args[2], manifest.Version, app.Repository);
    await new PackageInstaller(store).InstallAsync(args[2], app);
    if(args[0]=="--seed")
    {
        await HubStore.WriteAtomicAsync(Path.Combine(store.CacheRoot, "catalog.json"), HubJson.Write(catalog));
        await store.ChangeAsync(s => s.CheckOnStartup = false);
    }
    Console.WriteLine("Installed " + id); return;
}
var root = Path.Combine(Path.GetTempPath(), "thodhub-tests-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(root);
int passed = 0;
try
{
    await Test("Reject path traversal, Windows alternate streams and reserved names", () =>
    {
        foreach(var bad in new[] { "../out.exe", "a/../../out.exe", "C:/out", "a:stream", "a\\b", "nul.txt", "a./x", "a /x", "/root" })
            ExpectThrows(() => Contract.Inside(root, bad));
        return Task.CompletedTask;
    });
    await Test("Semantic version comparison, malformed versions rejected", () =>
    {
        Assert(Contract.IsNewer("1.10.0", "1.9.0")); Assert(!Contract.IsNewer("1.0.0", "2.0.0")); ExpectThrows(() => Contract.Version("1.02.0"));
        return Task.CompletedTask;
    });
    await Test("JSON accepts UTF-8 BOM from PowerShell writers", () =>
    {
        Assert(HubJson.Read<Catalog>("\uFEFF{\"schemaVersion\":1,\"apps\":[]}").Apps.Count==0);
        return Task.CompletedTask;
    });
    await Test("Windows ZIP separators are normalized before traversal checks", async () =>
    {
        var zipPath=await MakePackage("1.0.0", "ui\\nested\\readme.txt");
        var state=new HubStore(Path.Combine(root,"windows-zip"));
        var candidate=new CatalogApp{Id="test-app",Name="Test",Description="Test",Repository="ThodProd/Test",Release=await Describe(zipPath,"1.0.0","ThodProd/Test")};
        await new PackageInstaller(state).InstallAsync(zipPath,candidate);
        Assert(File.Exists(Path.Combine(state.AppVersion("test-app","1.0.0"),"ui","nested","readme.txt")));
        var evil=await MakePackage("1.1.0","..\\outside.txt");
        await ExpectThrowsAsync(()=>new PackageInstaller(state).InstallAsync(evil,candidate with {Release=Describe(evil,"1.1.0",candidate.Repository).Result}));
        Assert(state.Settings.Installed.Single().Version=="1.0.0");
    });
    var store = new HubStore(Path.Combine(root, "state"));
    var installer = new PackageInstaller(store);
    var package = await MakePackage("1.0.0");
    var app = new CatalogApp { Id = "test-app", Name = "Test", Description = "Test", Repository = "ThodProd/Test", Release = await Describe(package, "1.0.0", "ThodProd/Test") };
    await Test("Install, persisted state and independent configuration", async () =>
    {
        await installer.InstallAsync(package, app);
        await store.SaveModuleSettingsAsync("test-app", JsonSerializer.SerializeToElement(new { value = "preserve" }));
        Assert(new HubStore(store.Root).Settings.Installed.Single().Version == "1.0.0");
    });
    await Test("Checksum mismatch leaves active installation unchanged", async () =>
    {
        var bad = app with { Release = app.Release! with { Sha256 = new string('0',64) } };
        await ExpectThrowsAsync(() => installer.InstallAsync(package, bad));
        Assert(store.Manifest("test-app").Version == "1.0.0");
    });
    await Test("Traversal archive rejected before activation", async () =>
    {
        var evil = await MakePackage("1.1.0", "../escaped.txt");
        await ExpectThrowsAsync(() => installer.InstallAsync(evil, app with { Release = Describe(evil,"1.1.0",app.Repository).Result }));
        Assert(!File.Exists(Path.Combine(store.AppsRoot,"escaped.txt")));
        Assert(store.Settings.Installed.Single().Version == "1.0.0");
    });
    await Test("Incompatible API cannot replace a working version", async () =>
    {
        var bad = await MakePackage("1.1.0", hubApi: 999);
        await ExpectThrowsAsync(() => installer.InstallAsync(bad, app with { Release = Describe(bad,"1.1.0",app.Repository).Result }));
        Assert(store.Manifest("test-app").Version == "1.0.0");
    });
    await Test("Update and rollback preserve module data", async () =>
    {
        var next = await MakePackage("1.1.0");
        await installer.InstallAsync(next, app with { Release = await Describe(next,"1.1.0",app.Repository) });
        Assert(store.Manifest("test-app").Version == "1.1.0");
        await installer.RollbackAsync("test-app");
        Assert(store.Manifest("test-app").Version == "1.0.0");
        Assert(store.LoadModuleSettings("test-app")!.Value.GetProperty("value").GetString() == "preserve");
    });
    await Test("Concurrent settings writes retain unrelated changes", async () =>
    {
        await Task.WhenAll(store.ChangeAsync(s => s.Theme = "dark"), store.ChangeAsync(s => s.CheckOnStartup = false));
        var persisted = new HubStore(store.Root);
        Assert(persisted.Settings.Theme == "dark" && !persisted.Settings.CheckOnStartup);
    });
    await Test("Catalog rejects duplicates and foreign release repositories", () =>
    {
        ExpectThrows(() => GitHubCatalog.Validate(new Catalog { Apps = [app,app] }));
        ExpectThrows(() => Contract.Release(app.Release! with { PackageUrl = "https://github.com/another/repo/releases/download/v1/pkg.zip" },app.Repository));
        return Task.CompletedTask;
    });
    await Test("Removal keeps configuration", async () =>
    {
        await installer.UninstallAsync("test-app");
        Assert(store.Settings.Installed.Count == 0);
        Assert(!Directory.Exists(Path.Combine(store.AppsRoot,"test-app")));
        Assert(store.LoadModuleSettings("test-app")!.Value.GetProperty("value").GetString() == "preserve");
    });
    await Test("Reinstall after removal succeeds and reuses data", async () =>
    {
        await installer.InstallAsync(package,app);
        Assert(store.Manifest("test-app").Version=="1.0.0");
        Assert(store.LoadModuleSettings("test-app")!.Value.GetProperty("value").GetString()=="preserve");
    });
    await Test("Locked file prevents removal without partial deletion", async () =>
    {
        var entry=Path.Combine(store.AppVersion("test-app","1.0.0"),"ui","index.html");
        using var locked=File.Open(entry,FileMode.Open,FileAccess.Read,FileShare.None);
        bool failed=false;
        try { await installer.UninstallAsync("test-app",true); } catch(IOException) { failed=true; }
        Assert(failed && store.Settings.Installed.Count==1 && File.Exists(entry));
        Assert(store.LoadModuleSettings("test-app") is not null);
    });
    await Test("Full removal deletes versions and app data only", async () =>
    {
        await store.SaveModuleSettingsAsync("other-app",JsonSerializer.SerializeToElement(new { keep=true }));
        await installer.UninstallAsync("test-app",true);
        Assert(!Directory.Exists(Path.Combine(store.AppsRoot,"test-app")));
        Assert(!Directory.Exists(Path.Combine(store.DataRoot,"test-app")));
        Assert(store.LoadModuleSettings("other-app")!.Value.GetProperty("keep").GetBoolean());
    });
    await Test("Standalone manifest validates exe without web UI", () =>
    {
        Contract.Manifest(new ModuleManifest { Id="native-app",Version="1.0.0",Mode="standalone",Launch=new WorkerDefinition { Executable="app/Native.exe" } });
        ExpectThrows(()=>Contract.Manifest(new ModuleManifest {Id="bad-app",Version="1.0.0",Mode="standalone"}));
        ExpectThrows(()=>Contract.Manifest(new ModuleManifest {Id="bad-app",Version="1.0.0",Mode="standalone",Launch=new WorkerDefinition { Executable="../Native.exe" }}));
        return Task.CompletedTask;
    });
    await Test("Dated backups preserve multiple snapshots and restore only chosen app", async () =>
    {
        var backups=new BackupStore(store);
        await store.SaveModuleSettingsAsync("backup-app",JsonSerializer.SerializeToElement(new{value="first"}));
        var first=await backups.CreateAsync("backup-app","1.0.0");
        await store.SaveModuleSettingsAsync("backup-app",JsonSerializer.SerializeToElement(new{value="second"}));
        var second=await backups.CreateAsync("backup-app","1.0.0");
        Assert(first.Name!=second.Name&&backups.List().Count==2);
        await backups.RestoreAsync(first.Name,"backup-app");
        Assert(store.LoadModuleSettings("backup-app")!.Value.GetProperty("value").GetString()=="first");
        Assert(backups.List().Count==3&&store.LoadModuleSettings("other-app")!.Value.GetProperty("keep").GetBoolean());
        await ExpectThrowsAsync(()=>backups.RestoreAsync(first.Name,"other-app"));
    });
    await Test("Backup restore rejects traversal without changing current data",async()=>
    {
        var backups=new BackupStore(store);var entry=await backups.CreateAsync("backup-app","1.0.0");
        using(var zip=ZipFile.Open(backups.FilePath(entry.Name),ZipArchiveMode.Update))using(var writer=new StreamWriter(zip.CreateEntry("data/../../escape.txt").Open()))await writer.WriteAsync("bad");
        await ExpectThrowsAsync(()=>backups.RestoreAsync(entry.Name,"backup-app"));
        Assert(store.LoadModuleSettings("backup-app")!.Value.GetProperty("value").GetString()=="first");
    });
    await Test("Hub preference backup never restores installed program registrations",async()=>
    {
        var backups=new BackupStore(store);await store.ChangeAsync(s=>s.Theme="dark");
        var entry=await backups.CreateAsync("thodhub-settings",null);await store.ChangeAsync(s=>s.Theme="light");
        await backups.RestoreAsync(entry.Name,"thodhub-settings");Assert(store.Settings.Theme=="dark"&&store.Settings.Installed.Count==0);
    });
    await Test("Repair and update after rollback replace a retained version without losing data", async () =>
    {
        var state=new HubStore(Path.Combine(root,"repair"));var installer=new PackageInstaller(state);
        var v1=await MakePackage("1.0.0");var v2=await MakePackage("1.1.0");
        var app=new CatalogApp{Id="test-app",Name="Test",Description="Test",Repository="ThodProd/Test",Release=await Describe(v1,"1.0.0","ThodProd/Test")};
        await installer.InstallAsync(v1,app);await File.WriteAllTextAsync(Path.Combine(state.DataDirectory(app.Id),"keep.txt"),"keep");
        app=app with{Release=await Describe(v2,"1.1.0",app.Repository)};
        await installer.InstallAsync(v2,app);await installer.RollbackAsync(app.Id);await installer.InstallAsync(v2,app);
        await installer.InstallAsync(v2,app);
        var current=state.Settings.Installed.Single();Assert(current.Version=="1.1.0"&&current.PreviousVersion=="1.0.0");
        Assert(await File.ReadAllTextAsync(Path.Combine(state.DataDirectory(app.Id),"keep.txt"))=="keep");
    });
    Console.WriteLine($"PASS: {passed} tests");
}
finally
{
    if(Path.GetFullPath(root).StartsWith(Path.GetFullPath(Path.GetTempPath()), StringComparison.OrdinalIgnoreCase) && Path.GetFileName(root).StartsWith("thodhub-tests-")) Directory.Delete(root,true);
}
async Task Test(string name, Func<Task> action) { await action(); passed++; Console.WriteLine("PASS " + name); }
async Task<string> MakePackage(string version, string? extraPath = null, int hubApi = 1)
{
    var path = Path.Combine(root,Guid.NewGuid()+".zip");
    using(var zip = ZipFile.Open(path,ZipArchiveMode.Create))
    {
        using(var w = new StreamWriter(zip.CreateEntry("manifest.json").Open())) await w.WriteAsync(HubJson.Write(new ModuleManifest { Id="test-app", Version=version, Ui="ui/index.html", HubApi=hubApi }));
        using(var w = new StreamWriter(zip.CreateEntry("ui/index.html").Open())) await w.WriteAsync("<!doctype html><title>Test</title>");
        if(extraPath is not null) { using var w = new StreamWriter(zip.CreateEntry(extraPath).Open()); await w.WriteAsync("outside"); }
    }
    return path;
}
static async Task<PackageRelease> Describe(string path, string version, string repo)
{
    await using var file = File.OpenRead(path);
    return new PackageRelease { Version=version, PackageUrl=$"https://github.com/{repo}/releases/download/v{version}/module.zip", Size=file.Length, Sha256=Convert.ToHexString(await SHA256.HashDataAsync(file)), Notes="Проверочный пакет." };
}
static void Assert(bool condition) { if(!condition) throw new Exception("Assertion failed"); }
static void ExpectThrows(Action action) { try { action(); } catch(InvalidDataException) { return; } throw new Exception("Expected InvalidDataException"); }
static async Task ExpectThrowsAsync(Func<Task> action) { try { await action(); } catch(InvalidDataException) { return; } throw new Exception("Expected InvalidDataException"); }
