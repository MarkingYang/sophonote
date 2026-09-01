@echo off
setlocal EnableExtensions
set "HERE=%~dp0.."
for %%I in ("%HERE%") do set "HERE=%%~fI"
set "PYTHONHOME=%HERE%\python"
set "PYTHONPATH=%HERE%\site-packages"
set "PYTHONDONTWRITEBYTECODE=1"
if exist "%HERE%\python\python.exe" (
  set "PY=%HERE%\python\python.exe"
) else (
  set "PY=%HERE%\python\bin\python.exe"
)
"%PY%" "%~dp0hermes_watchdog.py" %*
exit /b %ERRORLEVEL%
