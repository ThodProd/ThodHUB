# Проверка текущей сборки

`dotnet run --project tests/ThodHub.Tests`: 20 проверок установки, SHA-256, путей ZIP, BOM, отката,
повторной установки, удаления, блокировки файлов, сохранения данных и резервных копий.
`dotnet build src/ThodHub -c Release`: сборка Windows, без предупреждений.
`node --check src/ThodHub/wwwroot/app.js`: синтаксис интерфейса.

Пакеты ThodMedia 1.0.1 и ThodView 1.0.2 проверены настоящим установщиком в отдельной тестовой папке.
Интерфейс проверяется в родном WebView2. Размер скачивания — отдельный текст рядом с версией.
Не проверялись реальные VPN-подключения без профиля, все датчики оборудования и все внешние MIDI-устройства.
Windows требует подтверждать ассоциации файлов в системном интерфейсе.

## ThodHUB 0.1.2 (27.10.2026)
22 core tests passed. In a separate local app home: GitHub update CopyAll 1.1.0 -> 1.1.1 changed the running worker and versioned UI origin, preserved settings and registered both context menu commands. Actual --invoke copied recursive paths; clipboard restored afterward. Uninstall removed its owned registry keys and retained config. Embedded --launch activated the existing hub. Viewer 1.0.5 registration/default-settings CLI exits 0; supported OpenWithProgids entries verified. Canvas WebView regression: visible text outside prior selection, layers first, contextual panels, Ctrl+Z and Ctrl+Shift+Z with Russian key values. No screenshots or broad image/hardware tests.
Viewer 1.0.4 was withdrawn from stable discovery after native invocation exposed WPF StartupUri=null; immutable corrected release is 1.0.5.
