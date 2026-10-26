using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using System.Security.Cryptography;

namespace ThodHub.Core;
public sealed class GitHubBackups(string repository,string token):IDisposable
{
    private readonly HttpClient _http=new(new HttpClientHandler{AllowAutoRedirect=false}){Timeout=TimeSpan.FromMinutes(15)};
    private const string Tag="thodhub-backups-v1";
    private async Task<HttpResponseMessage> RequestAsync(HttpMethod method,string url,HttpContent? content=null,bool binary=false)
    {
        var u=new Uri(url);if(u.Scheme!="https"||u.Host is not("api.github.com" or "uploads.github.com"))throw new InvalidDataException("Некорректный адрес GitHub API.");
        using var request=new HttpRequestMessage(method,u){Content=content};
        request.Headers.Authorization=new AuthenticationHeaderValue("Bearer",token);request.Headers.UserAgent.ParseAdd("ThodHUB/0.1.0");request.Headers.Accept.ParseAdd(binary?"application/octet-stream":"application/vnd.github+json");
        var response=await _http.SendAsync(request,HttpCompletionOption.ResponseHeadersRead);
        if(!response.IsSuccessStatusCode&&response.StatusCode!=HttpStatusCode.Found&&response.StatusCode!=HttpStatusCode.NotFound)
        {var code=(int)response.StatusCode;response.Dispose();throw new IOException($"GitHub: HTTP {code}. Проверьте доступ к приватному репозиторию и сеть.");}
        return response;
    }
    private static StringContent Json(object value)=>new(HubJson.Write(value),Encoding.UTF8,"application/json");
    public async Task ValidatePrivateAsync()
    {
        Contract.Repository(repository);
        using var response=await RequestAsync(HttpMethod.Get,$"https://api.github.com/repos/{repository}");
        if(response.StatusCode==HttpStatusCode.NotFound)throw new IOException("Приватный репозиторий не найден или нет доступа.");
        var data=HubJson.Read<JsonElement>(await response.Content.ReadAsStringAsync());
        if(!data.GetProperty("private").GetBoolean())throw new IOException("Конфиги разрешено отправлять только в приватный репозиторий.");
    }
    private async Task<long?> ReleaseAsync(bool create)
    {
        using var response=await RequestAsync(HttpMethod.Get,$"https://api.github.com/repos/{repository}/releases/tags/{Tag}");
        if(response.StatusCode==HttpStatusCode.NotFound)
        {
            if(!create)return null;
            using var posted=await RequestAsync(HttpMethod.Post,$"https://api.github.com/repos/{repository}/releases",Json(new{tag_name=Tag,name="ThodHUB · история конфигураций",body="Private immutable dated configuration snapshots.",draft=false,prerelease=false}));
            posted.EnsureSuccessStatusCode();return HubJson.Read<JsonElement>(await posted.Content.ReadAsStringAsync()).GetProperty("id").GetInt64();
        }
        response.EnsureSuccessStatusCode();return HubJson.Read<JsonElement>(await response.Content.ReadAsStringAsync()).GetProperty("id").GetInt64();
    }
    public async Task<List<BackupEntry>> ListAsync()
    {
        await ValidatePrivateAsync();var id=await ReleaseAsync(false);if(id is null)return [];
        var result=new List<BackupEntry>();
        for(var page=1;page<=1000;page++)
        {
            using var response=await RequestAsync(HttpMethod.Get,$"https://api.github.com/repos/{repository}/releases/{id}/assets?per_page=100&page={page}");response.EnsureSuccessStatusCode();
            var assets=HubJson.Read<JsonElement>(await response.Content.ReadAsStringAsync());
            foreach(var a in assets.EnumerateArray())
            {
                var digest=a.TryGetProperty("digest",out var d)&&d.ValueKind==JsonValueKind.String?d.GetString():null;
                var entry=BackupStore.Describe(a.GetProperty("name").GetString()!,a.GetProperty("size").GetInt64(),false,true,a.GetProperty("id").GetInt64(),digest?.StartsWith("sha256:")==true?digest[7..]:null);
                if(entry is not null)result.Add(entry);
            }
            if(assets.GetArrayLength()<100)return result;
        }
        throw new IOException("Слишком большая история резервных копий.");
    }
    public async Task UploadAsync(string path)
    {
        await ValidatePrivateAsync();var id=await ReleaseAsync(true);
        await using var stream=File.OpenRead(path);if(stream.Length>BackupStore.MaxBytes)throw new IOException("Резервная копия превышает 256 МБ.");
        using var content=new StreamContent(stream);content.Headers.ContentType=new MediaTypeHeaderValue("application/zip");content.Headers.ContentLength=stream.Length;
        using var response=await RequestAsync(HttpMethod.Post,$"https://uploads.github.com/repos/{repository}/releases/{id}/assets?name={Uri.EscapeDataString(Path.GetFileName(path))}",content);response.EnsureSuccessStatusCode();
    }
    public async Task DownloadAsync(BackupEntry entry,string path)
    {
        await ValidatePrivateAsync();if(entry.AssetId is null||entry.Size>BackupStore.MaxBytes)throw new IOException("Некорректная копия.");
        using var response=await RequestAsync(HttpMethod.Get,$"https://api.github.com/repos/{repository}/releases/assets/{entry.AssetId}",binary:true);
        HttpResponseMessage? redirected=null;HttpClient? download=null;
        try
        {
            var source=response;
            if(response.StatusCode==HttpStatusCode.Found)
            {
                var location=response.Headers.Location;
                if(location is null||location.Scheme!="https"||!(location.Host.EndsWith(".githubusercontent.com",StringComparison.Ordinal)||location.Host=="release-assets.githubusercontent.com"))throw new IOException("Некорректное перенаправление GitHub.");
                download=new HttpClient{Timeout=TimeSpan.FromMinutes(15)};redirected=await download.GetAsync(location,HttpCompletionOption.ResponseHeadersRead);source=redirected;
            }
            source.EnsureSuccessStatusCode();var temp=path+".partial";
            try
            {
                await using(var input=await source.Content.ReadAsStreamAsync())
                await using(var output=new FileStream(temp,FileMode.CreateNew,FileAccess.Write))
                {
                    byte[] buffer=new byte[65536];long total=0;int count;
                    while((count=await input.ReadAsync(buffer))>0){total+=count;if(total>entry.Size||total>BackupStore.MaxBytes)throw new IOException("Размер копии изменился.");await output.WriteAsync(buffer.AsMemory(0,count));}
                    if(total!=entry.Size)throw new IOException("Копия скачана не полностью.");
                }
                if(entry.Sha256 is not null){await using var input=File.OpenRead(temp);if(!Convert.ToHexString(await SHA256.HashDataAsync(input)).Equals(entry.Sha256,StringComparison.OrdinalIgnoreCase))throw new IOException("SHA-256 резервной копии не совпадает.");}
                File.Move(temp,path);
            }
            finally{if(File.Exists(temp))File.Delete(temp);}
        }
        finally{redirected?.Dispose();download?.Dispose();}
    }
    public void Dispose()=>_http.Dispose();
}
