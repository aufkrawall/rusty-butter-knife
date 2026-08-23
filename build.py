#!/usr/bin/env python3
"""
Build script for GreenPostInstallDebloatNative.

Downloads llvm-mingw (if not already present) and compiles the C++ source.

Usage:
  python build.py                      # x86_64 build (default)
  python build.py --arch aarch64       # Windows-on-ARM64 build
  python build.py --sha256 <hex>       # verify the downloaded toolchain archive
  python build.py --clean              # remove the downloaded toolchain first
"""

import argparse
import hashlib
import os
import platform
import shutil
import socket
import subprocess
import sys
import urllib.request
import zipfile

LLVM_MINGW_VERSION = "20260616"
LLVM_MINGW_URL = (
    f"https://github.com/mstorsjo/llvm-mingw/releases/download/"
    f"{LLVM_MINGW_VERSION}/llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64.zip"
)
BASE_DIR = os.path.dirname(os.path.abspath(__file__))
MINGW_DIR = os.path.join(BASE_DIR, "mingw64")
SOURCE = os.path.join(BASE_DIR, "GreenPostInstallDebloatNative.cpp")
OUTPUT = os.path.join(BASE_DIR, "GreenPostInstallDebloatNative.exe")

ARCHIVE_NAME = f"llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64.zip"
ARCHIVE_PATH = os.path.join(BASE_DIR, ARCHIVE_NAME)

# Host toolchain is x86_64; these flags select the cross-compilation target.
ARCH_TARGETS = {
    "x86_64": None,
    "aarch64": "--target=aarch64-w64-mingw32",
}


def sha256_of_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def find_clangpp():
    clangpp = os.path.join(MINGW_DIR, "bin", "clang++.exe")
    if os.path.isfile(clangpp):
        return clangpp
    which = shutil.which("clang++")
    if which:
        return which
    return None


def download_and_extract(expected_sha256=None):
    if os.path.isdir(MINGW_DIR) and os.path.isfile(os.path.join(MINGW_DIR, "bin", "clang++.exe")):
        print(f"[*] MinGW LLVM already present at {MINGW_DIR}")
        return True

    have_archive = os.path.isfile(ARCHIVE_PATH)

    if not have_archive:
        print(f"[*] Downloading {LLVM_MINGW_URL} ...")
        try:
            urllib.request.urlretrieve(LLVM_MINGW_URL, ARCHIVE_PATH)
        except Exception as e:
            print(f"[!] Download failed: {e}")
            return False
    else:
        print(f"[*] Archive already cached: {ARCHIVE_PATH}")

    if expected_sha256:
        actual = sha256_of_file(ARCHIVE_PATH)
        if actual.lower() != expected_sha256.lower():
            print(f"[!] SHA256 mismatch for {ARCHIVE_NAME}:")
            print(f"    expected {expected_sha256}")
            print(f"    actual   {actual}")
            os.remove(ARCHIVE_PATH)
            print("[!] Corrupt/tampered archive deleted.")
            return False
        print("[*] SHA256 verified OK.")
    else:
        print("[*] Note: pass --sha256 <hex> to verify the toolchain download.")

    try:
        with open(ARCHIVE_PATH, "rb") as f:
            magic = f.read(4)
        if magic != b"PK\x03\x04":
            raise ValueError("not a ZIP archive")
    except Exception as e:
        print(f"[!] Archive integrity check failed: {e}")
        return False

    print(f"[*] Extracting to {MINGW_DIR} ...")
    try:
        with zipfile.ZipFile(ARCHIVE_PATH, "r") as zf:
            names = zf.namelist()
            if not names:
                raise ValueError("Empty archive")
            top_level = names[0].split("/")[0]
            extract_dir = os.path.join(BASE_DIR, "_extract")
            os.makedirs(extract_dir, exist_ok=True)
            zf.extractall(extract_dir)
            extracted_top = os.path.join(extract_dir, top_level.rstrip("/"))
            if os.path.isdir(extracted_top):
                if os.path.isdir(MINGW_DIR):
                    shutil.rmtree(MINGW_DIR)
                shutil.move(extracted_top, MINGW_DIR)
            shutil.rmtree(extract_dir)
    except Exception as e:
        print(f"[!] Extraction failed: {e}")
        return False

    if os.path.isfile(os.path.join(MINGW_DIR, "bin", "clang++.exe")):
        print(f"[*] Extracted successfully to {MINGW_DIR}")
        return True
    print("[!] clang++.exe not found after extraction")
    return False


def build(clangpp_path, arch):
    if not os.path.isfile(SOURCE):
        print(f"[!] Source not found: {SOURCE}")
        return False

    cmd = [clangpp_path]
    target_flag = ARCH_TARGETS.get(arch)
    if target_flag:
        cmd.append(target_flag)
    cmd += [
        "-std=c++17",
        "-municode",
        "-O2",
        "-Wall",
        "-Wextra",
        "-static",
        SOURCE,
        "-o", OUTPUT,
        "-ladvapi32",
        "-lshell32",
        "-lole32",
        "-loleaut32",
        "-ltaskschd",
    ]
    print(f"[*] Compiling ({arch}): {' '.join(cmd)}")
    env = os.environ.copy()
    mingw_bin = os.path.join(MINGW_DIR, "bin")
    if os.path.isdir(mingw_bin):
        env["PATH"] = mingw_bin + os.pathsep + env.get("PATH", "")
    result = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if result.stdout:
        print(result.stdout)
    if result.stderr:
        print(result.stderr, file=sys.stderr)
    if result.returncode != 0:
        print(f"[!] Compilation failed with exit code {result.returncode}")
        return False

    if arch != "x86_64":
        print(f"[!] Note: output written to {OUTPUT} ({arch} binary). "
              f"Rebuild with default --arch to restore the x86_64 binary.")
    print(f"[*] Successfully built: {OUTPUT}")
    return True


def clean():
    removed_something = False
    for path in (MINGW_DIR, os.path.join(BASE_DIR, "_extract"), ARCHIVE_PATH):
        if os.path.isdir(path):
            print(f"[*] Removing directory: {path}")
            shutil.rmtree(path)
            removed_something = True
        elif os.path.isfile(path):
            print(f"[*] Removing file: {path}")
            os.remove(path)
            removed_something = True
    if not removed_something:
        print("[*] Nothing to clean.")


def main():
    parser = argparse.ArgumentParser(description="Build GreenPostInstallDebloatNative.")
    parser.add_argument("--arch", choices=sorted(ARCH_TARGETS), default="x86_64",
                        help="target architecture (default: x86_64)")
    parser.add_argument("--sha256", metavar="HEX", default=None,
                        help="expected SHA256 of the llvm-mingw archive (verified after download)")
    parser.add_argument("--clean", action="store_true",
                        help="remove the downloaded toolchain and cached archive before building")
    args = parser.parse_args()

    # Bound network operations instead of hanging indefinitely.
    socket.setdefaulttimeout(120)

    if platform.system() != "Windows":
        print("[!] This build targets Windows. You are on " + platform.system())
        sys.exit(1)

    print(f"[*] llvm-mingw version: {LLVM_MINGW_VERSION}")
    print(f"[*] MinGW directory:    {MINGW_DIR}")
    print(f"[*] Source:             {SOURCE}")
    print(f"[*] Output:             {OUTPUT}")
    print(f"[*] Architecture:       {args.arch}")
    print()

    if args.clean:
        clean()
        print()

    if not download_and_extract(args.sha256):
        print("[!] Falling back to system clang++...")
        clangpp = shutil.which("clang++")
        if not clangpp:
            print("[!] No system clang++ found either. Aborting.")
            sys.exit(1)
        if not build(clangpp, args.arch):
            sys.exit(1)
        return

    clangpp = find_clangpp()
    if not clangpp:
        print("[!] clang++ not found in mingw64. Aborting.")
        sys.exit(1)
    if not build(clangpp, args.arch):
        sys.exit(1)


if __name__ == "__main__":
    main()
