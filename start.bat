@echo off
cd /d "%~dp0"
if exist dist\PhotoSort-0.2.1\PhotoSort.exe (
  start "PhotoSort" dist\PhotoSort-0.2.1\PhotoSort.exe
) else (
  cargo run --release
)
if errorlevel 1 pause
