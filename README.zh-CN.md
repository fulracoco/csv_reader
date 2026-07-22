# CSV Reader

[English](README.md) | [简体中文](README.zh-CN.md)

CSV Reader 是一款跨平台桌面应用，用于读取、搜索、编辑和导出大型分隔文本文件。项目基于原生 egui 窗口与 Rust，可在不把全部文件数据载入物理内存的情况下流畅处理数 GB 文件。

![CSV Reader 主界面](screenshot-main.png)

## 功能特性

- 使用内存映射访问文件，并在内存中维护行偏移索引
- 虚拟滚动，仅渲染当前可见行
- 支持按列筛选以及区分或忽略大小写的并行搜索
- 表格仅展示长内容预览，点击后可查看单元格完整内容
- 支持行列选择、单元格编辑和连续行范围导出
- 自动识别逗号、制表符、分号和竖线分隔符
- 自动识别 UTF-8、UTF-16 LE/BE、GBK/GB18030、Big5、日韩编码及常见传统单字节编码

![CSV Reader 搜索结果](screenshot-search.png)

## 下载与安装

请从 [GitHub Releases](https://github.com/fulracoco/csv_reader/releases) 下载对应平台的安装包。

### Windows

大多数电脑请选择 `x64`，Windows ARM 设备请选择 `arm64`。原生界面不依赖 WebView2 运行时。

### macOS

Apple Silicon（M1 及更新机型）请选择 `aarch64.dmg`，Intel Mac 请选择 `x64.dmg`。目前 GitHub 生成的 macOS 包未经过 Apple 签名和公证，因此系统可能提示“无法验证开发者”或“应用已损坏”。

将 **CSV Reader** 拖入“应用程序”后，先按住 Control 点击应用并选择“打开”。如果仍被 Gatekeeper 阻止，请执行：

```bash
xattr -dr com.apple.quarantine "/Applications/CSV Reader.app"
```

仅对从本仓库官方 Releases 页面下载的安装包移除隔离属性。

### Linux

请选择与处理器架构匹配的安装包：Debian 系发行版使用 `.deb`，便携运行使用 `.AppImage`。首次运行 AppImage 前需添加执行权限：

```bash
chmod +x CSV.Reader_*.AppImage
```

## 使用方法

| 操作 | 方法 |
|---|---|
| 打开文件 | 点击 **Open** 或按 `Ctrl/Cmd+O` |
| 搜索 | 按 `Ctrl/Cmd+F`，输入关键词并选择列，然后按 `Enter` |
| 查看单元格 | 点击单元格，在侧边面板查看完整内容 |
| 编辑单元格 | 双击单元格，或打开详情后点击 **Edit** |
| 选择行或列 | 点击行号或表头；Shift 连续选择，Ctrl/Cmd 切换选择 |
| 导出 | 点击 **Export**，选择列和连续行范围后保存 |
| 调整显示密度 | 在 **Density** 菜单中选择行高 |

## 开发环境

### 前置依赖

- [Rust](https://www.rust-lang.org/tools/install) stable 工具链
- 对应平台的原生窗口依赖（Linux 构建需要 X11/Wayland 开发头文件）

Ubuntu/Debian 可安装与 CI 一致的依赖：

```bash
sudo apt install libx11-dev libxi-dev libxrandr-dev libxcursor-dev \
  libxinerama-dev libwayland-dev libxkbcommon-dev
```

### 本地运行

```bash
git clone https://github.com/fulracoco/csv_reader.git csv-reader
cd csv-reader
cargo run
```

### 常用命令

| 命令 | 用途 |
|---|---|
| `cargo run` | 启动原生 egui 开发版应用 |
| `cargo build --release` | 构建优化后的原生二进制文件 |
| 先运行 `cargo build --release`，再运行 `cargo packager --release` | 构建并打包当前平台 |
| 修改 `Cargo.toml` 中的 `version` | 更新唯一应用版本源 |
| `cargo test` | 编译并运行 Rust 测试 |

## 项目架构

| 路径 | 职责 |
|---|---|
| `src/app.rs` | 原生 egui 界面、虚拟表格、交互、搜索、编辑和导出 |
| `src/csv_engine.rs` | 内存映射、索引、解析、缓存、搜索、编辑和导出 |
| `icons/` | 桌面端及各平台应用图标 |
| `.github/workflows/build.yml` | 跨平台构建与 GitHub Release 发布 |

应用通过内存映射访问文件，并在首次扫描时记录每一行的字节偏移。可见行按需解析，并保留在 500 行缓存中。搜索会并行扫描各行，不创建持久搜索索引；界面最多返回 500 条结果。

## 性能与限制

- 已测试超过 2 GB、包含 1000 万行的数据文件。
- 不超过 4 GiB 的文件每个行偏移占 4 字节，更大的文件占 8 字节；1000 万行的索引约需 40 MB 或 80 MB 内存。
- 导出过程逐行写入带 BOM 的 UTF-8 CSV，不会在内存中保留完整导出数据。
- 编辑通过同目录临时文件流式重写，只在内存中处理目标行；替换期间需要接近源文件大小的可用磁盘空间。
- 索引与搜索速度取决于存储设备、编码、行宽和 CPU 资源。

## 许可证

MIT
