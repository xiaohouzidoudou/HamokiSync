#!/usr/bin/env python3
"""Package audited release binaries; never read user configuration."""
import ctypes
import pathlib
import plistlib
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / "dist"
OUT.mkdir(exist_ok=True)
ICON = ROOT / "assets/hamokisync-green.png"
BINS = ("hamokisync-daemon", "hamokisync-settings")

def command(*args):
    subprocess.run([str(a) for a in args], check=True)

def embed_windows_icon(exe):
    # A PNG-compressed 128 px RT_ICON and its group directory.
    data = ICON.read_bytes()
    width, height = struct.unpack(">II", data[16:24])
    if not (0 < width <= 256 and 0 < height <= 256):
        raise ValueError("Unexpected icon size")
    group = struct.pack("<HHHBBBBHHIH", 0, 1, 1, width % 256, height % 256,
                        0, 0, 1, 32, len(data), 1)
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.BeginUpdateResourceW.argtypes = [ctypes.c_wchar_p, ctypes.c_int]
    api.BeginUpdateResourceW.restype = ctypes.c_void_p
    api.UpdateResourceW.argtypes = [ctypes.c_void_p, ctypes.c_void_p,
                                   ctypes.c_void_p, ctypes.c_ushort,
                                   ctypes.c_void_p, ctypes.c_uint]
    api.UpdateResourceW.restype = ctypes.c_int
    api.EndUpdateResourceW.argtypes = [ctypes.c_void_p, ctypes.c_int]
    api.EndUpdateResourceW.restype = ctypes.c_int
    h = api.BeginUpdateResourceW(str(exe), 0)
    if not h:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        for typ, payload in ((3, data), (14, group)):
            buf = ctypes.create_string_buffer(payload)
            if not api.UpdateResourceW(h, typ, 1, 0, buf, len(payload)):
                raise ctypes.WinError(ctypes.get_last_error())
    except BaseException:
        api.EndUpdateResourceW(h, 1)
        raise
    if not api.EndUpdateResourceW(h, 0):
        raise ctypes.WinError(ctypes.get_last_error())

def windows(target_dir):
    name = "HamokiSync-windows-x64"
    with tempfile.TemporaryDirectory() as tmp:
        d = pathlib.Path(tmp) / name
        d.mkdir()
        for b in BINS:
            shutil.copy2(target_dir / (b + ".exe"), d / (b + ".exe"))
        embed_windows_icon(d / "hamokisync-settings.exe")
        shutil.copy2(ROOT / "LICENSE", d / "LICENSE")
        shutil.copy2(ROOT / "README.md", d / "README.md")
        shutil.copy2(ICON, d / "hamokisync.png")
        for b in BINS:
            shutil.copy2(d / (b + ".exe"), OUT / (b + "-windows-x64.exe"))
        with zipfile.ZipFile(OUT / (name + ".zip"), "w", zipfile.ZIP_DEFLATED) as z:
            for f in sorted(d.iterdir()):
                z.write(f, name + "/" + f.name)

def linux(target_dir):
    name = "HamokiSync-linux-x64"
    with tempfile.TemporaryDirectory() as tmp:
        d = pathlib.Path(tmp) / name
        d.mkdir()
        for b in BINS:
            shutil.copy2(target_dir / b, d / b)
            (d / b).chmod(0o755)
            shutil.copy2(d / b, OUT / (b + "-linux-x64"))
        shutil.copy2(ICON, d / "hamokisync.png")
        shutil.copy2(ROOT / "README.md", d / "README.md")
        shutil.copy2(ROOT / "LICENSE", d / "LICENSE")
        shutil.copy2(ROOT / "scripts/install-linux.sh", d / "install.sh")
        (d / "install.sh").chmod(0o755)
        with tarfile.open(OUT / (name + ".tar.gz"), "w:gz") as t:
            t.add(d, arcname=name)

def macos(target_root):
    name = "HamokiSync-macos-universal"
    with tempfile.TemporaryDirectory() as tmp:
        d = pathlib.Path(tmp)
        app = d / "HamokiSync.app"
        exe = app / "Contents/MacOS"
        resources = app / "Contents/Resources"
        exe.mkdir(parents=True)
        resources.mkdir(parents=True)
        for b in BINS:
            command("lipo", "-create",
                    target_root / "aarch64-apple-darwin/release" / b,
                    target_root / "x86_64-apple-darwin/release" / b,
                    "-output", exe / b)
            (exe / b).chmod(0o755)
            command("lipo", exe / b, "-verify_arch", "arm64", "x86_64")
        # ICNS with native-size PNG and high-resolution icons from project art.
        iconset = d / "HamokiSync.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                filename = f"icon_{size}x{size}" + ("@2x" if scale == 2 else "") + ".png"
                command("sips", "-z", size * scale, size * scale, ICON,
                        "--out", iconset / filename)
        command("iconutil", "-c", "icns", iconset, "-o", resources / "HamokiSync.icns")
        with (app / "Contents/Info.plist").open("wb") as f:
            plistlib.dump({
                "CFBundleName": "HamokiSync", "CFBundleDisplayName": "HamokiSync",
                "CFBundleIdentifier": "io.hamoki.HamokiSync",
                "CFBundleExecutable": "hamokisync-settings",
                "CFBundlePackageType": "APPL", "CFBundleIconFile": "HamokiSync.icns",
                "CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "1",
                "NSHighResolutionCapable": True,
            }, f)
        command("codesign", "--force", "--sign", "-", "--timestamp=none", exe / BINS[0])
        command("codesign", "--force", "--sign", "-", "--timestamp=none", app)
        command("codesign", "--verify", "--deep", "--strict", app)
        for b in BINS:
            raw = OUT / (b + "-macos-universal")
            shutil.copy2(exe / b, raw)
            command("codesign", "--force", "--sign", "-", "--timestamp=none", raw)
            command("codesign", "--verify", "--strict", raw)
        command("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", app, OUT / (name + ".zip"))

if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("Usage: package.py windows|linux|macos TARGET_DIR")
    {"windows": windows, "linux": linux, "macos": macos}[sys.argv[1]](pathlib.Path(sys.argv[2]).resolve())
