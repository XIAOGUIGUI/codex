#!/usr/bin/env node

const { runPowerShell } = require("./powershell.js");

process.exit(runPowerShell("restore.ps1", process.argv.slice(2)));
