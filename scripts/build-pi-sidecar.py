#!/usr/bin/env python3
"""Fetch the pinned official Pi standalone distribution; never uses a global Pi/Node."""
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
VERSION = "0.85.1"
ASSETS = {
    "aarch64-apple-darwin": ("pi-darwin-arm64.tar.gz", "d5f70e3c0cf7398eac239fd0261ee074d98b7ba7f6b43fe3617f052ed5b79d06"),
    "x86_64-apple-darwin": ("pi-darwin-x64.tar.gz", "adb918b845625f184d8bea408d55eacaf21aa87238793c0f5b4f3b9737bce62b"),
    "x86_64-pc-windows-msvc": ("pi-windows-x64.zip", "002fa95b90d521245b9985d8f168caebc237ad56e7e30b319807dee1b2e17e1c"),
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    if platform.system() not in ("Darwin", "Windows") and not os.environ.get("PI_TARGET"):
        raise RuntimeError("Specify PI_TARGET when downloading from a non-target host")
    machine = platform.machine().lower()
    default = ("aarch64" if machine in ("arm64", "aarch64") else "x86_64") + (
        "-apple-darwin" if platform.system() == "Darwin" else "-pc-windows-msvc")
    target = os.environ.get("PI_TARGET", default)
    asset, expected = ASSETS[target]
    base = ROOT / "src-tauri/resources/pi"
    cache = ROOT / "src-tauri/target/pi-downloads"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"{VERSION}-{asset}"
    if not archive.exists() or digest(archive) != expected:
        print(f"Downloading official Pi {VERSION}: {asset}", flush=True)
        url = f"https://github.com/earendil-works/pi/releases/download/v{VERSION}/{asset}"
        temporary = archive.with_suffix(".part")
        with urllib.request.urlopen(url, timeout=60) as response, temporary.open("wb") as output:
            shutil.copyfileobj(response, output)
        if digest(temporary) != expected:
            temporary.unlink()
            raise RuntimeError("Official Pi archive SHA-256 mismatch")
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
    executable = "pi.exe" if "windows" in target else "pi"
    candidates = [p for p in staging.rglob(executable) if p.is_file()]
    if len(candidates) != 1:
        raise RuntimeError("Expected exactly one standalone Pi executable")
    binary = candidates[0]
    shutil.copy2(ROOT / "scripts/assets/pi-policy.ts", staging / "sophonote-policy.ts")
    shutil.copy2(ROOT / "scripts/assets/LICENSE.pi", staging / "LICENSE.pi")
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
    print(f"Verified Pi {VERSION} bundled at {destination}", flush=True)


if __name__ == "__main__":
    main()
