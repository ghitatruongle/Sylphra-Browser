@echo off
setlocal
set "APP_EXE=%~dp0sylphra.exe"
if not exist "%APP_EXE%" (
  echo ERROR: sylphra.exe was not found next to this launcher.
  exit /b 1
)
start "" "%APP_EXE%"
