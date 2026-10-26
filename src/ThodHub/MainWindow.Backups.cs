using System.IO;
using System.Text.Json;
using System.Diagnostics;
using ThodHub.Core;

namespace ThodHub;
public partial class MainWindow
{
    private readonly SemaphoreSlim _backupGate=new(1,1);
    private BackupStore BackupFiles=>new(_store);
    private GitHubCredentials BackupCredentials=>new(_store.Root);
    private GitHubBackups CloudBackups()=>new(_store.Settings.BackupRepository,BackupCredentials.Read());
    private async Task<object> BackupListAsync()
    {
        var local=BackupFiles.List();string? warning=null;
        if(BackupCredentials.Exists)
        {
            try
            {
                using var cloud=CloudBackups();var remote=await cloud.ListAsync();
                foreach(var entry in remote){var index=local.FindIndex(l=>l.Name==entry.Name);if(index>=0)local[index]=entry with{Local=true};else local.Add(entry);}
            }
            catch(Exception e){warning=e.Message;}
        }
        var ids=_store.Settings.Installed.Select(a=>a.Id).Concat(local.Select(a=>a.AppId)).Concat(Directory.EnumerateDirectories(_store.DataRoot).Select(Path.GetFileName).OfType<string>().Where(n=>!n.StartsWith('.'))).Append("thodhub-settings").Distinct().ToArray();
        return new{entries=local.OrderByDescending(x=>x.CreatedAt),apps=ids.Select(id=>new{id,name=id=="thodhub-settings"?"ThodHUB · оформление и порядок":_catalog.Apps.FirstOrDefault(a=>a.Id==id)?.Name??id}),connected=BackupCredentials.Exists,repository=_store.Settings.BackupRepository,localPath=BackupFiles.Root,warning};
    }
    private async Task<BackupEntry> CreateBackupAsync(string id,bool upload)
    {
        var result=await Task.Run(()=>BackupFiles.CreateAsync(id,_store.Settings.Installed.FirstOrDefault(a=>a.Id==id)?.Version));
        if(upload&&BackupCredentials.Exists)
        {
            Send(new{@event="notice",text="Локальная копия сохранена. Загружаем её в приватный GitHub…"});
            try{using var cloud=CloudBackups();await cloud.UploadAsync(BackupFiles.FilePath(result.Name));result=result with{Cloud=true};}
            catch(Exception e){throw new IOException("Локальная копия сохранена, но отправить в GitHub не удалось. "+e.Message);}
        }
        Send(new{@event="notice",text=""});return result;
    }
    private async Task<object?> HandleBackupAsync(string method,JsonElement p)
    {
        await _backupGate.WaitAsync();
        try
        {
            switch(method)
            {
                case "backups.list":return await BackupListAsync();
                case "backups.folder":Directory.CreateDirectory(BackupFiles.Root);Process.Start(new ProcessStartInfo(BackupFiles.Root){UseShellExecute=true});return true;
                case "backups.tokenPage":Process.Start(new ProcessStartInfo("https://github.com/settings/personal-access-tokens/new"){UseShellExecute=true});return true;
                case "backups.disconnect":BackupCredentials.Forget();return await BackupListAsync();
                case "backups.connect":
                case "backups.connectCli":
                    var repository=p.GetProperty("repository").GetString()!.Trim();Contract.Repository(repository);
                    var token=method=="backups.connectCli"?await GitHubCredentials.ReadCliAsync():p.GetProperty("token").GetString()!;
                    using(var cloud=new GitHubBackups(repository,token))await cloud.ValidatePrivateAsync();
                    BackupCredentials.Save(token);await _store.ChangeAsync(s=>s.BackupRepository=repository);return await BackupListAsync();
                case "backups.create":
                case "backups.restore":
                    var id=p.GetProperty("id").GetString()!;Contract.Id(id);
                    if(_downloads.ContainsKey(id)||_activeModule==id||!_removing.Add(id))throw new IOException("Сначала закройте программу и дождитесь окончания её установки.");
                    try
                    {
                        if(_workers.Remove(id,out var worker))await worker.DisposeAsync();
                        if(_store.Settings.Installed.Any(a=>a.Id==id))await Task.Run(()=>StandaloneLauncher.AssertNotRunning(_store,id));
                        if(method=="backups.create")return await CreateBackupAsync(id,true);
                        var name=p.GetProperty("name").GetString()!;var path=BackupFiles.FilePath(name);
                        if(!File.Exists(path))
                        {
                            using var cloud=CloudBackups();var entry=(await cloud.ListAsync()).SingleOrDefault(e=>e.Name==name&&e.AppId==id)??throw new IOException("Копия не найдена в GitHub.");
                            await cloud.DownloadAsync(entry,path);
                        }
                        await Task.Run(()=>BackupFiles.RestoreAsync(name,id));return Snapshot();
                    }
                    finally{_removing.Remove(id);}
                default:throw new InvalidOperationException("Неизвестная команда резервных копий.");
            }
        }
        finally{_backupGate.Release();}
    }
}
