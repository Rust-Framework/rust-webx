@echo off
setlocal
set "ZIG=%~dp0zig-unzip\zig-windows-x86_64-0.14.0\zig.exe"
"%ZIG%" c++ -target x86_64-linux-gnu %*