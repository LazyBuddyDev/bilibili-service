# B 站服务维护指南

Windows x64 独立视频服务，负责扫码登录、搜索、推荐、关注动态、分 P 与合集目录和受控 MP4。宿主负责媒体解码、呈现和用户界面。

[实现入口](README.md#实现入口) · [业务 API](docs/module-api.md) · [构建与打包](README.md)。源码、文档、开发方案和构建产物按[统一文件放置规范](https://github.com/LazyBuddyDev/LazyBuddy/blob/main/docs/file-layout.md)归位。

- 登录门槛、任务取消、账号和媒体会话按[方法契约](docs/module-api.md#methods)处理。隔离验证使用独立凭据作用域。
- 上游访问、凭据和私有回环资源遵循[网络与凭据边界](docs/module-api.md#network-and-credential-boundary)。资源权限与实际媒体呈现分别验证。
- 使用 [package.ps1](scripts/package.ps1) 生成不可变完整包。服务、样例和真实媒体的验收范围见[验证边界](docs/module-api.md#verification-boundary)。
