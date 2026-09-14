# Codex Windows x64 npm 补丁

本目录用于构建发布为 `@chenronggui/codex-win-patch` 的非官方 Windows x64
补丁。它会替换版本匹配的全局 `@openai/codex` 安装所使用的原生程序。

`Windows custom Codex overlay` 流水线会根据 `docs/templates` 自动生成所有
版本相关说明。不要手工复制版本号、commit 或哈希。经过测试的流水线附件包含 npm
tarball、对应的 SHA-256 文件、最终中文操作说明、发布清单和验证报告。

安装方式只支持 npm。生成的包包含：

- `README.md` 和 `docs/operation-guide.zh-CN.md`
- `docs/validation-prompt.zh-CN.md`
- `docs/diagnostics-guide.zh-CN.md`
- `build-info.json` 和 `SHA256SUMS`
- `codex.exe`、`apply_patch.exe` 以及安装/恢复命令入口

tarball 不能包含自己的 SHA-256，因为写入该值会再次改变 tarball。该哈希因此作为
同一流水线附件中的配套文件生成；两个 EXE 的哈希会同时写入 npm 包和最终操作说明。

发布物中的路径和文件名只使用 ASCII；`.zh-CN.md` 表示正文为中文，避免 ZIP、TAR
或企业解压软件错误解释 UTF-8 文件名。

npm 发布使用 GitHub OIDC trusted publishing。只有 Windows 测试、打包、隔离安装和
发布材料校验全部通过后，流水线才会标记 `PUBLISHABLE` 并允许发布。
