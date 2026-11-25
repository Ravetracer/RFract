# Installation

Download the package for your system from the releases page:
<https://github.com/Ravetracer/RFract/releases>.

| System | Package |
|--------|---------|
| Linux x86_64 | `RFract-<version>-linux-x86_64.tar.gz` or `RFract-<version>-x86_64.AppImage` |
| Windows 10/11 x64 | `RFract-<version>-windows-x64.zip` |
| macOS, Apple silicon | `RFract-<version>-macos-arm64.zip` |
| macOS, Intel | `RFract-<version>-macos-x86_64.zip` |

Every package contains the program (`rfr-app`), the command line tool (`rfr-cli`), the
denoiser libraries and this manual as PDF. INSTALL.txt in each package lists the details.

## Linux

Unpack the archive anywhere and start `./rfr-app`. Keep the `lib/` folder next to the
programs: it holds the denoiser. The AppImage needs no unpacking: make it executable
(`chmod +x`) and run it; `… cli` as the first argument runs the command line tool.

## Windows

Unpack the zip and start `rfr-app.exe`. The programs are not signed: Windows SmartScreen may
show "Windows protected your PC" on the first start. Click "More info", then "Run anyway".

## macOS

Move `RFract.app` to the Applications folder. The app is not signed by Apple, so macOS
refuses to open it at first. Remove the quarantine flag once in the Terminal:

```
xattr -dr com.apple.quarantine /Applications/RFract.app
```

!!! note
    RFract renders on the graphics card (Vulkan, DirectX 12 or Metal). `rfr-cli info` shows
    which graphics card and denoiser it found.
