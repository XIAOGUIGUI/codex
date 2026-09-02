#!/usr/bin/env node

const { runPowerShell } = require("./powershell.js");

const args = process.argv.slice(2);
const postinstallIndex = args.indexOf("--postinstall");
if (postinstallIndex !== -1) {
  args.splice(postinstallIndex, 1);
  const globalInstall = process.env.npm_config_global;
  if (globalInstall !== "true" && globalInstall !== "1") {
    console.error(
      "@chenronggui/codex-win-patch must be installed globally with npm install -g.",
    );
    process.exit(1);
  }
}

process.exit(runPowerShell("install.ps1", args));
