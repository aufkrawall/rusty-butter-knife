#!/usr/bin/env python3
"""
Build script for GreenPostInstallDebloatNative.

Downloads llvm-mingw (if not already present) and compiles the C++ source.
"""

import os
import sys
import subprocess
import urllib.request
import zipfile
import shutil
import platform

LLVM_MINGW_VERSION = "20260616"
LLVM_MINGW_URL = (
    f"https://github.com/mstorsjo/llvm-mingw/releases/download/"
    f"{LLVM_MINGW_VERSION}/llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64.zip"
)
MINGW_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "mingw64")
SOURCE = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "GreenPostInstallDebloatNative.cpp")
OUTPUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "GreenPostInstallDebloatNative.exe")

ARCHIVE_NAME = f"llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64.zip"


def find_clangpp():
    clangpp = os.path.join(MINGW_DIR, "bin", "clang++.exe")
    if os.path.isfile(clangpp):
        return clangpp
    which = shutil.which("clang++")
    if which:
        return which
    return None


def download_and_extract():
    if os.path.isdir(MINGW_DIR) and os.path.isfile(os.path.join(MINGW_DIR, "bin", "clang++.exe")):
        print(f"[*] MinGW LLVM already present at {MINGW_DIR}")
        return True

    archive_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), ARCHIVE_NAME)

    if not os.path.isfile(archive_path):
        print(f"[*] Downloading {LLVM_MINGW_URL} ...")
        try:
            urllib.request.urlretrieve(LLVM_MINGW_URL, archive_path)
        except Exception as e:
            print(f"[!] Download failed: {e}")
            return False
    else:
        print(f"[*] Archive already cached: {archive_path}")

    print(f"[*] Extracting to {MINGW_DIR} ...")
    try:
        with zipfile.ZipFile(archive_path, "r") as zf:
            names = zf.namelist()
            if not names:
                raise ValueError("Empty archive")
            top_level = names[0].split("/")[0]
            extract_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_extract")
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


def build(clangpp_path):
    if not os.path.isfile(SOURCE):
        print(f"[!] Source not found: {SOURCE}")
        return False

    cmd = [
        clangpp_path,
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
    print(f"[*] Compiling: {' '.join(cmd)}")
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
    print(f"[*] Successfully built: {OUTPUT}")
    return True


def main():
    if platform.system() != "Windows":
        print("[!] This build targets Windows. You are on " + platform.system())
        sys.exit(1)

    print(f"[*] llvm-mingw version: {LLVM_MINGW_VERSION}")
    print(f"[*] MinGW directory:    {MINGW_DIR}")
    print(f"[*] Source:             {SOURCE}")
    print(f"[*] Output:             {OUTPUT}")
    print()

    if not download_and_extract():
        print("[!] Falling back to system clang++...")
        clangpp = shutil.which("clang++")
        if not clangpp:
            print("[!] No system clang++ found either. Aborting.")
            sys.exit(1)
        if not build(clangpp):
            sys.exit(1)
        return

    clangpp = find_clangpp()
    if not clangpp:
        print("[!] clang++ not found in mingw64. Aborting.")
        sys.exit(1)
    if not build(clangpp):
        sys.exit(1)


if __name__ == "__main__":
    main()
