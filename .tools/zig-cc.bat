@echo off
setlocal EnableDelayedExpansion
set "ZIG=%~dp0zig-unzip\zig-windows-x86_64-0.14.0\zig.exe"
set "ARGS="
:loop
if "%~1"=="" goto run
set "A=%~1"
if /I "!A!"=="--target=x86_64-unknown-linux-gnu" (
  shift
  goto loop
)
if /I "!A!"=="--target" (
  shift
  if not "%~1"=="" shift
  goto loop
)
set ARGS=!ARGS! "!A!"
shift
goto loop
:run
"%ZIG%" cc -target x86_64-linux-gnu !ARGS!
