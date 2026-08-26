#!/usr/bin/env python3
"""
Build script for GreenPostInstallDebloatNative.

Default output is the PRIMARY RUST BUILD into dist/rust-<arch>/; the legacy
C++17 translation unit is reference code and can be built explicitly via
--variant cpp (or --variant all).

  dist/rust-<arch>/GreenPostInstallDebloatNative.exe   (primary Rust build)
  dist/cpp-<arch>/GreenPostInstallDebloatNative.exe    (legacy C++17, reference)

The C++ leg downloads llvm-mingw (if not already present; its SHA256 is
verified against a built-in pin) and compiles the legacy single-TU source.
The Rust leg drives cargo (release profile) and copies the binary out of
target/. EVERY artifact's PE machine type is verified against the requested
architecture before it lands in dist/ (x86_64 == 0x8664, ARM64 == 0xAA64),
so a host/target mismatch can never silently masquerade as a cross-build
(audit finding on RUST_ARCH_TARGETS[x86_64]=None).

Usage:
  python build.py                      # build Rust variant (x86_64 default)
  python build.py --variant cpp        # legacy C++ only
  python build.py --variant all        # both variants
  python build.py --arch aarch64       # cross-compile for Windows-on-ARM64
  python build.py --sha256 <hex>       # override the pinned toolchain SHA256 check
  python build.py --clean              # remove toolchain, cache and dist/
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
# SHA256 of the pinned release asset, taken from the GitHub release API
# asset digest. Verified automatically on every fresh download; --sha256
# overrides only when deliberately switching archives.
LLVM_MINGW_SHA256 = (
    "b9b68a4d276e16fa25802aaba458e4638f64b3884c290aaccdc2d87083b6ca35"
)
BASE_DIR = os.path.dirname(os.path.abspath(__file__))
MINGW_DIR = os.path.join(BASE_DIR, "mingw64")
SOURCE = os.path.join(BASE_DIR, "GreenPostInstallDebloatNative.cpp")
DIST_ROOT = os.path.join(BASE_DIR, "dist")

ARCHIVE_NAME = f"llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64.zip"
ARCHIVE_PATH = os.path.join(BASE_DIR, ARCHIVE_NAME)

# Host toolchain is x86_64; these flags select the cross-compilation target.
# Every entry maps to an EXPLICIT target: "use whatever the host is" would
# make --arch x86_64 produce an ARM64 binary on an ARM64 machine (audit P0
# finding in build.py).
ARCH_TARGETS = {
    "x86_64": "--target=x86_64-w64-mingw32",
    "aarch64": "--target=aarch64-w64-mingw32",
}

PE_MACHINE = {"x86_64": 0x8664, "aarch64": 0xAA64}

# Rust cross-compilation targets are equally explicit and msvc-based; add the
# matching rustup target when cross-building ARM64.
RUST_ARCH_TARGETS = {
    "x86_64": "x86_64-pc-windows-msvc",
    "aarch64": "aarch64-pc-windows-msvc",
}


def pe_machine_type(path):
    """Read the COFF 'Machine' field from a PE file (None if unreadable)."""
    try:
        with open(path, "rb") as f:
            dos = f.read(64)
            if len(dos) < 64 or dos[:2] != b"MZ":
                return None
            import struct

            e_lfanew = struct.unpack_from("<I", dos, 0x3C)[0]
            f.seek(e_lfanew)
            pe_sig = f.read(6)
            if pe_sig[:4] != b"PE\x00\x00":
                return None
            return struct.unpack_from("<H", pe_sig, 4)[0]
    except OSError:
        return None


def verify_pe_machine(path, arch):
    expected = PE_MACHINE.get(arch)
    actual = pe_machine_type(path)
    if actual != expected:
        print(
            f"[!] PE machine verification FAILED for {path}: "
            f"expected 0x{expected:04X}, found "
            + (f"0x{actual:04X}" if actual is not None else "unreadable")
        )
        return False
    print(f"[*] PE machine type verified: {path} ({arch})")
    return True


def dist_dir(variant, arch):
    return os.path.join(DIST_ROOT, f"{variant}-{arch}")


def output_exe(variant, arch):
    return os.path.join(dist_dir(variant, arch), "GreenPostInstallDebloatNative.exe")


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
    if expected_sha256 is None:
        # Default to the pinned asset digest so supply-chain verification is
        # on by default, not opt-in.
        expected_sha256 = LLVM_MINGW_SHA256
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
        print("[*] SHA256 verified OK against the pinned digest.")
    else:
        print("[*] Note: pass --sha256 <hex> to verify a custom toolchain archive.")

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


def build_cpp(clangpp_path, arch, out_path):
    if not os.path.isfile(SOURCE):
        print(f"[!] Source not found: {SOURCE}")
        return False

    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    cmd = [clangpp_path]
    target_flag = ARCH_TARGETS[arch]
    cmd.append(target_flag)
    cmd += [
        "-std=c++17",
        "-municode",
        "-O2",
        "-Wall",
        "-Wextra",
        "-static",
        SOURCE,
        "-o",
        out_path,
        "-ladvapi32",
        "-lshell32",
        "-lole32",
        "-loleaut32",
        "-ltaskschd",
    ]
    print(f"[*] Compiling cpp ({arch}): {' '.join(cmd)}")
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
    if not verify_pe_machine(out_path, arch):
        return False

    print(f"[*] Successfully built: {out_path}")
    return True


def cargo_env():
    """Extend PATH so cargo is found even when ~/.cargo/bin is not on it."""
    env = os.environ.copy()
    cargo_bin = os.path.expanduser("~/.cargo/bin")
    if os.path.isdir(cargo_bin):
        env["PATH"] = cargo_bin + os.pathsep + env.get("PATH", "")
    return env


def find_cargo():
    """Absolute path to cargo; checks ~/.cargo/bin when not on PATH."""
    which = shutil.which("cargo")
    if which:
        return which
    candidate = os.path.expanduser(os.path.join("~", ".cargo", "bin", "cargo.exe"))
    if os.path.isfile(candidate):
        return candidate
    return None


def rust_target_triple(arch, cargo=None):
    # Always explicit (see RUST_ARCH_TARGETS note); cargo host detection kept
    # only as a sanity check that the requested triple exists.
    triple = RUST_ARCH_TARGETS[arch]
    if cargo is None:
        return triple
    res = subprocess.run([cargo, "-vV"], capture_output=True, text=True, env=cargo_env())
    hosts = [line.split(":", 1)[1].strip() for line in (res.stdout or "").splitlines() if line.startswith("host:")]
    if hosts and hosts[0] != triple:
        print(f"[*] Cross-compiling: host={hosts[0]} -> target={triple}")
    return triple


def build_rust(arch):
    cargo = find_cargo()
    if not cargo:
        print("[!] cargo not found. Install Rust via https://rustup.rs "
              "or add ~/.cargo/bin to PATH.")
        return False

    triple = rust_target_triple(arch, cargo)
    if not triple:
        print("[!] Could not determine the Rust host target triple.")
        return False

    cmd = [cargo, "build", "--release"]
    if triple != rust_target_triple("x86_64", cargo) or arch == "aarch64":
        cmd += ["--target", triple]

    print(f"[*] Compiling rust ({triple}): {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=BASE_DIR, env=cargo_env())
    if result.returncode != 0:
        print(f"[!] cargo build failed with exit code {result.returncode}")
        return False

    src = os.path.join(
        BASE_DIR, "target", triple, "release", "GreenPostInstallDebloatNative.exe"
    )
    if not os.path.isfile(src):
        src = os.path.join(
            BASE_DIR, "target", "release", "GreenPostInstallDebloatNative.exe"
        )
    if not os.path.isfile(src):
        print(f"[!] Built binary not found at {src}")
        return False

    out_path = output_exe("rust", arch)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    if not verify_pe_machine(src, arch):
        print("[!] Refusing to publish an executable of the wrong architecture.")
        return False
    shutil.copy2(src, out_path)
    if not verify_pe_machine(out_path, arch):  # belt-and-braces on the copy
        return False
    print(f"[*] Successfully built: {out_path}")
    return True


def clean():
    removed_something = False
    for path in (
        MINGW_DIR,
        os.path.join(BASE_DIR, "_extract"),
        ARCHIVE_PATH,
        DIST_ROOT,
    ):
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


def build_cpp_variant(arch, sha256):
    """Ensure the toolchain, then compile the legacy source. Returns bool."""
    if not download_and_extract(sha256):
        print("[!] Falling back to system clang++...")
        clangpp = shutil.which("clang++")
        if not clangpp:
            print("[!] No system clang++ found either. Aborting cpp leg.")
            return False
        return build_cpp(clangpp, arch, output_exe("cpp", arch))

    clangpp = find_clangpp()
    if not clangpp:
        print("[!] clang++ not found in mingw64. Aborting cpp leg.")
        return False
    return build_cpp(clangpp, arch, output_exe("cpp", arch))


def main():
    parser = argparse.ArgumentParser(description="Build GreenPostInstallDebloatNative.")
    parser.add_argument(
        "--variant",
        choices=("all", "cpp", "rust"),
        default="rust",
        help="which variant(s) to build (default: rust; legacy cpp is reference code)",
    )
    parser.add_argument(
        "--arch",
        choices=sorted(ARCH_TARGETS),
        default="x86_64",
        help="target architecture (default: x86_64)",
    )
    parser.add_argument(
        "--sha256",
        metavar="HEX",
        default=None,
        help="override the built-in pinned SHA256 check for the llvm-mingw archive",
    )
    parser.add_argument(
        "--clean",
        action="store_true",
        help="remove the downloaded toolchain, cached archive and dist/ before building",
    )
    args = parser.parse_args()

    # Bound network operations instead of hanging indefinitely.
    socket.setdefaulttimeout(120)

    if platform.system() != "Windows":
        print("[!] This build targets Windows. You are on " + platform.system())
        sys.exit(1)

    print(f"[*] llvm-mingw version: {LLVM_MINGW_VERSION}")
    print(f"[*] MinGW directory:    {MINGW_DIR}")
    print(f"[*] Architecture:       {args.arch}")
    print(f"[*] Variant:            {args.variant}  ->  dist/<variant>-{args.arch}/")
    print()

    if args.clean:
        clean()
        print()

    ok = True

    if args.variant in ("all", "rust"):
        ok = build_rust(args.arch) and ok

    if args.variant in ("all", "cpp"):
        ok = build_cpp_variant(args.arch, args.sha256) and ok

    if not ok:
        print("[!] One or more build legs FAILED.")
        sys.exit(1)

    print(f"[*] All requested variant(s) built into {DIST_ROOT}{os.sep}<variant>-{args.arch}{os.sep}")


if __name__ == "__main__":
    main()
