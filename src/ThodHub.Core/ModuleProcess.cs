using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace ThodHub.Core;

public sealed class ModuleProcess : IAsyncDisposable
{
    private readonly Process _process;
    private readonly SemaphoreSlim _writeGate = new(1, 1);
    private readonly ConcurrentDictionary<string, TaskCompletionSource<JsonElement>> _pending = new();
    private readonly CancellationTokenSource _lifetime = new();
    private readonly Task _reader;
    private readonly Task _errors;
    public bool IsRunning => !_process.HasExited;
    public event Action<string>? Diagnostic;
    public ModuleProcess(string executable, IEnumerable<string> arguments, string workingDirectory, string dataDirectory)
    {
        var start = new ProcessStartInfo(executable)
        {
            WorkingDirectory = workingDirectory, UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true,
            StandardInputEncoding = new UTF8Encoding(false), StandardOutputEncoding = new UTF8Encoding(false), StandardErrorEncoding = new UTF8Encoding(false)
        };
        foreach(var a in arguments) start.ArgumentList.Add(a);
        start.Environment["THODHUB_DATA_DIR"] = dataDirectory;
        start.Environment["THODHUB_API"] = "1";
        start.Environment["THODHUB_LAUNCHER"] = Environment.ProcessPath;
        _process = Process.Start(start) ?? throw new InvalidOperationException("Не удалось запустить утилиту.");
        _reader = ReadAsync();
        _errors = ReadErrorsAsync();
    }
    public async Task<JsonElement> CallAsync(string method, JsonElement? parameters = null, CancellationToken ct = default)
    {
        if(_process.HasExited) throw new InvalidOperationException("Процесс утилиты завершён.");
        var id = Guid.NewGuid().ToString("N");
        var completion = new TaskCompletionSource<JsonElement>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = completion;
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct, _lifetime.Token);
        timeout.CancelAfter(TimeSpan.FromSeconds(30));
        using var registration = timeout.Token.Register(() => completion.TrySetCanceled(timeout.Token));
        try
        {
            var request = JsonSerializer.Serialize(new { jsonrpc = "2.0", id, method, @params = parameters });
            await _writeGate.WaitAsync(timeout.Token);
            try { await _process.StandardInput.WriteLineAsync(request.AsMemory(), timeout.Token); await _process.StandardInput.FlushAsync(timeout.Token); }
            finally { _writeGate.Release(); }
            return await completion.Task;
        }
        finally { _pending.TryRemove(id, out _); }
    }
    private async Task ReadAsync()
    {
        try
        {
            await foreach(var line in ReadBoundedLinesAsync(_process.StandardOutput, _lifetime.Token))
            {
                using var doc = JsonDocument.Parse(line);
                var root = doc.RootElement;
                if(!root.TryGetProperty("id", out var idNode) || idNode.ValueKind != JsonValueKind.String) continue;
                if(!_pending.TryGetValue(idNode.GetString()!, out var pending)) continue;
                if(root.TryGetProperty("error", out var error)) pending.TrySetException(new InvalidOperationException(error.GetProperty("message").GetString()));
                else if(root.TryGetProperty("result", out var result)) pending.TrySetResult(result.Clone());
                else pending.TrySetException(new InvalidDataException("Некорректный ответ утилиты."));
            }
        }
        catch(Exception e) when(e is IOException or JsonException or OperationCanceledException or InvalidDataException) { Diagnostic?.Invoke(e.Message); }
        finally { foreach(var p in _pending.Values) p.TrySetException(new IOException("Связь с утилитой завершилась.")); }
    }
    private async Task ReadErrorsAsync()
    {
        try { await foreach(var line in ReadBoundedLinesAsync(_process.StandardError, _lifetime.Token)) Diagnostic?.Invoke(line); }
        catch(Exception e) when(e is IOException or OperationCanceledException or InvalidDataException) { }
    }
    private static async IAsyncEnumerable<string> ReadBoundedLinesAsync(StreamReader reader, [System.Runtime.CompilerServices.EnumeratorCancellation] CancellationToken ct)
    {
        var line = new StringBuilder(); var buffer = new char[4096]; int count;
        while((count = await reader.ReadAsync(buffer.AsMemory(), ct).ConfigureAwait(false)) != 0)
        {
            for(var i=0;i<count;i++)
            {
                if(buffer[i]=='\n') { yield return line.ToString().TrimEnd('\r'); line.Clear(); continue; }
                if(line.Length>=8*1024*1024) throw new InvalidDataException("Ответ утилиты превышает 8 МБ.");
                line.Append(buffer[i]);
            }
        }
        if(line.Length>0) yield return line.ToString();
    }
    public async ValueTask DisposeAsync()
    {
        if(!_process.HasExited)
        {
            try { _process.StandardInput.Close(); await _process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(2)); }
            catch(TimeoutException) { if(!_process.HasExited) _process.Kill(true); }
            catch(InvalidOperationException) { }
        }
        await _lifetime.CancelAsync();
        try { await Task.WhenAll(_reader, _errors); } catch(OperationCanceledException) { }
        _process.Dispose(); _lifetime.Dispose(); _writeGate.Dispose();
    }
}
