@echo off
setlocal
set "ROBOTUTOR_ROOT=%~dp0"
set "pointer=%LOCALAPPDATA%\Robotutor\desktop-release.txt"
if /i "%~1"=="--rebuild" goto build
if exist "%pointer%" goto launch
:build
powershell.exe -NoProfile -File "%~dp0tools\Publish-Windows.ps1"
if errorlevel 1 exit /b 1
:launch
set /p "release="<"%pointer%"
if not exist "%release%\robotutor-desktop.exe" (
    echo The Robotutor installation is incomplete. Run Robotutor.cmd --rebuild.
    exit /b 1
)
if /i "%~1"=="--rebuild" (
    start "" "%release%\robotutor-desktop.exe" %2 %3 %4 %5 %6 %7 %8 %9
) else (
    start "" "%release%\robotutor-desktop.exe" %*
)
