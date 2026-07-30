@echo off
setlocal

cd /d "%~dp0"

where pnpm >nul 2>nul
if errorlevel 1 (
  echo SyncForge needs pnpm to run in development mode.
  echo Install pnpm or run "corepack enable", then try again.
  pause
  exit /b 1
)

pnpm start
if errorlevel 1 (
  echo.
  echo SyncForge did not start successfully.
  pause
  exit /b 1
)
