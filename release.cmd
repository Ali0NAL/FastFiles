@echo off
rem FastFiles yayın kısayolu.
rem Windows .ps1 betiklerini varsayılan olarak engeller ("running scripts is disabled").
rem Bu dosya kısıtlamayı yalnızca bu çalıştırma için atlar; sistem ayarı değişmez.
rem
rem Kullanım:   .\release.cmd -Version 1.4.1
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0release.ps1" %*
exit /b %ERRORLEVEL%
