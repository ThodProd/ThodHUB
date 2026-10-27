using Microsoft.Win32;
using System.IO;
using ThodHub.Core;

namespace ThodHub;

public static class WindowsInstallation
{
    private const string BaseKey=@"Software\Microsoft\Windows\CurrentVersion\Uninstall\";
    private static string Owner(HubStore store)=>store.Root;
    public static void Register(HubStore store,string id,string name)
    {
        var m=store.Manifest(id);var keyName=BaseKey+"ThodHUB."+id;
        using var existing=Registry.CurrentUser.OpenSubKey(keyName);
        if(existing is not null&&!string.Equals(existing.GetValue("ThodHUB.Owner") as string,Owner(store),StringComparison.OrdinalIgnoreCase))
            throw new IOException("Программа уже зарегистрирована другим экземпляром хаба.");
        using var key=Registry.CurrentUser.CreateSubKey(keyName);
        key.SetValue("ThodHUB.Owner",Owner(store));key.SetValue("DisplayName",name);
        key.SetValue("DisplayVersion",m.Version);key.SetValue("Publisher","ThodProd");
        var directory=store.AppVersion(id,m.Version);key.SetValue("InstallLocation",directory);
        key.SetValue("UninstallString",$"\"{Environment.ProcessPath}\" --uninstall {id}");
        key.SetValue("NoModify",1,RegistryValueKind.DWord);key.SetValue("NoRepair",1,RegistryValueKind.DWord);
        var exe=m.Launch??m.Worker;
        key.SetValue("DisplayIcon",$"\"{(exe is null?Environment.ProcessPath:Contract.Inside(directory,exe.Executable))}\",0");
    }
    public static void Remove(HubStore store,string id)
    {
        Contract.Id(id);var path=BaseKey+"ThodHUB."+id;
        using var key=Registry.CurrentUser.OpenSubKey(path);
        if(string.Equals(key?.GetValue("ThodHUB.Owner") as string,Owner(store),StringComparison.OrdinalIgnoreCase))
            Registry.CurrentUser.DeleteSubKeyTree(path,false);
    }
}
