#!/usr/bin/env python3
"""
Build script for Rusty Butter Knife.

Drives cargo (release profile) and places the verified PE binary into
dist/<arch>/RustyButterKnife.exe. Automatically injects compiler path remapping
so host usernames and build directory trees are never baked into release
executables via panic paths or debug info.

Usage:
  python build.py                      # build release binary (x86_64 default)
  python build.py --arch aarch64       # cross-compile for Windows-on-ARM64
  python build.py --clean              # remove target/ and dist/
"""

import argparse
import os
import platform
import shutil
import struct
import subprocess
import sys

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
DIST_ROOT = os.path.join(BASE_DIR, "dist")
BINARY_NAME = "RustyButterKnife.exe"

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


def dist_dir(arch):
    return os.path.join(DIST_ROOT, arch)


def output_exe(arch):
    return os.path.join(dist_dir(arch), BINARY_NAME)


def cargo_env():
    """Extend PATH and inject privacy path-remapping into RUSTFLAGS."""
    env = os.environ.copy()
    cargo_bin = os.path.expanduser("~/.cargo/bin")
    if os.path.isdir(cargo_bin):
        env["PATH"] = cargo_bin + os.pathsep + env.get("PATH", "")

    # Remap host user and workspace paths so they are never baked into release binaries.
    remap_args = []
    user_home = os.path.expanduser("~")
    if user_home:
        remap_args.append(f"--remap-path-prefix={user_home}=~")
        remap_args.append(f"--remap-path-prefix={user_home.replace(os.sep, '/')}=~")
    if BASE_DIR:
        remap_args.append(f"--remap-path-prefix={BASE_DIR}=.")
        remap_args.append(f"--remap-path-prefix={BASE_DIR.replace(os.sep, '/')}=.")

    extra_rustflags = " ".join(remap_args)
    existing = env.get("RUSTFLAGS", "")
    env["RUSTFLAGS"] = (existing + " " + extra_rustflags).strip()
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
        BASE_DIR, "target", triple, "release", BINARY_NAME
    )
    if not os.path.isfile(src):
        src = os.path.join(
            BASE_DIR, "target", "release", BINARY_NAME
        )
    if not os.path.isfile(src):
        print(f"[!] Built binary not found at {src}")
        return False

    out_path = output_exe(arch)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    if not verify_pe_machine(src, arch):
        print("[!] Refusing to publish an executable of the wrong architecture.")
        return False
    shutil.copy2(src, out_path)
    if not verify_pe_machine(out_path, arch):
        return False
    print(f"[*] Successfully built: {out_path}")
    return True


def clean():
    removed_something = False
    for path in (
        os.path.join(BASE_DIR, "target"),
        DIST_ROOT,
        os.path.join(BASE_DIR, "mingw64"),
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


def main():
    parser = argparse.ArgumentParser(description="Build Rusty Butter Knife.")
    parser.add_argument(
        "--arch",
        choices=sorted(RUST_ARCH_TARGETS),
        default="x86_64",
        help="target architecture (default: x86_64)",
    )
    parser.add_argument(
        "--clean",
        action="store_true",
        help="remove target/ and dist/ before building",
    )
    args = parser.parse_args()

    if platform.system() != "Windows":
        print("[!] This build targets Windows. You are on " + platform.system())
        sys.exit(1)

    if args.clean:
        clean()
        print()

    print(f"[*] Architecture: {args.arch} -> dist/{args.arch}/")
    ok = build_rust(args.arch)

    if not ok:
        print("[!] Build FAILED.")
        sys.exit(1)

    print(f"[*] Built artifact in {DIST_ROOT}{os.sep}{args.arch}{os.sep}{BINARY_NAME}")


if __name__ == "__main__":
    main()
