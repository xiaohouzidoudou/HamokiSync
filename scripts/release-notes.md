## HamokiSync v0.1.0 — first public release

轻量级 Rust 剪贴板同步客户端。需要自行配置可信的 SyncClipboard 兼容服务器，本项目不提供公共服务器。

### 下载 / Downloads

| 系统 | 推荐下载 |
| --- | --- |
| Windows 10/11 x64 | HamokiSync-windows-x64.zip |
| macOS 12+（Apple Silicon / Intel）| HamokiSync-macos-universal.zip |
| Linux x86_64（X11，glibc 2.35+）| HamokiSync-linux-x64.tar.gz |

另提供各系统独立的 daemon / settings 二进制和 SHA256SUMS.txt。独立 daemon 需要已经配置的服务器与系统凭据；首次使用推荐完整包。

- Windows：解压后运行 hamokisync-settings.exe；保留同目录下的后台程序。未购买商业代码签名证书。
- macOS：解压，移动 HamokiSync.app 到 Applications，再通过 Finder 打开。使用临时签名，未经过 Apple 公证；系统可能提示确认。Mac 是早期预览，菜单栏、登录启动及剪贴板互传尚需真机验收。
- Linux：解压后运行 hamokisync-settings，或执行 install.sh 安装当前用户桌面入口。不需要 sudo。需要图形会话和系统密钥存储服务。
- 所有平台：请只同步到可信服务器；HTTPS 不等于端到端加密，避免同步敏感凭据。

GitHub Actions 通过各平台自动化测试与构建后发布；并不代表所有平台已完成实机互传验收。
