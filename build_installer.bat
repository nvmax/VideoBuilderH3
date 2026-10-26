@echo off
rem Builds the installer. Double-click it or run it from a terminal. It copies the node pack's current Video Builder files first, committed or not. Add -NoSync to skip that.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build_installer.ps1" %*
set "code=%errorlevel%"
rem A double-click closes the window when the script ends, so keep it open there. Explorer starts cmd with the batch file's full path, a terminal does not.
echo %cmdcmdline% | "%SystemRoot%\System32\find.exe" /i "%~f0" >nul
if not errorlevel 1 pause
exit /b %code%
