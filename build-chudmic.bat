@echo off
setlocal
title ChudMic builder
rem Builds ChudMic.exe (PC) and ChudMic.apk (Android).
rem Tools are downloaded once into .buildtools next to this file (about 1.5 GB). Nothing needs admin rights.

set "ROOT=%~dp0"
set "T=%ROOT%.buildtools"
if not exist "%T%" mkdir "%T%"
set "LOG=%ROOT%build-log.txt"
echo ChudMic build started %date% %time% > "%LOG%"

rem ================= 1. Rust (GNU toolchain, no Visual Studio needed) =================
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
  echo [1/6] Downloading Rust...
  curl -L --fail -o "%T%\rustup-init.exe" https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe || goto fail
  "%T%\rustup-init.exe" -y --default-host x86_64-pc-windows-gnu --profile minimal || goto fail
) else (
  echo [1/6] Rust already installed.
)

rem ---- MinGW: provides dlltool.exe, which the Rust GNU toolchain needs ----
if not exist "%T%\mingw64\bin\dlltool.exe" (
  echo [1/6] Downloading MinGW tools - 233 MB, one time only...
  curl -L --fail -o "%T%\mingw.zip" https://github.com/brechtsanders/winlibs_mingw/releases/download/14.3.0posix-12.0.0-msvcrt-r1/winlibs-x86_64-posix-seh-gcc-14.3.0-mingw-w64msvcrt-12.0.0-r1.zip || goto fail
  tar -xf "%T%\mingw.zip" -C "%T%" || goto fail
  del "%T%\mingw.zip"
)
set "PATH=%PATH%;%T%\mingw64\bin"

rem ================= 2. Build the PC app =================
echo [2/6] Building the PC app - first time takes a few minutes...
if not exist "%ROOT%pc\vendor\VBCABLE_Driver_Pack.zip" curl -L --fail -o "%ROOT%pc\vendor\VBCABLE_Driver_Pack.zip" https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip
pushd "%ROOT%pc"
cargo build --release || (popd & goto fail)
popd
copy /y "%ROOT%pc\target\release\chudmic.exe" "%ROOT%ChudMic.exe" >nul || goto fail
echo        OK: ChudMic.exe

rem ================= 3. Java 17 (portable) =================
if not exist "%T%\jdk\bin\java.exe" (
  echo [3/6] Downloading Java 17...
  curl -L --fail -o "%T%\jdk.zip" "https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jdk/hotspot/normal/eclipse" || goto fail
  tar -xf "%T%\jdk.zip" -C "%T%" || goto fail
  for /d %%d in ("%T%\jdk-17*") do move "%%d" "%T%\jdk" >nul
  del "%T%\jdk.zip"
) else (
  echo [3/6] Java already downloaded.
)
set "JAVA_HOME=%T%\jdk"
set "PATH=%JAVA_HOME%\bin;%PATH%"

rem ================= 4. Android SDK (command-line tools only) =================
set "SDK=%T%\android-sdk"
set "SDKM=%SDK%\cmdline-tools\latest\bin\sdkmanager.bat"
if not exist "%SDKM%" (
  echo [4/6] Downloading Android SDK tools...
  mkdir "%SDK%\cmdline-tools" 2>nul
  curl -L --fail -o "%T%\cmdtools.zip" https://dl.google.com/android/repository/commandlinetools-win-11076708_latest.zip || goto fail
  tar -xf "%T%\cmdtools.zip" -C "%SDK%\cmdline-tools" || goto fail
  move "%SDK%\cmdline-tools\cmdline-tools" "%SDK%\cmdline-tools\latest" >nul
  del "%T%\cmdtools.zip"
)
echo [4/6] Installing Android platform 35 and build tools...
(for /l %%i in (1,1,30) do @echo y) | call "%SDKM%" --sdk_root="%SDK%" --licenses >nul
call "%SDKM%" --sdk_root="%SDK%" "platforms;android-35" "build-tools;34.0.0" || goto fail
set "ANDROID_HOME=%SDK%"
set "ANDROID_SDK_ROOT=%SDK%"

rem ================= 5. Gradle =================
set "GRADLE=%T%\gradle-8.11.1\bin\gradle.bat"
if not exist "%GRADLE%" (
  echo [5/6] Downloading Gradle...
  curl -L --fail -o "%T%\gradle.zip" https://services.gradle.org/distributions/gradle-8.11.1-bin.zip || goto fail
  tar -xf "%T%\gradle.zip" -C "%T%" || goto fail
  del "%T%\gradle.zip"
) else (
  echo [5/6] Gradle already downloaded.
)

rem ================= 6. Build the Android app =================
echo [6/6] Building the Android app - first time takes several minutes...
pushd "%ROOT%android"
call "%GRADLE%" assembleRelease --no-daemon --console=plain || (popd & goto fail)
popd
copy /y "%ROOT%android\app\build\outputs\apk\release\app-release.apk" "%ROOT%ChudMic.apk" >nul || goto fail

echo.
echo ==========================================================
echo  DONE!  ChudMic.exe and ChudMic.apk are in:
echo  %ROOT%
echo ==========================================================
echo DONE >> "%LOG%"
start "" explorer "%ROOT%"
pause
exit /b 0

:fail
echo.
echo ==========================================================
echo  BUILD FAILED - scroll up to see the error.
echo ==========================================================
echo FAILED >> "%LOG%"
pause
exit /b 1
