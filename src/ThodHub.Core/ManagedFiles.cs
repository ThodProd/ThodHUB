namespace ThodHub.Core;

public static class ManagedFiles
{
    public static string Check(string root, string path)
    {
        var parent = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar);
        var full = Path.GetFullPath(path);
        if(!full.StartsWith(parent + Path.DirectorySeparatorChar,StringComparison.OrdinalIgnoreCase)) throw new IOException("Операция выходит за пределы папки хаба.");
        for(var current = Path.GetDirectoryName(full); current is not null && current.Length >= parent.Length; current = Path.GetDirectoryName(current))
        {
            if(Directory.Exists(current) && (File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0) throw new IOException("Папка хаба не должна проходить через символическую ссылку.");
        }
        return full;
    }
    public static void AssertUnlocked(string root, string path)
    {
        path = Check(root,path);
        if(!Directory.Exists(path)) return;
        if((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0) throw new IOException("Каталог программы заменён ссылкой. Удаление отменено.");
        foreach(var item in Directory.EnumerateFileSystemEntries(path))
        {
            Check(root,item);
            var attrs = File.GetAttributes(item);
            if((attrs & FileAttributes.ReparsePoint) != 0) continue;
            if((attrs & FileAttributes.Directory) != 0) AssertUnlocked(root,item);
            else { using var handle = File.Open(item,FileMode.Open,FileAccess.Read,FileShare.None); }
        }
    }
    public static void DeleteTree(string root, string path)
    {
        path = Check(root,path);
        if(!Directory.Exists(path)) return;
        if((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0) { Directory.Delete(path); return; }
        foreach(var item in Directory.EnumerateFileSystemEntries(path))
        {
            Check(root,item);
            var attrs=File.GetAttributes(item);
            if((attrs & FileAttributes.Directory) != 0) DeleteTree(root,item);
            else { if((attrs & FileAttributes.ReadOnly)!=0) File.SetAttributes(item,attrs & ~FileAttributes.ReadOnly);File.Delete(item); }
        }
        Directory.Delete(path);
    }
}
