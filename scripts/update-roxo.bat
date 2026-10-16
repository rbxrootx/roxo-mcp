@echo off
setlocal
set ROXO_REPO=C:\SideProjects\GithubRepo\roxo-mcp
set BLACKPINE=C:\Users\Elisha Trice\Documents\ChatGPT\Rbxl2Rojo\BlackpineGame
set TDS=C:\SideProjects\GithubRepo\tower-defense-simulator-n2c2-transparency

echo Stopping running roxo serve sessions...
taskkill /F /IM roxo.exe >nul 2>&1
timeout /t 2 /nobreak >nul

echo Building and installing roxo from %ROXO_REPO%...
cd /d "%ROXO_REPO%"
git submodule update --init --recursive || goto :fail
cargo install --path . --locked --force || goto :fail

echo Installing the Studio plugin...
roxo plugin install || goto :fail

echo Restarting serve sessions...
start "Roxo Blackpine Lobby" /d "%BLACKPINE%" cmd /k roxo serve lobby.project.json --port 34374
start "Roxo Blackpine Game" /d "%BLACKPINE%" cmd /k roxo serve gameplay.project.json --port 34375
start "Roxo TDS" /d "%TDS%" cmd /k roxo serve default.project.json --port 34376

timeout /t 5 /nobreak >nul
roxo sessions
echo.
echo Done. Studio reconnects on its own within a few seconds.
pause
exit /b 0

:fail
echo.
echo Update failed. Fix the error above and run this again.
pause
exit /b 1
