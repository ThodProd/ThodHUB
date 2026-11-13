# ThodHUB (Rust)

Rust-порт оболочки ThodHUB на `tao` + `wry` (WebView2). Интерфейс, мост
`chrome.webview`, установка программ, резервные копии, обновления и печать
повторяют C#-версию. Дополнительно программы, файлы которых удалены с диска,
остаются в библиотеке с пометкой «Файлы не найдены» и кнопками «Скачать» и
«Удалить запись».

## Сборка

```powershell
.\build.ps1
```

Скрипт прогоняет `cargo test`, собирает release, добавляет
`WebView2Loader.dll`, `wwwroot`, `catalog.json` и `agent-prompt.md` и кладёт
ZIP в `artifacts\release`. Для сборки нужны LLVM-MinGW и Rust, пути к которым
уже прописаны в `.cargo\config.toml` и `build.ps1`.

## Запуск для проверки

```powershell
$env:THODHUB_HOME = "$env:TEMP\ThodHubRustTest"
.\dist\ThodHUB.exe
```

`THODHUB_HOME` изолирует данные от установленного C#-хаба, поэтому обе версии
можно запускать одновременно. Без переменной используется тот же каталог, что
и у C#-хаба (`%LOCALAPPDATA%\ThodHUB`), и срабатывает общий single-instance
mutex.

## Тесты

```powershell
cargo test
```

Покрыты проверки путей и манифестов, установка/обновление/откат, проверка
SHA-256, отклонение path traversal, резервные копии, распаковка обновления и
определение отсутствующих программ.

## Отладочный self-test

```powershell
.\target\debug\ThodHUB.exe --selftest "$env:TEMP\ThodHubSelfTest" `
  --catalog "$env:TEMP\catalog.json" thodtorrent
```

Без списка id проверяет весь каталог: скачивает, устанавливает, проверяет
наличие файлов, имитирует пропажу папки и удаляет такую программу.
