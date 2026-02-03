@echo off
setlocal enabledelayedexpansion

set "REPO=sfc-gh-kkeller/snowflake_cortex_ai_proxy"
set "APP=cortex-proxy"
set "BIN_DIR=%USERPROFILE%\bin"
set "CONFIG_DIR=%USERPROFILE%\.config\cortex-proxy"
set "CONFIG_FILE=%CONFIG_DIR%\config.toml"
set "EXAMPLE_CONFIG=cortex-proxy.example.toml"

where powershell >nul 2>&1
if errorlevel 1 (
  echo PowerShell is required.
  exit /b 1
)

set "ARCH=x64"
if /i "%PROCESSOR_ARCHITECTURE%"=="ARM64" set "ARCH=arm64"

for /f "usebackq delims=" %%T in (`powershell -NoProfile -Command "(Invoke-RestMethod https://api.github.com/repos/%REPO%/releases/latest).tag_name"`) do set "TAG=%%T"
if "%TAG%"=="" (
  echo Failed to determine latest release tag.
  exit /b 1
)

set "ASSET=%APP%-v%TAG:~1%-windows-%ARCH%.zip"
set "URL=https://github.com/%REPO%/releases/download/%TAG%/%ASSET%"

set "TMP_DIR=%TEMP%\cortex-proxy-install"
if exist "%TMP_DIR%" rmdir /s /q "%TMP_DIR%"
mkdir "%TMP_DIR%"

echo Downloading %URL%
powershell -NoProfile -Command "Invoke-WebRequest -Uri '%URL%' -OutFile '%TMP_DIR%\%ASSET%'" || exit /b 1

mkdir "%BIN_DIR%" 2>nul
powershell -NoProfile -Command "Expand-Archive -Path '%TMP_DIR%\%ASSET%' -DestinationPath '%TMP_DIR%' -Force" || exit /b 1
copy /y "%TMP_DIR%\%APP%.exe" "%BIN_DIR%\%APP%.exe" >nul

echo %PATH% | find /i "%BIN_DIR%" >nul
if errorlevel 1 (
  setx PATH "%BIN_DIR%;%PATH%" >nul
  echo Added %BIN_DIR% to PATH. Open a new terminal.
)

mkdir "%CONFIG_DIR%" 2>nul
if not exist "%CONFIG_FILE%" (
  copy /y "%EXAMPLE_CONFIG%" "%CONFIG_FILE%" >nul
  echo Wrote sample config to %CONFIG_FILE%
)

echo.
echo Next steps:
echo 1^) Edit %CONFIG_FILE% and set:
echo    - snowflake.base_url (your account URL)
echo    - snowflake.pat (Programmatic Access Token)
echo    - snowflake.default_model (e.g. claude-opus-4-5)
echo 2^) Run: %APP% --config %CONFIG_FILE%
echo 3^) Test: curl http://localhost:8766/
echo.

endlocal
