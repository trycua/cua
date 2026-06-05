@echo off
set GEMINI_API_KEY=AIzaSyAKuZUJgRj-kGt_au2Vkz1JC2S4xOzY7lk

REM Install dependencies if needed
if not exist node_modules (
    echo Installing dependencies...
    call bun install
)

bun ui.js
