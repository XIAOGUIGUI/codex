# Codex Windows {{PACKAGE_VERSION}} npm 补丁

这是面向 Windows x64 的非官方 Codex 补丁，必须与
`@openai/codex@{{UPSTREAM_VERSION}}` 配套使用。

```powershell
npm install -g @openai/codex@{{UPSTREAM_VERSION}}
npm install -g --allow-scripts=@chenronggui/codex-win-patch @chenronggui/codex-win-patch@{{PACKAGE_VERSION}}
codex-win-patch-install
codex --version
```

完整操作、AI 验收及日志反馈说明位于：

- `docs/操作说明.md`
- `docs/验证提示词.md`
- `docs/日志反馈说明.md`

构建 commit：`{{COMMIT}}`

`codex.exe` SHA-256：`{{CODEX_SHA256}}`

`apply_patch.exe` SHA-256：`{{APPLY_PATCH_SHA256}}`

本包由 [{{REPOSITORY}}]({{REPOSITORY_URL}}) 的
[Windows 构建流水线]({{WORKFLOW_RUN_URL}}) 生成，不受 OpenAI 官方支持。
