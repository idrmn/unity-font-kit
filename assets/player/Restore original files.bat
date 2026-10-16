@echo off
rem A PowerShell 7 window passes its PSModulePath on, and Windows PowerShell then cannot load its own modules.
set "PSModulePath="
if not exist "%~dp0ufk-patch\patch.ps1" (
    echo Unpack the whole archive first, then run this file from the unpacked folder.
    pause
    exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0ufk-patch\patch.ps1" restore
pause
