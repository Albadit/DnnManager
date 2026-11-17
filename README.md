# DNN Manager

DNN Manager runs DNN (DotNetNuke) websites on your PC's IIS. It creates them from a DNN release or from a backup,
clones, backs up, restores, upgrades and removes them, keeps them warm, and shows each site's state, logs, settings
and database in one window. It is written in Rust with [GPUI](https://gpui.rs), Zed's UI framework.

## Download

Every [release](https://github.com/Albadit/DnnManager.NET/releases) has a package per system:

| System | Package |
|---|---|
| Windows 10/11, x64 | `DnnManager-windows-x86_64.msi` |
| Windows 11, ARM | `DnnManager-windows-arm64.msi` |
| macOS, Apple silicon | `DnnManager-macos-arm64.dmg` |
| macOS, Intel | `DnnManager-macos-x86_64.dmg` |
| Debian / Ubuntu, x64 | `DnnManager-linux-amd64.deb` (also as `-x86_64.deb`) |
| Debian / Ubuntu, ARM | `DnnManager-linux-arm64.deb` (also as `-aarch64.deb`) |

`SHA256SUMS.txt` lists every file's checksum. The packages aren't code-signed yet: Windows SmartScreen and macOS
Gatekeeper ask before the first start.

**IIS is Windows only**, so managing websites needs Windows. On macOS and Linux DNN Manager opens and says IIS
isn't available there - see the [user guide](.docs/user-guide.md#on-macos-and-linux).

## Build from source

```powershell
cargo build -p dnnmanager      # target\debug\DnnManager.exe
cargo test --workspace         # no IIS, SQL Server or administrator rights needed
```

Prerequisites, running against a stand-in IIS and the project's layout: [.docs/development.md](.docs/development.md).

## Documentation

| | |
|---|---|
| [User guide](.docs/user-guide.md) | Installing, the pages, settings, troubleshooting |
| [Development](.docs/development.md) | Setting up, building, running, debugging, where code goes |
| [Architecture](.docs/architecture.md) | Crates, data flow, IIS, shared data, decisions |
| [Design system](.docs/design-system.md) | Colours, sizes and controls |
| [Testing](.docs/testing.md) | The tests, the stand-in IIS, what was checked by hand |
| [Releasing](.docs/releasing.md) | Versions, release notes, the release workflow and its packages |

## License

[MIT](LICENSE)
