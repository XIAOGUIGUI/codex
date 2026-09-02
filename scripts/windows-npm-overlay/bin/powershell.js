const { spawnSync } = require("node:child_process");
const path = require("node:path");

function powershellCandidates() {
  const configured = process.env.CODEX_WINDOWS_PATCH_POWERSHELL;
  if (configured) {
    return [configured];
  }
  return ["pwsh.exe", "powershell.exe"];
}

function runPowerShell(scriptName, args) {
  if (process.platform !== "win32") {
    console.error("This package supports Windows only.");
    return 1;
  }
  if (process.arch !== "x64") {
    console.error("This package supports Windows x64 only.");
    return 1;
  }

  const scriptPath = path.join(__dirname, "..", scriptName);
  const commandArgs = [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    scriptPath,
    ...args,
  ];

  for (const command of powershellCandidates()) {
    const result = spawnSync(command, commandArgs, {
      stdio: "inherit",
      windowsHide: true,
    });
    if (result.error?.code === "ENOENT") {
      continue;
    }
    if (result.error) {
      console.error(`Could not start ${command}: ${result.error.message}`);
      return 1;
    }
    if (typeof result.status !== "number") {
      console.error(`${command} exited without reporting a status code.`);
      return 1;
    }
    return result.status;
  }

  console.error(
    "Could not find PowerShell 7 (pwsh.exe) or Windows PowerShell 5.1 (powershell.exe).",
  );
  return 1;
}

module.exports = { runPowerShell };
