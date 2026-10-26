using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace ThodHub;
public sealed class GitHubCredentials(string root)
{
    private string PathName=>Path.Combine(root,"Secrets","github.bin");
    public bool Exists=>File.Exists(PathName);
    public void Save(string token)
    {
        token=token.Trim();if(token.Length<20||token.Length>500||token.Any(char.IsWhiteSpace))throw new IOException("Некорректный токен GitHub.");
        Directory.CreateDirectory(Path.GetDirectoryName(PathName)!);File.WriteAllBytes(PathName,Transform(Encoding.UTF8.GetBytes(token),true));
    }
    public string Read()=>Encoding.UTF8.GetString(Transform(File.ReadAllBytes(PathName),false));
    public void Forget(){if(Exists)File.Delete(PathName);}
    public static async Task<string> ReadCliAsync()
    {
        var candidates=(Environment.GetEnvironmentVariable("PATH")??"").Split(Path.PathSeparator).Where(p=>p.Length>0).Select(p=>Path.Combine(p,"gh.exe")).ToList();
        // Development checkout can use its already-authorized local CLI installation.
        for(var p=new DirectoryInfo(AppContext.BaseDirectory);p is not null;p=p.Parent)candidates.Add(Path.Combine(p.FullName,".tools","gh","bin","gh.exe"));
        var exe=candidates.FirstOrDefault(File.Exists)??throw new IOException("GitHub CLI не найден. Введите токен только для приватного репозитория в поле ниже.");
        using var process=Process.Start(new ProcessStartInfo(exe,"auth token --hostname github.com"){UseShellExecute=false,CreateNoWindow=true,RedirectStandardOutput=true,RedirectStandardError=true})!;
        var output=process.StandardOutput.ReadToEndAsync();var error=process.StandardError.ReadToEndAsync();
        try{await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(15));}catch(TimeoutException){process.Kill();throw new IOException("GitHub CLI не ответил.");}
        await error;if(process.ExitCode!=0)throw new IOException("В GitHub CLI нет сохранённого входа. Выполните gh auth login или укажите токен.");
        return (await output).Trim();
    }
    private static byte[] Transform(byte[] bytes,bool protect)
    {
        var input=new Blob{Length=bytes.Length,Data=Marshal.AllocHGlobal(bytes.Length)};Blob output=default;
        try
        {
            Marshal.Copy(bytes,0,input.Data,bytes.Length);
            var ok=protect?CryptProtectData(ref input,null,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,1,out output):CryptUnprotectData(ref input,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,1,out output);
            if(!ok)throw new Win32Exception(Marshal.GetLastWin32Error(),"Не удалось открыть сохранённый вход GitHub. Подключите его заново.");
            var result=new byte[output.Length];Marshal.Copy(output.Data,result,0,result.Length);return result;
        }
        finally{Marshal.Copy(new byte[bytes.Length],0,input.Data,bytes.Length);Marshal.FreeHGlobal(input.Data);if(output.Data!=IntPtr.Zero)LocalFree(output.Data);System.Security.Cryptography.CryptographicOperations.ZeroMemory(bytes);}
    }
    [StructLayout(LayoutKind.Sequential)]private struct Blob{public int Length;public IntPtr Data;}
    [DllImport("crypt32.dll",CharSet=CharSet.Unicode,SetLastError=true)][return:MarshalAs(UnmanagedType.Bool)]private static extern bool CryptProtectData(ref Blob input,string? description,IntPtr entropy,IntPtr reserved,IntPtr prompt,uint flags,out Blob output);
    [DllImport("crypt32.dll",SetLastError=true)][return:MarshalAs(UnmanagedType.Bool)]private static extern bool CryptUnprotectData(ref Blob input,IntPtr description,IntPtr entropy,IntPtr reserved,IntPtr prompt,uint flags,out Blob output);
    [DllImport("kernel32.dll")]private static extern IntPtr LocalFree(IntPtr value);
}
