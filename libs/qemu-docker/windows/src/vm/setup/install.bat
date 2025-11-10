@echo off

SET ScriptFolder=\\host.lan\Data
SET LogFile=%ScriptFolder%\ps_script_log.txt

echo Running PowerShell script... > %LogFile%

:: Check for PowerShell availability
where powershell >> %LogFile% 2>&1
if %ERRORLEVEL% neq 0 (
    echo PowerShell is not available! >> %LogFile%
    echo PowerShell is not available!
    exit /b 1
)

:: Add a 30-second delay
echo Waiting for 30 seconds before continuing... >> %LogFile%
timeout /t 30 /nobreak >> %LogFile% 2>&1

:: Run PowerShell script with ExecutionPolicy Bypass and log errors
echo Running setup.ps1... >> %LogFile%

powershell -ExecutionPolicy Bypass -File "%ScriptFolder%\setup.ps1" >> %LogFile% 2>&1

if %ERRORLEVEL% neq 0 (
    echo An error occurred. See %LogFile% for details.
) else (
    echo PowerShell script has completed successfully.
)

echo PowerShell script has completed.