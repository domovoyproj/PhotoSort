@echo off
cd /d "%~dp0"
if exist .venv\Scripts\python.exe (
  .venv\Scripts\python.exe -m photosort.desktop
) else (
  python -m photosort.desktop
)
if errorlevel 1 pause
