# HamokiSync

<img src="assets/hamokisync-green.png" width="80" alt="HamokiSync logo">

轻量级跨设备剪贴板同步客户端，使用 Rust 编写，兼容 SyncClipboard 服务端协议。

- 独立常驻后台与按需打开的设置窗口。
- 自动同步文字和 PNG 图片，不传输文件或文件夹。
- Ctrl + Alt + Z 恢复上一条剪贴板内容。
- 托盘双箭头显示连接状态：绿色正常，红色异常，灰色等待。
- 登录凭据通过系统凭据存储保存，服务器和用户名由用户自行配置。
- Windows、Linux（X11/XFCE 等支持 StatusNotifierItem 的桌面），以及 macOS 早期预览。

## 下载

前往 [GitHub Releases](https://github.com/xiaohouzidoudou/HamokiSync/releases) 获取 Windows x64 便携 ZIP、macOS Apple Silicon / Intel 通用 APP ZIP、Linux x86_64 TAR.GZ。各版本同时提供独立后台与设置二进制以及 SHA256 校验文件。首次使用推荐完整包。Mac 应用未经过 Apple 公证，仍需真机功能验收；Linux 需要 X11 及系统密钥存储服务。

## 使用

需要你自己部署或拥有一个兼容的 [SyncClipboard Server](https://github.com/Jeric-X/SyncClipboard)。
本项目只是独立客户端，不包含服务器，不提供公共同步服务。首次运行设置程序，输入自己的 HTTPS 服务器根地址、用户名和密码，点击保存。远程服务仅接受 HTTPS；本机回环地址可用 HTTP。设置窗口关闭后后台继续同步；托盘菜单可退出后台。

剪贴板可能含有隐私信息；只连接信任的服务器，服务端可能保存明文内容。不要将密码、令牌或私钥复制到启用同步的剪贴板。HTTPS 传输并不等于端到端加密。不要与其他自动同步客户端同时运行以避免冲突。

## 从源码构建

安装 Rust stable 和平台所需的编译工具。Linux 建议使用 X11 桌面，安装编译器、pkg-config、X11/XCB、OpenGL 和 D-Bus 相关开发库，确保系统凭据存储服务可用。

```sh
cargo build --locked --release --features settings --bin hamokisync-daemon --bin hamokisync-settings
cargo test --locked --features settings
```

将两个二进制放在同一目录，Windows 运行 hamokisync-settings.exe，Linux 运行 hamokisync-settings，macOS 解压并将 HamokiSync.app 移入 Applications 后打开。首次验证并保存成功后自动注册登录启动；移动目录后需重新保存。Linux 的桌面启动器可使用 assets/hamokisync-green.png。

目前是早期版本，平台兼容性与真实设备互传仍需持续测试。若要卸载，先从托盘停止后台，移除系统登录启动项，再删除程序目录；如需清除已保存凭据及设置，请使用系统凭据管理器和用户配置目录。

## License

MIT。第三方依赖遵循各自许可证。此项目为独立实现的客户端，不隶属于 SyncClipboard。
