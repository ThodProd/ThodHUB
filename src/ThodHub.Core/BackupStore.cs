using System.IO.Compression;
using System.Text.Json;

namespace ThodHub.Core;

public sealed record BackupEntry(string Name,string AppId,DateTimeOffset CreatedAt,long Size,bool Local,bool Cloud,long? AssetId=null,string? Sha256=null);
public sealed record BackupMetadata(int SchemaVersion,string AppId,string? AppVersion,DateTimeOffset CreatedAt);
public sealed class BackupStore(HubStore store)
{
    public const long MaxBytes=256L*1024*1024;
    public string Root=>Path.Combine(store.Root,"Backups");
    private static readonly HashSet<string> Excluded=new(["cache","logs","temp","tmp","runtime"],StringComparer.OrdinalIgnoreCase);
    public static BackupEntry? Describe(string name,long size,bool local,bool cloud,long? assetId=null,string? sha256=null)
    {
        var parts=name.Split("--");
        if(parts.Length!=3||!name.EndsWith(".zip",StringComparison.Ordinal))return null;
        try{Contract.Id(parts[0]);}catch(InvalidDataException){return null;}
        if(!DateTimeOffset.TryParseExact(parts[1],"yyyyMMdd'T'HHmmssfff'Z'",System.Globalization.CultureInfo.InvariantCulture,System.Globalization.DateTimeStyles.AssumeUniversal,out var date))return null;
        if(parts[2].Length!=36||!Guid.TryParseExact(parts[2][..32],"N",out _))return null;
        return new(name,parts[0],date,size,local,cloud,assetId,sha256);
    }
    public List<BackupEntry> List()
    {
        Directory.CreateDirectory(Root);
        return Directory.EnumerateFiles(Root,"*.zip").Select(p=>Describe(Path.GetFileName(p),new FileInfo(p).Length,true,false)).OfType<BackupEntry>().OrderByDescending(x=>x.CreatedAt).ToList();
    }
    public string FilePath(string name)
    {
        if(Describe(name,0,true,false) is null||Path.GetFileName(name)!=name)throw new InvalidDataException("Некорректное имя резервной копии.");
        return Contract.Inside(Root,name);
    }
    public async Task<BackupEntry> CreateAsync(string id,string? version,CancellationToken ct=default)
    {
        Contract.Id(id);Directory.CreateDirectory(Root);
        var now=DateTimeOffset.UtcNow;var name=$"{id}--{now:yyyyMMddTHHmmssfffZ}--{Guid.NewGuid():N}.zip";
        var path=FilePath(name);var tmp=path+".partial";
        try
        {
            await using(var file=new FileStream(tmp,FileMode.CreateNew,FileAccess.ReadWrite,FileShare.None))
            using(var zip=new ZipArchive(file,ZipArchiveMode.Create,true))
            {
                using(var writer=new StreamWriter(zip.CreateEntry("backup.json").Open()))await writer.WriteAsync(HubJson.Write(new BackupMetadata(1,id,version,now)));
                if(id=="thodhub-settings")
                {
                    using var writer=new StreamWriter(zip.CreateEntry("data/preferences.json").Open());
                    await writer.WriteAsync(HubJson.Write(new {store.Settings.Theme,store.Settings.CheckOnStartup,store.Settings.SidebarOrder}));
                }
                else
                {
                    var source=store.DataDirectory(id);long total=0;int count=0;
                    var stack=new Stack<string>();stack.Push(source);
                    while(stack.TryPop(out var dir))foreach(var item in Directory.EnumerateFileSystemEntries(dir))
                    {
                        ct.ThrowIfCancellationRequested();ManagedFiles.Check(store.DataRoot,item);
                        var relative=Path.GetRelativePath(source,item).Replace('\\','/');
                        if(Excluded.Contains(relative.Split('/')[0]))continue;
                        var attrs=File.GetAttributes(item);
                        if((attrs&FileAttributes.ReparsePoint)!=0)throw new IOException("Резервная копия не обходит ссылки: "+relative);
                        if((attrs&FileAttributes.Directory)!=0){stack.Push(item);continue;}
                        if(++count>20000||(total+=new FileInfo(item).Length)>4*MaxBytes)throw new IOException("Конфиги превышают предел 1 ГБ / 20 000 файлов.");
                        await using var input=new FileStream(item,FileMode.Open,FileAccess.Read,FileShare.Read);
                        await using var output=zip.CreateEntry("data/"+relative,CompressionLevel.Optimal).Open();
                        await input.CopyToAsync(output,ct);
                    }
                }
            }
            if(new FileInfo(tmp).Length>MaxBytes)throw new IOException("Сжатая копия превышает 256 МБ.");
            File.Move(tmp,path);return Describe(name,new FileInfo(path).Length,true,false)!;
        }
        finally{if(File.Exists(tmp))File.Delete(tmp);}
    }
    public async Task RestoreAsync(string name,string id,CancellationToken ct=default)
    {
        Contract.Id(id);var source=FilePath(name);
        if(new FileInfo(source).Length>MaxBytes)throw new InvalidDataException("Копия слишком большая.");
        var staging=Path.Combine(store.CacheRoot,"restore-"+Guid.NewGuid().ToString("N"));Directory.CreateDirectory(staging);
        try
        {
            using(var zip=ZipFile.OpenRead(source))
            {
                var metaEntry=zip.GetEntry("backup.json")??throw new InvalidDataException("В архиве нет описания резервной копии.");
                if(metaEntry.Length>65536||zip.Entries.Count>20001)throw new InvalidDataException("Некорректный архив.");
                using var reader=new StreamReader(metaEntry.Open());var meta=HubJson.Read<BackupMetadata>(await reader.ReadToEndAsync(ct));
                if(meta.SchemaVersion!=1||meta.AppId!=id)throw new InvalidDataException("Копия относится к другой программе.");
                var seen=new HashSet<string>(StringComparer.OrdinalIgnoreCase);long total=0;
                foreach(var entry in zip.Entries)
                {
                    ct.ThrowIfCancellationRequested();if(entry.FullName=="backup.json")continue;
                    var normalized=entry.FullName.Replace('\\','/');
                    if(!normalized.StartsWith("data/",StringComparison.Ordinal)||normalized.EndsWith('/'))throw new InvalidDataException("Некорректный путь в копии.");
                    var target=Contract.Inside(staging,normalized[5..]);
                    if(!seen.Add(target)||((entry.ExternalAttributes>>16)&0xf000)==0xa000||(entry.ExternalAttributes&(int)FileAttributes.ReparsePoint)!=0||(total+=entry.Length)>4*MaxBytes)throw new InvalidDataException("Некорректный архив резервной копии.");
                    Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                    await using var input=entry.Open();await using var output=new FileStream(target,FileMode.CreateNew,FileAccess.Write);await input.CopyToAsync(output,ct);
                }
            }
            if(id=="thodhub-settings")
            {
                var prefs=HubJson.Read<HubSettings>(await File.ReadAllTextAsync(Path.Combine(staging,"preferences.json"),ct));
                if(prefs.Theme is not("light" or "dark" or "system"))throw new InvalidDataException("Некорректная тема в копии.");
                await CreateAsync(id,null,ct);
                await store.ChangeAsync(s=>{s.Theme=prefs.Theme;s.CheckOnStartup=prefs.CheckOnStartup;s.SidebarOrder=prefs.SidebarOrder.Where(id=>s.Installed.Any(a=>a.Id==id)).Concat(s.Installed.Select(a=>a.Id)).Distinct().ToList();});return;
            }
            // Preserve a dated snapshot of the current data before replacing it.
            await CreateAsync(id,store.Settings.Installed.FirstOrDefault(a=>a.Id==id)?.Version,ct);
            var destination=ManagedFiles.Check(store.DataRoot,store.DataDirectory(id));ManagedFiles.AssertUnlocked(store.DataRoot,destination);
            var old=Path.Combine(store.DataRoot,".restore-old-"+Guid.NewGuid().ToString("N"));
            Directory.Move(destination,old);
            try{Directory.Move(staging,destination);}catch{Directory.Move(old,destination);throw;}
            ManagedFiles.DeleteTree(store.DataRoot,old);
        }
        finally{if(Directory.Exists(staging))ManagedFiles.DeleteTree(store.CacheRoot,staging);}
    }
}
