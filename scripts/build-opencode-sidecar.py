#!/usr/bin/env python3
"""Fetch the pinned official OpenCode standalone distribution; never uses a global OpenCode/Node."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
VERSION = "1.18.31"
ASSETS = {
    "aarch64-apple-darwin": ("opencode-darwin-arm64.zip", "caf7f31fa1aec2353ea859d4ef9ab824c6273d941b016e88d51193fa3028d34e"),
    "x86_64-apple-darwin": ("opencode-darwin-x64.zip", "f8510eaf400f07c3a2014e3a517e3650c705bcd6ac3e6740351b723ee685042f"),
    "x86_64-pc-windows-msvc": ("opencode-windows-x64.zip", "0ecd7ffc7f26390ce7799e7bcd409e4f11c410144308a6a5b0fcdce63d871006"),
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    if platform.system() not in ("Darwin", "Windows") and not os.environ.get("OPENCODE_TARGET"):
        raise RuntimeError("Specify OPENCODE_TARGET when downloading from a non-target host")
    machine = platform.machine().lower()
    default = ("aarch64" if machine in ("arm64", "aarch64") else "x86_64") + (
        "-apple-darwin" if platform.system() == "Darwin" else "-pc-windows-msvc")
    target = os.environ.get("OPENCODE_TARGET", default)
    asset, expected = ASSETS[target]
    base = ROOT / "src-tauri/resources/opencode"
    cache = ROOT / "src-tauri/target/opencode-downloads"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"{VERSION}-{asset}"
    if not archive.exists() or digest(archive) != expected:
        print(f"Downloading official OpenCode {VERSION}: {asset}", flush=True)
        url = f"https://github.com/anomalyco/opencode/releases/download/v{VERSION}/{asset}"
        temporary = archive.with_suffix(".part")
        with urllib.request.urlopen(url, timeout=60) as response, temporary.open("wb") as output:
            shutil.copyfileobj(response, output)
        if digest(temporary) != expected:
            temporary.unlink()
            raise RuntimeError("Official OpenCode archive SHA-256 mismatch")
        temporary.replace(archive)
    staging = base / f".staging-{target}"
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir(parents=True)
    if asset.endswith(".zip"):
        with zipfile.ZipFile(archive) as source:
            for member in source.infolist():
                if not (staging / member.filename).resolve().is_relative_to(staging.resolve()):
                    raise RuntimeError("Unsafe archive path")
            source.extractall(staging)
    else:
        with tarfile.open(archive) as source:
            source.extractall(staging, filter="data")
    executable = "opencode.exe" if "windows" in target else "opencode"
    candidates = [p for p in staging.rglob(executable) if p.is_file()]
    if len(candidates) != 1:
        raise RuntimeError("Expected exactly one standalone OpenCode executable")
    binary = candidates[0]
    if "windows" not in target:
        binary.chmod(0o755)
    shutil.copy2(ROOT / "scripts/assets/LICENSE.opencode", staging / "LICENSE.opencode")
    if platform.system() == "Darwin" and target == default:
        identity = os.environ.get("APPLE_SIGNING_IDENTITY", "-")
        args = ["codesign", "--force", "--sign", identity, "--entitlements", str(ROOT / "scripts/assets/pi-entitlements.plist")]
        args += ["--timestamp=none"] if identity == "-" else ["--timestamp", "--options", "runtime"]
        subprocess.run(args + [str(binary)], check=True)
    manifest = {
        "version": VERSION, "target": target,
        "archiveSha256": expected,
        "executable": binary.relative_to(staging).as_posix(),
        "files": {p.relative_to(staging).as_posix(): digest(p)
                  for p in sorted(staging.rglob("*")) if p.is_file()},
    }
    (staging / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    destination = base / target
    if destination.exists():
        shutil.rmtree(destination)
    staging.replace(destination)
    print(f"Verified OpenCode {VERSION} bundled at {destination}", flush=True)


if __name__ == "__main__":
    main()
